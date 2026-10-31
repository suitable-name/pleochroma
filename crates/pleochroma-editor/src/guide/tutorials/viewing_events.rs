//! The UI events the viewing, output, library and app tutorials wait for.
//!
//! A tutorial step can read most of what it needs from the design (a tier moved, a tier
//! added) or from the view mode. The rest happens only on screen: a facet was clicked, the
//! Cut slider moved to the rough, a file was exported, the Rough Planner opened. The desktop
//! reports each of those once, by name, through `GuideModel.event`; a step then waits for the
//! name with `Goal::Event`.
//!
//! Every name here is listed in [`ALL`], which `catalog::EVENTS` includes, so a step that
//! waits for a name nobody reports fails the registry test. The desktop raises them through
//! `gui::tutorial_events::raise`, one call at the place the action succeeds, and the
//! desktop's own guard test (`tests/guide_highlights.rs`) checks that every name below has a
//! call site.

/// A tier became the selected one: a row was clicked in the tier table, or a facet was
/// clicked in the Solid or Diagram view.
pub const TIER_SELECTED: &str = "tier_selected";

/// A facet was clicked in the Solid, Path-Traced, Solid with Path-Traced, Ray-Traced or Diagram view and it belongs to a
/// tier.
pub const FACET_PICKED: &str = "facet_picked";

/// The Snap pill was clicked.
pub const SNAP_TOGGLED: &str = "snap_toggled";

/// Slice mode was switched on (the Slice pill, or the S key).
pub const SLICE_STARTED: &str = "slice_started";

/// A line drawn across the stone became a provisional tier.
pub const SLICE_DRAWN: &str = "slice_drawn";

/// The provisional tier was flipped to cut the other side.
pub const SLICE_FLIPPED: &str = "slice_flipped";

/// The provisional tier now touches the stone (its depth handle was dragged inward), so
/// Keep will accept it.
pub const SLICE_CUTS_STONE: &str = "slice_cuts_stone";

/// The Symmetric pill of the Slice tool was clicked.
pub const SLICE_SYMMETRIC_TOGGLED: &str = "slice_symmetric_toggled";

/// The Cut slider was moved all the way left, to the rough.
pub const CUT_SLIDER_ROUGH: &str = "cut_slider_rough";

/// The Cut slider was moved to a position between the rough and the finished stone.
pub const CUT_SLIDER_STEP: &str = "cut_slider_step";

/// The Cut slider was moved all the way right, to the finished stone.
pub const CUT_SLIDER_FINISHED: &str = "cut_slider_finished";

/// One of the Diagram view's three panels was enlarged to fill the view.
pub const DIAGRAM_PANEL_ENLARGED: &str = "diagram_panel_enlarged";

/// A step was marked done in cutting mode.
pub const CUTTING_STEP_MARKED: &str = "cutting_step_marked";

/// An `.asc` file was exported from the editor.
pub const ASC_EXPORTED: &str = "asc_exported";

/// A Gem Cut Studio `.gcs` file was exported from the editor.
pub const GCS_EXPORTED: &str = "gcs_exported";

/// The printable HTML cutting sheet was exported.
pub const SHEET_EXPORTED: &str = "sheet_exported";

/// The diagram PNG was exported.
pub const DIAGRAM_EXPORTED: &str = "diagram_exported";

/// The design was saved to its `.psdd` file.
pub const DESIGN_SAVED: &str = "design_saved";

/// A different design replaced the open one: New Design, Load Selected or Open.
pub const DESIGN_REPLACED: &str = "design_replaced";

/// Designs were imported into the library.
pub const LIBRARY_IMPORTED: &str = "library_imported";

/// The library search box was edited and now holds text.
pub const LIBRARY_SEARCHED: &str = "library_searched";

/// The library search box was edited and is now empty: the learner cleared the search.
pub const LIBRARY_SEARCH_CLEARED: &str = "library_search_cleared";

/// A library filter changed: the shape or gear drop-down, a range slider or the sort order.
pub const LIBRARY_FILTERED: &str = "library_filtered";

/// The library had a search or a filter on, and a change took the last one off: nothing narrows
/// the list any more (the sort order does not count).
pub const LIBRARY_FILTERS_RESET: &str = "library_filters_reset";

/// A design was selected in the library list.
pub const LIBRARY_DESIGN_SELECTED: &str = "library_design_selected";

/// The Rough Planner window opened.
pub const ROUGH_PLANNER_OPENED: &str = "rough_planner_opened";

/// A rough plan finished and its layouts are on screen.
pub const ROUGH_PLAN_FINISHED: &str = "rough_plan_finished";

/// A rough shape was chosen in the Rough Planner: Block, Cylinder, Pebble or Mesh (OBJ/STL/PLY).
pub const ROUGH_SHAPE_CHOSEN: &str = "rough_shape_chosen";

/// The Rough Planner's 3D view was moved: orbited, zoomed, or set with Front, Top, Side or Reset.
pub const ROUGH_VIEW_MOVED: &str = "rough_view_moved";

/// A cut was added by clicking the Rough Planner's 3D view (an edge, a corner or a face).
pub const ROUGH_CUT_FROM_VIEW: &str = "rough_cut_from_view";

/// The Rough Planner's Undo took back a model edit.
pub const ROUGH_MODEL_UNDONE: &str = "rough_model_undone";

/// A result card was picked in the Rough Planner and its layout is in the 3D view.
pub const ROUGH_RESULT_SELECTED: &str = "rough_result_selected";

/// A design row of a result was picked in the Rough Planner, which outlines its stones.
pub const ROUGH_DESIGN_SELECTED: &str = "rough_design_selected";

/// A result's Cut Plan was folded open in the Rough Planner.
pub const ROUGH_CUT_PLAN_OPENED: &str = "rough_cut_plan_opened";

/// The designs of the Rough Planner's results were shown in the library list.
pub const ROUGH_SHOWN_IN_LIBRARY: &str = "rough_shown_in_library";

/// Rough Planner results were saved as a plan.
pub const ROUGH_PLAN_SAVED: &str = "rough_plan_saved";

/// A saved Rough Planner plan was opened from the Saved Plans list.
pub const ROUGH_SAVED_PLAN_OPENED: &str = "rough_saved_plan_opened";

/// A saved Rough Planner plan was exported to a file.
pub const ROUGH_PLAN_EXPORTED: &str = "rough_plan_exported";

/// A planned stone was opened in the editor from the Rough Planner, with its piece of rough.
pub const ROUGH_STONE_OPENED: &str = "rough_stone_opened";

/// The Rough Color wizard window opened from the Rough Planner
/// (`gui/rough_colour/wizard/host.rs`).
pub const ROUGH_COLOR_WIZARD_OPENED: &str = "rough_color_wizard_opened";

/// Best Color was chosen under Stone Orientation in the Rough Planner
/// (`gui/rough_plan/zoning_hooks.rs`).
pub const ROUGH_COLOR_BEST_CHOSEN: &str = "rough_color_best_chosen";

/// Use Color was pressed on a design row of the Rough Planner and the design was handed to the
/// editor with the rough's color (`gui/rough_plan/zoning_hooks.rs`).
pub const ROUGH_COLOR_USED: &str = "rough_color_used";

/// A custom material was saved in the Material Editor.
pub const CUSTOM_MATERIAL_SAVED: &str = "custom_material_saved";

/// A custom material was saved with its refractive index typed as Sellmeier or Cauchy
/// coefficients.
pub const COEFFICIENT_MATERIAL_SAVED: &str = "coefficient_material_saved";

/// The Color Pattern window opened for a material (`gui/color_pattern/wiring.rs`).
pub const PATTERN_EDITOR_OPENED: &str = "pattern_editor_opened";

/// A template was picked in the Color Pattern window (`gui/color_pattern/actions.rs`).
pub const PATTERN_TEMPLATE_PICKED: &str = "pattern_template_picked";

/// The Color Pattern window stored the pattern with its material, with Apply or Save
/// (`gui/color_pattern/actions.rs`).
pub const PATTERN_SAVED: &str = "pattern_saved";

/// The Live Render tab was shown.
pub const LIVE_RENDER_OPENED: &str = "live_render_opened";

/// A lighting rig was chosen, in the Live Render toolbar or the Lighting & Environment tab.
pub const LIGHTING_PRESET_CHOSEN: &str = "lighting_preset_chosen";

/// The lighting on screen was saved for the open design.
pub const DESIGN_LIGHTING_SAVED: &str = "design_lighting_saved";

/// The open design's saved lighting was forgotten.
pub const DESIGN_LIGHTING_FORGOTTEN: &str = "design_lighting_forgotten";

/// The Inclusion Haze slider of the Stone tab changed the design's haze (the committed edit,
/// `gui/render/stone_effects.rs`).
pub const STONE_HAZE_CHANGED: &str = "stone_haze_changed";

/// The Frosted Girdle switch of the Stone tab changed the design (`gui/render/stone_effects.rs`).
pub const STONE_GIRDLE_FROSTED_CHANGED: &str = "stone_girdle_frosted_changed";

/// The Edge Rounding slider of the Stone tab changed the design (`gui/render/stone_effects.rs`).
pub const STONE_EDGE_ROUNDING_CHANGED: &str = "stone_edge_rounding_changed";

/// A Stone Width was set in the design from the Stone tab or the Preform tab
/// (`gui/editor/callbacks/tier_actions/viewport_material.rs`).
pub const STONE_WIDTH_SET: &str = "stone_width_set";

/// The toolbar's Crystal Axis button was pressed, to show or to hide the axis over the stone
/// (`gui/render/crystal_axis_ui.rs`).
pub const CRYSTAL_AXIS_TOGGLED: &str = "crystal_axis_toggled";

/// Set for This Stone was chosen in the Stone tab's Crystal Axis section
/// (`gui/render/material.rs`).
pub const CRYSTAL_AXIS_SET_FOR_STONE: &str = "crystal_axis_set_for_stone";

/// As Material was chosen in the Stone tab's Crystal Axis section (`gui/render/material.rs`).
pub const CRYSTAL_AXIS_AS_MATERIAL: &str = "crystal_axis_as_material";

/// A geometry preset of the Crystal Axis section was chosen: Through the Table, In the Table Plane
/// or Along the Width (`gui/render/crystal_axis_ui.rs`).
pub const CRYSTAL_AXIS_PRESET_CHOSEN: &str = "crystal_axis_preset_chosen";

/// A preset named by the color it gives was chosen in the Crystal Axis section, for example Best
/// Blue Face-Up (`gui/render/crystal_axis_ui.rs`).
pub const CRYSTAL_AXIS_COLOR_PRESET_CHOSEN: &str = "crystal_axis_color_preset_chosen";

/// The Second Axis switch or its Turn slider of the Crystal Axis section changed
/// (`gui/render/crystal_axis_ui.rs`).
pub const CRYSTAL_SECOND_AXIS_CHANGED: &str = "crystal_second_axis_changed";

/// A drag of the Crystal Axis handle over the stone was released and kept
/// (`gui/editor/manipulate/crystal_axis_overlay.rs`).
pub const CRYSTAL_AXIS_DRAGGED: &str = "crystal_axis_dragged";

/// A drag of the Crystal Axis handle was cancelled with Escape, which put the axis back
/// (`gui/editor/manipulate/crystal_axis_overlay.rs`).
pub const CRYSTAL_AXIS_DRAG_CANCELLED: &str = "crystal_axis_drag_cancelled";

/// The rough's Crystal Axis was set in the Rough Planner: Set, a typed angle, Align to Face or
/// Edge, Fit to Crystal Length, From Color Zones or a drag of the axis line
/// (`gui/rough_plan/axis/panel.rs`).
pub const ROUGH_AXIS_SET: &str = "rough_axis_set";

/// One stone of a Rough Planner result was given its own pose or a Reason
/// (`gui/rough_plan/axis/stone.rs`).
pub const ROUGH_STONE_OVERRIDDEN: &str = "rough_stone_overridden";

/// One stone of a Rough Planner result was locked, so a new plan cuts the rest of the rough
/// around it (`gui/rough_plan/axis/stone.rs`).
pub const ROUGH_STONE_LOCKED: &str = "rough_stone_locked";

/// The Simple | Advanced switch was used.
pub const INTERFACE_MODE_CHANGED: &str = "interface_mode_changed";

/// The High contrast switch in Preferences was used.
pub const HIGH_CONTRAST_CHANGED: &str = "high_contrast_changed";

/// The Interface scale in Preferences was changed.
pub const UI_SCALE_CHANGED: &str = "ui_scale_changed";

/// The Larger handles switch in Preferences was used.
pub const LARGE_HANDLES_CHANGED: &str = "large_handles_changed";

/// The keyboard shortcuts list opened (Help menu, or the ? key).
pub const SHORTCUTS_OPENED: &str = "shortcuts_opened";

/// The manual opened in the help window (from the Help menu, F1 or a round ? button).
pub const HELP_OPENED: &str = "help_opened";

/// The glossary opened.
pub const GLOSSARY_OPENED: &str = "glossary_opened";

/// The export dialog of Live Render opened (`gui/render/render_export/wiring.rs`).
pub const RENDER_EXPORT_OPENED: &str = "render_export_opened";

/// Add to Queue stored a picture or a tilt video as a render job (`gui/render_jobs`).
pub const RENDER_JOB_ADDED: &str = "render_job_added";

/// The Render Jobs window opened (`gui/render_jobs/wiring.rs`).
pub const RENDER_JOBS_OPENED: &str = "render_jobs_opened";

/// Start Queue began rendering the waiting jobs (`gui/render_jobs/wiring.rs`).
pub const RENDER_QUEUE_STARTED: &str = "render_queue_started";

/// Pause Queue was pressed in the Render Jobs window (`gui/render_jobs/wiring.rs`).
pub const RENDER_QUEUE_PAUSED: &str = "render_queue_paused";

/// A paused or failed render job was put back in line with its Resume button
/// (`gui/render_jobs/wiring.rs`).
pub const RENDER_JOB_RESUMED: &str = "render_job_resumed";

/// Export Script wrote a script folder for the unfinished render jobs
/// (`gui/render_jobs/script_export.rs`).
pub const RENDER_SCRIPT_EXPORTED: &str = "render_script_exported";

/// Every event above, for `catalog::EVENTS` and for the desktop's guard test.
pub const ALL: &[&str] = &[
    TIER_SELECTED,
    FACET_PICKED,
    SNAP_TOGGLED,
    SLICE_STARTED,
    SLICE_DRAWN,
    SLICE_FLIPPED,
    SLICE_CUTS_STONE,
    SLICE_SYMMETRIC_TOGGLED,
    CUT_SLIDER_ROUGH,
    CUT_SLIDER_STEP,
    CUT_SLIDER_FINISHED,
    DIAGRAM_PANEL_ENLARGED,
    CUTTING_STEP_MARKED,
    ASC_EXPORTED,
    GCS_EXPORTED,
    SHEET_EXPORTED,
    DIAGRAM_EXPORTED,
    DESIGN_SAVED,
    DESIGN_REPLACED,
    LIBRARY_IMPORTED,
    LIBRARY_SEARCHED,
    LIBRARY_SEARCH_CLEARED,
    LIBRARY_FILTERED,
    LIBRARY_FILTERS_RESET,
    LIBRARY_DESIGN_SELECTED,
    ROUGH_PLANNER_OPENED,
    ROUGH_PLAN_FINISHED,
    ROUGH_SHAPE_CHOSEN,
    ROUGH_VIEW_MOVED,
    ROUGH_CUT_FROM_VIEW,
    ROUGH_MODEL_UNDONE,
    ROUGH_RESULT_SELECTED,
    ROUGH_DESIGN_SELECTED,
    ROUGH_CUT_PLAN_OPENED,
    ROUGH_SHOWN_IN_LIBRARY,
    ROUGH_PLAN_SAVED,
    ROUGH_SAVED_PLAN_OPENED,
    ROUGH_PLAN_EXPORTED,
    ROUGH_STONE_OPENED,
    ROUGH_COLOR_WIZARD_OPENED,
    ROUGH_COLOR_BEST_CHOSEN,
    ROUGH_COLOR_USED,
    CUSTOM_MATERIAL_SAVED,
    COEFFICIENT_MATERIAL_SAVED,
    PATTERN_EDITOR_OPENED,
    PATTERN_TEMPLATE_PICKED,
    PATTERN_SAVED,
    LIVE_RENDER_OPENED,
    LIGHTING_PRESET_CHOSEN,
    DESIGN_LIGHTING_SAVED,
    DESIGN_LIGHTING_FORGOTTEN,
    STONE_HAZE_CHANGED,
    STONE_GIRDLE_FROSTED_CHANGED,
    STONE_EDGE_ROUNDING_CHANGED,
    STONE_WIDTH_SET,
    CRYSTAL_AXIS_TOGGLED,
    CRYSTAL_AXIS_SET_FOR_STONE,
    CRYSTAL_AXIS_AS_MATERIAL,
    CRYSTAL_AXIS_PRESET_CHOSEN,
    CRYSTAL_AXIS_COLOR_PRESET_CHOSEN,
    CRYSTAL_SECOND_AXIS_CHANGED,
    CRYSTAL_AXIS_DRAGGED,
    CRYSTAL_AXIS_DRAG_CANCELLED,
    ROUGH_AXIS_SET,
    ROUGH_STONE_OVERRIDDEN,
    ROUGH_STONE_LOCKED,
    INTERFACE_MODE_CHANGED,
    HIGH_CONTRAST_CHANGED,
    UI_SCALE_CHANGED,
    LARGE_HANDLES_CHANGED,
    SHORTCUTS_OPENED,
    HELP_OPENED,
    GLOSSARY_OPENED,
    RENDER_EXPORT_OPENED,
    RENDER_JOB_ADDED,
    RENDER_JOBS_OPENED,
    RENDER_QUEUE_STARTED,
    RENDER_QUEUE_PAUSED,
    RENDER_JOB_RESUMED,
    RENDER_SCRIPT_EXPORTED,
];

/// The cut slider event for a `SolidPreviewModel.tier_cutoff` value: `-2` is the rough,
/// `-1` the finished stone and `0` or more a position after that many steps.
#[must_use]
pub const fn cut_slider_event(cutoff: i32) -> &'static str {
    match cutoff {
        -2 => CUT_SLIDER_ROUGH,
        -1 => CUT_SLIDER_FINISHED,
        _ => CUT_SLIDER_STEP,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_name_is_listed_once_and_is_lower_case_words() {
        for (index, name) in ALL.iter().enumerate() {
            assert!(!ALL[..index].contains(name), "{name} is listed twice");
            assert!(
                name.chars().all(|c| c.is_ascii_lowercase() || c == '_'),
                "{name} is not lower_snake_case"
            );
        }
    }

    #[test]
    fn the_cut_slider_events_follow_the_cutoff() {
        assert_eq!(cut_slider_event(-2), CUT_SLIDER_ROUGH);
        assert_eq!(cut_slider_event(-1), CUT_SLIDER_FINISHED);
        assert_eq!(cut_slider_event(0), CUT_SLIDER_STEP);
        assert_eq!(cut_slider_event(7), CUT_SLIDER_STEP);
    }
}
