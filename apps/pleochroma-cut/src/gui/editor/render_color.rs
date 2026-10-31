//! The editor's side of the Live Render toolbar's "Color" control
//! (`gui::render::render_body_color`): reading and editing the open design's body-colour
//! override while the view shows that design with the render material linked to it.
//!
//! A pick is the same edit the Edit tab's color box makes ([`Edit::SetMaterial`] with the
//! design's material and a new `body_color_override`): one undo step, the design is marked
//! changed, and the Edit tab's own controls follow through the usual panel refresh. Unlike
//! the color box it applies only the colour, never a half-typed material or RI in the
//! Design settings form.
//!
//! The Design settings color editor ("Custom...") authors one color per direction of the stone
//! (see [`AxisState`]): the design is stored as a Custom Color variant whose tensor has the host's
//! rank, so a pleochroic stone never becomes isotropic. The material editor dialog's color editor
//! has the same rows for a uniaxial or biaxial material ([`open_dialog_rows`]), held as per-ray
//! band rows until Save. "Save as Variant...", "Update" and "Nearest Variant" live in
//! [`color_variants`].

use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use pleochroma::{
    color::body_color::srgb_to_lab,
    optics::materials::{
        AbsorptionUnit, OpticalCharacter,
        variants::{TensorRank, host_rank},
    },
};
use pleochroma_cut_core::{
    Edit, MaterialSelection,
    material::{RayBandRows, color::nearest_legacy_rgb},
};
use pleochroma_editor::lch_color::{self, LchSolve};
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};

mod color_variants;

use super::{
    body_color_combo::host_material,
    state::EditorState,
    view::{refresh_editor_panel_stale, submit_preview_replan},
};
use crate::{
    BodyColorEditorModel, MainWindow, PhysicscolorModel,
    bridge::render_thread::{RenderContext, ViewColor},
    gui::{
        color_pattern::TARGET_PATTERN_ZONE,
        optics::{
            color_variant_store::saved_variants,
            crystal_optics::optical_character_from_index,
            dialog_color::{self, DialogColor},
        },
        render::render_body_color::{
            ColorChange, DesignColorPort, apply_view_choice, apply_view_color,
            setup_render_body_color_callbacks, view_color_selection, view_host,
        },
        show_toast,
        solid_preview::preview_state::{SolidLastSolved, SolidPreviewState},
    },
};

/// The selection the design would have with `change` applied (a legacy body-colour override, whose
/// L*C*h band form is dropped with it, or a color variant snapshot that replaces every legacy
/// field), or `None` when it already has exactly that colour (nothing to edit, so no undo step is
/// added). A plain color replaces the stone's pattern.
fn recoloured(material: &MaterialSelection, change: &ColorChange) -> Option<MaterialSelection> {
    match change {
        ColorChange::Legacy(color) => {
            let next = material
                .clone()
                .with_color_pattern(None)
                .with_body_color(*color);
            (next != *material).then_some(next)
        }
        ColorChange::Variant(choice) => lch_color::recoloured_with_choice(material, choice.clone()),
    }
}

/// Changes the open design's material: `change` maps the current selection to the new one
/// (`None`: nothing to change). One undo step, panel and preview refreshed.
pub(super) type MaterialEdit =
    Rc<dyn Fn(&dyn Fn(&MaterialSelection) -> Option<MaterialSelection>) -> Result<(), String>>;

/// Registers the toolbar's "Color" control against the open design.
pub(super) fn setup_render_color_callbacks(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    // `try_borrow`: the control refreshes from Slint `changed` handlers, which can run
    // while a writer elsewhere holds the editor state (a dialog pumping the event loop).
    // It then keeps what it shows and refreshes on the next change.
    let state_read = Rc::clone(state);
    let current = Box::new(move || {
        state_read
            .try_borrow()
            .ok()
            .map(|st| st.design.material.clone())
    });

    let state_edit = Rc::clone(state);
    let render_ctx_edit = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    let edit: MaterialEdit = Rc::new(move |change: &dyn Fn(
        &MaterialSelection,
    ) -> Option<MaterialSelection>|
          -> Result<(), String> {
        let Some(ui) = ui_weak.upgrade() else {
            return Ok(());
        };
        let mut st = state_edit.try_borrow_mut().map_err(|_| {
            "The design is busy right now. Try the color again in a moment.".to_string()
        })?;
        let Some(material) = change(&st.design.material) else {
            return Ok(());
        };
        st.apply(Edit::SetMaterial { material: Box::new(material) })
            .map_err(|error| error.to_string())?;
        // A material change never moves a tier's own mast.
        refresh_editor_panel_stale(&ui, &render_ctx_edit, &st, &BTreeSet::new());
        submit_preview_replan(
            &ui,
            &render_ctx_edit,
            &preview_state,
            &solid_last_solved,
            &st,
            BTreeSet::new(),
            false,
        );
        Ok(())
    });

    let edit_color = Rc::clone(&edit);
    let port = Rc::new(DesignColorPort {
        current,
        apply: Box::new(move |change| edit_color(&|material| recoloured(material, &change))),
        // The cutter's saved variants of the material the toolbar lists, read without waiting for
        // the vault (`saved_variants` keeps the list last read while a worker holds it).
        saved_variants: Box::new(saved_variants),
    });
    setup_lch_editor(ui, state, render_ctx, Rc::clone(&edit), Rc::clone(&port));
    super::stone_pattern::setup(ui, state, render_ctx, edit);
    setup_render_body_color_callbacks(ui, render_ctx, &port);
}

/// Narrows a UI value to the `f32` the Slint models hold.
#[expect(
    clippy::cast_possible_truncation,
    reason = "Slint model properties are f32; the values are lightness, chroma and hue in the normal float range"
)]
const fn to_f32(value: f64) -> f32 {
    value as f32
}

/// Pushes a finished solve (or the solves of every direction) to the editor: the swatches
/// with their captions, the badge, `busy` off.
fn push_state(
    model: &BodyColorEditorModel<'_>,
    swatches: &[[f32; 3]],
    labels: Vec<String>,
    delta_e: f64,
    reachable: bool,
) {
    let colours: Vec<slint::Color> = swatches.iter().map(|rgb| swatch_color(*rgb)).collect();
    let labels: Vec<SharedString> = labels.into_iter().map(SharedString::from).collect();
    model.set_swatches(ModelRc::new(VecModel::from(colours)));
    model.set_swatch_labels(ModelRc::new(VecModel::from(labels)));
    model.set_badge_text(lch_color::badge_text(delta_e, reachable).into());
    model.set_reachable(reachable);
    model.set_has_solution(true);
    model.set_busy(false);
}

/// An sRGB swatch (0..1) as the colour Slint paints.
fn swatch_color(rgb: [f32; 3]) -> slint::Color {
    let [r, g, b] = lch_color::swatch_bytes(rgb);
    slint::Color::from_rgb_u8(r, g, b)
}

/// Pushes a finished single-colour solve: its swatches at 3 mm, 5 mm, 10 mm and the stone's own.
fn push_solve(model: &BodyColorEditorModel<'_>, solve: &LchSolve) {
    push_state(
        model,
        &solve.swatches,
        lch_color::swatch_labels(solve.path_mm),
        solve.delta_e,
        solve.reachable,
    );
}

/// Starts one solve for `[L*, C*, h]` (the callback every slider and the picker share).
type LchStart = dyn Fn(&MainWindow, [f64; 3]);

/// Starts a solve of every colour row of the design target (see [`AxisState`]).
type AxesStart = dyn Fn(&MainWindow);

/// The stone a color editor target is solved for: its width, and the solver's reference path
/// (`lch_color::reference_path_mm`, the girdle times 1.5), both in millimetres.
///
/// A Custom Color stores the WIDTH (`custom_choice`); the solver and the editor's own swatches use
/// the PATH.
#[derive(Debug, Clone, Copy, PartialEq)]
struct StoneSize {
    /// The stone's width (mm).
    width_mm: f64,
    /// The reference path (mm) the rows are solved for.
    path_mm: f64,
}

impl StoneSize {
    /// The size of a stone whose girdle is `girdle_diameter_mm` wide (`None` or zero: the default
    /// stone).
    fn of_girdle(girdle_diameter_mm: Option<f64>) -> Self {
        Self {
            width_mm: lch_color::stone_width_mm(girdle_diameter_mm),
            path_mm: lch_color::reference_path_mm(girdle_diameter_mm),
        }
    }
}

/// The stone-size callback of the colour editor for the editor's `BodyColorEditorModel.target`
/// (one of the `TARGET_*` constants).
type SizeOf = dyn Fn(i32) -> StoneSize;

// `BodyColorEditorModel.target` 0 (anything not below) is the open design's material: Design
// settings, and the Live Render popup while it targets the design.

/// `BodyColorEditorModel.target`: "Apply colour" sets the material editor dialog's colour.
const TARGET_DIALOG: i32 = 1;
/// `BodyColorEditorModel.target`: "Apply colour" sets the Live Render popup's view-only color
/// (the legacy triple plus the seven-band rows, see [`ViewColor`]).
const TARGET_VIEW: i32 = 2;

/// Whether the editor edits the open design's color (one row per direction of the stone) rather
/// than the dialog's, the view-only color or a zone color (one single color each).
const fn is_design_target(target: i32) -> bool {
    !matches!(target, TARGET_DIALOG | TARGET_VIEW | TARGET_PATTERN_ZONE)
}

/// Whether the editor shows one row per direction of the stone for `target`: the open design's
/// color and the Live Render popup's view-only color (which edits the viewed material's colors).
const fn has_axis_rows(target: i32) -> bool {
    is_design_target(target) || target == TARGET_VIEW
}

/// Whether the editor solves `target` for the render's stone width (the material editor
/// dialog, a Color Pattern zone and the view-only color: all colored for the stone the Live
/// Render shows, whose width the library preview sets) rather than for the open design's girdle.
const fn path_target_uses_render_width(target: i32) -> bool {
    !is_design_target(target)
}

/// Whether "Apply colour" on the design keeps the stored color instead of storing the fresh
/// solve: the editor was opened on exactly `current` (the seed, untouched since) and the design
/// already has a stored color. The seed is a lossy reconstruction of that color, so an untouched
/// open must not replace it with a refit.
fn apply_keeps_stored(opened: Option<&[[f64; 3]]>, current: &[[f64; 3]], has_stored: bool) -> bool {
    has_stored && opened == Some(current)
}

/// Whether the design's selection already stores a color the editor reads back: a color variant
/// (a picked one or an earlier Custom Color) or, on a one-color host, the L*C*h editor's band rows.
///
/// On a pleochroic host those band rows are a LEGACY single color (no pleochroism): "Convert"
/// opens the editor seeded from it and an untouched Apply must store the per-axis Custom Color,
/// so they do not count there.
fn design_has_stored_color(material: &MaterialSelection, rank: TensorRank) -> bool {
    material.color_variant_override.is_some()
        || (rank == TensorRank::Isotropic
            && material
                .body_color_bands_override
                .as_ref()
                .is_some_and(|rows| !rows.is_empty()))
}
// `BodyColorEditorModel.target` 3 is a zone color of the Color Pattern window
// (`gui::color_pattern::TARGET_PATTERN_ZONE`).

/// The design target's colour rows: one `[L*, C*, h]` and its solve per direction of the stone.
///
/// The solves arrive from a worker thread and are read on the Slint thread, so the state sits
/// behind a `Mutex`.
#[derive(Clone)]
struct AxisState {
    /// The design's material resolved, so [`Self::rank`] is the host's. `false` (a design that
    /// names no material) keeps the old single legacy colour: nothing is traced anyway.
    host_known: bool,
    /// The host's rank: one, two or three rows.
    rank: TensorRank,
    /// The reference path (mm) every row is solved for: the design's.
    path_mm: f64,
    /// The stone's width (mm): what a Custom Color stores as its `reference_path_mm`.
    stone_width_mm: f64,
    /// The typed target of each row.
    lch: Vec<[f64; 3]>,
    /// The solve of each row, `None` until it finished.
    solves: Vec<Option<LchSolve>>,
    /// The row the sliders edit.
    active: usize,
    /// The seed the editor opened at (`None` once a slider or the picker moved, and for a design
    /// with no stored colour).
    opened: Option<Vec<[f64; 3]>>,
}

impl AxisState {
    /// The state of a closed editor: no rows.
    const fn closed() -> Self {
        Self {
            host_known: false,
            rank: TensorRank::Isotropic,
            path_mm: lch_color::DEFAULT_PATH_MM,
            stone_width_mm: 7.0,
            lch: Vec::new(),
            solves: Vec::new(),
            active: 0,
            opened: None,
        }
    }
}

/// The shared [`AxisState`].
type SharedAxes = Arc<Mutex<AxisState>>;

/// Locks `axes`, ignoring a poisoned lock (a panicked solve thread leaves valid data behind).
fn lock_axes(axes: &SharedAxes) -> MutexGuard<'_, AxisState> {
    axes.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A Slint string model of `items`.
fn string_model<'a>(items: impl IntoIterator<Item = &'a str>) -> ModelRc<SharedString> {
    ModelRc::new(VecModel::from(
        items
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    ))
}

/// Pushes the rows' coarse colour names ("Blue") for their typed `lch`.
fn push_families(model: &BodyColorEditorModel<'_>, lch: &[[f64; 3]]) {
    model.set_axis_families(string_model(
        lch.iter().map(|row| lch_color::color_family(*row)),
    ));
}

/// Empties the colour rows (every target but the design's has none).
fn clear_axis_rows(model: &BodyColorEditorModel<'_>) {
    model.set_axis_labels(string_model([]));
    model.set_axis_hints(string_model([]));
    model.set_axis_families(string_model([]));
    model.set_axis_swatches(ModelRc::new(VecModel::from(Vec::<slint::Color>::new())));
    model.set_axis_active(0);
}

/// Pushes a finished solve of every row: for a stone with several directions the face-up swatch
/// and one swatch per direction at the stone's size, else the plain single-colour swatches; the
/// rows' own swatches and names; `busy` off.
fn push_axes(model: &BodyColorEditorModel<'_>, axes: &AxisState) {
    let solves: Vec<LchSolve> = axes.solves.iter().flatten().cloned().collect();
    if solves.is_empty() || solves.len() != axes.lch.len() {
        model.set_busy(false);
        return;
    }
    model.set_axis_swatches(ModelRc::new(VecModel::from(
        solves
            .iter()
            .map(|solve| swatch_color(solve.swatches.last().copied().unwrap_or([1.0; 3])))
            .collect::<Vec<_>>(),
    )));
    push_families(model, &axes.lch);
    let rows = lch_color::axis_rows(axes.rank);
    let tensor = (rows.len() > 1 && axes.host_known)
        .then(|| lch_color::tensor_from_solves(axes.rank, &solves))
        .flatten();
    let Some(tensor) = tensor else {
        push_solve(model, &solves[0]);
        return;
    };
    let swatches = lch_color::direction_swatches(&tensor, axes.path_mm);
    let labels = std::iter::once("Face-Up")
        .chain(rows.iter().map(|row| row.short))
        .map(str::to_owned)
        .collect();
    let worst = solves.iter().map(|solve| solve.delta_e).fold(0.0, f64::max);
    let reachable = solves.iter().all(|solve| solve.reachable);
    push_state(model, &swatches, labels, worst, reachable);
}

/// Builds the "start a solve of every colour row" callback of the design target.
///
/// It shares `latest` and `running` with [`lch_start_solver`], so a newer request of either kind
/// supersedes an older one. The solve runs on a background thread; a row whose target did not
/// change is not solved again (`lch_color::solve_axes`). When still the newest, the result is
/// stored in `axes` and pushed.
fn axes_start_solver(
    latest: Arc<AtomicU64>,
    running: Rc<RefCell<Arc<AtomicBool>>>,
    axes: SharedAxes,
) -> Rc<AxesStart> {
    Rc::new(move |ui: &MainWindow| {
        let ticket = latest.fetch_add(1, Ordering::SeqCst) + 1;
        let flag = Arc::new(AtomicBool::new(false));
        running
            .replace(Arc::clone(&flag))
            .store(true, Ordering::SeqCst);
        let model = ui.global::<BodyColorEditorModel>();
        let (lch, previous, path) = {
            let state = lock_axes(&axes);
            (state.lch.clone(), state.solves.clone(), state.path_mm)
        };
        if lch.is_empty() {
            model.set_busy(false);
            return;
        }
        model.set_busy(true);
        let (latest, axes, ui_weak) = (Arc::clone(&latest), Arc::clone(&axes), ui.as_weak());
        std::thread::spawn(move || {
            let solved = lch_color::solve_axes(&lch, &previous, path, &flag);
            let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                if latest.load(Ordering::SeqCst) != ticket {
                    return;
                }
                let model = ui.global::<BodyColorEditorModel>();
                let Some(solved) = solved else {
                    model.set_busy(false);
                    return;
                };
                let shown = {
                    let mut state = lock_axes(&axes);
                    state.solves = solved.into_iter().map(Some).collect();
                    state.clone()
                };
                push_axes(&model, &shown);
            });
        });
    })
}

/// What a color-editor target other than the design's solves into: the last finished solve.
type LastSolve = Arc<Mutex<Option<LchSolve>>>;

/// Builds the shared "start a solve" callback of the colour editor for the targets with one
/// colour (the material editor dialog, the view-only color, a zone color).
///
/// Each call supersedes the previous one (`latest` ticket, `running` cancel flag), solves on
/// a background thread and, when still the newest, pushes the result into the model and
/// `last`.
fn lch_start_solver(
    latest: Arc<AtomicU64>,
    running: Rc<RefCell<Arc<AtomicBool>>>,
    last: LastSolve,
    size_of: Rc<SizeOf>,
) -> Rc<LchStart> {
    Rc::new(move |ui: &MainWindow, lch: [f64; 3]| {
        let ticket = latest.fetch_add(1, Ordering::SeqCst) + 1;
        let flag = Arc::new(AtomicBool::new(false));
        running
            .replace(Arc::clone(&flag))
            .store(true, Ordering::SeqCst);
        let model = ui.global::<BodyColorEditorModel>();
        let path = size_of(model.get_target()).path_mm;
        model.set_busy(true);
        let (latest, last, ui_weak) = (Arc::clone(&latest), Arc::clone(&last), ui.as_weak());
        std::thread::spawn(move || {
            let solve = lch_color::solve_lch(lch, path, &flag);
            let _ = ui_weak.upgrade_in_event_loop(move |ui| {
                if latest.load(Ordering::SeqCst) != ticket {
                    return;
                }
                let model = ui.global::<BodyColorEditorModel>();
                let Some(solve) = solve else {
                    model.set_busy(false);
                    return;
                };
                push_solve(&model, &solve);
                *last.lock().unwrap_or_else(PoisonError::into_inner) = Some(solve);
            });
        });
    })
}

/// The Design settings colour editor (the combo's "Custom..." entry): Tone / Saturation / Hue
/// sliders and the hue-saturation-brightness picker feed ONE solver ([`lch_color::solve_lch`])
/// that runs on a background thread for the design's reference path; a newer request cancels
/// and supersedes an older one that is still running. Nothing reaches the design until
/// "Apply colour", which stores the last finished solve as ONE undoable edit through `edit`.
///
/// For the open design the editor has one colour row per direction of the stone (one for a
/// material that looks the same in every direction, two for a uniaxial and three for a biaxial
/// one): the sliders edit the chosen row, and "Apply Color" stores all rows together as one
/// Custom Color whose tensor has the host's rank. The dialog, the view-only color and a zone
/// color keep their single color.
///
/// No `changed` handler here touches the editor state: the callbacks only read it with
/// `try_borrow` (the design's girdle and material) and write Slint properties, so the
/// re-entrancy rule of `slint-changed-handler-reentrancy` is not at risk.
fn setup_lch_editor(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    edit: MaterialEdit,
    port: Rc<DesignColorPort>,
) {
    let latest = Arc::new(AtomicU64::new(0));
    let running: Rc<RefCell<Arc<AtomicBool>>> = Rc::new(RefCell::new(Arc::default()));
    let last: LastSolve = Arc::default();
    let axes: SharedAxes = Arc::new(Mutex::new(AxisState::closed()));

    // The dialog, a pattern zone and the view-only color take the render's stone width; the
    // design targets take the open design's girdle diameter.
    let state_size = Rc::clone(state);
    let ctx_size = Arc::clone(render_ctx);
    let size_of: Rc<SizeOf> = Rc::new(move |target: i32| {
        if path_target_uses_render_width(target) {
            let width = RenderContext::lock(&ctx_size).stone_width_mm;
            return StoneSize::of_girdle(Some(f64::from(width)));
        }
        let girdle = state_size
            .try_borrow()
            .ok()
            .and_then(|st| st.design.girdle_diameter_mm);
        StoneSize::of_girdle(girdle)
    });

    let start_axes = axes_start_solver(Arc::clone(&latest), Rc::clone(&running), Arc::clone(&axes));
    let start = lch_start_solver(latest, running, Arc::clone(&last), Rc::clone(&size_of));

    register_lch_open(
        ui,
        LchOpenPorts {
            state: Rc::clone(state),
            render_ctx: Arc::clone(render_ctx),
            start: Rc::clone(&start),
            start_axes: Rc::clone(&start_axes),
            last: Arc::clone(&last),
            size_of,
            axes: Arc::clone(&axes),
        },
    );
    register_lch_edits(ui, &start, &start_axes, &axes);
    register_axis_selected(ui, &axes);

    // "Save as Variant...", the suggested name, "Update" and "Nearest Variant".
    color_variants::setup(ui, state, render_ctx, &edit, &axes);

    register_lch_apply(
        ui,
        LchApplyPorts {
            render_ctx: Arc::clone(render_ctx),
            edit,
            port,
            axes,
        },
        last,
    );
}

/// Registers the sliders and the picker: both edit the chosen row of the stone (or the single
/// color of the other targets) and start a solve.
fn register_lch_edits(
    ui: &MainWindow,
    start: &Rc<LchStart>,
    start_axes: &Rc<AxesStart>,
    axes: &SharedAxes,
) {
    let model = ui.global::<BodyColorEditorModel>();
    let (start_edit, start_axes_edit, ui_weak, axes_edit) = (
        Rc::clone(start),
        Rc::clone(start_axes),
        ui.as_weak(),
        Arc::clone(axes),
    );
    model.on_lch_edited(move |tone, chroma, hue| {
        if let Some(ui) = ui_weak.upgrade() {
            let lch = [f64::from(tone), f64::from(chroma), f64::from(hue)];
            let target = ui.global::<BodyColorEditorModel>().get_target();
            if rows_active(target, &axes_edit) {
                edit_active_row(&ui, &axes_edit, lch);
                start_axes_edit(&ui);
            } else {
                start_edit(&ui, lch);
            }
        }
    });

    let (start, start_axes, axes_pick, ui_weak) = (
        Rc::clone(start),
        Rc::clone(start_axes),
        Arc::clone(axes),
        ui.as_weak(),
    );
    model.on_picker_picked(move |picked| {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let [l, c, h] = lch_color::lch_from_srgb([
            f64::from(picked.red()) / 255.0,
            f64::from(picked.green()) / 255.0,
            f64::from(picked.blue()) / 255.0,
        ]);
        let model = ui.global::<BodyColorEditorModel>();
        model.set_tone(to_f32(l));
        model.set_chroma(to_f32(c));
        model.set_hue(to_f32(h));
        if rows_active(model.get_target(), &axes_pick) {
            edit_active_row(&ui, &axes_pick, [l, c, h]);
            start_axes(&ui);
        } else {
            start(&ui, [l, c, h]);
        }
    });
}

/// Registers the row chips ("Along", "Across", ...): the sliders move to the chosen row's color.
fn register_axis_selected(ui: &MainWindow, axes: &SharedAxes) {
    let model = ui.global::<BodyColorEditorModel>();
    let (axes_select, ui_weak) = (Arc::clone(axes), ui.as_weak());
    model.on_axis_selected(move |row| {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let Ok(row) = usize::try_from(row) else {
            return;
        };
        let lch = {
            let mut state = lock_axes(&axes_select);
            let Some(lch) = state.lch.get(row).copied() else {
                return;
            };
            state.active = row;
            lch
        };
        let model = ui.global::<BodyColorEditorModel>();
        model.set_axis_active(i32::try_from(row).unwrap_or(0));
        model.set_tone(to_f32(lch[0]));
        model.set_chroma(to_f32(lch[1]));
        model.set_hue(to_f32(lch[2]));
    });
}

/// Stores `lch` as the typed colour of the chosen row (the sliders or the picker moved): the
/// untouched-open guard is off and the row's name follows.
fn edit_active_row(ui: &MainWindow, axes: &SharedAxes, lch: [f64; 3]) {
    let mut state = lock_axes(axes);
    let active = state.active;
    if let Some(row) = state.lch.get_mut(active) {
        *row = lch;
    }
    state.opened = None;
    push_families(&ui.global::<BodyColorEditorModel>(), &state.lch);
}

/// What "Apply colour" writes through: the open design (`edit`, `axes`) and the view-only color
/// (`render_ctx`, `port`).
struct LchApplyPorts {
    render_ctx: Arc<Mutex<RenderContext>>,
    edit: MaterialEdit,
    port: Rc<DesignColorPort>,
    axes: SharedAxes,
}

/// Registers "Apply colour": stores the last finished solve through the target's own path.
fn register_lch_apply(ui: &MainWindow, ports: LchApplyPorts, last: LastSolve) {
    let LchApplyPorts {
        render_ctx,
        edit,
        port,
        axes,
    } = ports;
    let model = ui.global::<BodyColorEditorModel>();
    let ui_weak = ui.as_weak();
    model.on_apply_requested(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let target = ui.global::<BodyColorEditorModel>().get_target();
        if is_design_target(target) {
            apply_design_color(&ui, &edit, &axes);
            return;
        }
        if target == TARGET_VIEW {
            apply_view_rows(&ui, &render_ctx, &port, &axes);
            return;
        }
        if target == TARGET_DIALOG && rows_active(target, &axes) {
            apply_dialog_rows(&ui, &axes);
            return;
        }
        let solve = last.lock().unwrap_or_else(PoisonError::into_inner).clone();
        let Some(solve) = solve else {
            return;
        };
        let [r, g, b] = solve.triple;
        match target {
            TARGET_DIALOG => {
                // The bands wait in the dialog until Save; the triple goes through the same
                // path as the old picker's result (fantasy colour, preset selection).
                dialog_color::set(DialogColor {
                    bands: (!solve.bands.is_empty()).then(|| solve.bands.clone()),
                    triple: Some(solve.triple),
                    rays: None,
                });
                ui.global::<PhysicscolorModel>()
                    .invoke_fixed_color_picked(r, g, b);
            }
            // The view-only color has its rows and is applied above (`apply_view_rows`).
            TARGET_VIEW => {}
            // A zone color of the Color Pattern window: the solve goes to the zone that asked.
            _ => crate::gui::color_pattern::picked(&solve),
        }
    });
}

/// The rows "Apply Color" stores, as the editor holds them.
struct AppliedRows<'a> {
    /// The design's material is known (see [`AxisState::host_known`]).
    host_known: bool,
    /// The host's rank.
    rank: TensorRank,
    /// The stone's width (mm), stored as the Custom Color's `reference_path_mm`.
    stone_width_mm: f64,
    /// The typed target of each row.
    lch: &'a [[f64; 3]],
    /// The finished solve of each row.
    solves: &'a [LchSolve],
    /// The seed the editor opened at, while no slider or picker has moved ([`AxisState::opened`]).
    opened: Option<&'a [[f64; 3]]>,
}

/// The selection "Apply Color" makes of `material`: `None` when there is nothing to store (the
/// editor was opened on a stored color and left alone, or the design already has exactly that).
///
/// A design whose material is known gets a Custom Color with the host's rank
/// (`lch_color::custom_choice`) that replaces the legacy fields and the pattern; one without keeps
/// the old single legacy color. An untouched open keeps a stored color ([`apply_keeps_stored`]),
/// but not a legacy single color on a pleochroic host: "Convert" starts from it and an untouched
/// Apply stores the per-axis color ([`design_has_stored_color`]).
fn applied_selection(
    material: &MaterialSelection,
    rows: &AppliedRows<'_>,
) -> Option<MaterialSelection> {
    if apply_keeps_stored(
        rows.opened,
        rows.lch,
        design_has_stored_color(material, rows.rank),
    ) {
        return None;
    }
    let base = material.clone().with_color_pattern(None);
    let stored = if rows.host_known {
        lch_color::custom_choice(rows.rank, rows.solves, rows.stone_width_mm)
            .and_then(|choice| lch_color::recoloured_with_choice(&base, choice))
    } else {
        rows.solves
            .first()
            .and_then(|solve| lch_color::recoloured_with_solve(&base, solve))
    };
    let next = stored.unwrap_or(base);
    (next != *material).then_some(next)
}

/// "Apply Color" of the Live Render popup's view-only color: every row's last finished solve as
/// one Custom Color of the viewed material (`apply_view_choice`, which routes to the design when
/// the view changed under the popup); a material that cannot be resolved keeps the single legacy
/// color. An untouched open keeps the stored color ([`apply_keeps_stored`]).
fn apply_view_rows(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    port: &DesignColorPort,
    axes: &SharedAxes,
) {
    let (host_known, rank, width, lch, solves, opened) = {
        let state = lock_axes(axes);
        (
            state.host_known,
            state.rank,
            state.stone_width_mm,
            state.lch.clone(),
            state.solves.clone(),
            state.opened.clone(),
        )
    };
    let solves: Vec<LchSolve> = solves.into_iter().flatten().collect();
    if solves.is_empty() || solves.len() != lch.len() {
        return;
    }
    if apply_keeps_stored(
        opened.as_deref(),
        &lch,
        design_has_stored_color(&view_color_selection(render_ctx), rank),
    ) {
        return;
    }
    match host_known
        .then(|| lch_color::custom_choice(rank, &solves, width))
        .flatten()
    {
        Some(choice) => apply_view_choice(ui, render_ctx, port, choice),
        None => apply_view_color(
            ui,
            render_ctx,
            port,
            ViewColor {
                triple: solves[0].triple,
                bands: (!solves[0].bands.is_empty()).then(|| solves[0].bands.clone()),
                variant: None,
            },
        ),
    }
}

/// "Apply Color" on the open design: every row's last finished solve as ONE undoable edit
/// ([`applied_selection`]).
fn apply_design_color(ui: &MainWindow, edit: &MaterialEdit, axes: &SharedAxes) {
    let (host_known, rank, width, lch, solves, opened) = {
        let state = lock_axes(axes);
        (
            state.host_known,
            state.rank,
            state.stone_width_mm,
            state.lch.clone(),
            state.solves.clone(),
            state.opened.clone(),
        )
    };
    let solves: Vec<LchSolve> = solves.into_iter().flatten().collect();
    if solves.is_empty() || solves.len() != lch.len() {
        return;
    }
    let rows = AppliedRows {
        host_known,
        rank,
        stone_width_mm: width,
        lch: &lch,
        solves: &solves,
        opened: opened.as_deref(),
    };
    if let Err(message) = edit(&|material| applied_selection(material, &rows)) {
        show_toast(ui, &message, "error");
    }
}

/// What the "open" callback needs: the design, the render context (custom materials, stone
/// width), the solvers and the shared states.
struct LchOpenPorts {
    state: Rc<RefCell<EditorState>>,
    render_ctx: Arc<Mutex<RenderContext>>,
    start: Rc<LchStart>,
    start_axes: Rc<AxesStart>,
    last: LastSolve,
    size_of: Rc<SizeOf>,
    axes: SharedAxes,
}

/// Opens the rows of the design target or the view-only color ([`has_axis_rows`]): seeds one
/// `[L*, C*, h]` per direction of the host (`lch_color::seed_axis_lch`: the stored color, a legacy
/// color on every row, or the material's own tensor direction by direction), shows the rows and
/// solves them. The design's host is the design's material; the view's is the material the Live
/// Render shows with the view-only color it holds.
fn open_design_rows(ui: &MainWindow, ports: &LchOpenPorts, target: i32, size: StoneSize) {
    let model = ui.global::<BodyColorEditorModel>();
    let (material, host) = if target == TARGET_VIEW {
        (
            view_color_selection(&ports.render_ctx),
            view_host(&ports.render_ctx),
        )
    } else {
        ports.state.try_borrow().ok().map_or_else(
            || (MaterialSelection::none(), None),
            |st| {
                let ctx = RenderContext::lock(&ports.render_ctx);
                (
                    st.design.material.clone(),
                    host_material(&st.design, &ctx.custom_materials),
                )
            },
        )
    };
    let (rank, host_known) = host
        .as_ref()
        .map_or((TensorRank::Isotropic, false), |host| {
            (host_rank(host), true)
        });
    let seeded: Vec<[f64; 3]> = host.as_ref().map_or_else(
        || {
            lch_color::lch_from_material(&material, size.path_mm)
                .into_iter()
                .collect()
        },
        |host| lch_color::seed_axis_lch(&material, host, size.width_mm, size.path_mm),
    );
    // "Save as Variant..." needs the material the variant belongs to, and belongs to a design.
    model.set_can_save_variant(host_known && is_design_target(target));
    show_rows(
        ui,
        ports,
        RowsOpening {
            rank,
            host_known,
            size,
            seeded,
        },
    );
}

/// What a row editor opens on.
struct RowsOpening {
    /// The rank whose rows are shown.
    rank: TensorRank,
    /// The material the rows belong to is known (the design's; the dialog's is always known).
    host_known: bool,
    /// The stone the rows are solved for.
    size: StoneSize,
    /// The seed: one `[L*, C*, h]` per row, or any other length for "none" (every row then starts
    /// at the sliders' own value).
    seeded: Vec<[f64; 3]>,
}

/// Shows one color row per direction of `opening.rank`, seeded, and solves them.
fn show_rows(ui: &MainWindow, ports: &LchOpenPorts, opening: RowsOpening) {
    let RowsOpening {
        rank,
        host_known,
        size,
        seeded,
    } = opening;
    let model = ui.global::<BodyColorEditorModel>();
    let row_count = lch_color::axis_rows(rank).len();
    // The seed this open starts at; a slider or the picker clears it. Without one (a design with
    // no color on a material that cannot be resolved) every row starts at the sliders' own value.
    let (opened, lch) = if seeded.len() == row_count {
        (Some(seeded.clone()), seeded)
    } else {
        let own = [
            f64::from(model.get_tone()),
            f64::from(model.get_chroma()),
            f64::from(model.get_hue()),
        ];
        (None, vec![own; row_count])
    };
    *lock_axes(&ports.axes) = AxisState {
        host_known,
        rank,
        path_mm: size.path_mm,
        stone_width_mm: size.width_mm,
        lch: lch.clone(),
        solves: vec![None; row_count],
        active: 0,
        opened,
    };
    let rows = lch_color::axis_rows(rank);
    model.set_axis_labels(string_model(rows.iter().map(|row| row.label)));
    model.set_axis_hints(string_model(rows.iter().map(|row| row.hint)));
    model.set_axis_swatches(ModelRc::new(VecModel::from(vec![
        slint::Color::from_argb_u8(
            0, 0, 0, 0
        );
        row_count
    ])));
    push_families(&model, &lch);
    model.set_axis_active(0);
    model.set_tone(to_f32(lch[0][0]));
    model.set_chroma(to_f32(lch[0][1]));
    model.set_hue(to_f32(lch[0][2]));
    (ports.start_axes)(ui);
}

/// The rank of the material editor dialog's material, from its optical-character combo index.
fn dialog_rank(character_idx: i32) -> TensorRank {
    match optical_character_from_index(character_idx) {
        Some(OpticalCharacter::UniaxialPositive | OpticalCharacter::UniaxialNegative) => {
            TensorRank::Uniaxial
        }
        Some(OpticalCharacter::BiaxialPositive | OpticalCharacter::BiaxialNegative) => {
            TensorRank::Biaxial
        }
        _ => TensorRank::Isotropic,
    }
}

/// The rows the dialog's color editor opens on for a material of `rank`: the held per-direction
/// color, else the held single color on every row (what "Convert" does for a design), else none.
fn dialog_seed(held: &DialogColor, rank: TensorRank, size: StoneSize) -> Vec<[f64; 3]> {
    let rows = lch_color::axis_rows(rank).len();
    if let Some(rays) = dialog_color::rays_for(held, rows) {
        // The dialog's per-ray rows are per millimetre, like a Custom Color.
        return lch_color::axis_lch_from_tensor(
            &rays.to_tensor(),
            AbsorptionUnit::PerMm,
            rank,
            size.width_mm,
            size.path_mm,
        );
    }
    lch_color::lch_from_material(&dialog_color::as_selection(held), size.path_mm)
        .map(|lch| vec![lch; rows])
        .unwrap_or_default()
}

/// Opens the dialog target's rows for the optical character at `character_idx`: one row per
/// direction of a uniaxial or biaxial material. Returns `false` for an isotropic one, which keeps
/// the single color editor.
fn open_dialog_rows(
    ui: &MainWindow,
    ports: &LchOpenPorts,
    character_idx: i32,
    size: StoneSize,
) -> bool {
    let rank = dialog_rank(character_idx);
    if lch_color::axis_rows(rank).len() < 2 {
        return false;
    }
    let seeded = dialog_seed(&dialog_color::current(), rank, size);
    show_rows(
        ui,
        ports,
        RowsOpening {
            rank,
            host_known: true,
            size,
            seeded,
        },
    );
    true
}

/// The color the dialog holds after "Apply Color" on a uniaxial or biaxial material: the rows of
/// every direction as per-ray band rows, and the nearest legacy triple of the face-up color (what
/// an older build shows). `None` when the solves do not match `rank`.
fn dialog_rows(rank: TensorRank, solves: &[LchSolve], path_mm: f64) -> Option<DialogColor> {
    let tensor = lch_color::tensor_from_solves(rank, solves)?;
    let face_up = lch_color::direction_swatches(&tensor, path_mm)
        .first()
        .copied()?;
    let triple = nearest_legacy_rgb(srgb_to_lab(face_up.map(f64::from)));
    // A colorless stone has no rows to keep: the triple alone describes it.
    let rays = RayBandRows::from_tensor(&tensor).filter(|rays| {
        !rays.o.is_empty()
            && !rays.e.is_empty()
            && rays.beta.as_ref().is_none_or(|beta| !beta.is_empty())
    });
    Some(DialogColor {
        bands: None,
        triple: Some(triple),
        rays,
    })
}

/// "Apply Color" of the dialog's row editor: the rows are held by the dialog until Save, the
/// triple goes through the same path as the single editor's result.
fn apply_dialog_rows(ui: &MainWindow, axes: &SharedAxes) {
    let (rank, path, rows, solves) = {
        let state = lock_axes(axes);
        (
            state.rank,
            state.path_mm,
            state.lch.len(),
            state.solves.clone(),
        )
    };
    let solves: Vec<LchSolve> = solves.into_iter().flatten().collect();
    if solves.is_empty() || solves.len() != rows {
        return;
    }
    let Some(held) = dialog_rows(rank, &solves, path) else {
        return;
    };
    let [r, g, b] = held.triple.unwrap_or_default();
    dialog_color::set(held);
    ui.global::<PhysicscolorModel>()
        .invoke_fixed_color_picked(r, g, b);
}

/// Whether the sliders and the picker edit one row per direction now: always for the design's
/// color and the view-only color ([`has_axis_rows`]), and for the material editor dialog while its
/// rows are open (a uniaxial or biaxial material).
fn rows_active(target: i32, axes: &SharedAxes) -> bool {
    has_axis_rows(target) || (target == TARGET_DIALOG && !lock_axes(axes).lch.is_empty())
}

/// Registers the colour editor's "open" callback: seeds the sliders from the target's stored
/// colour (the editor's own default for a design with the material's colour), forgets the
/// previous solve and solves.
fn register_lch_open(ui: &MainWindow, ports: LchOpenPorts) {
    let model = ui.global::<BodyColorEditorModel>();
    let ui_weak = ui.as_weak();
    model.on_open_requested(move || {
        let Some(ui) = ui_weak.upgrade() else {
            return;
        };
        let model = ui.global::<BodyColorEditorModel>();
        *ports.last.lock().unwrap_or_else(PoisonError::into_inner) = None;
        model.set_has_solution(false);
        let target = model.get_target();
        let size = (ports.size_of)(target);
        if has_axis_rows(target) {
            open_design_rows(&ui, &ports, target, size);
            return;
        }
        // The other targets are not saved as a variant of a material.
        model.set_can_save_variant(false);
        // The material editor dialog's material has rows too while it is uniaxial or biaxial.
        if target == TARGET_DIALOG
            && open_dialog_rows(&ui, &ports, model.get_dialog_character_idx(), size)
        {
            return;
        }
        // The other targets have one colour and no rows.
        *lock_axes(&ports.axes) = AxisState::closed();
        clear_axis_rows(&model);
        let seeded = match target {
            TARGET_DIALOG => lch_color::lch_from_material(
                &dialog_color::as_selection(&dialog_color::current()),
                size.path_mm,
            ),
            // (The view-only color has rows and opens above.)
            _ => crate::gui::color_pattern::pick_seed(),
        };
        let [l, c, h] = seeded.unwrap_or_else(|| {
            [
                f64::from(model.get_tone()),
                f64::from(model.get_chroma()),
                f64::from(model.get_hue()),
            ]
        });
        model.set_tone(to_f32(l));
        model.set_chroma(to_f32(c));
        model.set_hue(to_f32(h));
        (ports.start)(&ui, [l, c, h]);
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma::optics::materials::GemMaterial;

    const YELLOW: [f32; 3] = [0.2, 0.4, 2.8];

    fn sapphire() -> MaterialSelection {
        MaterialSelection {
            name: Some("Sapphire".to_string()),
            specific_gravity_override: Some(4.0),
            refractive_index_override: Some(1.77),
            body_color_override: None,
            body_color_bands_override: None,
            absorption_path_scale_override: None,
            color_pattern_override: None,
            color_variant_override: None,
        }
    }

    #[test]
    fn a_new_colour_changes_only_the_colour() {
        let before = sapphire();
        let after = recoloured(&before, &ColorChange::Legacy(Some(YELLOW)))
            .expect("a different colour is an edit");
        assert_eq!(after.body_color_override, Some(YELLOW));
        assert_eq!(after.name, before.name);
        assert_eq!(after.specific_gravity_override, Some(4.0));
        assert_eq!(after.refractive_index_override, Some(1.77));
    }

    #[test]
    fn material_default_clears_the_colour() {
        let before = sapphire().with_body_color(Some(YELLOW));
        let after =
            recoloured(&before, &ColorChange::Legacy(None)).expect("clearing a colour is an edit");
        assert_eq!(after.body_color_override, None);
    }

    /// The editor's copy of the physics colour editor's reference-path convention.
    #[test]
    fn the_lch_reference_path_matches_the_physics_editors() {
        use crate::gui::optics::physics_state::default_reference_path_mm;
        for width in [0.0f32, 6.0, 9.5] {
            let ours = lch_color::reference_path_mm(Some(f64::from(width)));
            let theirs = f64::from(default_reference_path_mm(width));
            assert!((ours - theirs).abs() < 1e-4, "{width}: {ours} vs {theirs}");
        }
        assert!(
            (lch_color::reference_path_mm(None) - f64::from(default_reference_path_mm(0.0))).abs()
                < 1e-9
        );
    }

    #[test]
    fn a_toolbar_pick_drops_the_lch_editors_bands() {
        let banded = sapphire().with_body_color_bands(
            Some(YELLOW),
            Some(vec![[460.0, 45.0, 0.25]]),
            Some(2.0),
        );
        let after = recoloured(&banded, &ColorChange::Legacy(Some(YELLOW)))
            .expect("dropping the bands is an edit");
        assert_eq!(after.body_color_override, Some(YELLOW));
        assert_eq!(after.body_color_bands_override, None);
        assert_eq!(after.absorption_path_scale_override, None);
        let cleared = recoloured(&banded, &ColorChange::Legacy(None)).expect("clearing is an edit");
        assert_eq!(cleared.body_color_bands_override, None);
    }

    #[test]
    fn the_view_only_color_is_solved_for_the_renders_stone_width() {
        assert!(path_target_uses_render_width(TARGET_VIEW));
        assert!(path_target_uses_render_width(TARGET_DIALOG));
        assert!(path_target_uses_render_width(TARGET_PATTERN_ZONE));
        assert!(
            !path_target_uses_render_width(0),
            "the design target takes the open design's girdle"
        );
        assert!(!path_target_uses_render_width(-1));
    }

    #[test]
    fn only_the_design_target_is_the_designs_own_color() {
        assert!(is_design_target(0));
        assert!(!is_design_target(TARGET_DIALOG));
        assert!(!is_design_target(TARGET_VIEW));
        assert!(!is_design_target(TARGET_PATTERN_ZONE));
    }

    #[test]
    fn an_untouched_open_keeps_the_stored_color_and_every_other_apply_stores_the_solve() {
        let rows = [[62.0, 41.5, 273.25], [55.0, 30.0, 140.0]];
        // Opened here, untouched, a color stored: keep it.
        assert!(apply_keeps_stored(Some(&rows), &rows, true));
        // The user moved a slider or the picker (opened cleared), or a row differs.
        assert!(!apply_keeps_stored(None, &rows, true));
        let moved = [[62.0, 41.5, 273.5], [55.0, 30.0, 140.0]];
        assert!(!apply_keeps_stored(Some(&rows), &moved, true));
        // A design with no stored color takes the first solve even when untouched.
        assert!(!apply_keeps_stored(Some(&rows), &rows, false));
    }

    #[test]
    fn a_variant_counts_as_a_stored_color_and_band_rows_only_on_a_one_color_host() {
        use pleochroma::optics::absorption::AbsorptionTensor;
        use pleochroma_cut_core::material::VariantChoice;
        let uniaxial = TensorRank::Uniaxial;
        assert!(!design_has_stored_color(&sapphire(), uniaxial));
        assert!(!design_has_stored_color(
            &sapphire().with_body_color(Some(YELLOW)),
            uniaxial
        ));
        let banded =
            sapphire().with_body_color_bands(Some(YELLOW), Some(vec![[460.0, 3000.0, 0.25]]), None);
        // A one-color host keeps its band rows through an untouched open ...
        assert!(design_has_stored_color(&banded, TensorRank::Isotropic));
        // ... a pleochroic host's band rows are a legacy single color that "Convert" replaces.
        assert!(!design_has_stored_color(&banded, uniaxial));
        let variant = sapphire().with_color_variant(Some(VariantChoice {
            key: "custom".to_owned(),
            label: "Custom Color".to_owned(),
            absorption: AbsorptionTensor::uniaxial(Vec::new(), Vec::new()),
            unit: AbsorptionUnit::PerMm,
            reference_path_mm: 7.0,
        }));
        assert!(design_has_stored_color(&variant, uniaxial));
        assert!(design_has_stored_color(&variant, TensorRank::Isotropic));
    }

    /// A solve that never ran the solver: one energy band, the typed `lch`.
    fn fake_solve(lch: [f64; 3]) -> LchSolve {
        LchSolve {
            lch,
            path_mm: 10.5,
            delta_e: 0.0,
            reachable: true,
            bands: vec![[460.0, 1500.0, 0.2]],
            triple: [0.2, 0.4, 1.0],
            swatches: vec![[0.5, 0.5, 0.5]],
        }
    }

    /// The rows an untouched "Convert" holds: the seed, solved as it stands.
    fn converted(material: &MaterialSelection, host: &GemMaterial) -> Option<MaterialSelection> {
        let seed = lch_color::seed_axis_lch(material, host, 7.0, 10.5);
        let solves: Vec<LchSolve> = seed.iter().copied().map(fake_solve).collect();
        applied_selection(
            material,
            &AppliedRows {
                host_known: true,
                rank: host_rank(host),
                stone_width_mm: 7.0,
                lch: &seed,
                solves: &solves,
                opened: Some(&seed),
            },
        )
    }

    /// Web review, desktop side: "Convert" on a legacy single color (a preset triple, or the old
    /// band rows) of a pleochroic host stores a per-axis Custom Color even when the editor was
    /// not touched, and keeps it pleochroic; the legacy fields and the pattern are gone.
    #[test]
    fn an_untouched_convert_stores_a_per_axis_custom_color() {
        let host = GemMaterial::sapphire();
        let banded =
            sapphire().with_body_color_bands(Some(YELLOW), Some(vec![[460.0, 3000.0, 0.25]]), None);
        for legacy in [sapphire().with_body_color(Some(YELLOW)), banded] {
            let stored = converted(&legacy, &host).expect("Convert is an edit");
            let choice = stored.color_variant_override.expect("a Custom Color");
            assert_eq!(
                choice.key,
                pleochroma_cut_core::material::CUSTOM_VARIANT_KEY
            );
            assert!(choice.absorption.is_pleochroic);
            assert_eq!(choice.unit, AbsorptionUnit::PerMm);
            assert!(
                (choice.reference_path_mm - 7.0).abs() < 1e-6,
                "a stone width"
            );
            assert_eq!(stored.body_color_override, None);
            assert_eq!(stored.body_color_bands_override, None);
        }
    }

    /// An untouched open on a stored Custom Color (a variant) is not an edit, and neither is one
    /// on the band rows of a one-color host.
    #[test]
    fn an_untouched_open_on_a_stored_color_stores_nothing() {
        let host = GemMaterial::sapphire();
        let legacy = sapphire().with_body_color(Some(YELLOW));
        let stored = converted(&legacy, &host).expect("Convert is an edit");
        assert_eq!(converted(&stored, &host), None, "the Custom Color stays");
        let diamond = GemMaterial::diamond();
        let banded = MaterialSelection {
            name: Some("Diamond".to_owned()),
            ..MaterialSelection::none()
        }
        .with_body_color_bands(Some(YELLOW), Some(vec![[460.0, 3000.0, 0.25]]), None);
        assert_eq!(converted(&banded, &diamond), None);
    }

    /// The editor opens on the host's own color, not black (review finding 1): the seed of a plain
    /// Sapphire is its two rays at the face-up path, and "Apply Color" on the untouched open of a
    /// plain design stores exactly those colors (a Custom Color the first row of which is light).
    #[test]
    fn the_seed_of_a_plain_built_in_is_not_black() {
        let host = GemMaterial::sapphire();
        let size = StoneSize::of_girdle(Some(7.0));
        let seed = lch_color::seed_axis_lch(&sapphire(), &host, size.width_mm, size.path_mm);
        assert_eq!(seed.len(), 2);
        assert!(seed.iter().all(|row| row[0] > 20.0), "{seed:?}");
    }

    #[test]
    fn the_stone_size_of_a_girdle_and_of_no_girdle() {
        let sized = StoneSize::of_girdle(Some(8.0));
        assert!((sized.width_mm - 8.0).abs() < 1e-12);
        assert!((sized.path_mm - 12.0).abs() < 1e-12);
        let unset = StoneSize::of_girdle(None);
        assert!((unset.width_mm - 7.0).abs() < 1e-12);
        assert!((unset.path_mm - lch_color::DEFAULT_PATH_MM).abs() < 1e-12);
        assert_eq!(StoneSize::of_girdle(Some(0.0)), unset);
    }

    /// The dialog's optical-character combo (isotropic, uniaxial +/-, biaxial +/-) gives the rows.
    #[test]
    fn the_dialogs_optical_character_decides_how_many_color_rows_it_has() {
        use crate::gui::optics::crystal_optics::optical_character_to_index;
        let rows = |character| {
            lch_color::axis_rows(dialog_rank(optical_character_to_index(character))).len()
        };
        assert_eq!(rows(OpticalCharacter::Isotropic), 1);
        assert_eq!(rows(OpticalCharacter::UniaxialPositive), 2);
        assert_eq!(rows(OpticalCharacter::UniaxialNegative), 2);
        assert_eq!(rows(OpticalCharacter::BiaxialPositive), 3);
        assert_eq!(rows(OpticalCharacter::BiaxialNegative), 3);
        assert_eq!(
            dialog_rank(-1),
            TensorRank::Isotropic,
            "out of range is the single color"
        );
    }

    /// Two solved directions become per-ray rows of a pleochroic tensor, with the nearest legacy
    /// triple beside them, and no isotropic bands.
    #[test]
    fn the_dialogs_rows_become_per_ray_bands_beside_a_legacy_triple() {
        use std::sync::atomic::AtomicBool;
        let cancel = AtomicBool::new(false);
        let blue = lch_color::solve_lch([50.0, 40.0, 265.0], 5.0, &cancel).expect("solves");
        let green = lch_color::solve_lch([60.0, 40.0, 140.0], 5.0, &cancel).expect("solves");
        let held = dialog_rows(TensorRank::Uniaxial, &[blue.clone(), green.clone()], 5.0)
            .expect("two rows fit a uniaxial material");
        assert_eq!(held.bands, None);
        assert!(held.triple.is_some());
        let rays = held.rays.as_ref().expect("per-ray rows");
        assert!(!rays.is_biaxial());
        assert_eq!(rays.o, blue.bands);
        assert_eq!(rays.e, green.bands);
        assert!(rays.to_tensor().is_pleochroic);
        // The wrong number of solves for the rank gives nothing.
        assert!(dialog_rows(TensorRank::Biaxial, &[blue.clone(), green], 5.0).is_none());
        assert!(dialog_rows(TensorRank::Uniaxial, &[blue], 5.0).is_none());
    }

    /// The rows the dialog's editor opens on: the held per-direction color, else the held single
    /// color on every row, else nothing.
    #[test]
    fn the_dialogs_rows_open_on_the_held_color() {
        let rays = RayBandRows {
            o: vec![[460.0, 2500.0, 0.25]],
            e: vec![[620.0, 2500.0, 0.25]],
            beta: None,
        };
        let held = DialogColor {
            rays: Some(rays.clone()),
            ..DialogColor::default()
        };
        let size = StoneSize::of_girdle(Some(5.0 / 1.5));
        let seed = dialog_seed(&held, TensorRank::Uniaxial, size);
        assert_eq!(seed.len(), 2);
        assert_eq!(
            seed,
            lch_color::axis_lch_from_tensor(
                &rays.to_tensor(),
                AbsorptionUnit::PerMm,
                TensorRank::Uniaxial,
                size.width_mm,
                size.path_mm
            )
        );
        // Rows of the other rank are not used: a biaxial material starts from nothing here.
        assert!(dialog_seed(&held, TensorRank::Biaxial, size).is_empty());
        // A single legacy color goes on every row.
        let single = DialogColor {
            triple: Some([0.2, 0.4, 2.8]),
            ..DialogColor::default()
        };
        let seed = dialog_seed(&single, TensorRank::Biaxial, size);
        assert_eq!(seed.len(), 3);
        assert!(seed.iter().all(|row| *row == seed[0]));
        assert!(dialog_seed(&DialogColor::default(), TensorRank::Uniaxial, size).is_empty());
    }

    #[test]
    fn the_sliders_edit_rows_for_the_design_and_for_an_open_dialog_row_editor() {
        let axes: SharedAxes = Arc::new(Mutex::new(AxisState::closed()));
        assert!(rows_active(0, &axes), "the design always has rows");
        assert!(
            !rows_active(TARGET_DIALOG, &axes),
            "a closed dialog editor has none"
        );
        lock_axes(&axes).lch = vec![[50.0, 30.0, 265.0]; 2];
        assert!(rows_active(TARGET_DIALOG, &axes));
        assert!(
            rows_active(TARGET_VIEW, &axes),
            "the view-only color has a row per direction too"
        );
        assert!(!rows_active(TARGET_PATTERN_ZONE, &axes));
    }

    #[test]
    fn the_design_and_the_view_have_rows_and_a_zone_color_does_not() {
        assert!(has_axis_rows(0));
        assert!(has_axis_rows(TARGET_VIEW));
        assert!(!has_axis_rows(TARGET_DIALOG));
        assert!(!has_axis_rows(TARGET_PATTERN_ZONE));
    }

    #[test]
    fn a_closed_editor_has_no_rows() {
        let closed = AxisState::closed();
        assert!(closed.lch.is_empty() && closed.solves.is_empty());
        assert!(!closed.host_known);
        assert!(closed.opened.is_none());
    }

    #[test]
    fn the_colour_it_already_has_is_not_an_edit() {
        assert!(recoloured(&sapphire(), &ColorChange::Legacy(None)).is_none());
        assert!(
            recoloured(
                &sapphire().with_body_color(Some(YELLOW)),
                &ColorChange::Legacy(Some(YELLOW))
            )
            .is_none()
        );
    }
}
