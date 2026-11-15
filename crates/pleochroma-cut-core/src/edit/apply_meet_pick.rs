//! [`Edit::SetTierMeetPick`]'s apply/inverse half, the undo step a removed tier's own pick
//! needs, and the gear-remap step that moves the facet positions of every picked corner. Split
//! from `apply` to keep that file readable.

use super::{
    apply::remap_index,
    edit_type::{Edit, EditError, RemapRounding},
};
use crate::design::{Design, TierId, TierMeetPick};

impl Design {
    /// [`Edit::SetTierMeetPick`]'s own apply/inverse half. Keyed by the current occupant's
    /// [`TierId`] (via [`Design::tier_id_at`]), so -- like [`Design::tier_targets`] -- it never
    /// needs renumbering on `AddTier`/`RemoveTier`/`MoveTier`. Changes only the pick map.
    pub(super) fn apply_set_tier_meet_pick(
        &mut self,
        index: usize,
        pick: Option<TierMeetPick>,
        tier_count: usize,
    ) -> Result<Edit, EditError> {
        if index >= tier_count {
            return Err(EditError { index, tier_count });
        }
        // Self-heals `tier_ids`, only after the bounds check -- see `sync_tier_ids`.
        self.sync_tier_ids();
        let Some(id) = self.tier_id_at(index) else {
            return Err(EditError { index, tier_count });
        };
        let previous = match pick {
            Some(pick) => self.tier_meet_picks.insert(id, pick),
            None => self.tier_meet_picks.remove(&id),
        };
        Ok(Edit::SetTierMeetPick {
            index,
            pick: previous,
        })
    }

    /// Takes the pick of the removed tier `id` out of the map and returns the step that puts it
    /// back on the tier restored at `index` (part of `RemoveTier`'s inverse). `None` for a tier
    /// without a pick, which is every tier of a design that does not use them. Picks of other
    /// tiers that name the removed tier stay: they read as lost until the tier comes back.
    pub(super) fn take_tier_meet_pick_step(&mut self, id: TierId, index: usize) -> Option<Edit> {
        self.tier_meet_picks
            .remove(&id)
            .map(|pick| Edit::SetTierMeetPick {
                index,
                pick: Some(pick),
            })
    }

    /// Moves the index-wheel position of every picked facet from `from_gear` to `to_gear`
    /// teeth, with the same rounding [`Edit::RemapIndices`] gives the tiers' own `indices`, so
    /// a picked corner stays on the facets it names.
    ///
    /// Returns one [`Edit::SetTierMeetPick`] per tier it changed, each holding the pick the tier
    /// had, to be part of the remap's inverse. Empty (and nothing is touched) for a design
    /// without picks, which is every design that does not use them.
    pub(super) fn remap_meet_picks(
        &mut self,
        from_gear: i32,
        to_gear: i32,
        rounding: RemapRounding,
    ) -> Vec<Edit> {
        let mut inverse = Vec::new();
        let ids: Vec<TierId> = self.tier_meet_picks.keys().copied().collect();
        for id in ids {
            let Some(old) = self.tier_meet_picks.get(&id).copied() else {
                continue;
            };
            let Some(index) = self.index_of_tier_id(id) else {
                continue;
            };
            let mut moved = old;
            for facet in &mut moved.facets {
                facet.index = remap_index(facet.index, from_gear, to_gear, rounding);
            }
            self.tier_meet_picks.insert(id, moved);
            inverse.push(Edit::SetTierMeetPick {
                index,
                pick: Some(old),
            });
        }
        inverse
    }
}
