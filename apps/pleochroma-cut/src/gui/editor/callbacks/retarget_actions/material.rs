//! Resolves and displays the retarget dialog's own TARGET material -- see this
//! group's own `mod.rs` doc comment ("Where the target material comes from"). The
//! logic moved unchanged to `pleochroma_editor::retarget::view` (shared with the web
//! app); re-exported here at its old paths.

pub(super) use pleochroma_editor::retarget::view::{
    initial_target_index, resolve_target_selection, resolved_material_from_selection_for_design,
    target_display_name, target_material_selection,
};
// Only the tests resolve a target without a design's orientation.
#[cfg(test)]
pub(super) use pleochroma_editor::retarget::view::resolved_material_from_selection;
