//! The crystal axis state of the planner window that is not part of the undoable model: which
//! pick the next click on the 3D view makes, the stone being turned by hand, the open stone menu
//! and the materials looked up in the library.

use crate::gui::rough_plan::session::LayoutEdits;
use glam::DVec3;
use pleochroma::optics::materials::GemMaterial;
use pleochroma_cut_core::rough_plan::{
    DesignHull, PlacedStone, PlanSettings, RoughLayout, shaped::ShapedCtx,
};
use std::collections::{BTreeMap, BTreeSet};

/// What the next click on the 3D view picks for the Crystal Axis.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(in crate::gui::rough_plan) enum PickMode {
    /// No pick: a click does what it always did.
    #[default]
    Off,
    /// The next click picks a face of the rough; its normal becomes the Crystal Axis.
    Face,
    /// The next click picks the first end of an edge.
    EdgeFirst,
    /// The first end is picked (rough frame, mm); the next click picks the second.
    EdgeSecond([f64; 3]),
}

impl PickMode {
    /// The code the window shows: `0` none, `1` a face, `2` the first end of an edge, `3` the
    /// second end.
    #[must_use]
    pub(in crate::gui::rough_plan) const fn code(self) -> i32 {
        match self {
            Self::Off => 0,
            Self::Face => 1,
            Self::EdgeFirst => 2,
            Self::EdgeSecond(_) => 3,
        }
    }

    /// Whether a pick is under way.
    #[must_use]
    pub(in crate::gui::rough_plan) const fn is_on(self) -> bool {
        !matches!(self, Self::Off)
    }
}

/// What turning one stone by hand (Free Orientation) needs, loaded once when it starts so a drag
/// does not read the library at every step.
pub(in crate::gui::rough_plan) struct FreeKit {
    /// The design's convex outline, in its caliper frame.
    pub(in crate::gui::rough_plan) hull: DesignHull,
    /// The rough's usable region (inset by skin and allowance), rough frame.
    pub(in crate::gui::rough_plan) region: Vec<(DVec3, f64)>,
    /// The shaped planner's context, for a rough that has cuts that are not box faces or a mesh.
    pub(in crate::gui::rough_plan) ctx: Option<ShapedCtx>,
    /// The settings the plan was made with.
    pub(in crate::gui::rough_plan) settings: PlanSettings,
    /// The rough's volume, for the layout's yield.
    pub(in crate::gui::rough_plan) model_volume_mm3: f64,
    /// The stones of the layout after the first (the locked ones), which a re-built layout keeps.
    pub(in crate::gui::rough_plan) tail: Vec<PlacedStone>,
    /// The kit's region and mesh lie in a frame this far from the rough's own (the rough with the
    /// locked pieces left out may have moved): `original = kit frame + offset_mm`.
    pub(in crate::gui::rough_plan) offset_mm: [f64; 3],
}

/// The stone being turned by hand: its result, its place in the result, and what the turning
/// needs.
pub(in crate::gui::rough_plan) struct FreeEdit {
    /// The result's place in the shown results.
    pub(in crate::gui::rough_plan) result: usize,
    /// The stone's place in the result.
    pub(in crate::gui::rough_plan) stone: usize,
    /// What the turning needs.
    pub(in crate::gui::rough_plan) kit: FreeKit,
}

/// The selection the window had before a result list was shown again (a re-fit or a turned stone
/// changes the figures of a result, which are formatted afresh), put back afterwards.
#[derive(Debug, Clone, Default)]
pub(in crate::gui::rough_plan) struct SavedSelection {
    /// The selected result, `-1` for none.
    pub(in crate::gui::rough_plan) result: i32,
    /// The selected design row, `-1` for none.
    pub(in crate::gui::rough_plan) group: i32,
    /// The keep tick of every result.
    pub(in crate::gui::rough_plan) keep: Vec<bool>,
}

/// The crystal axis state of the window.
#[derive(Default)]
pub(in crate::gui::rough_plan) struct AxisState {
    /// Catalogue materials looked up so far, by the name the material list shows.
    pub(in crate::gui::rough_plan) materials: BTreeMap<String, GemMaterial>,
    /// Names whose lookup has been asked for (so a material is looked up once).
    pub(in crate::gui::rough_plan) asked: BTreeSet<String>,
    /// The pick the next click on the 3D view makes.
    pub(in crate::gui::rough_plan) pick: PickMode,
    /// The stone being turned by hand, if any.
    pub(in crate::gui::rough_plan) free: Option<FreeEdit>,
    /// The note under the axis panel (the result of the last pick or fit) and its level: `0`
    /// neutral, `1` good, `2` weak, `3` error.
    pub(in crate::gui::rough_plan) note: Option<(String, i32)>,
    /// The selection to put back once the results are shown again.
    pub(in crate::gui::rough_plan) restore: Option<SavedSelection>,
    /// The layouts as the plan (or the saved plan) made them: what the layouts the cutter
    /// changed are laid over.
    pub(in crate::gui::rough_plan) base: Vec<RoughLayout>,
    /// The changed layouts the shown (or the next shown) results were built with.
    pub(in crate::gui::rough_plan) shown_edits: LayoutEdits,
    /// The cut plan text of every result as the formatter made it, before the lines about the
    /// crystal axis were added.
    pub(in crate::gui::rough_plan) cut_plans: Vec<String>,
}

impl AxisState {
    /// Forgets the edit in progress (a pick, a stone being turned): the model it belonged to is
    /// gone.
    pub(in crate::gui::rough_plan) fn reset_edit(&mut self) {
        self.pick = PickMode::Off;
        self.free = None;
        self.note = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pick_has_a_code_and_only_off_is_not_a_pick() {
        assert_eq!(PickMode::Off.code(), 0);
        assert_eq!(PickMode::Face.code(), 1);
        assert_eq!(PickMode::EdgeFirst.code(), 2);
        assert_eq!(PickMode::EdgeSecond([0.0; 3]).code(), 3);
        assert!(!PickMode::Off.is_on());
        assert!(PickMode::EdgeSecond([1.0, 2.0, 3.0]).is_on());
    }

    #[test]
    fn resetting_the_edit_clears_the_pick_and_the_note() {
        let mut state = AxisState {
            pick: PickMode::Face,
            note: Some(("x".to_string(), 1)),
            ..AxisState::default()
        };
        state.reset_edit();
        assert_eq!(state.pick, PickMode::Off);
        assert!(state.note.is_none() && state.free.is_none());
    }
}
