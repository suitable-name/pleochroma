//! What the Color Pattern window's callbacks do.
//!
//! Every edit goes through [`edit`] / [`edit_select`]: the handler computes the changed pattern
//! from the session (`edit.rs`, pure), and `Session::commit` records the undo step, clears Redo,
//! fills colors the solver already knows and keeps the hidden zones on their zones. This module
//! then solves the rest in the background, validates and redraws.
//!
//! The layer list shows the top layer first, so every row index the window sends is a list row;
//! `order.rs` maps it to the stored index. The sliders report a change for the values Rust writes
//! too, so a handler first compares the reading with the value it holds and ignores a reading
//! that is the same.

use super::{
    colors::{Solved, cache_key, fill_from_cache, lch_of_rows, missing_colors},
    cross_section::{dragged_value, pick_handle},
    edit,
    form::host_form,
    gallery,
    model::{
        AXIS_CUSTOM, axis_for_index, axis_from_deg, f64_of, from_display, param_specs, size_spec,
        softness_from_slider, softness_to_slider, tilt_turn_deg,
    },
    order::{Remap, flip, row_to_stored},
    picker,
    presets::{gallery_preset_at, is_built_in_name, suggested_name},
    preview,
    session::{Drag, Host, Session, VIEW_CRYSTAL, VIEW_SLICE, VIEW_STONE, with_session},
    store,
    undo::EditKey,
    view,
};
use crate::{
    ColorPatternModel, GuideModel, Zoning,
    gui::{rough_colour::wizard::zone_rows::ShapeKind, show_toast, tutorial_events::raise},
};
use pleochroma::optics::materials::{AbsorptionUnit, variants::tensor_rank};
use pleochroma_cut_core::material::pattern::{
    ColorPattern, PatternAxis, TemplateId, ZoneColor, typical_patterns,
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

/// Pushes the session and redraws the stage.
pub(super) fn refresh_all(host: &Host) {
    present(host);
    preview::refresh(host);
}

/// A sentence for the cutter: a toast in the main window.
pub(super) fn notify(host: &Host, text: &str, kind: &str) {
    if let Some(ui) = host.main.upgrade() {
        show_toast(&ui, text, kind);
    }
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

/// The worker finished a request: it remembers the colors and fills the pattern.
///
/// It goes on if more colors are missing. When a solve `failed`, it stops (no endless retry)
/// and says which step to take. The picker reads its color from the same cache. The stage is
/// only redrawn when the pattern got a color.
pub(super) fn solved(host: &Host, solved: Solved, failed: bool) {
    let filled = with_session(host, |session| {
        for (lch, bands) in solved {
            session.cache.insert(cache_key(lch), bands);
        }
        fill_from_cache(&mut session.pattern, &session.cache)
    })
    .unwrap_or(false);
    if failed {
        with_session(host, |session| {
            fill_from_cache(&mut session.pattern, &session.cache);
            session.solving = false;
        });
        notify(
            host,
            "A color could not be worked out. Pick it again.",
            "error",
        );
    } else {
        ensure_solved(host);
    }
    picker::after_solve(host, failed);
    if filled || failed {
        refresh_all(host);
    } else {
        present(host);
    }
}

/// Replaces the pattern with `change(session)` (and selects a zone) as one undo step; `remap` says
/// what the edit did to the zone indices. Returns whether anything changed.
fn edit_select(
    host: &Host,
    key: Option<EditKey>,
    remap: Remap,
    change: impl FnOnce(&Session) -> Option<(ColorPattern, usize)>,
) -> bool {
    let changed = with_session(host, |session| {
        let (next, selected) = change(session)?;
        session
            .commit(next, selected, key, remap, Instant::now())
            .then_some(())
    })
    .flatten()
    .is_some();
    if changed {
        ensure_solved(host);
        refresh_all(host);
    }
    changed
}

/// [`edit_select`] that keeps the selected zone and the zone indices.
pub(super) fn edit(
    host: &Host,
    key: Option<EditKey>,
    change: impl FnOnce(&Session) -> Option<ColorPattern>,
) -> bool {
    edit_select(host, key, Remap::Keep, |session| {
        change(session).map(|pattern| (pattern, session.selected()))
    })
}

/// The stored index of the top layer of `pattern`: what a new pattern selects.
const fn top_layer(pattern: &ColorPattern) -> usize {
    pattern.zones.len()
}

// ---- gallery: templates, typical patterns, presets --------------------------------------------

/// "Start From...": opens the gallery.
pub(super) fn open_gallery(host: &Host) {
    with_session(host, |session| session.gallery_open = true);
    gallery::push(host);
    present(host);
}

/// Closes the gallery.
pub(super) fn close_gallery(host: &Host) {
    with_session(host, |session| {
        session.gallery_open = false;
        session.naming_preset = false;
    });
    present(host);
}

/// Closes the gallery and, if a pattern was built, puts it in place as one undo step.
fn pick_from_gallery(host: &Host, change: impl FnOnce(&Session) -> Option<ColorPattern>) {
    with_session(host, |session| session.gallery_open = false);
    let changed = edit_select(host, None, Remap::Clear, |session| {
        change(session).map(|pattern| {
            let top = top_layer(&pattern);
            (pattern, top)
        })
    });
    if !changed {
        present(host);
    }
}

/// A template tile was picked (an index into `gallery::templates`).
pub(super) fn template_picked(host: &Host, index: i32) {
    let Some(id) = with_session(host, |session| {
        let at = usize::try_from(index).ok()?;
        gallery::templates(&session.base_material.name)
            .get(at)
            .copied()
    })
    .flatten() else {
        return;
    };
    pick_from_gallery(host, |session| {
        Some(edit::with_template(
            &session.pattern,
            id,
            session.stone_width_mm,
            session.rank,
        ))
    });
    // The Watermelon Tourmaline lesson waits for this template.
    raise_watermelon(host, id);
}

/// Tells the tutorial that the Watermelon pattern was picked.
fn raise_watermelon(host: &Host, id: TemplateId) {
    if id == TemplateId::Watermelon
        && let Some(ui) = host.main.upgrade()
    {
        raise(&ui, events::PATTERN_TEMPLATE_PICKED);
    }
}

/// A tile of the "Typical for ..." section was picked: the stone's species' pattern, built on the
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
    pick_from_gallery(host, |session| {
        Some(edit::with_typical(
            &session.pattern,
            &entry,
            &session.own,
            &session.variants,
            session.stone_width_mm,
        ))
    });
    raise_watermelon(host, entry.template);
}

/// A tile of My Presets was picked.
pub(super) fn preset_picked(host: &Host, index: i32) {
    let Some(preset) = with_session(host, |session| {
        gallery_preset_at(&session.saved_presets, index)
    })
    .flatten() else {
        return;
    };
    pick_from_gallery(host, |session| {
        Some(edit::with_preset(
            &session.pattern,
            &preset.pattern,
            session.stone_width_mm,
        ))
    });
}

/// The × of a saved preset's tile: deletes the preset from the vault.
pub(super) fn preset_delete(host: &Host, index: i32) {
    let Some(preset) = with_session(host, |session| {
        gallery_preset_at(&session.saved_presets, index)
    })
    .flatten() else {
        return;
    };
    if preset.built_in {
        notify(host, "A built-in preset cannot be deleted.", "warning");
        return;
    }
    let result = {
        let guard = host.deps.db.lock().unwrap_or_else(PoisonError::into_inner);
        store::delete_preset(&guard, &preset.name).and_then(|_| store::load_presets(&guard))
    };
    match result {
        Ok(presets) => {
            with_session(host, |session| session.saved_presets = presets);
            gallery::push(host);
            present(host);
            notify(
                host,
                &format!("Deleted the preset '{}'.", preset.name),
                "info",
            );
        }
        Err(error) => notify(host, &format!("{error:#}"), "error"),
    }
}

/// "Save Current...": opens the name field with a suggestion.
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
            gallery::push(host);
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

// ---- layers -----------------------------------------------------------------------------------

/// The stored index of list row `row`, if the list has it.
fn stored_of(session: &Session, row: i32) -> Option<usize> {
    row_to_stored(row, session.pattern.zones.len())
}

/// A layer row was picked.
pub(super) fn select_row(host: &Host, row: i32) {
    with_session(host, |session| {
        if let Some(stored) = stored_of(session, row) {
            session.selected_zone = stored;
        }
        session.hover = None;
        session.pending_pick = None;
        session.picker = None;
    });
    refresh_all(host);
}

/// "Add" with the shape at `shape_index` of the menu: the new layer goes on top and is selected.
pub(super) fn add_zone(host: &Host, shape_index: i32) {
    let Some(kind) = ShapeKind::from_index(shape_index) else {
        return;
    };
    edit_select(host, None, Remap::Keep, |session| {
        let (mut next, index) =
            edit::add_zone(&session.pattern, kind, session.stone_width_mm).ok()?;
        // A new zone starts in the form the material's rank asks for, so it is never a legacy one.
        next.set_color_form(index, host_form(session.rank));
        Some((next.with_required_version(), index))
    });
}

/// The stored index of list row `row`, read from the open session.
fn stored_row(host: &Host, row: i32) -> Option<usize> {
    with_session(host, |session| stored_of(session, row)).flatten()
}

/// The duplicate of the layer in row `row`, which goes right above it and is selected.
pub(super) fn duplicate_row(host: &Host, row: i32) {
    let Some(stored) = stored_row(host, row) else {
        return;
    };
    // The copy sits right after the original, so the zones above it move up by one.
    edit_select(host, None, Remap::Insert(stored + 1), |session| {
        edit::duplicate_zone(&session.pattern, stored, session.stone_width_mm).ok()
    });
}

/// "Delete" of the layer in row `row`. The layer below it is selected when it was selected.
pub(super) fn delete_row(host: &Host, row: i32) {
    let Some(stored) = stored_row(host, row) else {
        return;
    };
    edit_select(host, None, Remap::Remove(stored), |session| {
        let pattern = edit::delete_zone(&session.pattern, stored)?;
        let selected = session.selected();
        let keep = match selected.cmp(&stored) {
            std::cmp::Ordering::Less => selected,
            std::cmp::Ordering::Equal => stored - 1,
            std::cmp::Ordering::Greater => selected - 1,
        };
        Some((pattern, keep))
    });
}

/// Moves the layer in row `row` one place: `delta` -1 up the list (toward the top), +1 down.
pub(super) fn move_row(host: &Host, row: i32, delta: i32) {
    let Some(stored) = stored_row(host, row) else {
        return;
    };
    // The list is the stored order upside down: up the list is up the stack (a higher index).
    let to = if delta < 0 {
        stored + 1
    } else {
        stored.saturating_sub(1)
    };
    edit_select(host, None, Remap::Move { from: stored, to }, |session| {
        edit::move_zone(&session.pattern, stored, -delta)
    });
}

/// The drag of a card: the layer in row `from` ends up in row `to` (the base's row is never a
/// target). One undo step, however far it travels.
pub(super) fn reorder_row(host: &Host, from: i32, to: i32) {
    let Some(from) = stored_row(host, from) else {
        return;
    };
    let Some(zones) = with_session(host, |session| session.pattern.zones.len()) else {
        return;
    };
    // Rows run 0 (top) to `zones` (the base); the base cannot be passed.
    let Some(to_row) = usize::try_from(to)
        .ok()
        .map(|row| row.min(zones.saturating_sub(1)))
    else {
        return;
    };
    let to = flip(to_row, zones);
    edit_select(host, None, Remap::Move { from, to }, |session| {
        edit::reorder_zone(&session.pattern, from, to)
    });
}

/// The eye of the layer in row `row`: hides it in the pictures or shows it again. Nothing is
/// stored and nothing is undoable.
pub(super) fn toggle_hidden(host: &Host, row: i32) {
    with_session(host, |session| {
        if let Some(stored) = stored_of(session, row) {
            session.hidden.toggle(stored);
        }
    });
    refresh_all(host);
}

/// The name of the layer in row `row` was committed.
pub(super) fn rename_row(host: &Host, row: i32, name: &str) {
    if !edit(host, None, |session| {
        edit::rename_zone(&session.pattern, stored_of(session, row)?, name)
    }) {
        present(host);
    }
}

/// The Shape icons of the selected zone (Advanced).
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

/// The Size slider of the selected zone.
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

/// "Convert" of a legacy one-color zone: it takes the form of the material's rank, keeping its
/// colors.
pub(super) fn convert_legacy(host: &Host) {
    if !edit(host, None, |session| {
        edit::with_color_form(
            &session.pattern,
            session.selected(),
            host_form(session.rank),
        )
    }) {
        present(host);
    }
}

/// The pattern with zone `zone`'s variant turned into a picked color of the same rank.
///
/// The color keeps the variant's band rows. It is `None` when the zone holds a picked color
/// already or does not exist. A variant whose bands the rows cannot carry keeps its color per
/// direction and gets bands solved for it (see `edit::custom_color_resolves`). No band is ever
/// dropped.
pub(super) fn custom_of(session: &Session, zone: usize) -> Option<ColorPattern> {
    let (path_mm, width_mm) = (session.path_mm, session.stone_width_mm);
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

/// The From Variant menu of the selected zone.
///
/// Entry 0 is Custom Color, which turns the variant the zone holds into a picked color. Entry
/// `n` copies variant `n - 1` of the material into the zone. An entry past the list is the
/// variant the zone already keeps.
pub(super) fn variant_picked(host: &Host, index: i32) {
    let mut resolves = false;
    let changed = edit(host, None, |session| {
        let zone = session.selected();
        if index <= 0 {
            resolves = edit::custom_color_resolves(&session.pattern, zone);
            custom_of(session, zone)
        } else {
            let variant = session.variants.get(usize::try_from(index - 1).ok()?)?;
            edit::with_variant(&session.pattern, zone, variant)
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

/// The `[L*, C*, h]` the main window's color editor starts from for the pending pick. The
/// window's own picker does not use it.
pub(super) fn pick_seed(host: &Host) -> Option<[f64; 3]> {
    with_session(host, |session| {
        let at = session.pending_pick?;
        session.pattern.color(at).map(|color| color.lch)
    })
    .flatten()
}

/// The main window's color editor's "Apply Color" for the pending pick: the solved color goes to
/// the zone.
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

/// "Scales With Stone" (`true`) or "Fixed in mm".
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

// ---- the stage ----------------------------------------------------------------------------

/// The view switch (0 Stone, 1 Slice, 2 Crystal). Stone needs an open design; without one the
/// Slice is shown.
pub(super) fn view_changed(host: &Host, view: i32) {
    with_session(host, |session| {
        session.view = if view == VIEW_STONE && session.preview_design.is_none() {
            VIEW_SLICE
        } else {
            view.clamp(VIEW_STONE, VIEW_CRYSTAL)
        };
        session.drag = None;
        session.hover = None;
    });
    refresh_all(host);
}

/// The inset was clicked: the other picture becomes the main one. Slice and Crystal swap with the
/// stone (Slice with Crystal when no design is open); the Crystal view swaps with the slice.
pub(super) fn swap_views(host: &Host) {
    with_session(host, |session| {
        let has_design = session.preview_design.is_some();
        session.view = match session.view {
            VIEW_STONE | VIEW_CRYSTAL => VIEW_SLICE,
            _ if has_design => VIEW_STONE,
            _ => VIEW_CRYSTAL,
        };
        session.drag = None;
        session.hover = None;
    });
    refresh_all(host);
}

/// The slice rail.
pub(super) fn slice_changed(host: &Host, position: f32) {
    with_session(host, |session| {
        let position = position.clamp(0.0, 1.0);
        if (session.slice_pos - position).abs() > f32::EPSILON {
            session.slice_pos = position;
            preview::redraw_slice(host, session);
        }
    });
}

/// The pointer over the Slice: `kind` 0 down, 1 move, 2 up.
pub(super) fn pointer(host: &Host, fx: f32, fy: f32, kind: i32) {
    if with_session(host, |session| session.view != VIEW_SLICE).unwrap_or(true) {
        return;
    }
    let (fx, fy) = (f64_of(fx), f64_of(fy));
    match kind {
        0 => pointer_down(host, fx, fy),
        1 => pointer_move(host, fx, fy),
        _ => pointer_up(host),
    }
}

/// A press: picks the handle under the pointer and starts dragging it; a press that misses every
/// handle selects the zone under it.
fn pointer_down(host: &Host, fx: f64, fy: f64) {
    let zone = with_session(host, |session| {
        let view = preview::area_view(&host.window, session.stone_width_mm);
        if let Some(index) = pick_handle(&session.handles, view, fx, fy) {
            session.drag = Some(Drag {
                handle: session.handles[index],
                started: false,
            });
            session.hover = Some(index);
            preview::push_handles(&host.window, session);
            return None;
        }
        preview::zone_under(session, view, fx, fy)
    })
    .flatten();
    let Some(zone) = zone else {
        return;
    };
    let selected = with_session(host, |session| {
        let changed = session.selected_zone != zone;
        if changed {
            session.selected_zone = zone;
            session.hover = None;
            session.pending_pick = None;
            session.picker = None;
        }
        changed
    })
    .unwrap_or(false);
    if selected {
        refresh_all(host);
    }
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

/// The window watches its own size: the `narrow` flag follows it, and the pictures are drawn
/// again for a picture area that changed (no drag running).
pub(super) fn tick(host: &Host) {
    if !host.window.window().is_visible() {
        return;
    }
    let model = host.window.global::<ColorPatternModel>();
    let narrow = view::window_narrow(&host.window);
    if model.get_narrow() != narrow {
        model.set_narrow(narrow);
    }
    let area = preview::area_of(&host.window);
    let resized = with_session(host, |session| {
        let (old, new) = (session.drawn_area, area);
        session.drag.is_none() && ((old.0 - new.0).abs() > 2.0 || (old.1 - new.1).abs() > 2.0)
    })
    .unwrap_or(false);
    if resized {
        preview::refresh(host);
    }
}

// ---- history and the buttons ----------------------------------------------------------------

/// Takes the last step back.
pub(super) fn undo(host: &Host) {
    step(host, |session| {
        let mut previous = session.undo.undo(&session.pattern)?;
        fill_from_cache(&mut previous, &session.cache);
        Some(previous)
    });
}

/// Brings the step back that Undo took.
pub(super) fn redo(host: &Host) {
    step(host, |session| {
        let mut next = session.undo.redo(&session.pattern)?;
        fill_from_cache(&mut next, &session.cache);
        Some(next)
    });
}

/// Puts the pattern `take` returns in place (an undo or a redo): the picker closes, and the
/// hidden zones are shown again when the number of zones changed.
fn step(host: &Host, take: impl FnOnce(&mut Session) -> Option<ColorPattern>) {
    let changed = with_session(host, |session| {
        let pattern = take(session)?;
        let count = session.pattern.zones.len();
        session.pattern = pattern;
        if session.pattern.zones.len() != count {
            session.hidden.apply(Remap::Clear);
        }
        session.hidden.retain_within(session.pattern.zones.len());
        session.confirm_discard = false;
        session.pending_pick = None;
        session.picker = None;
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
    host.picker_timer.stop();
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

/// Cancel (and Escape and the title bar): closes the picker first, then the gallery, then the
/// window when nothing changed, else asks first.
pub(super) fn cancel(host: &Host) {
    let overlay = with_session(host, |session| {
        if session.picker.take().is_some() {
            return true;
        }
        if session.gallery_open {
            session.gallery_open = false;
            session.naming_preset = false;
            return true;
        }
        false
    })
    .unwrap_or(false);
    if overlay {
        present(host);
        return;
    }
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
