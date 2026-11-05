//! Bragg domains: the structural color of precious opal and labradorite.
//!
//! Neither stone is colored by absorption. Opal is a close-packed (face-centred cubic) array of
//! silica spheres of diameter `D`; labradorite carries Bøggild exsolution lamellae of one pair
//! period `d`. Both diffract: inside an ordered patch, light travelling at the angle `theta` to the
//! lattice-plane normal `g` is reflected in a narrow band around the first-order Bragg wavelength
//!
//! ```text
//! lambda_B(theta) = lambda_B(0) cos(theta)
//! opal:        lambda_B(0) = 2 sqrt(2/3) D n_eff,   n_eff^2 = phi n_s^2 + (1 - phi) n_m^2
//! labradorite: lambda_B(0) = 2 n d
//! ```
//!
//! (`phi = 0.74`, `n_s = 1.45` silica, `n_m = 1.365` the water-filled matrix, which makes
//! `lambda_B(0)` 2.33 `D`; `n = 1.56` for the lamellae). The second order, `lambda_B / 2`, is
//! included while it stays at or above 380 nm.
//!
//! # The medium
//!
//! A [`BraggStructure`] describes the stone's domains. In an active domain the volumetric
//! reflection coefficient is
//!
//! ```text
//! sigma_B(x, w, lambda) = kappa G(lambda; lambda_B(|w . g_d|)),   kappa = -ln(1 - R0) / s
//! ```
//!
//! with `s` the domain (cell) size, `R0` the peak reflectance over one domain and `G` the
//! Gaussian profile of relative width `w` ("Color Purity"). An event mirrors the ray in the
//! lattice normal perturbed by the mosaic spread; **the direction does not depend on the
//! wavelength**, so all eight channels of a hero comb share one path. The domain map is in
//! [`super::domains`], the estimator in `raytracer::scattering::delta_tracking`.
//!
//! A zoned host does not show the structure: the tracer takes its zoned branch (the families
//! only, `scattering::oriented_zoned`) before the structure branch, on the CPU and in WGSL
//! alike (`08b_extend_scatter.wgsl`), so a structure on a zoned stone is carried but never
//! scatters.
//!
//! The numbers are cited in `r0_render_physics_2026-10-21.md` section 3.5 and 3.6. The domain
//! sizes and the mosaic spread have no published values and are appearance parameters.

use super::{CrystalDir, CrystalFrame, platelet::platelet_sample};
use crate::optics::materials::GemMaterial;
use glam::Vec3;

/// Refractive index of the silica spheres of opal (the opal infobox, 1.450).
pub const SILICA_INDEX: f32 = 1.45;
/// Refractive index of the matrix between the spheres (water-filled gaps, 1.33 to 1.40).
pub const MATRIX_INDEX: f32 = 1.365;
/// Volume fraction of the spheres in a close-packed lattice.
pub const PACKING_FRACTION: f32 = 0.74;
/// Mean refractive index of the labradorite lamellae.
pub const LAMELLA_INDEX: f32 = 1.56;
/// Strength of the second-order reflection relative to the first.
pub const SECOND_ORDER_STRENGTH: f32 = 0.3;
/// The second order counts only at or above this wavelength, nanometres.
pub const MIN_SECOND_ORDER_NM: f32 = 380.0;

/// The effective index of the opal lattice, `sqrt(phi n_s^2 + (1 - phi) n_m^2)`.
#[must_use]
pub fn opal_effective_index() -> f32 {
    PACKING_FRACTION
        .mul_add(
            SILICA_INDEX * SILICA_INDEX,
            (1.0 - PACKING_FRACTION) * MATRIX_INDEX * MATRIX_INDEX,
        )
        .sqrt()
}

/// The ratio `lambda_B(0) / D` of opal: `2 sqrt(2/3) n_eff`, about 2.33.
#[must_use]
pub fn opal_bragg_ratio() -> f32 {
    2.0 * (2.0f32 / 3.0).sqrt() * opal_effective_index()
}

/// The normal-incidence Bragg wavelength of opal spheres of diameter `diameter_nm`.
#[must_use]
pub fn opal_wavelength_nm(diameter_nm: f32) -> f32 {
    opal_bragg_ratio() * diameter_nm
}

/// The sphere diameter whose normal-incidence Bragg wavelength is `lambda_nm`.
#[must_use]
pub fn opal_diameter_nm(lambda_nm: f32) -> f32 {
    lambda_nm / opal_bragg_ratio()
}

/// The normal-incidence Bragg wavelength of labradorite lamellae of pair period `period_nm`.
#[must_use]
pub fn lamella_wavelength_nm(period_nm: f32) -> f32 {
    2.0 * LAMELLA_INDEX * period_nm
}

/// The Bragg wavelength at the angle whose cosine is `cos_theta` to the lattice normal.
#[must_use]
pub fn bragg_wavelength_nm(lambda_b0_nm: f32, cos_theta: f32) -> f32 {
    lambda_b0_nm * cos_theta.abs()
}

/// The spectral profile of the reflection of a domain whose first-order peak is at
/// `lambda_b_nm`, at `lambda_nm`, with relative bandwidth `purity`.
///
/// A Gaussian of standard deviation `purity * lambda_b` around `lambda_b`, plus the second order
/// (`SECOND_ORDER_STRENGTH` times a Gaussian around `lambda_b / 2`) while that stays at or above
/// [`MIN_SECOND_ORDER_NM`]. The orders are far apart, so the value never exceeds
/// `1 + SECOND_ORDER_STRENGTH`.
#[must_use]
#[expect(
    clippy::suboptimal_flops,
    reason = "the WGSL twin and the pinned goldens use this exact arithmetic"
)]
pub fn spectral_profile(lambda_nm: f32, lambda_b_nm: f32, purity: f32) -> f32 {
    if lambda_b_nm <= 0.0 {
        return 0.0;
    }
    let gaussian = |center: f32| {
        let d = (lambda_nm - center) / (purity * center);
        (-0.5 * d * d).exp()
    };
    let mut value = gaussian(lambda_b_nm);
    let half = 0.5 * lambda_b_nm;
    if half >= MIN_SECOND_ORDER_NM {
        value += SECOND_ORDER_STRENGTH * gaussian(half);
    }
    value
}

/// The reflected direction: `w` mirrored in the lattice normal `g` perturbed by the mosaic
/// spread `mosaic` (the GGX width of the perturbed normal), from two uniform randoms.
///
/// With `mosaic == 0` this is exactly `w - 2 (w . g) g`.
#[must_use]
pub fn bragg_reflect(w: Vec3, g: Vec3, mosaic: f32, u1: f32, u2: f32) -> Vec3 {
    platelet_sample(w, g, mosaic, u1, u2)
}

/// The domain structure of a stone, in the stone frame.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct BraggStructure {
    /// The play-of-color seed of the design: the same seed gives the same domain map.
    pub seed: u32,
    /// The side of a domain cell, model units; also the length `s` in `kappa`.
    pub cell_size: f32,
    /// The shortest normal-incidence Bragg wavelength of any domain, nanometres.
    pub lambda_low_nm: f32,
    /// The longest normal-incidence Bragg wavelength of any domain, nanometres.
    pub lambda_high_nm: f32,
    /// The relative bandwidth `w` of the reflection ("Color Purity"), `0.01..=0.3`.
    pub purity: f32,
    /// The fraction of domains that reflect, `0..=1`.
    pub coverage: f32,
    /// The peak reflectance `R0` over one domain.
    pub peak_reflectance: f32,
    /// The mosaic spread: the GGX width of the reflecting normal about the lattice normal.
    pub mosaic: f32,
    /// The unit lattice normal shared by every domain (labradorite), or zero for a random
    /// normal per domain (opal).
    pub normal: Vec3,
}

impl BraggStructure {
    /// A structure with every value limited to the range the estimator is valid for.
    #[must_use]
    #[expect(
        clippy::too_many_arguments,
        reason = "one argument per field of the structure, which has no builder of its own"
    )]
    pub fn new(
        seed: u32,
        cell_size: f32,
        lambda_range_nm: (f32, f32),
        purity: f32,
        coverage: f32,
        peak_reflectance: f32,
        mosaic: f32,
        normal: Vec3,
    ) -> Self {
        let finite = |x: f32, fallback: f32| if x.is_finite() { x } else { fallback };
        let low = finite(lambda_range_nm.0, 450.0).clamp(350.0, 900.0);
        let high = finite(lambda_range_nm.1, 650.0)
            .clamp(350.0, 900.0)
            .max(low);
        Self {
            seed,
            cell_size: finite(cell_size, 0.2).clamp(0.01, 5.0),
            lambda_low_nm: low,
            lambda_high_nm: high,
            purity: finite(purity, 0.05).clamp(0.01, 0.3),
            coverage: finite(coverage, 0.8).clamp(0.0, 1.0),
            peak_reflectance: finite(peak_reflectance, 0.5).clamp(0.01, 0.95),
            mosaic: finite(mosaic, 0.05).clamp(0.0, 0.5),
            normal: normal.normalize_or_zero(),
        }
    }

    /// The reflection coefficient of an active domain at its peak, per model unit:
    /// `-ln(1 - R0) / s`.
    #[must_use]
    pub fn kappa(&self) -> f32 {
        -(1.0 - self.peak_reflectance).ln() / self.cell_size
    }

    /// A true upper bound of [`Self::sigma`] over every domain, direction and wavelength, per
    /// model unit: the delta-tracking majorant. It includes the second order only when a domain
    /// can reach it (`lambda_high >= 2 * MIN_SECOND_ORDER_NM`).
    #[must_use]
    pub fn majorant(&self) -> f32 {
        let second = if self.lambda_high_nm >= 2.0 * MIN_SECOND_ORDER_NM {
            SECOND_ORDER_STRENGTH
        } else {
            0.0
        };
        self.kappa() * (1.0 + second)
    }

    /// The reflection coefficient of `domain` for a ray at cosine `cos_theta` to its normal, at
    /// `lambda_nm`, per model unit.
    #[must_use]
    pub fn sigma(&self, domain: &super::domains::Domain, cos_theta: f32, lambda_nm: f32) -> f32 {
        if !domain.active {
            return 0.0;
        }
        let lambda_b = bragg_wavelength_nm(domain.lambda_b0_nm, cos_theta);
        self.kappa() * spectral_profile(lambda_nm, lambda_b, self.purity)
    }
}

/// The three layouts a design can ask for. A small cell is Pinfire; a large one is Harlequin or
/// Broad Flash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum StructurePattern {
    /// Many tiny points of color.
    Pinfire,
    /// Medium patches.
    Harlequin,
    /// A few large flashes.
    BroadFlash,
}

impl StructurePattern {
    /// The domain cell size of the pattern, model units.
    #[must_use]
    pub const fn cell_size(self) -> f32 {
        match self {
            Self::Pinfire => 0.05,
            Self::Harlequin => 0.2,
            Self::BroadFlash => 0.5,
        }
    }

    /// The label shown to the user.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Pinfire => "Pinfire",
            Self::Harlequin => "Harlequin",
            Self::BroadFlash => "Broad Flash",
        }
    }
}

/// What kind of structure a built-in material carries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StructureKind {
    /// Precious opal: random lattice normal per domain.
    Opal,
    /// Labradorite: one lamella normal fixed in the crystal frame.
    Labradorite,
}

impl StructureKind {
    /// The stable lowercase name a custom material's stored structure carries.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Opal => "opal",
            Self::Labradorite => "labradorite",
        }
    }

    /// Inverse of [`Self::name`]; `None` for any other text.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        match name.trim() {
            "opal" => Some(Self::Opal),
            "labradorite" => Some(Self::Labradorite),
            _ => None,
        }
    }
}

/// The kind of structure of the built-in material `material_name`, or `None` for a material
/// without play-of-color or schiller of this kind.
#[must_use]
pub fn structure_kind(material_name: &str) -> Option<StructureKind> {
    match material_name {
        "Precious Opal (White)" | "Precious Opal (Black)" => Some(StructureKind::Opal),
        "Labradorite" => Some(StructureKind::Labradorite),
        _ => None,
    }
}

/// The labradorite lamella normal in the crystal frame `(n_alpha, c x n_alpha, c = n_gamma)`.
///
/// The lamellae lie about parallel to (010), tilted about 11 degrees (literature 9 to 21) from
/// it. The app's frame is optical, not crystallographic, and the orientation of a labradorite's
/// indicatrix depends on its composition, so this is an approximation (unverifiable here): the
/// normal is taken perpendicular to `n_alpha` and 11 degrees off the perpendicular to `n_gamma`.
pub const LABRADORITE_NORMAL: CrystalDir = CrystalDir::Tilted {
    tilt_deg: 79.0,
    turn_deg: 90.0,
};

/// The per-design play-of-color choice (`[render_effects.structure]`).
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct StructureSpec {
    /// The layout.
    pub pattern: StructurePattern,
    /// The seed of the domain map, stored with the design (a Reseed button draws a new one).
    pub seed: u32,
    /// The cell size in model units, or `None` for the pattern's own.
    pub size: Option<f32>,
    /// The shortest normal-incidence Bragg wavelength, nanometres.
    pub color_low_nm: f32,
    /// The longest normal-incidence Bragg wavelength, nanometres.
    pub color_high_nm: f32,
    /// The relative bandwidth ("Color Purity").
    pub purity: f32,
    /// The fraction of domains that reflect ("Coverage").
    pub coverage: f32,
}

/// The seed a new design starts with.
pub const DEFAULT_SEED: u32 = 12345;

impl StructureSpec {
    /// The default choice for `material_name`, or `None` when it has no structure.
    #[must_use]
    pub fn default_for(material_name: &str) -> Option<Self> {
        Some(match structure_kind(material_name)? {
            StructureKind::Opal => Self {
                pattern: StructurePattern::Harlequin,
                seed: DEFAULT_SEED,
                size: None,
                color_low_nm: 440.0,
                color_high_nm: 680.0,
                purity: 0.05,
                coverage: 0.8,
            },
            // A domain's wavelength is Gaussian about the middle of the range (`domains`), so
            // the range sets the most common flash: 400-560 nm centres on blue (480 nm) with some
            // green, as the dealer photos flash (2026-11-04); the 470-740 nm of earlier versions
            // centred on green-yellow and, face up with the lamellae along the table, flashed
            // green, orange and red.
            StructureKind::Labradorite => Self {
                pattern: StructurePattern::BroadFlash,
                seed: DEFAULT_SEED,
                size: None,
                color_low_nm: 400.0,
                color_high_nm: 560.0,
                purity: 0.06,
                coverage: 0.9,
            },
        })
    }

    /// The structure of this choice for `material`, in the stone frame, or `None` when the
    /// material has no structure.
    ///
    /// A labradorite's lamella normal is [`LABRADORITE_NORMAL`] in the frame of the material's
    /// `c_axis` (`n_gamma`) and `alpha_axis`, so it follows the Crystal Axis; an opal's lattice
    /// normals are random.
    #[must_use]
    pub fn resolve(&self, material: &GemMaterial) -> Option<BraggStructure> {
        let kind = structure_kind(&material.name)?;
        Some(self.resolve_as(kind, material))
    }

    /// The structure of this choice for `material` read as a structure of `kind`, whatever the
    /// material is called: a custom material that stores the kind it imitates. For a built-in
    /// material of that kind the result is exactly [`Self::resolve`]'s.
    #[must_use]
    pub fn resolve_as(&self, kind: StructureKind, material: &GemMaterial) -> BraggStructure {
        let (peak_reflectance, mosaic, normal) = match kind {
            StructureKind::Opal => (0.6, 0.03, Vec3::ZERO),
            StructureKind::Labradorite => (
                0.5,
                0.10,
                CrystalFrame::new(material.c_axis, material.alpha_axis)
                    .direction(LABRADORITE_NORMAL),
            ),
        };
        BraggStructure::new(
            self.seed,
            self.size.unwrap_or_else(|| self.pattern.cell_size()),
            (self.color_low_nm, self.color_high_nm),
            self.purity,
            self.coverage,
            peak_reflectance,
            mosaic,
            normal,
        )
    }
}

impl GemMaterial {
    /// The material with the Bragg structure `structure` added to its inclusions (the families
    /// it already carries stay).
    #[must_use]
    pub fn with_structure(mut self, structure: BraggStructure) -> Self {
        let set = self
            .inclusions
            .unwrap_or(super::InclusionSet::EMPTY)
            .with_structure(structure);
        self.inclusions = Some(set);
        self
    }

    /// [`Self::with_structure`] for a structure resolved in this material's own crystal frame
    /// (`c_axis`, `alpha_axis`): a set that remembers no frame yet takes that one, so a later
    /// [`super::InclusionSet::rebased`] turns the lamella normal with the design's Crystal Axis.
    fn with_structure_in_own_frame(self, structure: BraggStructure) -> Self {
        let frame = CrystalFrame::new(self.c_axis, self.alpha_axis);
        let mut material = self.with_structure(structure);
        if let Some(set) = material.inclusions.as_mut()
            && set.frame.is_none()
        {
            set.frame = Some(frame);
        }
        material
    }

    /// The material with the design's play-of-color choice applied; a material without a
    /// structure is returned untouched.
    #[must_use]
    pub fn with_structure_spec(self, spec: &StructureSpec) -> Self {
        match spec.resolve(&self) {
            Some(structure) => self.with_structure_in_own_frame(structure),
            None => self,
        }
    }

    /// The material with the play-of-color choice applied as a structure of `kind`, whatever the
    /// material is called (a custom material that stores the kind it imitates).
    #[must_use]
    pub fn with_structure_spec_as(self, kind: StructureKind, spec: &StructureSpec) -> Self {
        let structure = spec.resolve_as(kind, &self);
        self.with_structure_in_own_frame(structure)
    }
}
