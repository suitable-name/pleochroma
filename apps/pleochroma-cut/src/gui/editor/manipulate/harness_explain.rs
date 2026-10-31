//! Harness-only: why the drag handles are hidden right now, step by step through the same
//! checks [`handles::refresh_handles`] makes, for the tutorial videos' failure messages.

use super::{CTX, diagram, handles};
use crate::{EditorModel, MainWindow, ManipulateModel, gui::editor::callbacks::selected_facet_id};
use slint::ComponentHandle as _;
use std::sync::PoisonError;

/// The first check that hides the handles, or "placeable" when none does (the handles then
/// show on the next frame that lands).
pub fn handles_hidden_reason(ui: &MainWindow) -> String {
    let Some(ctx) = CTX.with(|cell| cell.borrow().clone()) else {
        return "the manipulate callbacks are not set up".into();
    };
    let Some(geometry) = ctx
        .geometry
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
    else {
        return "no frame geometry stored yet".into();
    };
    if diagram::is_diagram_view(ui) {
        return "Diagram view (handles are placed by the diagram code)".into();
    }
    if geometry.diagram.is_some() {
        return "the last frame is a Diagram frame".into();
    }
    if ui.global::<ManipulateModel>().get_slice_mode() {
        return "Slice mode".into();
    }
    let Ok(tier) = usize::try_from(ui.global::<EditorModel>().get_selected_tier_index()) else {
        return "no tier selected".into();
    };
    let Ok(st) = ctx.state.try_borrow() else {
        return "the editor state is borrowed".into();
    };
    let solved = ctx
        .solid_last_solved
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .as_ref()
        .map(|(generation, masts)| (*generation, masts.len()));
    let Some(map) = handles::facet_map_for(&ctx, st.current_generation(), &st.design) else {
        return format!(
            "no facet map: design has {} tiers, cached solved masts (generation, count) {solved:?}",
            st.design.tiers.len()
        );
    };
    let centroids = geometry.facet_centroids.len();
    if !handles::ids_aligned(map.facet_count(), centroids) {
        return format!(
            "facet ids not aligned: map has {} facets, the frame {centroids} centroids",
            map.facet_count()
        );
    }
    let facet = selected_facet_id();
    let on_tier = map.facets_of_tier(tier).len();
    if handles::target_for(&geometry, &map, &st.design, tier, facet, false).is_none() {
        return format!(
            "no target: tier {tier} has {on_tier} facets in the map, remembered facet {facet:?}, \
             camera {:?}, frame size {:?}",
            geometry.camera, geometry.size
        );
    }
    "placeable".into()
}
