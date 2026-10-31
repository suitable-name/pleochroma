//! Abraded edges: a band of ground surface along every facet edge.
//!
//! Wear rounds and frosts the edges of a stone first. In a band of width `width` (model units)
//! around every edge the surface is Ground with roughness `alpha`. The band is found with the same
//! nearest-plane search edge rounding uses: for a convex polyhedron the edges of a facet are
//! exactly where another half-space boundary comes within reach.
//!
//! Wear also dulls the polish next to the band. The optional TIS boost raises the roughness of a
//! polished (or ideal) facet to `halo_rq_nm` at the band's rim and lets it fall to nothing over one
//! more band width, so the total integrated scatter of the worn zone grows toward the edge.

use glam::Vec3;

use crate::geometry::plane::GpuFacetPlane;

/// The abraded-edge setting of a render.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct EdgeAbrasion {
    /// The width of the band in model units; `0` switches the band off.
    pub width: f32,
    /// The GGX roughness of the band.
    pub alpha: f32,
    /// The TIS boost: the RMS roughness in nanometres the polish next to the band is raised to at
    /// the band's rim; `0` switches the boost off.
    pub halo_rq_nm: f32,
}

impl EdgeAbrasion {
    /// No abrasion.
    pub const NONE: Self = Self {
        width: 0.0,
        alpha: 0.0,
        halo_rq_nm: 0.0,
    };

    /// `true` when the ground band exists.
    #[must_use]
    pub fn ground_active(&self) -> bool {
        self.width > 0.0 && self.alpha > 0.0
    }

    /// `true` when the TIS boost exists.
    #[must_use]
    pub fn boost_active(&self) -> bool {
        self.width > 0.0 && self.halo_rq_nm > 0.0
    }

    /// `true` when the abrasion changes anything at all.
    #[must_use]
    pub fn active(&self) -> bool {
        self.ground_active() || self.boost_active()
    }

    /// The roughness floor (nm Rq) of a polish at `distance` from the nearest other facet plane:
    /// `halo_rq_nm` up to the band's width, falling linearly to `0` over one more width.
    #[must_use]
    pub fn wear_rq_nm(&self, distance: f32) -> f32 {
        if !self.boost_active() {
            return 0.0;
        }
        let t = ((distance - self.width) / self.width).clamp(0.0, 1.0);
        self.halo_rq_nm * (1.0 - t)
    }
}

/// The signed distance of `point` on facet `facet_idx` to the nearest OTHER facet plane.
///
/// Negated so that it is zero at a shared edge and positive inside the stone (the convention of
/// `shading_normal_near_edge`). `f32::INFINITY` for a stone with one plane. Planar stones only.
#[must_use]
pub fn edge_distance(planes: &[GpuFacetPlane], point: Vec3, facet_idx: usize) -> f32 {
    planes
        .iter()
        .enumerate()
        .filter(|&(i, _)| i != facet_idx)
        .map(|(_, plane)| -(plane.d + Vec3::from_array(plane.normal).dot(point)))
        .fold(f32::INFINITY, f32::min)
}

/// Whether `point` on facet `facet_idx` lies within `width` of another facet's plane, that is,
/// in the band along an edge. Planar stones only.
#[must_use]
pub fn in_edge_band(planes: &[GpuFacetPlane], point: Vec3, facet_idx: usize, width: f32) -> bool {
    planes.iter().enumerate().any(|(i, plane)| {
        // The signed distance to plane `i`, negated so that it is zero at a shared edge and
        // positive inside the stone (the convention of `shading_normal_near_edge`).
        i != facet_idx && -(plane.d + Vec3::from_array(plane.normal).dot(point)) < width
    })
}

/// The ground band as the next-event estimators see it: the planes and the width, so a shadow
/// ray's exit point can be tested against the band the bounce loop would test it against.
#[derive(Clone, Copy)]
pub(crate) struct EdgeBand<'a> {
    pub(crate) planes: &'a [GpuFacetPlane],
    pub(crate) width: f32,
}

impl<'a> EdgeBand<'a> {
    /// The band of a planar stone's abrasion, or `None` when there is no ground band (or tools,
    /// which the band does not cover).
    pub(crate) fn of(
        abrasion: &EdgeAbrasion,
        planes: &'a [GpuFacetPlane],
        has_tools: bool,
    ) -> Option<Self> {
        (abrasion.ground_active() && !has_tools).then_some(Self {
            planes,
            width: abrasion.width,
        })
    }

    /// Whether `point` on facet `facet_idx` is inside the band.
    pub(crate) fn contains(&self, point: Vec3, facet_idx: usize) -> bool {
        in_edge_band(self.planes, point, facet_idx, self.width)
    }
}
