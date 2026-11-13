//! Rust-side wiring for the Diagram view mode's own hover/click callbacks.
//!
//! `SolidPreviewModel`'s `diagram_facet_hover`/`diagram_facet_click`, raised by
//! `solid_viewport.slint` -- see that component's own doc comment for why Diagram mode
//! needs separate callbacks from the Solid rasterizer's `facet_hover`/`facet_click`.
//!
//! These callbacks need no `Design` or `FacetMap`: the worker builds the facet-id-indexed
//! hover text and tier tables ([`super::preview_state::PreviewFrame::diagram_hover_text`])
//! and hands them over as shared `Arc<Mutex<..>>` state. So this module needs no
//! dependency on `gui::editor`.

use super::{
    facet_selection::{
        clear_clicked_facet, clicked_tier_row, select_clicked_facet, selected_facet_id,
        set_hovered_facet,
    },
    preview_state::{FacetOwners, PickBuffer, SolidPreviewState},
};
use crate::{
    EditorModel, MainWindow, ManipulateModel, SolidPreviewModel, gui::tutorial_events::raise,
};
use pleochroma_editor::guide::viewing_events as events;
use pleochroma_solid::preview::{panel_kind_from_index, view::toggle_enlarged_panel};
use slint::ComponentHandle;
use std::sync::{Arc, Mutex, PoisonError};

/// The diagram's own per-pixel facet-picking buffer, one frame behind.
///
/// Same contract as `gui::mod`'s `solid_pick` for the ordinary Solid view --
/// written by `SlintSolidSink::apply` from `PreviewFrame::diagram_pick`.
///
/// The index wheel's tooth-picking buffer (`PreviewFrame::diagram_tooth_pick`) has this
/// same type; see the `tooth_pick` parameter of [`setup_diagram_hover_and_click_callbacks`].
pub type DiagramPick = Arc<Mutex<Option<PickBuffer>>>;
/// Facet id -> hover tooltip text, from `PreviewFrame::diagram_hover_text`.
pub type DiagramHoverText = Arc<Mutex<Option<Vec<String>>>>;
/// Facet id -> owning flat and concave tier, from `PreviewFrame::diagram_facet_owners`.
pub type DiagramFacetOwners = Arc<Mutex<Option<FacetOwners>>>;

/// Returns the index-wheel tooth at physical pixel `(px, py)` in `tooth_pick`, if any.
/// Shared by the hover and click callbacks.
fn tooth_at(tooth_pick: &DiagramPick, px: u32, py: u32) -> Option<u32> {
    tooth_pick
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .and_then(|tooth_pick| tooth_pick.facet_at(px, py))
}

/// Wires `SolidPreviewModel`'s `diagram_facet_hover`/`diagram_facet_click`. Call once from
/// `gui::build_main_window`, alongside the ordinary Solid hover/click wiring.
///
/// The writes go through [`super::facet_selection`], which merges them into the overlay
/// the Solid view shares: a hover sets only the hovered facet, a click sets the clicked
/// facet as the manipulation handles' anchor, and neither clears the other's outline,
/// multi-selection or drag followers. The hover highlight clears exactly when
/// `diagram_hover_text` does.
///
/// `x`/`y` arrive from Slint in logical pixels, while the pick buffers are sized to the
/// window's physical resolution, so they are scaled by `scale_factor()` before indexing.
pub fn setup_diagram_hover_and_click_callbacks(
    ui: &MainWindow,
    pick: &DiagramPick,
    tooth_pick: &DiagramPick,
    hover_text: &DiagramHoverText,
    facet_owners: &DiagramFacetOwners,
    preview_state: &Arc<SolidPreviewState>,
) {
    {
        let pick = Arc::clone(pick);
        let tooth_pick = Arc::clone(tooth_pick);
        let hover_text = Arc::clone(hover_text);
        let preview_state = Arc::clone(preview_state);
        let ui_weak = ui.as_weak();
        ui.global::<SolidPreviewModel>()
            .on_diagram_facet_hover(move |x: f32, y: f32| {
                let Some(ui) = ui_weak.upgrade() else {
                    return;
                };
                let scale = ui.window().scale_factor();
                let px = pixel_coordinate(x * scale);
                let py = pixel_coordinate(y * scale);
                let (pick_dimensions, facet_id) = {
                    let pick = pick.lock().unwrap_or_else(PoisonError::into_inner);
                    pick.as_ref().map_or((None, None), |pick| {
                        (Some((pick.width, pick.height)), pick.facet_at(px, py))
                    })
                };
                let (label_count, hit_label, selected_label) = {
                    let guard = hover_text.lock().unwrap_or_else(PoisonError::into_inner);
                    let labels = guard.as_ref();
                    let label_count = labels.map(Vec::len);
                    let hit_label = facet_id
                        .and_then(|id| labels.and_then(|table| table.get(id as usize).cloned()));
                    // The clicked facet's label is read from the CURRENT frame's table each
                    // time (not cached at click time), so an angle dragged since the click
                    // reads at its new value.
                    let selected_label = selected_facet_id()
                        .and_then(|id| labels.and_then(|table| table.get(id as usize).cloned()))
                        .unwrap_or_default();
                    drop(guard);
                    (label_count, hit_label, selected_label)
                };
                #[cfg(not(feature = "ui-harness"))]
                let _ = (pick_dimensions, label_count);
                let Some(facet_id) = facet_id else {
                    // No facet here: report the index-wheel tooth under the cursor, if any.
                    let tooth = tooth_at(&tooth_pick, px, py);
                    // With neither a facet nor a tooth, keep the last clicked facet's label
                    // so the selection stays readable after the pointer leaves it.
                    let text = tooth.map_or(selected_label, |t| format!("Index {t}"));
                    #[cfg(feature = "ui-harness")]
                    crate::harness_support::record_hover_diagnostic(
                        crate::harness_support::HoverViewport::Diagram,
                        x,
                        y,
                        scale,
                        pick_dimensions,
                        None,
                        label_count,
                        Some(text.clone()),
                    );
                    ui.global::<SolidPreviewModel>()
                        .set_diagram_hover_text(text.into());
                    // Only the hover clears; the clicked facet stays lit.
                    set_hovered_facet(&preview_state, None);
                    return;
                };
                let text = hit_label.unwrap_or_default();
                #[cfg(feature = "ui-harness")]
                crate::harness_support::record_hover_diagnostic(
                    crate::harness_support::HoverViewport::Diagram,
                    x,
                    y,
                    scale,
                    pick_dimensions,
                    Some(facet_id),
                    label_count,
                    Some(text.clone()),
                );
                ui.global::<SolidPreviewModel>()
                    .set_diagram_hover_text(text.into());
                set_hovered_facet(&preview_state, Some(facet_id));
            });
    }

    {
        let pick = Arc::clone(pick);
        let tooth_pick = Arc::clone(tooth_pick);
        let facet_owners = Arc::clone(facet_owners);
        let hover_text = Arc::clone(hover_text);
        let preview_state = Arc::clone(preview_state);
        let ui_weak = ui.as_weak();
        ui.global::<SolidPreviewModel>()
            .on_diagram_facet_click(move |x: f32, y: f32| {
                let Some(ui) = ui_weak.upgrade() else {
                    return;
                };
                handle_diagram_facet_click(
                    &ui,
                    x,
                    y,
                    DiagramPickBuffers {
                        pick: &pick,
                        tooth_pick: &tooth_pick,
                        facet_owners: &facet_owners,
                        hover_text: &hover_text,
                    },
                    &preview_state,
                );
            });
    }
}

/// Wires `SolidPreviewModel.diagram_double_click`: enlarge the panel under the pointer.
///
/// A double-click on a panel enlarges it to fill the view, and a double-click while
/// a panel is enlarged (or on no panel) goes back to all three -- the same rule as
/// the web app, through `pleochroma_solid::preview::view::toggle_enlarged_panel`.
///
/// `panel_pick` is the last diagram frame's per-pixel panel buffer
/// (`PreviewFrame::diagram_panel_pick`), read with the same logical-to-physical scaling as
/// the hover and click callbacks. Only the property is written here; the redraw follows
/// from its `changed` handler, so a pill click, the Escape key and this double-click share
/// one redraw path.
pub fn setup_diagram_double_click_callback(ui: &MainWindow, panel_pick: &DiagramPick) {
    let panel_pick = Arc::clone(panel_pick);
    let ui_weak = ui.as_weak();
    ui.global::<SolidPreviewModel>()
        .on_diagram_double_click(move |x: f32, y: f32| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let scale = ui.window().scale_factor();
            let px = pixel_coordinate(x * scale);
            let py = pixel_coordinate(y * scale);
            let clicked = panel_pick
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .as_ref()
                .and_then(|buffer| buffer.facet_at(px, py))
                .and_then(|index| panel_kind_from_index(i32::try_from(index).ok()?));
            let model = ui.global::<SolidPreviewModel>();
            let next = toggle_enlarged_panel(model.get_diagram_enlarged_panel(), clicked);
            model.set_diagram_enlarged_panel(next);
        });
}

/// A pixel position as an unsigned pixel index: a negative or NaN position saturates to
/// `0`, which is what the pick-buffer lookups rely on (they bounds-check against the
/// frame's size).
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "saturating float-to-int cast is the intent: negative or NaN maps to 0, the pick buffer bounds-checks"
)]
const fn pixel_coordinate(position: f32) -> u32 {
    position as u32
}

/// The Diagram view's shared pick-buffer/hover-text/facet-tier handles --
/// bundled (rather than four more parameters) purely to keep
/// [`handle_diagram_facet_click`] under clippy's argument-count lint.
#[derive(Clone, Copy)]
struct DiagramPickBuffers<'a> {
    pick: &'a DiagramPick,
    tooth_pick: &'a DiagramPick,
    facet_owners: &'a DiagramFacetOwners,
    hover_text: &'a DiagramHoverText,
}

/// [`setup_diagram_hover_and_click_callbacks`]'s click handler body -- split
/// out purely to keep that function under clippy's function-length lint.
fn handle_diagram_facet_click(
    ui: &MainWindow,
    x: f32,
    y: f32,
    buffers: DiagramPickBuffers<'_>,
    preview_state: &SolidPreviewState,
) {
    let DiagramPickBuffers {
        pick,
        tooth_pick,
        facet_owners,
        hover_text,
    } = buffers;
    let scale = ui.window().scale_factor();
    let px = pixel_coordinate(x * scale);
    let py = pixel_coordinate(y * scale);
    let Some(facet_id) = pick
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .and_then(|pick| pick.facet_at(px, py))
    else {
        // A miss on every facet may still hit a tooth of the index wheel.
        let tooth = tooth_at(tooth_pick, px, py);
        if let Some(tooth) = tooth {
            let tooth = tooth.cast_signed();
            ui.global::<SolidPreviewModel>()
                .set_diagram_clicked_tooth(tooth);
            // Invoked directly as well as through the `tracked_clicked_tooth` mirror in
            // `editor_tier_table.slint`: that mirror fires only on a changed value, so a second
            // click on the same tooth would otherwise do nothing.
            ui.global::<EditorModel>().invoke_highlight_tooth(tooth);
            return;
        }
        // A miss on both the silhouette and the wheel clears the tier selection. Setting
        // `selected_tier_index` alone is enough: its `changed` handler clears the inspector.
        clear_clicked_facet(preview_state);
        ui.global::<EditorModel>().set_selected_tier_index(-1);
        // Reset the clicked tooth too, so a later click on the same tooth still changes the
        // value and the mirror's `changed` handler fires.
        ui.global::<SolidPreviewModel>()
            .set_diagram_clicked_tooth(-1);
        ui.global::<EditorModel>().invoke_highlight_tooth(-1);
        // Nothing is selected any more, so the tooltip must not keep the old facet's label.
        ui.global::<SolidPreviewModel>()
            .set_diagram_hover_text("".into());
        return;
    };
    // While the Slice tool holds a provisional tier the frame may name a tier the editor
    // does not have (one past the flat tiers, which is where the first concave row sits),
    // so a click selects no row until Keep or Discard -- the Solid view's own rule.
    let tier_row = if ui.global::<ManipulateModel>().get_provisional_active() {
        None
    } else {
        facet_owners
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .and_then(|owners| clicked_tier_row(ui, owners, facet_id))
    };
    // The same selected facet the Solid view's click sets, so the manipulation handles
    // have an anchor facet whichever view the click was made in; merged into the
    // overlay, so the hover and the multi-select survive it.
    select_clicked_facet(preview_state, facet_id);
    if let Some(tier_row) = tier_row {
        ui.global::<EditorModel>()
            .set_selected_tier_index(i32::try_from(tier_row).expect("tier row index fits in i32"));
        // A tutorial step may wait for a facet of a tier to be picked.
        raise(ui, events::FACET_PICKED);
    }
    // The clicked facet's own label, the same one the hover shows. The hover's miss branch
    // re-reads it for the selected facet, so it survives the pointer leaving the facet.
    let label = hover_text
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .and_then(|table| table.get(facet_id as usize))
        .cloned()
        .unwrap_or_default();
    ui.global::<SolidPreviewModel>()
        .set_diagram_hover_text(label.into());
}
