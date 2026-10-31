//! The per-design inclusion choice and its resolution into an [`InclusionSet`].
//!
//! A design stores a preset name, a density and a few look controls
//! (`[render_effects.inclusions]`); the families themselves are per material, in the catalogue.
//! [`InclusionSpec::resolve`] looks the preset up for the stone's material, scales the strengths
//! with the density, applies the sheen and color choices, and rotates the families into the
//! stone frame with [`CrystalFrame`].

use super::{CrystalFrame, Family, InclusionSet, catalogue};
use crate::optics::{birefringence::alpha_axis_from_turn, materials::GemMaterial};
use glam::Vec3;

/// The density at which a preset's catalogue strengths apply unchanged.
pub const DEFAULT_DENSITY: f32 = 0.35;

/// How sharply the scattered light concentrates on the needle cone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Sheen {
    /// A wider cone: a diffuse glow.
    Soft,
    /// A narrower cone: sharp streaks.
    Sharp,
}

/// The color of the scattered light, as the spectral exponent `p`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum SilkColor {
    /// `p = 0`: white.
    Grey,
    /// `p = 1`.
    SlightlyBlue,
    /// `p = 2`: fine silk looks bluish.
    Blue,
}

impl SilkColor {
    /// The spectral exponent.
    #[must_use]
    pub const fn exponent(self) -> u8 {
        match self {
            Self::Grey => 0,
            Self::SlightlyBlue => 1,
            Self::Blue => 2,
        }
    }
}

/// A design's inclusion choice.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct InclusionSpec {
    /// The catalogue preset, by name (for example `"Fine Silk"`).
    pub preset: String,
    /// `0..=1`; see [`density_factor`].
    pub density: f32,
    /// The Silk Turn in degrees about the c axis, only when it differs from the stored Second
    /// Axis turn. `None` follows the stored one. The turn is about the canonical sign of the
    /// axis (see `canonical_line`), so it names the same line as a Second Axis of that turn.
    pub turn_deg: Option<f32>,
    /// `None` keeps the preset's own cone width.
    pub sheen: Option<Sheen>,
    /// `None` keeps the preset's own spectral exponent.
    pub color: Option<SilkColor>,
}

/// What a spec resolves to: the families and the haze asymmetry of the species.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResolvedInclusions {
    /// The families in the stone frame.
    pub set: InclusionSet,
    /// The Henyey-Greenstein asymmetry the plain haze takes while the preset is on.
    pub haze_g: f32,
}

/// The factor on a preset's catalogue strengths at `density`: `8 * density^2`.
///
/// One at [`DEFAULT_DENSITY`] (to 2 %), zero at zero and eight at one (the square, like the haze
/// slider's `position_to_sigma_s`, spends the travel near the clean end).
#[must_use]
pub fn density_factor(density: f32) -> f32 {
    if density.is_nan() {
        return 0.0;
    }
    let d = density.clamp(0.0, 1.0);
    8.0 * d * d
}

/// How close to the table plane (`y == 0`) a unit axis counts as lying on it.
const EQUATOR_TOLERANCE: f32 = 1e-6;

/// The sign of the unit axis `c` that names its line canonically: the upper hemisphere
/// (`y > 0`), and on the equator the azimuth in `[0, 180)` (`z > 0`, or `z == 0` with `x > 0`).
///
/// Twin of `pleochroma_cut_core::crystal_axis::CrystalAxis::from_direction(c).direction()`, which
/// the stored Second Axis turns about (`CrystalOrientation::alpha`). The Silk Turn must turn about
/// the same signed vector, or "Silk Turn 30" and "Second Axis 30" would be different lines for any
/// axis whose slider vector points into the lower hemisphere (tilt beyond 90, azimuth past 180 on
/// the equator). A zero vector stays zero.
fn canonical_line(c: Vec3) -> Vec3 {
    let flip = if c.y.abs() < EQUATOR_TOLERANCE {
        c.z < -EQUATOR_TOLERANCE || (c.z.abs() <= EQUATOR_TOLERANCE && c.x < 0.0)
    } else {
        c.y < 0.0
    };
    if flip { -c } else { c }
}

impl InclusionSpec {
    /// The families of this spec for `material` in the stone frame, or `None` when the preset
    /// is unknown for the material or the density is zero.
    ///
    /// `material` supplies the principal axis (`c_axis`, with any orientation override
    /// already applied) and, for a biaxial one, the `n_alpha` direction (`alpha_axis`).
    /// `stored_second_axis` is the stone's stored Second Axis for a uniaxial material, which the
    /// optics ignore but the silk reads as its Silk Turn. The turn of the spec, when set, wins.
    #[must_use]
    pub fn resolve(
        &self,
        material: &GemMaterial,
        stored_second_axis: Option<Vec3>,
    ) -> Option<ResolvedInclusions> {
        let preset = catalogue::find_preset(&material.name, &self.preset)?;
        let factor = density_factor(self.density);
        if factor <= 0.0 {
            return None;
        }
        let c = material.c_axis.normalize_or_zero();
        let second = match self.turn_deg.filter(|t| t.is_finite()) {
            Some(turn) => Some(alpha_axis_from_turn(canonical_line(c), turn)),
            None if material.biaxial_delta_beta_alpha.is_some() => material.alpha_axis,
            None => stored_second_axis,
        };
        let families: Vec<Family> = preset
            .families
            .iter()
            .map(|f| {
                let lobe = match (self.sheen, f.kind) {
                    (Some(Sheen::Soft), super::FamilyKind::Platelet) => f.lobe * 1.5,
                    (Some(Sheen::Sharp), super::FamilyKind::Platelet) => f.lobe * 0.6,
                    (Some(Sheen::Soft), _) => f.lobe * 2.0,
                    (Some(Sheen::Sharp), _) => f.lobe * 0.5,
                    (None, _) => f.lobe,
                };
                Family {
                    sigma: f.sigma * factor,
                    p: self.color.map_or(f.p, SilkColor::exponent),
                    lobe,
                    ..*f
                }
            })
            .collect();
        let set = CrystalFrame::new(c, second).resolve(&families);
        set.is_active().then_some(ResolvedInclusions {
            set,
            haze_g: preset.haze_g,
        })
    }
}

impl GemMaterial {
    /// The material with the design's inclusion choice applied: the resolved families, and the
    /// species haze asymmetry for the plain haze. A spec that resolves to nothing (unknown
    /// preset, zero density) returns the material untouched.
    #[must_use]
    pub fn with_inclusion_spec(
        self,
        spec: &InclusionSpec,
        stored_second_axis: Option<Vec3>,
    ) -> Self {
        match spec.resolve(&self, stored_second_axis) {
            Some(resolved) => {
                let mut material = self.with_inclusions(resolved.set);
                material.scattering_g = resolved.haze_g;
                material
            }
            None => self,
        }
    }
}
