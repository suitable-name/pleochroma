//! What the Color Pattern window's callbacks do.
//!
//! Every edit goes through [`edit`] / [`edit_select`]: the handler computes the changed pattern
//! from the session (`edit.rs`, pure), and this module records the undo step, clears Redo, fills
//! colors the solver already knows, solves the rest in the background, validates and redraws.
//!
//! The sliders report a change for the values Rust writes too, so a handler first compares the
//! reading with the value it holds and ignores a reading that is the same.

use super::{
    TARGET_PATTERN_ZONE,
    colors::{Solved, cache_key, fill_from_cache, lch_of_rows, missing_colors},
    cross_section::{dragged_value, pick_handle},
    edit,
    form::{form_options, host_form, slot_for},
    model::{
        AXIS_CUSTOM, axis_for_index, axis_from_deg, f64_of, from_display, param_specs, size_spec,
        softness_from_slider, softness_to_slider, tilt_turn_deg,
    },
    presets::{is_built_in_name, preset_at, suggested_name},
    preview,
    session::{Drag, Host, Session, with_session},
    store,
    undo::EditKey,
    view,
};
use crate::{
    BodyColorEditorModel, ColorPatternModel, GuideModel, Zoning,
    gui::{rough_colour::wizard::zone_rows::ShapeKind, show_toast, tutorial_events::raise},
};
use pleochroma::optics::materials::{AbsorptionUnit, variants::tensor_rank};
use pleochroma_cut_core::material::pattern::{
    ColorPattern, ColorRef, PatternAxis, TemplateId, ZoneColor, typical_patterns,
};
use pleochroma_editor::{guide::viewing_events as events, lch_color::axis_lch_from_tensor};
use slint::ComponentHandle;
use std::{sync::PoisonError, time::Instant};

/// Whether two slider readings are the same value (a reading that echoes a value Rust wrote).
fn same(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-4 * a.abs().max(b.abs()).max(1.0)
}

/// Whether the Advanced controls are shown right now (`GuideModel.shows_advanced()`).
fn advanced(host: &Host) -> bool {
    host.main
        .upgrade()
        .is_some_and(|ui| ui.global::<GuideModel>().invoke_shows_advanced())
}

/// Pushes the session to the window.
pub(super) fn present(host: &Host) {
    let advanced = advanced(host);
    if let Ok(guard) = host.session.try_borrow()
        && let Some(session) = guard.as_ref()
    {
        view::present(&host.window, session, advanced);
    }
}

/// Pushes the session and redraws the preview.
pub(super) fn refresh_all(host: &Host) {
    present(host);
    preview::refresh(host);
}

/// A sentence for the cutter: a toast in the main window and a line under the preview.
pub(super) fn notify(host: &Host, text: &str, kind: &str) {
    if let Some(ui) = host.main.upgrade() {
        show_toast(&ui, text, kind);
    }
    host.window
        .global::<ColorPatternModel>()
        .set_preview_note(text.into());
}

/// Solves the colors that no solve has covered yet, in the background.
pub(super) fn ensure_solved(host: &Host) {
    let work = with_session(host, |session| {
        fill_from_cache(&mut session.pattern, &session.cache);
        let missing = missing_colors(&session.pattern, &session.cache);
        session.solving = !missing.is_empty();
        (missing, session.path_mm)
    });
    if let Some((missing, path_mm)) = work
        && !missing.is_empty()
    {
        host.solver.submit(missing, path_mm);
    }
}

/// The worker finished a request: remember the colors, fill the pattern, go on if more are
/// missing. When a solve `failed`, stop (no endless retry) and say which step to take.
pub(super) fn solved(host: &Host, solved: Solved, failed: bool) {
    with_session(host, |session| {
        for (lch, bands) in solved {
            session.cache.insert(cache_key(lch), bands);
        }
    });
    if failed {
        with_session(host, |session| {
            fill_from_cache(&mut session.pattern, &session.cache);
            session.solving = false;
        });
        notify(
            host,
            "A color could not be worked out. Pick it again with Pick\u{2026}.",
            "error",
        );
    } else {
        ensure_solved(host);
    }
    refresh_all(host);
}

/// Replaces the pattern with `change(session)` (and selects a zone) as one undo step; returns
/// whether anything changed.
fn edit_select(
    host: &Host,
    key: Option<EditKey>,
    change: impl FnOnce(&Session) -> Option<(ColorPattern, usize)>,
) -> bool {
    let changed = with_session(host, |session| {
        let Some((mut next, selected)) = change(session) else {
            return false;
        };
        fill_from_cache(&mut next, &session.cache);
        if next == session.pattern && selected == session.selected() {
            return false;
        }
        let before = std::mem::replace(&mut session.pattern, next);
        if before != session.pattern {
            session.undo.record(&before, key, Instant::now());
        }
        session.selected_zone = selected;
        session.confirm_discard = false;
        session.hover = None;
        session.pending_pick = None;
        true
    })
    .unwrap_or(false);
    if changed {
        ensure_solved(host);
        refresh_all(host);
    }
    changed
}

/// [`edit_select`] that keeps the selected zone.
pub(super) fn edit(
    host: &Host,
    key: Option<EditKey>,
    change: impl FnOnce(&Session) -> Option<ColorPattern>,
) -> bool {
    edit_select(host, key, |session| {
        change(session).map(|pattern| (pattern, session.selected()))
    })
}

// ---- templates, presets ---------------------------------------------------------------------

/// A template pill was pressed.
pub(super) fn template_picked(host: &Host, index: i32) {
    let Some(id) = usize::try_from(index)
        .ok()
        .and_then(|i| TemplateId::ALL.get(i))
        .copied()
    else {
        return;
    };
    edit_select(host, None, |session| {
        Some((
            edit::with_template(&session.pattern, id, session.stone_width_mm, session.rank),
            0,
        ))
    });
    // The Watermelon Tourmaline lesson waits for this template.
    if id == TemplateId::Watermelon
        && let Some(ui) = host.main.upgrade()
    {
        raise(&ui, events::PATTERN_TEMPLATE_PICKED);
    }
}

/// A pill of the "Typical for ..." row was pressed: the stone's species' pattern, built on the
/// color the stone has without a pattern.
pub(super) fn typical_picked(host: &Host, index: i32) {
    let Some(index) = usize::try_from(index).ok() else {
        return;
    };
    let Some(entry) = with_session(host, |session| {
        typical_patterns(&session.base_material.name)
            .get(index)
            .copied()
    })
    .flatten() else {
        return;
    };
    edit_select(host, None, |session| {
        Some((
            edit::with_typical(
                &session.pattern,
                &entry,
                &session.own,
                &session.variants,
                session.stone_width_mm,
            ),
            0,
        ))
    });
}

/// An entry of the Presets menu was picked (the prompt is never reported).
pub(super) fn preset_picked(host: &Host, index: i32) {
    let Some(preset) =
        with_session(host, |session| preset_at(&session.saved_presets, index)).flatten()
    else {
        return;
    };
    edit_select(host, None, |session| {
        Some((
            edit::with_preset(&session.pattern, &preset.pattern, session.stone_width_mm),
            0,
        ))
    });
}

/// "Save as Preset...": opens the name field with a suggestion.
pub(super) fn save_preset(host: &Host) {
    let suggestion = with_session(host, |session| {
        session.naming_preset = true;
        suggested_name(&session.pattern, &session.saved_presets)
    });
    if let Some(name) = suggestion {
        host.window
            .global::<ColorPatternModel>()
            .set_preset_name(name.into());
    }
    present(host);
}

/// The name field's Save (or Enter).
pub(super) fn preset_name_committed(host: &Host, name: &str) {
    let name = name.trim();
    let Some((pattern, can_store)) = with_session(host, |session| {
        (session.pattern.clone(), session.can_store())
    }) else {
        return;
    };
    if name.is_empty() {
        notify(host, "Give the preset a name.", "warning");
        return;
    }
    if !can_store {
        notify(
            host,
            "Finish the pattern before saving it as a preset.",
            "warning",
        );
        return;
    }
    if is_built_in_name(name) {
        notify(
            host,
            "That name belongs to a built-in preset. Choose another one.",
            "warning",
        );
        return;
    }
    let saved = {
        let guard = host.deps.db.lock().unwrap_or_else(PoisonError::into_inner);
        store::save_preset(&guard, name, &pattern).and_then(|()| store::load_presets(&guard))
    };
    match saved {
        Ok(presets) => {
            with_session(host, |session| {
                session.saved_presets = presets;
                session.naming_preset = false;
            });
            present(host);
            notify(host, &format!("Saved the preset '{name}'."), "success");
        }
        Err(error) => notify(host, &format!("{error:#}"), "error"),
    }
}

/// The name field's Cancel.
pub(super) fn preset_name_cancelled(host: &Host) {
    with_session(host, |session| session.naming_preset = false);
    present(host);
}

// ---- zones ------------------------------------------------------------------------------------

/// A zone row was picked.
pub(super) fn select_zone(host: &Host, index: i32) {
    with_session(host, |session| {
        session.selected_zone = usize::try_from(index)
            .unwrap_or(0)
            .min(session.pattern.zones.len());
        session.hover = None;
        session.pending_pick = None;
    });
    refresh_all(host);
}

/// "Add Zone" with the shape at `shape_index` of the menu.
pub(super) fn add_zone(host: &Host, shape_index: i32) {
    let Some(kind) = ShapeKind::from_index(shape_index) else {
        return;
    };
    edit_select(host, None, |session| {
        let (mut next, index) =
            edit::add_zone(&session.pattern, kind, session.stone_width_mm).ok()?;
        // A new zone starts in the form the material's rank asks for, so it is never a legacy one.
        next.set_color_form(index, host_form(session.rank));
        Some((next.with_required_version(), index))
    });
}

/// "Duplicate".
pub(super) fn duplicate_zone(host: &Host) {
    edit_select(host, None, |session| {
        edit::duplicate_zone(&session.pattern, session.selected(), session.stone_width_mm).ok()
    });
}

/// "Delete".
pub(super) fn delete_zone(host: &Host) {
    edit_select(host, None, |session| {
        let index = session.selected();
        edit::delete_zone(&session.pattern, index).map(|pattern| {
            let keep = index.min(pattern.zones.len());
            (pattern, keep)
        })
    });
}

/// "Up" (-1) and "Down" (+1).
pub(super) fn move_zone(host: &Host, delta: i32) {
    edit_select(host, None, |session| {
        edit::move_zone(&session.pattern, session.selected(), delta)
    });
}

/// The name field was committed.
pub(super) fn rename_zone(host: &Host, name: &str) {
    if !edit(host, None, |session| {
        edit::rename_zone(&session.pattern, session.selected(), name)
    }) {
        present(host);
    }
}

/// The Shape menu of the selected zone (Advanced).
pub(super) fn shape_changed(host: &Host, index: i32) {
    let Some(kind) = ShapeKind::from_index(index) else {
        return;
    };
    if !edit(host, None, |session| {
        edit::change_shape(
            &session.pattern,
            session.selected(),
            kind,
            session.stone_width_mm,
        )
    }) {
        present(host);
    }
}

/// A parameter slider of the selected zone (Advanced).
pub(super) fn param_changed(host: &Host, row: i32, value: f32) {
    let (Ok(row), Some(zone)) = (
        usize::try_from(row),
        with_session(host, |session| session.selected()),
    ) else {
        return;
    };
    let shown = f64_of(value);
    let changed = edit(host, Some(EditKey::Param { zone, row }), |session| {
        let relative = session.pattern.relative_to_stone;
        let spec = param_specs(
            session.pattern.zone(zone)?,
            relative,
            session.stone_width_mm,
        )
        .into_iter()
        .nth(row)?;
        if same(spec.value, shown) {
            return None;
        }
        edit::with_param(
            &session.pattern,
            zone,
            spec.parameter,
            from_display(spec.parameter, shown, relative),
        )
    });
    if !changed {
        present(host);
    }
}

/// The Size slider of the selected zone (Simple).
pub(super) fn size_changed(host: &Host, value: f32) {
    let Some(zone) = with_session(host, |session| session.selected()) else {
        return;
    };
    let shown = f64_of(value);
    let changed = edit(host, Some(EditKey::Size { zone }), |session| {
        let spec = size_spec(
            &session.pattern.zone(zone)?.shape,
            session.pattern.relative_to_stone,
            session.stone_width_mm,
        )?;
        if same(spec.value, shown) {
            return None;
        }
        edit::with_zone_size(&session.pattern, zone, shown)
    });
    if !changed {
        present(host);
    }
}

// ---- colors -----------------------------------------------------------------------------------

/// The Colors menu of the selected zone: entry `index` of the forms the picker offers (the form of
/// the material's rank, and the form the zone has when that is another). Picking the zone's own
/// form changes nothing.
pub(super) fn color_form_changed(host: &Host, index: i32) {
    let changed = edit(host, None, |session| {
        let zone = session.selected();
        let options = form_options(session.rank, &session.pattern.zone(zone)?.color)?;
        let form = *options.forms.get(usize::try_from(index).ok()?)?;
        edit::with_color_form(&session.pattern, zone, form)
    });
    if !changed {
        // The menu does not keep its own state: write the real one back.
        present(host);
    }
}

/// The Source menu of the selected zone (0 Custom, 1 Variant): Variant copies the first variant of
/// the material into the zone (its Material Default), Custom turns the variant the zone holds into
/// the picked color of the same rank, with the variant's band rows. A variant whose bands the rows
/// cannot carry (a narrow measured energy band) keeps its color per direction and gets bands
/// solved for it, which the window says; no band is ever dropped.
pub(super) fn color_source_changed(host: &Host, index: i32) {
    let mut resolves = false;
    let changed = edit(host, None, |session| {
        let zone = session.selected();
        if index == 1 {
            if !session.pattern.zone(zone)?.color.is_custom() {
                return None;
            }
            let variant = session.variants.first()?;
            edit::with_variant(&session.pattern, zone, variant)
        } else {
            let (path_mm, width_mm) = (session.path_mm, session.stone_width_mm);
            resolves = edit::custom_color_resolves(&session.pattern, zone);
            edit::with_custom_color(
                &session.pattern,
                zone,
                session.rank,
                |rows| lch_of_rows(rows, path_mm),
                |tensor| {
                    axis_lch_from_tensor(
                        tensor,
                        AbsorptionUnit::PerMm,
                        tensor_rank(tensor),
                        width_mm,
                        path_mm,
                    )
                },
            )
        }
    });
    if changed && resolves {
        notify(
            host,
            "This color has narrow bands a picked color cannot hold, so its color is kept and new bands are solved for it.",
            "info",
        );
    }
    if !changed {
        present(host);
    }
}

/// The Variant menu of the selected zone: entry `index` of the variants of the material is copied
/// into the zone. An entry past the list is the variant the zone already keeps.
pub(super) fn variant_picked(host: &Host, index: i32) {
    let changed = edit(host, None, |session| {
        let variant = session.variants.get(usize::try_from(index).ok()?)?;
        edit::with_variant(&session.pattern, session.selected(), variant)
    });
    if !changed {
        present(host);
    }
}

/// "Pick..." of the selected zone's color `which` (0 the one color, then in slot order 1 Along or
/// Alpha, 2 Across or Beta, 3 Gamma): opens the body-color editor in the main window with the
/// pattern zone as its target.
pub(super) fn pick_color(host: &Host, which: i32) {
    let picked = with_session(host, |session| {
        let zone = session.selected();
        let slot = slot_for(&session.pattern.zone(zone)?.color, which)?;
        let at = ColorRef { zone, slot };
        session.pattern.color(at)?;
        session.pending_pick = Some(at);
        Some(())
    })
    .flatten();
    if picked.is_none() {
        return;
    }
    if with_session(host, |session| session.stone_apply.is_some()).unwrap_or(false) {
        super::picker::opened(host);
        return;
    }
    let Some(ui) = host.main.upgrade() else {
        return;
    };
    let editor = ui.global::<BodyColorEditorModel>();
    editor.set_target(TARGET_PATTERN_ZONE);
    ui.global::<Zoning>().set_pattern_pick_open(true);
    editor.invoke_open_requested();
}

/// The `[L*, C*, h]` the color editor opens on for the pending pick.
pub(super) fn pick_seed(host: &Host) -> Option<[f64; 3]> {
    with_session(host, |session| {
        let at = session.pending_pick?;
        session.pattern.color(at).map(|color| color.lch)
    })
    .flatten()
}

/// The color editor's "Apply Color" for the pending pick: the solved color goes to the zone.
pub(super) fn picked_color(host: &Host, lch: [f64; 3], bands: Vec<[f32; 3]>) {
    let Some(at) = with_session(host, |session| {
        session.cache.insert(cache_key(lch), bands.clone());
        session.pending_pick.take()
    })
    .flatten() else {
        return;
    };
    edit(host, None, |session| {
        let mut next = session.pattern.clone();
        next.set_color(at, ZoneColor::solved(lch, bands))
            .then_some(next)
    });
}

// ---- pattern-wide settings ---------------------------------------------------------------------

/// The axis menu.
pub(super) fn axis_changed(host: &Host, index: i32) {
    let imported = with_session(host, |session| {
        matches!(session.pattern.axis, PatternAxis::Frame { .. })
    })
    .unwrap_or(false);
    if imported {
        present(host);
        return;
    }
    if let Some(axis) = axis_for_index(index) {
        with_session(host, |session| session.custom_axis = false);
        if !edit(host, None, |session| {
            edit::with_axis(&session.pattern, axis)
        }) {
            present(host);
        }
    } else {
        with_session(host, |session| session.custom_axis = index == AXIS_CUSTOM);
        present(host);
    }
}

/// The Tilt and Turn sliders of the Custom axis (Advanced).
pub(super) fn tilt_turn_changed(host: &Host, tilt: f32, turn: f32) {
    let (tilt, turn) = (f64_of(tilt), f64_of(turn));
    let changed = edit(host, Some(EditKey::TiltTurn), |session| {
        // An imported frame and "Follows Crystal Axis" have no tilt and turn of their own.
        if matches!(
            session.pattern.axis,
            PatternAxis::Frame { .. } | PatternAxis::FollowsCrystalAxis
        ) {
            return None;
        }
        let (now_tilt, now_turn) = tilt_turn_deg(&session.pattern.axis);
        if same(now_tilt, tilt) && same(now_turn, turn) {
            return None;
        }
        edit::with_axis(&session.pattern, axis_from_deg(tilt, turn))
    });
    if !changed {
        present(host);
    }
}

/// "Scales With Stone Size" (`true`) or "Fixed in mm".
pub(super) fn scale_mode_changed(host: &Host, relative: bool) {
    if !edit(host, None, |session| {
        edit::with_scale_mode(&session.pattern, relative, session.stone_width_mm)
    }) {
        present(host);
    }
}

/// The boundary slider (0 sharp to 1 soft).
pub(super) fn softness_changed(host: &Host, slider: f32) {
    let changed = edit(host, Some(EditKey::Softness), |session| {
        let relative = session.pattern.relative_to_stone;
        let width = session.stone_width_mm;
        let current = softness_to_slider(session.pattern.softness, relative, width);
        if same(f64::from(current), f64::from(slider)) {
            return None;
        }
        edit::with_softness(
            &session.pattern,
            softness_from_slider(slider, relative, width),
        )
    });
    if !changed {
        present(host);
    }
}

// ---- preview ------------------------------------------------------------------------------------

/// A preview mode pill.
pub(super) fn preview_mode_changed(host: &Host, mode: i32) {
    with_session(host, |session| {
        session.preview_mode = if mode == 1 && session.preview_design.is_none() {
            0
        } else {
            mode.clamp(0, 2)
        };
    });
    refresh_all(host);
}

/// The Cross-Section's Slice slider.
pub(super) fn slice_changed(host: &Host, position: f32) {
    with_session(host, |session| {
        let position = position.clamp(0.0, 1.0);
        if (session.slice_pos - position).abs() > f32::EPSILON {
            session.slice_pos = position;
            preview::redraw_slice(host, session);
        }
    });
}

/// The pointer over the Cross-Section: `kind` 0 down, 1 move, 2 up.
pub(super) fn pointer(host: &Host, fx: f32, fy: f32, kind: i32) {
    if with_session(host, |session| session.preview_mode != 2).unwrap_or(true) {
        return;
    }
    let (fx, fy) = (f64_of(fx), f64_of(fy));
    match kind {
        0 => pointer_down(host, fx, fy),
        1 => pointer_move(host, fx, fy),
        _ => pointer_up(host),
    }
}

/// A press: picks the handle under the pointer and starts dragging it.
fn pointer_down(host: &Host, fx: f64, fy: f64) {
    with_session(host, |session| {
        let view = preview::area_view(&host.window, session.stone_width_mm);
        if let Some(index) = pick_handle(&session.handles, view, fx, fy) {
            session.drag = Some(Drag {
                handle: session.handles[index],
                started: false,
            });
            session.hover = Some(index);
        }
        preview::push_handles(&host.window, session);
    });
}

/// What a pointer move did.
enum Moved {
    /// A handle was dragged and the pattern changed.
    Dragged,
    /// The handle under the pointer changed.
    Hovered,
}

/// A move: drags the handle (one undo step for the whole drag) or updates the hover.
fn pointer_move(host: &Host, fx: f64, fy: f64) {
    let moved = with_session(host, |session| {
        let view = preview::area_view(&host.window, session.stone_width_mm);
        let relative = session.pattern.relative_to_stone;
        let width = session.stone_width_mm;
        let Some(drag) = session.drag else {
            let hover = pick_handle(&session.handles, view, fx, fy);
            if hover == session.hover {
                return None;
            }
            session.hover = hover;
            return Some(Moved::Hovered);
        };
        let (x_mm, z_mm) = view.at_fraction(fx, fy);
        let zone = session.pattern.zone(drag.handle.zone)?;
        let value = dragged_value(&drag.handle, zone, x_mm, z_mm, relative, width)?;
        let next = edit::with_param(
            &session.pattern,
            drag.handle.zone,
            drag.handle.parameter,
            value,
        )?;
        if next == session.pattern {
            return None;
        }
        if !drag.started {
            session.undo.begin_group(&session.pattern);
            session.drag = Some(Drag {
                started: true,
                ..drag
            });
        }
        session.pattern = next;
        session.confirm_discard = false;
        Some(Moved::Dragged)
    })
    .flatten();
    match moved {
        Some(Moved::Dragged) => {
            present(host);
            with_session(host, |session| preview::redraw_slice(host, session));
        }
        Some(Moved::Hovered) => {
            with_session(host, |session| {
                preview::push_handles(&host.window, session);
            });
        }
        None => {}
    }
}

/// A release: ends the drag; the Current Design preview catches up.
fn pointer_up(host: &Host) {
    let started = with_session(host, |session| {
        let drag = session.drag.take();
        session.undo.break_run();
        drag.is_some_and(|drag| drag.started)
    })
    .unwrap_or(false);
    if started {
        ensure_solved(host);
        refresh_all(host);
    }
}

// ---- history and the buttons --------------------------------------------------------------------

/// Takes the last step back.
pub(super) fn undo(host: &Host) {
    let changed = with_session(host, |session| {
        let mut previous = session.undo.undo(&session.pattern)?;
        fill_from_cache(&mut previous, &session.cache);
        session.pattern = previous;
        session.confirm_discard = false;
        session.pending_pick = None;
        Some(())
    })
    .flatten()
    .is_some();
    if changed {
        ensure_solved(host);
        refresh_all(host);
    }
}

/// Brings the step back that Undo took.
pub(super) fn redo(host: &Host) {
    let changed = with_session(host, |session| {
        let mut next = session.undo.redo(&session.pattern)?;
        fill_from_cache(&mut next, &session.cache);
        session.pattern = next;
        session.confirm_discard = false;
        session.pending_pick = None;
        Some(())
    })
    .flatten()
    .is_some();
    if changed {
        ensure_solved(host);
        refresh_all(host);
    }
}

/// Closes the window; the session and its workers are let go.
pub(super) fn close(host: &Host) {
    host.solver.cancel();
    host.preview.next_ticket();
    if let Ok(mut guard) = host.session.try_borrow_mut() {
        *guard = None;
    }
    let _ = host.window.hide();
    if let Some(ui) = host.main.upgrade() {
        let zoning = ui.global::<Zoning>();
        zoning.set_pattern_editor_open(false);
        zoning.set_pattern_pick_open(false);
    }
}

/// Cancel (and Escape and the title bar): closes when nothing changed, else asks first.
pub(super) fn cancel(host: &Host) {
    let ask = with_session(host, |session| {
        if session.dirty() {
            session.confirm_discard = true;
            true
        } else {
            false
        }
    })
    .unwrap_or(false);
    if ask {
        present(host);
    } else {
        close(host);
    }
}

/// "Discard" in the discard bar.
pub(super) fn discard_confirmed(host: &Host) {
    close(host);
}

/// "Keep Editing" in the discard bar.
pub(super) fn discard_declined(host: &Host) {
    with_session(host, |session| session.confirm_discard = false);
    present(host);
}
