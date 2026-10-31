//! Everything a metrics evaluation needs that does not depend on the camera pose.
//!
//! A tilt sweep evaluates the same stone under the same light 724 times, and only the camera
//! frame differs between the evaluations. [`PreparedEval`] holds the rest -- the plane arena,
//! the fan scale (a hash over every plane plus a thread-local cache lookup), the three
//! refractive indices, the lighting (a boxed `StudioRig` and the median of 96 radiance
//! lookups) and the `DIAG_FIRE_DEBUG` environment lookup -- so a sweep builds it once and
//! every pose, on any thread, only reads it. Each field is a pure function of
//! `(planes, tools, finishes, material, environment)`, so building it once changes no result.

use super::{birefringent::Modes, fan::FanGeometry, lighting::ExitLighting, ray_trace::StoneArena};
use crate::{
    color::principal_axes::PrincipalFrame,
    geometry::tool::{StoneGeometry, ToolPrimitive},
    optics::{
        materials::GemMaterial,
        raytracer::{EnvironmentSource, FacetFinish, build_plane_soa},
    },
    simd::PlanesSoA32,
};

/// The pose-independent half of one metrics evaluation; see the module docs.
///
/// Plain data (no interior mutability), so a sweep shares one by reference across its
/// worker threads.
pub(super) struct PreparedEval<'a> {
    /// SIMD slab arena of the stone's planes: every ray any pose fires intersects it.
    plane_soa: PlanesSoA32,
    /// The tools subtracted from the polyhedron (empty for a planar stone).
    tools: &'a [ToolPrimitive],
    /// The per-facet finishes, or EMPTY when no facet is frosted (an all-polished list is
    /// dropped on the way in, so a polished stone never reaches the frosted code at all).
    finishes: &'a [FacetFinish],
    /// Where the fan's rays start, scaled to the stone.
    pub(super) fan: FanGeometry,
    /// Refractive index at the d-line (clamped to at least 1.1).
    pub(super) nd: f32,
    /// Refractive index at the F-line, clamped defensively like `nd` so the entry
    /// refraction never hits the pathological `EntryBlocked` case.
    pub(super) n_f: f32,
    /// Refractive index at the C-line, clamped like `n_f`.
    pub(super) n_c: f32,
    /// The illumination an exit direction is judged against: the radiance the tracer lights
    /// the image with, relative to its ambient level (see `ExitLighting`).
    pub(super) lighting: ExitLighting<'a>,
    /// The material name when `DIAG_FIRE_DEBUG` is set (the diagnostic lines print it),
    /// `None` otherwise. Looked up once here, not once per pose.
    pub(super) diag_material: Option<String>,
    /// The material's principal frame when its absorption is pleochroic (`None` otherwise):
    /// what the face-up tone's per-axis path lengths are measured against.
    principal: Option<PrincipalFrame>,
    /// The two-mode optics of an anisotropic material ([`Modes::Isotropic`] otherwise, the
    /// integer fast path); every [`StoneArena`] this hands out carries a view of it.
    modes: Modes,
}

impl<'a> PreparedEval<'a> {
    /// Prepares `geom` made of `material` under `environment`; `None` when there are no
    /// planes to trace (every pose then reports the neutral placeholder values).
    ///
    /// Test-only: production callers always carry finishes and go through
    /// [`Self::new_finished`].
    #[cfg(test)]
    pub(super) fn new(
        geom: StoneGeometry<'a>,
        material: &GemMaterial,
        environment: EnvironmentSource<'a>,
    ) -> Option<Self> {
        Self::new_finished(geom, &[], material, environment)
    }

    /// [`Self::new`] for a stone whose facets carry `finishes` (`finishes[i]` for the plane
    /// `i`, tool `k` being facet `planes.len() + k`; a missing entry is polished). A list with
    /// no frosted facet is the polished stone, bit for bit: it is dropped here.
    pub(super) fn new_finished(
        geom: StoneGeometry<'a>,
        finishes: &'a [FacetFinish],
        material: &GemMaterial,
        environment: EnvironmentSource<'a>,
    ) -> Option<Self> {
        let planes = geom.planes;
        if planes.is_empty() {
            return None;
        }
        let nd = material.dispersion.evaluate(589.3).max(1.1);
        let n_f = material.dispersion.evaluate(486.1).max(1.001);
        let n_c = material.dispersion.evaluate(656.3).max(1.001);
        Some(Self {
            plane_soa: build_plane_soa(planes),
            tools: geom.tools,
            finishes: if finishes.contains(&FacetFinish::Frosted) {
                finishes
            } else {
                &[]
            },
            // The fan is scaled to the stone once (the measurement itself is also reused
            // while the design is unchanged -- see `FanGeometry`).
            fan: FanGeometry::for_planes(planes),
            nd,
            n_f,
            n_c,
            lighting: ExitLighting::new(environment),
            diag_material: std::env::var("DIAG_FIRE_DEBUG")
                .is_ok()
                .then(|| material.name.clone()),
            principal: PrincipalFrame::for_pleochroic(material),
            modes: Modes::for_material(material, [nd, n_f, n_c]),
        })
    }

    /// The stone every ray of every pose is traced against. Its traces carry no principal
    /// path (see [`Self::stone_with_principal`]).
    pub(super) const fn stone(&self) -> StoneArena<'_> {
        StoneArena {
            plane_soa: &self.plane_soa,
            tools: self.tools,
            finishes: self.finishes,
            modes: self.modes.anisotropic(),
            principal: None,
        }
    }

    /// [`Self::stone`] whose d-line traces also split every ray's path over the material's
    /// principal axes (`ExitPath::principal_path`); identical to `stone()` for a material
    /// that is not pleochroic.
    pub(super) const fn stone_with_principal(&self) -> StoneArena<'_> {
        StoneArena {
            plane_soa: &self.plane_soa,
            tools: self.tools,
            finishes: self.finishes,
            modes: self.modes.anisotropic(),
            principal: self.principal,
        }
    }

    /// Whether the material is pleochroic, i.e. the face-up tone needs per-ray principal
    /// paths ([`ToneAccumulator::with_principal_paths`](super::tone::ToneAccumulator)).
    pub(super) const fn is_pleochroic(&self) -> bool {
        self.principal.is_some()
    }
}
