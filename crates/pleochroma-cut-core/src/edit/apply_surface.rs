//! [`Edit::SetTierSurface`]'s apply/inverse half, the undo step a removed tier's surface needs,
//! and the gear-remap step that moves the facet positions of a surface finished facet by facet.
//! Split from `apply` to keep that file readable; it sits beside `apply_frosting`.

use super::{
    apply::remap_index,
    edit_type::{Edit, EditError, RemapRounding},
};
use crate::design::{Design, TierId, TierSurface};

impl Design {
    /// [`Edit::SetTierSurface`]'s own apply/inverse half. Keyed by the current occupant's
    /// [`TierId`] (via [`Design::tier_id_at`]), so -- like [`Design::tier_frosting`] -- it never
    /// needs renumbering on `AddTier`/`RemoveTier`/`MoveTier`. Changes only the surface map; the
    /// surface is normalised first, so one that states nothing is a clear.
    pub(super) fn apply_set_tier_surface(
        &mut self,
        index: usize,
        surface: Option<TierSurface>,
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
        let previous = match surface.and_then(TierSurface::normalised) {
            Some(surface) => self.tier_surfaces.insert(id, surface),
            None => self.tier_surfaces.remove(&id),
        };
        Ok(Edit::SetTierSurface {
            index,
            surface: previous,
        })
    }

    /// Takes the surface of the removed tier `id` out of the map and returns the step that puts
    /// it back on the tier restored at `index` (part of `RemoveTier`'s inverse). `None` for a
    /// tier without a surface, which is every tier of a design that does not use it.
    pub(super) fn take_tier_surface_step(&mut self, id: TierId, index: usize) -> Option<Edit> {
        self.tier_surfaces
            .remove(&id)
            .map(|surface| Edit::SetTierSurface {
                index,
                surface: Some(surface),
            })
    }

    /// Moves the facet positions of every tier finished facet by facet from `from_gear` to
    /// `to_gear` teeth, with the same rounding [`Edit::RemapIndices`] gives the tiers' own
    /// `indices`, so a facet's finish stays on the facet it names.
    ///
    /// Returns one [`Edit::SetTierSurface`] per tier it changed, each holding the surface the
    /// tier had, to be part of the remap's inverse. Empty (and nothing is touched) for a design
    /// without per-facet surfaces, which is every design that does not use them.
    pub(super) fn remap_surface_facets(
        &mut self,
        from_gear: i32,
        to_gear: i32,
        rounding: RemapRounding,
    ) -> Vec<Edit> {
        let mut inverse = Vec::new();
        let ids: Vec<TierId> = self.tier_surfaces.keys().copied().collect();
        for id in ids {
            let Some(old) = self
                .tier_surfaces
                .get(&id)
                .filter(|surface| !surface.facets.is_empty())
                .cloned()
            else {
                continue;
            };
            let Some(index) = self.index_of_tier_id(id) else {
                continue;
            };
            match old.with_positions_mapped(|position| {
                remap_index(position, from_gear, to_gear, rounding)
            }) {
                Some(moved) => {
                    self.tier_surfaces.insert(id, moved);
                }
                None => {
                    self.tier_surfaces.remove(&id);
                }
            }
            inverse.push(Edit::SetTierSurface {
                index,
                surface: Some(old),
            });
        }
        inverse
    }
}
