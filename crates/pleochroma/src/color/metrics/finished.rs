//! A stone together with the finish of each of its facets.
//!
//! [`StoneGeometry`] is the geometry every tracer shares (planes and tools); the render tracer
//! takes the facet finishes beside it, as a separate list. Every face-up metrics entry point
//! (the pose metrics, the angular profiles, the cache and the tilt sweeps) has a `_finished`
//! twin that takes the same pair, bundled as a [`FinishedStone`], so that a frosted facet
//! changes the light the metrics see the way it changes the render. The plain-geometry entry
//! points are those twins called with [`FinishedStone::polished`].

use crate::{geometry::tool::StoneGeometry, optics::raytracer::FacetFinish};

/// A stone's planes and tools plus its per-facet finishes.
///
/// `finishes[i]` is the finish of plane `i`, and of tool `k` at index `planes.len() + k`, like
/// the finish slice of `trace_spectral_ray_with_finish_soa_geom`; a missing entry is polished.
/// An empty list, and a list with no frosted facet, both mean the polished stone and give
/// bit-identical results to the plain-geometry entry points.
#[derive(Debug, Clone, Copy)]
pub struct FinishedStone<'a> {
    /// The planes and tools.
    pub geom: StoneGeometry<'a>,
    /// The finish of each facet; empty for a polished stone.
    pub finishes: &'a [FacetFinish],
}

impl<'a> FinishedStone<'a> {
    /// `geom` with the given facet `finishes` (for example from
    /// `geometry::facet_finishes_with_frosted`).
    #[must_use]
    pub const fn new(geom: StoneGeometry<'a>, finishes: &'a [FacetFinish]) -> Self {
        Self { geom, finishes }
    }

    /// `geom` with every facet polished: what every plain-geometry entry point sweeps.
    #[must_use]
    pub const fn polished(geom: StoneGeometry<'a>) -> Self {
        Self {
            geom,
            finishes: &[],
        }
    }
}
