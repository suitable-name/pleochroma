//! Resolves and displays the retarget dialog's own target material. The logic lives in
//! `pleochroma_editor::retarget::view`, shared with the web app, and is re-exported here.

pub(super) use pleochroma_editor::retarget::view::{
    initial_target_index, resolve_target_selection, resolved_material_from_selection_for_design,
    target_display_name, target_material_selection,
};
// Only the tests resolve a target without a design's orientation.
#[cfg(test)]
pub(super) use pleochroma_editor::retarget::view::resolved_material_from_selection;
