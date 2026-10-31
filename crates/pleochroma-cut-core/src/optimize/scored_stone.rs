//! [`ScoredStone`]: the stone every optimizer score measures: planes, concave tools and
//! facet finishes.

use super::objective::to_gpu_planes;
use crate::design::Design;
use glam::DVec3;
use pleochroma::{
    color::metrics::FinishedStone,
    geometry::{
        GpuFacetPlane, StoneGeometry, meet_solver::SolvedTier, stone_metrics::SolidMetrics,
        tool::ToolPrimitive,
    },
    optics::raytracer::FacetFinish,
};

/// The stone a design is scored on: its flat facets as the tracer takes them, the concave
/// tools cut out of them, and the finish of each facet.
///
/// Every scoring path of the optimizer measures this, so the figures describe the stone
/// the Compare window draws and not its flat shell. A design with no concave tiers (or
/// whose tiers do not resolve, see [`Design::tools_for_scoring`]) has no tools, and then
/// the geometry is the planes alone, bit for bit what the optimizer always scored. A design
/// with no frosted facet has no finishes (the empty, all-polished list), and then the
/// tracer takes the polished path, bit for bit as well.
pub(super) struct ScoredStone {
    planes: Vec<GpuFacetPlane>,
    tools: Vec<ToolPrimitive>,
    finishes: Vec<FacetFinish>,
}

impl ScoredStone {
    /// The stone of `design` whose solved tiers are `solved` and whose flat arrangement is
    /// `planes` ([`Design::planes_from_solved`] of `solved`).
    ///
    /// `solved` is only read for the frosting, which maps the authored frosted facets to
    /// plane indices ([`Design::facet_finishes_from_solved`]).
    pub(super) fn of(design: &Design, solved: &[SolvedTier], planes: &[(DVec3, f64)]) -> Self {
        let gpu_planes = to_gpu_planes(planes);
        let finishes = design.facet_finishes_from_solved(solved, &gpu_planes);
        Self {
            planes: gpu_planes,
            tools: design.tools_for_scoring(planes),
            finishes,
        }
    }

    /// The borrowed geometry the tracer takes, without the finishes.
    pub(super) fn geometry(&self) -> StoneGeometry<'_> {
        StoneGeometry {
            planes: &self.planes,
            tools: &self.tools,
        }
    }

    /// The geometry with the facet finishes: what every optimizer score measures.
    pub(super) fn finished(&self) -> FinishedStone<'_> {
        FinishedStone::new(self.geometry(), &self.finishes)
    }

    /// `flat` (the `measure_solid` of `planes`, the flat arrangement this stone was built
    /// from) with the volume of the stone carved by its tools: what the yield term reads, so
    /// a groove or a dimple costs yield the way it costs the cutter material.
    ///
    /// Without tools this is `flat` itself, bit for bit. When the tools leave no closed stone
    /// to measure the flat figures stand (as an unresolved tier scores flat): a yield loss of
    /// `0.0` would read as a free cut. Only the volume differs; the widths, heights and the
    /// table the shape penalty reads are the flat stone's by the carved measure's own
    /// definition ([`Design::carved_metrics`]), so the shape penalty has no carved variant.
    pub(super) fn yield_metrics(
        &self,
        planes: &[(DVec3, f64)],
        flat: Option<&SolidMetrics>,
    ) -> Option<SolidMetrics> {
        if self.tools.is_empty() {
            return flat.copied();
        }
        Design::carved_metrics(flat.copied(), planes, &self.tools).or_else(|| flat.copied())
    }
}
