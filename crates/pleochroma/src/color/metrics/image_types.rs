//! Versioned, backend-independent analytical image diagnostics.
use crate::{
    geometry::tool::StoneGeometry,
    optics::{
        materials::GemMaterial,
        raytracer::{Camera, EnvironmentSource, FacetFinish},
    },
};

/// Analytical model version; independent of persisted catalogue scores.
pub const OPTICAL_MODEL_VERSION: u32 = 1;
/// Hard memory bound for the result, checked before allocating (about 96 MiB).
pub const MAX_DIAGNOSTIC_PIXELS: usize = 1_048_576;

/// Why a path did not qualify as returned light. Termination is not physical absorption.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ExtinctionReason {
    #[default]
    None,
    ObserverShadow,
    InsufficientIllumination,
    SideExit,
    NoIntersection,
    BounceBudget,
    EntryBlocked,
    Invalid,
    /// Spectral attribution only: Russian roulette killed the path.
    RussianRoulette,
    /// Spectral attribution only: a scattering event absorbed the path.
    ScatterAbsorbed,
}
impl std::fmt::Display for ExtinctionReason {
    /// User-facing Title Case name. `None` (a path that was not extinguished) reads
    /// "Returned".
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::None => "Returned",
            Self::ObserverShadow => "Observer Shadow",
            Self::InsufficientIllumination => "Insufficient Illumination",
            Self::SideExit => "Side Exit",
            Self::NoIntersection => "No Intersection",
            Self::BounceBudget => "Bounce Budget",
            Self::EntryBlocked => "Entry Blocked",
            Self::Invalid => "Invalid",
            Self::RussianRoulette => "Russian Roulette",
            Self::ScatterAbsorbed => "Scatter Absorbed",
        })
    }
}
/// Centre-sample attribution. Other subpixel samples can meet different surfaces.
#[derive(Clone, Copy, Debug, Default)]
pub struct RayDetail {
    pub entry_facet: Option<usize>,
    pub exit_facet: Option<usize>,
    pub bounces: u32,
    pub reason: ExtinctionReason,
}
/// Counts retain unknown and entry-blocked observations rather than renormalising them away.
#[derive(Clone, Copy, Debug, Default)]
pub struct OpticalPixel {
    pub hits: u32,
    /// Windowing, extinction, brilliance, unclassified (including entry blocked).
    pub classes: [u32; 4],
    pub fire_sum_deg: f32,
    pub fire_qualified: u32,
    pub fire_rejected: u32,
    pub centre: RayDetail,
    pub mixed_facets: bool,
    /// Yaw poses (of [`Self::scint_poses`]) at which the pixel's centre ray was visibly
    /// returned (one per returned eigenmode for an anisotropic stone); always zero unless the
    /// image was asked for scintillation.
    pub scint_returned: u8,
    /// Yaw poses evaluated for the temporal term: 5 (10 for an anisotropic stone, where each
    /// pose counts both eigenmodes of the centre ray and [`Self::scint_returned`] counts the
    /// modes that returned), or 0 when the term was not computed for this pixel (not
    /// requested, no hit, or the centre ray misses at the base pose).
    pub scint_poses: u8,
    /// Eigenmode traces per hit camera ray: `0` (never set) for an isotropic stone, whose ray
    /// is traced once, and `2` for an anisotropic one, whose ray is traced as its ordinary
    /// and its extraordinary mode, each counting once in [`Self::classes`] and in the Fire
    /// counts. `hits` stays the number of camera rays that met the stone, so coverage is
    /// unchanged; the shares and the Fire mean divide by `hits * modes`.
    pub modes: u8,
}
impl OpticalPixel {
    /// The number of traces the counts of this pixel are out of: one per hit for an isotropic
    /// stone (the same float as before the modes existed), two per hit for an anisotropic one.
    fn trace_count(&self) -> f32 {
        self.hits.max(1) as f32 * f32::from(self.modes.max(1))
    }
    /// Fractions of hit samples, including the separately represented unknown share.
    #[must_use]
    pub fn fractions(&self) -> [f32; 4] {
        let total = self.trace_count();
        self.classes.map(|n| n as f32 / total)
    }
    /// Unfloored weighted angular separation per hit sample, in degrees.
    #[must_use]
    pub fn fire_deg(&self) -> f32 {
        self.fire_sum_deg / self.trace_count()
    }
}
/// Owned counts; no full paths or per-redraw array copies.
#[derive(Debug)]
pub struct OpticalImage {
    pub width: u32,
    pub height: u32,
    pub samples: u32,
    pub fire_computed: bool,
    /// Whether the per-pixel scintillation temporal term was requested and traced.
    pub scint_computed: bool,
    pub origin: [u32; 2],
    pub pixels: Vec<OpticalPixel>,
    pub total: OpticalPixel,
}
impl OpticalImage {
    /// Bernoulli variance `p (1 - p)` of the fraction `p` of yaw poses at which pixel `i`'s
    /// centre ray was visibly returned (0, 0.16 or 0.24 for an isotropic stone, which has five
    /// poses; an anisotropic stone, with ten mode-poses, adds 0.09, 0.21 and 0.25); `None`
    /// where it was not computed.
    #[must_use]
    pub fn temporal_variance(&self, pixel: usize) -> Option<f32> {
        let p = self.pixels.get(pixel)?;
        if p.scint_poses == 0 {
            return None;
        }
        let fraction = f32::from(p.scint_returned) / f32::from(p.scint_poses);
        Some(fraction.mul_add(-fraction, fraction))
    }

    /// The map-domain scintillation of the whole image, 0 to 100: the module's own spatial
    /// term over every hit pixel's brilliance fraction, combined 60/40 with the mean
    /// temporal variance of the pixels whose temporal term was computed.
    ///
    /// This is **not** the HUD's grid value: it samples the visible area in image pixels and
    /// judges each pixel against its own eye direction, where the grid fires a parallel fan
    /// judged against the camera axis. The two agree in trend, not in value.
    #[must_use]
    pub fn scintillation_pct(&self) -> f32 {
        super::image_scint::image_scintillation_pct(self)
    }
}
/// Validation/cancellation are explicit, never zero-valued valid maps.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OpticalImageError {
    EmptyGeometry,
    InvalidInput,
    ResourceLimit,
    Cancelled,
}
impl std::fmt::Display for OpticalImageError {
    /// User-facing Title Case name.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::EmptyGeometry => "Empty Geometry",
            Self::InvalidInput => "Invalid Input",
            Self::ResourceLimit => "Resource Limit",
            Self::Cancelled => "Cancelled",
        })
    }
}
/// Borrowed immutable snapshot. Call only on a bounded background worker.
pub struct OpticalImageRequest<'a> {
    pub camera: &'a Camera,
    pub width: u32,
    pub height: u32,
    pub samples: u32,
    /// Original beauty dimensions, preserving its exact aspect when sampling a draft.
    pub projection_size: Option<[u32; 2]>,
    /// Export tile origin in projection pixels; None samples the entire projection.
    pub tile_origin: Option<[u32; 2]>,
    pub geometry: StoneGeometry<'a>,
    /// The finish of each facet (plane `i`, tool `k` at `planes.len() + k`; a missing entry is
    /// polished), like [`FinishedStone::finishes`](super::FinishedStone). An empty list, and a
    /// list with no frosted facet, both trace the polished stone, bit for bit.
    pub finishes: &'a [FacetFinish],
    pub material: &'a GemMaterial,
    pub environment: EnvironmentSource<'a>,
    pub want_fire: bool,
    /// Also trace the per-pixel scintillation temporal term (five yaw poses, centre ray).
    pub want_scintillation: bool,
    /// The orbit pose `[yaw, pitch, distance]` that built [`Self::camera`]. Required when
    /// `want_scintillation` is set (the offset cameras are rebuilt from it); otherwise unused.
    pub pose: Option<[f32; 3]>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_names_are_title_case() {
        let reasons = [
            (ExtinctionReason::None, "Returned"),
            (ExtinctionReason::ObserverShadow, "Observer Shadow"),
            (
                ExtinctionReason::InsufficientIllumination,
                "Insufficient Illumination",
            ),
            (ExtinctionReason::SideExit, "Side Exit"),
            (ExtinctionReason::NoIntersection, "No Intersection"),
            (ExtinctionReason::BounceBudget, "Bounce Budget"),
            (ExtinctionReason::EntryBlocked, "Entry Blocked"),
            (ExtinctionReason::Invalid, "Invalid"),
            (ExtinctionReason::RussianRoulette, "Russian Roulette"),
            (ExtinctionReason::ScatterAbsorbed, "Scatter Absorbed"),
        ];
        for (reason, text) in reasons {
            assert_eq!(reason.to_string(), text);
        }
        let errors = [
            (OpticalImageError::EmptyGeometry, "Empty Geometry"),
            (OpticalImageError::InvalidInput, "Invalid Input"),
            (OpticalImageError::ResourceLimit, "Resource Limit"),
            (OpticalImageError::Cancelled, "Cancelled"),
        ];
        for (error, text) in errors {
            assert_eq!(error.to_string(), text);
        }
    }
}
