//! The crystal axis in the Rough Planner: the rough's orientation, the rules that say how stones
//! are cut against it, and the overrides a cutter makes.
//!
//! # The pieces
//!
//! - [`rough`]: how the rough's orientation is defined (a face, an edge, the crystal's length,
//!   the color zones) as a [`CrystalOrientation`](crate::crystal_axis::CrystalOrientation) in the
//!   rough frame. The orientation itself lives on `RoughModel::orientation`, so it is in the shape
//!   undo stack.
//! - [`resolved`]: a rule resolved for the search ([`ResolvedRule`], `Copy`): the target, the
//!   tolerance, Preferred or Required, and the rough's orientation. A `CandidateDesign` carries
//!   its own, and `Norm::of` turns it into the factor of each of the six assignments; every
//!   valuation of the block planner, the uniform pass, the refinement and the shaped planner goes
//!   through that one factor (see `piece::stone_value`), so the DPs themselves are unchanged.
//! - [`state`]: the cutter's rule settings ([`PlanSettingsOverrides`]): the plan's rule (the
//!   material's default until changed, switched on only once the rough has an orientation), a rule
//!   per design, a Reason for each, and the stone choices. The planner window keeps it beside the
//!   model in one undo entry.
//! - [`choices`]: per-stone choices ([`StoneChoice`]: one of the four box poses, a free pose,
//!   Lock, Reason) and the single-piece re-fit ("Re-fit This Piece").
//! - [`lock`]: Lock, the piece of a locked stone kept out of the next plan.
//! - [`report`]: what the results show for a stone: the TRUE orientation from its actual pose
//!   (never the rule's target), and the row data (angle to the table, followed or off,
//!   overridden).
//! - [`alternatives`]: the three layouts of a Preferred rule (best yield, best orientation, the
//!   plan's own weight).
//!
//! # Compatibility
//!
//! A plan without an orientation or a rule changes nothing: a design's `orientation` is `None`,
//! every factor is `1`, and no valuation changes by a single bit.

pub mod alternatives;
pub mod choices;
pub mod lock;
pub mod report;
pub mod resolved;
pub mod rough;
pub mod state;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_search;
#[cfg(test)]
mod tests_state;

pub use alternatives::{
    AlternativeKind, OrientationAlternative, apply_alternative_rules, plan_orientation_alternatives,
};
pub use choices::{
    StoneChoice, apply_suggested_poses, refit_block_piece, refit_shaped_piece, suggested_poses,
};
pub use lock::{
    LOCKED_FIRST_NOTE, LockError, LockedLayout, LockedRough, locked_shell, restore_frame,
    rough_with_locked_pieces,
};
pub use report::{
    OrientationRow, RuleStatus, stone_orientation_in_stone_frame, stone_orientation_row,
};
pub use resolved::{ResolvedRule, RuleHold, RuleTarget};
pub use rough::{
    CrystalLengthFit, align_to_edge, align_to_face, fit_to_crystal_length,
    fit_to_crystal_length_of, orientation_from_color_zones,
};
pub use state::{
    PlanSettingsOverrides, ResolveContext, RuleChoice, RuleLine, RuleSetting, RuleSource,
};
