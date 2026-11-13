//! [`TierId`]: a stable, never-reused identifier for one [`super::ConstraintTier`]
//! slot in a [`super::Design`], independent of its position.
//!
//! Stored in [`super::Design::tier_ids`], a list kept parallel to [`super::Design::tiers`]
//! and changed in step with it by every [`crate::edit::Edit`] that touches the tier list.
//! It lives beside the tiers rather than on [`super::ConstraintTier`], so that struct's many
//! construction sites need no change.
//!
//! Allocated once, from [`super::Design::next_tier_id`], when a tier first takes a slot. It
//! is never reused, even after the tier is removed, so a cached id tells a different tier
//! from one that merely moved. Add, remove, move, undo and redo keep it. The integration
//! tests `undoing_a_tier_removal_restores_the_exact_tier_id` and
//! `history_undo_and_redo_of_a_removal_keep_the_tier_ids` (`tests/edit_roundtrip.rs`) pin
//! this through [`super::Design::tier_ids_eq`].
/// Stable identifier of one tier slot; never reused within a design.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TierId(pub u64);

impl TierId {
    /// The raw counter value -- for display/debugging and for the design file
    /// mirror (`pleochroma_formats::native`), which stores this as a plain `u64`.
    #[must_use]
    pub const fn value(self) -> u64 {
        self.0
    }
}

impl std::fmt::Display for TierId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "#{}", self.0)
    }
}
