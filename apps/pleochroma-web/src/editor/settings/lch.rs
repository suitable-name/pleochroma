//! The Design settings color editor's callbacks (the desktop's
//! `gui::editor::render_color::setup_lch_editor`): Tone / Saturation / Hue sliders and the
//! hue-saturation-brightness picker edit one color row at a time, and "Apply Color" stores all
//! rows as ONE undoable `SetMaterial`.
//!
//! The rows are the host's directions: one for a stone that looks the same in every direction,
//! "Along the Axis" / "Across the Axis" for a two-color stone, "Alpha" / "Beta" / "Gamma" for a
//! three-color one (`pleochroma_web_core::design_color::AxisEditor`, on the shared
//! `pleochroma_editor::lch_color` rows). The swatches show the face-up color and each direction's.
//! "Save as Variant..." keeps the solved rows for the design's material in `IndexedDB`
//! (`storage::color_variants`), where the color combo lists them, and puts the saved variant on
//! the design as one undoable edit, as the desktop does.
//!
//! A solve is a 2187-point grid plus a short Levenberg-Marquardt polish in pure `f64`, so it
//! runs in the analysis Worker (`SolveRequest::BodyColor`) when a slider is let go, when the
//! picker returns a color and when the editor opens; the rows that changed are solved one after
//! the other, and the newest request wins (an older answer is dropped by its epoch). While a tilt
//! sweep holds that Worker, or when no Worker is available or it fails, the solve runs on the UI
//! thread instead, as it did before the request existed.

use std::{
    cell::{Cell, RefCell},
    rc::Rc,
    sync::atomic::AtomicBool,
};

use super::colors;
use crate::{
    AppWindow, DesignSettingsModel,
    app::{
        Ctx,
        push::{MessageKind, show_message},
    },
    editor::inspector::finish_no_tier,
    metrics::{hud, tilt},
    storage::color_variants,
};
use pleochroma::optics::materials::variants::ColorVariant;
use pleochroma_cut_core::Edit;
use pleochroma_editor::lch_color::{lch_from_srgb, reference_path_mm, solve_lch, swatch_bytes};
use pleochroma_web_core::{
    design_color::{AxisEditor, MATCHING_BADGE, applied_variant, host_material, stone_width_mm},
    solve::{BodyColorParams, SolveRequest, SolveResponse},
};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

thread_local! {
    /// Bumped by every solve request; an answer of an older one is ignored.
    static EPOCH: Cell<u64> = const { Cell::new(0) };
}

/// The open editor's state; `None` until the editor has been opened once.
type Editor = Rc<RefCell<Option<AxisEditor>>>;

/// The solver path (mm) of the open design: its girdle's, or the default.
fn design_path_mm(ctx: &Ctx) -> f64 {
    let girdle = ctx.state.try_borrow().ok().and_then(|app| {
        app.design
            .as_ref()
            .and_then(|design| design.session.design.girdle_diameter_mm)
    });
    reference_path_mm(girdle)
}

/// A Slint string model.
fn strings<S: Into<SharedString>>(items: impl IntoIterator<Item = S>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        items.into_iter().map(Into::into).collect::<Vec<_>>(),
    ))
}

/// A Slint color model from sRGB `0..=1` triples.
fn colors_of(items: &[[f32; 3]]) -> ModelRc<slint::Color> {
    ModelRc::new(VecModel::from(
        items
            .iter()
            .map(|rgb| {
                let [r, g, b] = swatch_bytes(*rgb);
                slint::Color::from_rgb_u8(r, g, b)
            })
            .collect::<Vec<_>>(),
    ))
}

/// Shows the editor's rows, swatches and badge, and the sliders of the active row.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the sliders are f32 properties; tone, chroma and hue are small values"
)]
fn push_editor(ctx: &Ctx, editor: &Editor, with_sliders: bool) {
    let Some(ui) = ctx.ui.upgrade() else {
        return;
    };
    let model = ui.global::<DesignSettingsModel>();
    let editor = editor.borrow();
    let Some(editor) = editor.as_ref() else {
        return;
    };
    let preview = editor.preview();
    let several = editor.rows().len() > 1;
    if several {
        model.set_lch_axis_labels(strings(editor.labels()));
        model.set_lch_axis_captions(strings(preview.captions.clone()));
        model.set_lch_axis_hints(strings(editor.hints()));
        model.set_lch_axis_colors(colors_of(&preview.row_colors));
    } else {
        model.set_lch_axis_labels(strings(Vec::<String>::new()));
        model.set_lch_axis_captions(strings(Vec::<String>::new()));
        model.set_lch_axis_hints(strings(Vec::<String>::new()));
        model.set_lch_axis_colors(colors_of(&[]));
    }
    model.set_lch_axis_active(i32::try_from(editor.active()).unwrap_or(0));
    if with_sliders {
        let lch = editor.active_lch();
        model.set_lch_tone(lch[0] as f32);
        model.set_lch_chroma(lch[1] as f32);
        model.set_lch_hue(lch[2] as f32);
    }
    model.set_lch_swatches(colors_of(&preview.swatches));
    model.set_lch_swatch_labels(strings(preview.labels));
    model.set_lch_badge_text(preview.badge.into());
    model.set_lch_reachable(preview.reachable);
    let ready = editor.is_ready();
    model.set_lch_has_solution(ready);
    model.set_lch_can_save_variant(ready && editor.has_host());
}

/// Solves every row that has no current answer and shows each one as it arrives. The newest
/// request wins: an older run stops at its next row.
fn solve_pending(ctx: &Ctx, editor: &Editor) {
    let (pending, path_mm) = match editor.borrow().as_ref() {
        Some(open) => (open.pending(), open.path_mm()),
        None => return,
    };
    let epoch = EPOCH.with(|e| {
        e.set(e.get() + 1);
        e.get()
    });
    if pending.is_empty() {
        push_editor(ctx, editor, false);
        return;
    }
    // A tilt sweep holds the analysis Worker (a request would end it): solve here then.
    let client = if tilt::is_running() {
        None
    } else {
        crate::workers::pool().and_then(|pool| pool.analysis()).ok()
    };
    if let Some(ui) = ctx.ui.upgrade() {
        let model = ui.global::<DesignSettingsModel>();
        model.set_lch_badge_text(MATCHING_BADGE.into());
        // Apply and Save wait for the answers.
        model.set_lch_has_solution(false);
        model.set_lch_can_save_variant(false);
    }
    let (ctx, editor) = (ctx.clone(), Rc::clone(editor));
    wasm_bindgen_futures::spawn_local(async move {
        for (row, lch) in pending {
            // One row's solve: in the analysis Worker when it is free, else (or when that
            // fails) on the UI thread. Written inline: a named async function would hold the
            // wasm-only (single-threaded) client in a future that is not `Send`.
            let solve = 'solved: {
                if let Some(client) = client.as_ref() {
                    let answer = client
                        .solve(
                            String::new(),
                            SolveRequest::BodyColor {
                                params: BodyColorParams { lch, path_mm },
                            },
                        )
                        .await;
                    if let Ok(SolveResponse::BodyColor(data)) = answer {
                        break 'solved Some(data.into_solve());
                    }
                    // Superseded by a sweep, a failed Worker or anything else: the editor
                    // still works.
                }
                solve_lch(lch, path_mm, &AtomicBool::new(false))
            };
            if EPOCH.with(Cell::get) != epoch {
                return;
            }
            if let Some(solve) = solve {
                if let Some(open) = editor.borrow_mut().as_mut() {
                    open.store_solve(row, solve);
                }
                push_editor(&ctx, &editor, false);
            }
        }
        // The metrics HUD may have been superseded by this job: it asks again now.
        hud::resume(&ctx);
    });
}

/// The editor opened: one row per direction of the design's host, seeded from the stored color
/// of each direction, then solved.
fn open(ctx: &Ctx, editor: &Editor) {
    let path = design_path_mm(ctx);
    let opened = ctx.state.try_borrow().ok().and_then(|app| {
        app.design.as_ref().map(|design| {
            let design = &design.session.design;
            AxisEditor::open(
                &design.material,
                host_material(design, &app.custom_materials),
                stone_width_mm(design),
                path,
            )
        })
    });
    let Some(opened) = opened else {
        return;
    };
    *editor.borrow_mut() = Some(opened);
    push_editor(ctx, editor, true);
    solve_pending(ctx, editor);
}

/// What an Apply did, as the sentence to show.
enum Applied {
    /// The design now has the color (one undo step).
    Done,
    /// Nothing changed; the sentence says why (the dialog closes after an Apply).
    Unchanged(&'static str),
    /// The editor refused the edit.
    Refused(String),
}

/// Apply color: every row as ONE `SetMaterial` that changes only the color.
fn apply(ctx: &Ctx, editor: &Editor) {
    let outcome = {
        let mut app = ctx.state.borrow_mut();
        let Some(design_state) = app.design.as_mut() else {
            return;
        };
        let editor = editor.borrow();
        let Some(editor) = editor.as_ref() else {
            return;
        };
        match editor.applied_to(&design_state.session.design.material) {
            None => Applied::Unchanged(editor.nothing_to_apply()),
            Some(material) => design_state
                .session
                .apply(Edit::SetMaterial {
                    material: Box::new(material),
                })
                .map_or_else(
                    |error| Applied::Refused(error.to_string()),
                    |_| Applied::Done,
                ),
        }
    };
    match outcome {
        Applied::Done => finish_no_tier(ctx),
        Applied::Unchanged(sentence) => show_message(ctx, MessageKind::Info, sentence),
        Applied::Refused(message) => show_message(ctx, MessageKind::Error, &message),
    }
}

/// Puts the saved `variant` on the open design as ONE `SetMaterial`, like the desktop's "Save as
/// Variant". `true` when the design now uses it (also when it already did); `false` when there
/// is no design or the editor refused the edit (then the sentence is shown).
fn use_saved_variant(ctx: &Ctx, variant: &ColorVariant) -> bool {
    let outcome = {
        let mut app = ctx.state.borrow_mut();
        let Some(design_state) = app.design.as_mut() else {
            return false;
        };
        applied_variant(&design_state.session.design.material, variant).map(|material| {
            design_state
                .session
                .apply(Edit::SetMaterial {
                    material: Box::new(material),
                })
                .map(|_| ())
                .map_err(|error| error.to_string())
        })
    };
    match outcome {
        // Already the design's color: nothing to add to the undo history.
        None => true,
        Some(Ok(())) => {
            finish_no_tier(ctx);
            true
        }
        Some(Err(message)) => {
            show_message(ctx, MessageKind::Error, &message);
            false
        }
    }
}

/// "Save as Variant...": keeps the solved rows under `name` for the design's material in
/// `IndexedDB`; the combo then lists it and the design uses it.
fn save_variant(ctx: &Ctx, editor: &Editor, name: &str) {
    let built = editor
        .borrow()
        .as_ref()
        .map(|open| open.variant_to_save(name, color_variants::fresh_key()));
    let variant = match built {
        Some(Ok(variant)) => variant,
        Some(Err(message)) => {
            show_message(ctx, MessageKind::Error, &message);
            return;
        }
        None => return,
    };
    if let Some(ui) = ctx.ui.upgrade() {
        ui.global::<DesignSettingsModel>()
            .set_lch_variant_name(SharedString::new());
    }
    let (ctx, editor) = (ctx.clone(), Rc::clone(editor));
    wasm_bindgen_futures::spawn_local(async move {
        match color_variants::save(&variant).await {
            Ok(()) => {
                let (label, material) = (variant.label.clone(), variant.material.clone());
                colors::remember_saved(variant.clone());
                let used = use_saved_variant(&ctx, &variant);
                if used {
                    // The editor now shows what the design stores: an Apply changes nothing.
                    if let Some(open) = editor.borrow_mut().as_mut() {
                        open.settle();
                    }
                }
                crate::editor::inspector::refresh(&ctx);
                let sentence = if used {
                    format!(
                        "'{label}' saved and used by this design. It is listed in the Color                          list of every {material} design in this browser."
                    )
                } else {
                    format!(
                        "'{label}' saved. It is listed in the Color list of every {material}                          design in this browser."
                    )
                };
                show_message(&ctx, MessageKind::Success, &sentence);
            }
            Err(message) => show_message(
                &ctx,
                MessageKind::Error,
                &format!("The color could not be saved: {message}"),
            ),
        }
    });
}

/// Registers the editor's callbacks.
pub(super) fn wire(ui: &AppWindow, ctx: &Ctx) {
    let model = ui.global::<DesignSettingsModel>();
    let editor: Editor = Rc::default();
    let (c, e) = (ctx.clone(), Rc::clone(&editor));
    model.on_lch_open(move || open(&c, &e));
    let (c, e) = (ctx.clone(), Rc::clone(&editor));
    model.on_lch_edited(move |tone, chroma, hue| {
        if let Some(open) = e.borrow_mut().as_mut() {
            open.set_active_lch([f64::from(tone), f64::from(chroma), f64::from(hue)]);
        }
        push_editor(&c, &e, false);
        solve_pending(&c, &e);
    });
    let (c, e) = (ctx.clone(), Rc::clone(&editor));
    model.on_lch_picked(move |picked| {
        let lch = lch_from_srgb(
            [picked.red(), picked.green(), picked.blue()].map(|v| f64::from(v) / 255.0),
        );
        if let Some(open) = e.borrow_mut().as_mut() {
            open.set_active_lch(lch);
        }
        push_editor(&c, &e, true);
        solve_pending(&c, &e);
    });
    let (c, e) = (ctx.clone(), Rc::clone(&editor));
    model.on_lch_axis_selected(move |row| {
        if let Some(open) = e.borrow_mut().as_mut() {
            open.select(usize::try_from(row).unwrap_or(0));
        }
        push_editor(&c, &e, true);
    });
    let (c, e) = (ctx.clone(), Rc::clone(&editor));
    model.on_lch_save_variant(move |name| save_variant(&c, &e, &name));
    let c = ctx.clone();
    model.on_lch_apply(move || apply(&c, &editor));
}
