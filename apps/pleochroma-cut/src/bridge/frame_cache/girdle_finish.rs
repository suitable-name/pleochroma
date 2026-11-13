//! Cached girdle-facet classification for the frosted (bruted) girdle toggle.
//!
//! `pleochroma::geometry::girdle::girdle_facet_finishes` walks every plane in the active
//! design to decide which ones are the girdle band -- cheap once, but the render loop
//! calls into whichever finish set is active many times per second, and the classified
//! band only ever changes when the design's own geometry does. [`GirdleFinishCache`]
//! recomputes only when `render_thread::hash_planes`'s key differs from the last call, the same
//! identity `bridge::frame_cache::guide_pass::GuideCache` keys on.

use crate::bridge::render_thread::hash_planes;
use pleochroma::{
    geometry::{
        girdle::{facet_finishes_with_frosted, girdle_facet_finishes},
        plane::GpuFacetPlane,
    },
    optics::raytracer::FacetFinish,
};

/// `key` starts `None`, so the first [`Self::ensure`] call populates `finishes` before anything
/// reads it.
#[derive(Debug, Default)]
pub struct GirdleFinishCache {
    key: Option<u64>,
    finishes: Vec<FacetFinish>,
    /// The key of `authored`: the planes' hash, the girdle flag and the frosted plane list.
    authored_key: Option<(u64, bool, Vec<u32>)>,
    /// The finishes for a design with authored frosting (see [`Self::ensure_with_frosted`]).
    authored: Vec<FacetFinish>,
}

impl GirdleFinishCache {
    /// Creates an empty instance.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            key: None,
            finishes: Vec::new(),
            authored_key: None,
            authored: Vec::new(),
        }
    }

    /// [`Self::ensure`] for a design with authored frosting: the frosted planes
    /// (`frosted`, indices into `planes`) are `Frosted` too, and the girdle band is
    /// `Frosted` as well when `girdle_frosted` is on. Recomputed only when the planes, the
    /// flag or the list change. A design without authored frosting uses [`Self::ensure`].
    pub fn ensure_with_frosted(
        &mut self,
        planes: &[GpuFacetPlane],
        girdle_frosted: bool,
        frosted: &[u32],
    ) -> &[FacetFinish] {
        let key = (hash_planes(planes), girdle_frosted, frosted.to_vec());
        if self.authored_key.as_ref() != Some(&key) {
            self.authored = facet_finishes_with_frosted(planes, girdle_frosted, frosted);
            self.authored_key = Some(key);
        }
        &self.authored
    }

    /// Returns the per-facet finishes for `planes` (girdle band `Frosted`, everything
    /// else `Polished`), reclassifying only when `planes`'s hash differs from the last
    /// call -- an unchanged design is a cache hit, costing nothing beyond the hash
    /// comparison.
    pub fn ensure(&mut self, planes: &[GpuFacetPlane]) -> &[FacetFinish] {
        let key = hash_planes(planes);
        if self.key != Some(key) {
            self.finishes = girdle_facet_finishes(planes);
            self.key = Some(key);
        }
        &self.finishes
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma::geometry::cuts::{STANDARD_ROUND_BRILLIANT_GIRDLE_FACETS, StandardGemCuts};

    #[test]
    fn ensure_matches_the_uncached_classification() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let mut cache = GirdleFinishCache::new();
        let cached = cache.ensure(&planes).to_vec();
        let direct = girdle_facet_finishes(&planes);
        assert_eq!(cached, direct);
        for i in STANDARD_ROUND_BRILLIANT_GIRDLE_FACETS {
            assert_eq!(cached[i], FacetFinish::Frosted, "index {i}");
        }
    }

    /// A repeat call with the same planes returns the same finishes.
    #[test]
    fn ensure_is_stable_across_repeated_calls_with_unchanged_planes() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let mut cache = GirdleFinishCache::new();
        let first = cache.ensure(&planes).to_vec();
        let second = cache.ensure(&planes).to_vec();
        assert_eq!(first, second);
    }

    /// A design change (different plane set) must reclassify, not keep serving the
    /// previous design's stale finishes.
    #[test]
    fn ensure_reclassifies_when_the_design_changes() {
        let srb = StandardGemCuts::standard_round_brilliant();
        let emerald = StandardGemCuts::emerald_cut();
        let mut cache = GirdleFinishCache::new();
        let srb_finishes = cache.ensure(&srb).to_vec();
        let emerald_finishes = cache.ensure(&emerald).to_vec();
        assert_eq!(emerald_finishes, girdle_facet_finishes(&emerald));
        assert_ne!(
            srb_finishes.len(),
            emerald_finishes.len(),
            "SRB and emerald_cut have different plane counts, so a stale (un-reclassified) \
             result would show up as a length mismatch"
        );
    }
}
