//! The Solid viewport's facet hover/click and the reverse tier-selection link.

use std::{
    cell::RefCell,
    collections::BTreeSet,
    rc::Rc,
    sync::{Arc, Mutex},
};

use slint::{ComponentHandle, Model};

use super::facet_overlay::{resubmit_facet_overlay, set_hovered_facet};
use crate::{
    EditorModel, EditorTierItem, MainWindow, SolidPreviewModel,
    bridge::render_thread::RenderContext,
    gui::{
        editor::{
            auto_solve,
            manipulate::{drop_provisional_for_selection, provisional_active},
            state::{EditorState, apply_multi_selection, push_multi_selected_count, push_tiers},
            tune_panel,
            view::{SolidLastSolved, push_selected_tier_chips, submit_preview_replan},
        },
        solid_preview::{
            facet_selection,
            preview_state::{SolidPickState, SolidPreviewState},
        },
        tutorial_events::raise,
    },
};
use pleochroma_editor::guide::viewing_events as events;

/// The last clicked facet's id, if the last click landed on a facet: the facet the
/// manipulation handles (`gui::editor::manipulate`) sit on. The Solid and the Diagram view
/// click into the SAME value (`facet_selection::selected_facet_id`), so the handles have
/// an anchor facet whichever view the click was made in. A selection made from the tier
/// table leaves it stale or unset, which the handles resolve by checking it belongs to the
/// selected tier and falling back to the tier's first facet.
#[must_use]
pub(in crate::gui::editor) fn selected_facet_id() -> Option<u32> {
    facet_selection::selected_facet_id()
}

/// A list selection replaces a picked facet without changing other overlay state.
pub(in crate::gui::editor) fn clear_selected_facet(
    ui: &MainWindow,
    preview_state: &SolidPreviewState,
) {
    facet_selection::clear_clicked_facet(preview_state);
    ui.global::<SolidPreviewModel>().set_hover_text("".into());
}

/// Maps a Solid-viewport pointer position (`SolidPreviewModel.on_facet_hover`/`on_facet_click`,
/// LOGICAL pixels) onto the pick buffer's PHYSICAL-pixel space.
///
/// In Path-traced/Both mode the rasters are requested at the letterboxed rectangle from
/// `render::camera_lighting::contained_request_size`, which mirrors `image-fit: contain`. That
/// rectangle is centred in the viewport, so the margin is subtracted as well as the scaling.
///
/// Falls back to the plain scaled position when `auto_solve::render_ctx` is not stashed yet or
/// nothing is requested. In Solid and Diagram modes the margin is zero.
pub(in crate::gui::editor) fn map_to_pick_coordinates(
    ui: &MainWindow,
    x: f32,
    y: f32,
) -> (f32, f32) {
    let scale = ui.window().scale_factor();
    logical_to_pick_pixels((x, y), scale, pick_margin(ui))
}

/// [`map_to_pick_coordinates`]'s arithmetic on plain numbers: a LOGICAL pointer
/// position scaled to physical pixels, less the letterbox `margin`.
#[must_use]
pub(in crate::gui::editor) fn logical_to_pick_pixels(
    logical: (f32, f32),
    scale: f32,
    margin: (f32, f32),
) -> (f32, f32) {
    // Scaled first, then shifted, exactly as the hover/click callbacks always did
    // (a fused multiply-add would round differently by a hair).
    let physical = (logical.0 * scale, logical.1 * scale);
    (physical.0 - margin.0, physical.1 - margin.1)
}

/// The exact inverse of [`logical_to_pick_pixels`]: a pick-frame pixel back to the
/// LOGICAL viewport position it is drawn at, `(pick + margin) / scale`. What the
/// manipulation handles use to place themselves.
#[must_use]
pub(in crate::gui::editor) fn pick_pixels_to_logical(
    pick: (f32, f32),
    scale: f32,
    margin: (f32, f32),
) -> (f32, f32) {
    ((pick.0 + margin.0) / scale, (pick.1 + margin.1) / scale)
}

/// A pixel position as an unsigned pixel index: a negative or NaN position saturates to
/// `0` and an oversized one to `u32::MAX`, which is the contract the pick-buffer lookups
/// rely on (`PickBuffer::facet_at` bounds-checks against the frame's size).
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "saturating float-to-int cast is the intent: negative or NaN maps to 0, the pick buffer bounds-checks"
)]
const fn pixel_coordinate(position: f32) -> u32 {
    position as u32
}

/// The PHYSICAL-pixel letterbox margin [`map_to_pick_coordinates`] subtracts: `(0, 0)`
/// in Solid/Diagram mode and before `auto_solve::render_ctx` is stashed, half the bars'
/// width/height in Path-traced/Both mode (see [`map_to_pick_coordinates`]'s doc).
pub(in crate::gui::editor) fn pick_margin(ui: &MainWindow) -> (f32, f32) {
    let scale = ui.window().scale_factor();
    let Some(render_ctx) = auto_solve::render_ctx() else {
        return (0.0, 0.0);
    };
    let view_mode = crate::gui::render::camera_lighting::view_mode_index(
        ui.global::<SolidPreviewModel>().get_view_mode(),
    );
    let viewport_physical = (
        pixel_coordinate(ui.global::<SolidPreviewModel>().get_viewport_width() * scale),
        pixel_coordinate(ui.global::<SolidPreviewModel>().get_viewport_height() * scale),
    );
    let render_size = {
        let ctx = render_ctx
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        (ctx.width, ctx.height)
    };
    let contained = crate::gui::render::camera_lighting::contained_request_size(
        view_mode,
        viewport_physical,
        render_size,
    );
    letterbox_margin(viewport_physical, contained)
}

/// The physical-pixel margin [`map_to_pick_coordinates`] subtracts. `image-fit: contain` centres
/// the smaller `contained_physical` rectangle, so the margin on each axis is half the leftover.
/// `saturating_sub` keeps the margin at zero if the rectangle is larger.
///
/// `pub(in crate::gui::editor)`: used by the tests in `super::tests` and in the manipulation
/// module.
#[must_use]
pub(in crate::gui::editor) fn letterbox_margin(
    viewport_physical: (u32, u32),
    contained_physical: (u32, u32),
) -> (f32, f32) {
    (
        viewport_physical.0.saturating_sub(contained_physical.0) as f32 / 2.0,
        viewport_physical.1.saturating_sub(contained_physical.1) as f32 / 2.0,
    )
}

/// The Solid viewport's hover callback. It looks up the facet under the cursor in the pick buffer
/// of the last rendered frame and shows that frame's hover text. Off the silhouette it shows the
/// selected facet's label instead, or nothing.
///
/// # Indexes the frame's own table instead of rebuilding a `FacetMap`
///
/// Rebuilding the facet map on every mouse move would be wasteful. The worker builds the hover
/// text once per rendered frame, so a hover is one `Vec::get`. An edit that has not re-rendered
/// yet shows the previous frame's text, as it shows the previous frame's picks.
///
/// `solid_hover_text` reaches this callback through [`SolidPickState`]. `gui::main_window` builds
/// it and shares it with `gui::solid_sink`'s `SlintSolidSink`, which writes it every frame.
pub(in crate::gui::editor) fn setup_solid_facet_hover_callback(
    ui: &MainWindow,
    solid_pick_state: &SolidPickState,
) {
    let solid_pick = Arc::clone(&solid_pick_state.pick);
    let solid_hover_text = Arc::clone(&solid_pick_state.hover_text);
    let ui_weak = ui.as_weak();
    ui.global::<SolidPreviewModel>()
        .on_facet_hover(move |x: f32, y: f32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            // The pick buffer is at the viewport's physical size, but `x`/`y` are LOGICAL, so
            // they are multiplied by `scale_factor` (at 2x, logical 400 is physical 800). Dividing
            // would collapse every pick into the top-left quarter. In Path-traced/Both mode
            // `map_to_pick_coordinates` also removes the letterbox margin. A negative position
            // saturates to `0`, and `PickBuffer::facet_at` bounds-checks, so an off-image
            // position misses (`None`) rather than reading garbage.
            let scale = ui.window().scale_factor();
            let (px, py) = map_to_pick_coordinates(&ui, x, y);
            let (pick_dimensions, facet_id) = {
                let pick = solid_pick
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                pick.as_ref().map_or((None, None), |pick| {
                    (
                        Some((pick.width, pick.height)),
                        pick.facet_at(pixel_coordinate(px), pixel_coordinate(py)),
                    )
                })
            };
            let (label_count, hit_label, selected_label) = {
                let labels = solid_hover_text
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                // The clicked facet's label is read from the CURRENT frame's table each time
                // (not cached at click time), so an angle dragged since the click reads at its
                // new value.
                let selected_label = selected_facet_id()
                    .and_then(|id| labels.get(id as usize).cloned())
                    .unwrap_or_default();
                (
                    Some(labels.len()),
                    facet_id.and_then(|id| labels.get(id as usize).cloned()),
                    selected_label,
                )
            };
            #[cfg(not(feature = "ui-harness"))]
            let _ = (scale, pick_dimensions, label_count);
            let Some(facet_id) = facet_id else {
                // Falls back to the last CLICKED facet's own
                // label (if any) instead of blanking the tooltip outright, so the
                // selection stays readable once the pointer leaves it.
                #[cfg(feature = "ui-harness")]
                crate::harness_support::record_hover_diagnostic(
                    crate::harness_support::HoverViewport::Solid,
                    x,
                    y,
                    scale,
                    pick_dimensions,
                    None,
                    label_count,
                    None,
                );
                ui.global::<SolidPreviewModel>()
                    .set_hover_text(selected_label.into());
                if let Some(preview_state) = auto_solve::preview_state() {
                    set_hovered_facet(&preview_state, None);
                }
                return;
            };
            let text = hit_label.unwrap_or_default();
            #[cfg(feature = "ui-harness")]
            crate::harness_support::record_hover_diagnostic(
                crate::harness_support::HoverViewport::Solid,
                x,
                y,
                scale,
                pick_dimensions,
                Some(facet_id),
                label_count,
                Some(text.clone()),
            );
            ui.global::<SolidPreviewModel>().set_hover_text(text.into());
            if let Some(preview_state) = auto_solve::preview_state() {
                set_hovered_facet(&preview_state, Some(facet_id));
            }
        });
}

/// The Solid viewport's click callback, the forward half of the "click selects the tier in the
/// list" link (see [`setup_solid_selected_tier_changed_callback`] for the reverse half).
///
/// # Indexes the frame's own table instead of rebuilding a `FacetMap`
///
/// See [`setup_solid_facet_hover_callback`]. `solid_facet_owners` is the last rendered frame's
/// `PreviewFrame::facet_owners`, so a clicked facet resolves to its owning tier with one lookup.
/// It reaches this callback through [`SolidPickState`], as `solid_hover_text` does.
pub(in crate::gui::editor) fn setup_solid_facet_click_callback(
    ui: &MainWindow,
    solid_pick_state: &SolidPickState,
) {
    let solid_pick = Arc::clone(&solid_pick_state.pick);
    let solid_facet_owners = Arc::clone(&solid_pick_state.facet_owners);
    let solid_hover_text = Arc::clone(&solid_pick_state.hover_text);
    let ui_weak = ui.as_weak();
    ui.global::<SolidPreviewModel>()
        .on_facet_click(move |x: f32, y: f32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            // While the Slice tool holds a provisional tier, the frame's `facet_tier`
            // table may name that tier -- an index `EditorState` does not have -- so a
            // click selects nothing until Keep or Discard.
            if provisional_active() {
                return;
            }
            // See `setup_solid_facet_hover_callback`'s matching comment for why the
            // incoming (logical) coordinates go through `map_to_pick_coordinates`
            // (scaled AND, in Path-traced/Both mode, letterbox-corrected) here.
            let (px, py) = map_to_pick_coordinates(&ui, x, y);
            let Some(facet_id) = solid_pick
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .as_ref()
                .and_then(|pick| pick.facet_at(pixel_coordinate(px), pixel_coordinate(py)))
            else {
                // A miss clears the tier selection, as the Diagram view's miss does. Setting
                // `selected_tier_index` is enough: its `changed` handler in `models/editor.slint`
                // fires `selected_tier_changed`.
                ui.global::<EditorModel>().set_selected_tier_index(-1);
                // A miss also clears the picked facet, so the tooltip stops reading it.
                if let Some(preview_state) = auto_solve::preview_state() {
                    facet_selection::clear_clicked_facet(&preview_state);
                } else {
                    facet_selection::set_selected_facet_id(None);
                }
                ui.global::<SolidPreviewModel>().set_hover_text("".into());
                return;
            };
            // A flat facet selects its tier, a concave tool's facet its concave tier (the
            // row after the flat ones); a tool facet with Ctrl/Shift selects that row
            // alone, since concave rows never join the multi-select.
            let tier_row = {
                let owners = solid_facet_owners
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                facet_selection::clicked_tier_row(&ui, &owners, facet_id)
            };
            if let Some(tier_row) = tier_row {
                ui.global::<EditorModel>().set_selected_tier_index(
                    i32::try_from(tier_row).expect("tier row index fits in i32"),
                );
                // A tutorial step may wait for a facet of a tier to be picked.
                raise(&ui, events::FACET_PICKED);
            }
            // The label names the clicked facet itself, the same per-facet label the hover shows.
            // The hover re-reads it for the selected facet, so it follows an angle edited since.
            let label = solid_hover_text
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(facet_id as usize)
                .cloned()
                .unwrap_or_default();
            ui.global::<SolidPreviewModel>()
                .set_hover_text(label.into());
            // The clicked facet stays lit regardless of whether it resolved to a
            // tier above -- a facet under the cursor is always a real pick.
            if let Some(preview_state) = auto_solve::preview_state() {
                facet_selection::select_clicked_facet(&preview_state, facet_id);
            } else {
                facet_selection::set_selected_facet_id(Some(facet_id));
            }
        });
}

/// The reverse link: when the tier list's selection changes (a row click, or a viewport click
/// setting `selected_tier_index`), this re-submits the overlay so the tint follows the new
/// `selected_tier`. The mesh is unchanged, so the worker's `MeshCache` hits and only the style
/// and render are redone.
///
/// It also clears [`EditorState::multi_selected`]. Every path that fires `selected_tier_changed`
/// is a plain selection, since Ctrl+click never changes the selected row. So this is where a
/// forgotten multi-select group is dropped, before it widens the next angle nudge. The tier rows
/// are patched in place, not refreshed: clearing the group is not a `Design` edit, so it must not
/// re-label the validation banner "Not solved".
///
/// Runs [`apply_selected_tier_change`] directly. Slint runs a `changed` handler only after the
/// Rust setter that caused it has returned, so no `RefCell` borrow is held at that point.
pub(in crate::gui::editor) fn setup_solid_selected_tier_changed_callback(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    let state = Rc::clone(state);
    let render_ctx = Arc::clone(render_ctx);
    let preview_state = Arc::clone(preview_state);
    let solid_last_solved = Arc::clone(solid_last_solved);
    let ui_weak = ui.as_weak();
    ui.global::<EditorModel>()
        .on_selected_tier_changed(move |index: i32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            apply_selected_tier_change(
                &ui,
                &state,
                &render_ctx,
                &preview_state,
                &solid_last_solved,
            );
            tune_panel::selection_changed();
            // Every route to a selected tier ends here (a table row, a facet in the Solid or
            // Diagram view); a tutorial step may wait for one. Clearing the selection is not
            // a selection.
            if index >= 0 {
                raise(&ui, events::TIER_SELECTED);
            }
        });
}

/// The body of [`setup_solid_selected_tier_changed_callback`]'s handler.
fn apply_selected_tier_change(
    ui: &MainWindow,
    state: &Rc<RefCell<EditorState>>,
    render_ctx: &Arc<Mutex<RenderContext>>,
    preview_state: &Arc<SolidPreviewState>,
    solid_last_solved: &SolidLastSolved,
) {
    // A provisional slice cannot outlive a selection change: this replan renders the
    // committed design, so the slice ends first (before `state` is borrowed).
    drop_provisional_for_selection(ui);
    let mut st = state.borrow_mut();
    // Ends the current coalescing run of nudges, so a run on the tier being left cannot merge a
    // later nudge of that tier. Runs on a real selection change and on a re-click of the selected
    // row (`editor_view.slint` calls `selected_tier_changed` itself, as a same-value write raises
    // no `changed`). It does not run on the nudge control's own ticks.
    st.history.end_coalesce_run();
    if !st.multi_selected.is_empty() {
        st.multi_selected.clear();
        let mut rows: Vec<EditorTierItem> = ui.global::<EditorModel>().get_tiers().iter().collect();
        apply_multi_selection(&mut rows, &st.multi_selected);
        push_tiers(ui, rows);
        push_multi_selected_count(ui, 0);
        resubmit_facet_overlay(preview_state, |overlay| overlay.multi_selected.clear());
    }
    // The inspector's index chips describe whichever tier is now loaded, and
    // a plain row click (no edit) reaches Rust nowhere else.
    push_selected_tier_chips(ui, &st);
    submit_preview_replan(
        ui,
        render_ctx,
        preview_state,
        solid_last_solved,
        &st,
        BTreeSet::new(),
        false,
    );
}
