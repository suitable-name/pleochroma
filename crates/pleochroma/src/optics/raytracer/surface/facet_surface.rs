//! The surface finish of one facet, as the renderer sees it.
//!
//! [`FacetSurface`] IS the packed `u32` the GPU reads per facet (binding 8, the word of
//! `encode_facet_finishes`): the CPU decodes the same quantised word, so the two twins see
//! identical parameters. Layout:
//!
//! | Bits  | Field |
//! |-------|-------|
//! | 0-1   | kind: 0 Ideal, 1 Frosted (today's girdle), 2 Polished with roughness, 3 Ground |
//! | 2-11  | level: `Rq` in 0.1 nm (Polished), or the Ground roughness `α` in 0.001 (0 = default) |
//! | 12-19 | anisotropy code: `16 × α_across / α_along` (0 = round) |
//! | 20-29 | scratch angle code: degrees `× 1023 / 180`, used with the Custom Angle bit |
//! | 30    | Custom Angle: the scratch angle is fixed, the machine is ignored |
//! | 31    | No Sweep: the fan of scratch angles collapses to the machine's nominal angle |
//!
//! Words 0 and 1 keep their meaning, so an old buffer decodes unchanged. Scratch Strength is not
//! stored: it is folded into the anisotropy when the word is built
//! ([`effective_anisotropy`]).

use super::ggx::GgxLobe;

/// The roughness of a Ground facet when the word names none: what today's frosted girdle becomes.
pub const DEFAULT_GROUND_ALPHA: f32 = 0.45;

/// The default Scratch Strength: how much of a preset's anisotropy a design carries.
pub const DEFAULT_SCRATCH_STRENGTH: f32 = 0.3;

const KIND_MASK: u32 = 0b11;
const LEVEL_SHIFT: u32 = 2;
const LEVEL_MASK: u32 = 0x3FF;
const ANISO_SHIFT: u32 = 12;
const ANISO_MASK: u32 = 0xFF;
const ANGLE_SHIFT: u32 = 20;
const ANGLE_MASK: u32 = 0x3FF;
const CUSTOM_ANGLE_BIT: u32 = 1 << 30;
const NO_SWEEP_BIT: u32 = 1 << 31;

/// The anisotropy code step: `code = 16 × ratio`.
const ANISO_STEPS_PER_UNIT: f32 = 16.0;

/// The anisotropy code of a ratio of exactly 1.
const ANISO_ROUND_CODE: u32 = 16;

/// The kind of surface a facet has.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SurfaceKind {
    /// A perfect polish: the exact interface physics, no scatter.
    #[default]
    Ideal,
    /// Today's frosted girdle; now a Ground facet of the default roughness.
    Frosted,
    /// A real polish: the exact interface physics plus a halo of total integrated scatter.
    Polished,
    /// A rough-dielectric (GGX) ground surface.
    Ground,
}

/// A named polish grade.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolishPreset {
    /// A 50k to 100k diamond or alumina lap.
    Mirror,
    /// A 14k diamond lap.
    Fine,
    /// An 8k diamond lap.
    Commercial,
    /// A 3k diamond lap.
    Prepolish,
}

impl PolishPreset {
    /// The RMS roughness in nanometres.
    #[must_use]
    pub const fn rq_nm(self) -> f32 {
        match self {
            Self::Mirror => 1.0,
            Self::Fine => 2.5,
            Self::Commercial => 6.0,
            Self::Prepolish => 25.0,
        }
    }

    /// The correlation length of the roughness in micrometres.
    #[must_use]
    pub const fn correlation_length_um(self) -> f32 {
        match self {
            Self::Mirror => 0.20,
            Self::Fine => 0.33,
            Self::Commercial => 0.48,
            Self::Prepolish => 1.25,
        }
    }

    /// The GGX width of the halo, `α = 2 Rq / ℓc`.
    #[must_use]
    pub fn halo_alpha(self) -> f32 {
        2.0 * self.rq_nm() / (self.correlation_length_um() * 1000.0)
    }

    /// The ratio of the roughness across the scratches to that along them at full strength.
    #[must_use]
    pub const fn anisotropy(self) -> f32 {
        match self {
            Self::Mirror => 1.0,
            Self::Fine => 2.0,
            Self::Commercial => 3.0,
            Self::Prepolish => 4.0,
        }
    }
}

/// The anchors of [`halo_alpha`]: `(Rq in nm, α = 2 Rq / ℓc)` of the presets (the Fine preset's
/// 0.01515 is the R0 table's rounded 0.015).
const HALO_ANCHORS: [(f32, f32); 4] = [(1.0, 0.010), (2.5, 0.015_152), (6.0, 0.025), (25.0, 0.040)];

/// The upper bound of the halo width.
const MAX_HALO_ALPHA: f32 = 0.5;

/// The GGX width of the halo of a polish of roughness `rq_nm`.
///
/// The correlation length is not stored in the packed word, so the width is interpolated in
/// log-log between the four presets (`α = 2 Rq / ℓc` at each anchor); it is proportional to `Rq`
/// below the first anchor and continues the last slope above the last.
#[must_use]
#[expect(
    clippy::suboptimal_flops,
    reason = "the WGSL twin and the pinned goldens use this exact arithmetic"
)]
pub fn halo_alpha(rq_nm: f32) -> f32 {
    if rq_nm <= 0.0 {
        return 0.0;
    }
    let (first_rq, first_alpha) = HALO_ANCHORS[0];
    if rq_nm < first_rq {
        return first_alpha * rq_nm / first_rq;
    }
    // The segment holding `rq_nm`; above the last anchor the last segment continues.
    let mut segment = (HALO_ANCHORS[2], HALO_ANCHORS[3]);
    for pair in HALO_ANCHORS.windows(2) {
        if rq_nm <= pair[1].0 {
            segment = (pair[0], pair[1]);
            break;
        }
    }
    let ((rq0, a0), (rq1, a1)) = segment;
    if rq_nm == rq0 {
        return a0.min(MAX_HALO_ALPHA);
    }
    let t = (rq_nm / rq0).ln() / (rq1 / rq0).ln();
    let alpha = t.mul_add(a1.ln() - a0.ln(), a0.ln()).exp();
    alpha.min(MAX_HALO_ALPHA)
}

/// The anisotropy a design carries: `1 + strength (preset − 1)`, so Scratch Strength `0` is a
/// round halo and `1` the preset's full ratio.
#[must_use]
#[expect(
    clippy::suboptimal_flops,
    reason = "the WGSL twin and the pinned goldens use this exact arithmetic"
)]
pub fn effective_anisotropy(preset_ratio: f32, strength: f32) -> f32 {
    1.0 + strength.clamp(0.0, 1.0) * (preset_ratio - 1.0).max(0.0)
}

/// One facet's surface finish, in the quantised form of the GPU word.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct FacetSurface(u32);

#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "every quantiser clamps its value into the field's range first"
)]
fn quantise(value: f32, scale: f32, max: u32) -> u32 {
    (value * scale).round().clamp(0.0, max as f32) as u32
}

impl FacetSurface {
    /// A perfect polish.
    pub const IDEAL: Self = Self(0);

    /// The surface of a word the GPU buffer holds.
    #[must_use]
    pub const fn from_packed(word: u32) -> Self {
        Self(word)
    }

    /// The packed word.
    #[must_use]
    pub const fn packed(self) -> u32 {
        self.0
    }

    /// Today's frosted girdle: a Ground facet of the default roughness.
    #[must_use]
    pub const fn frosted() -> Self {
        Self(1)
    }

    /// A polish of roughness `rq_nm` (clamped to 102.3 nm, the field's range) whose halo is
    /// stretched by `anisotropy` (at least 1) across the scratches. `rq_nm <= 0` is Ideal.
    #[must_use]
    pub fn polished(rq_nm: f32, anisotropy: f32) -> Self {
        let level = quantise(rq_nm, 10.0, LEVEL_MASK);
        if level == 0 {
            return Self::IDEAL;
        }
        Self(2 | (level << LEVEL_SHIFT) | Self::anisotropy_bits(anisotropy))
    }

    /// A preset polish at Scratch Strength `strength`.
    #[must_use]
    pub fn from_preset(preset: PolishPreset, strength: f32) -> Self {
        Self::polished(
            preset.rq_nm(),
            effective_anisotropy(preset.anisotropy(), strength),
        )
    }

    /// A GGX Ground surface of roughness `alpha` (clamped to `[0.001, 1]`) whose lobe is
    /// stretched by `anisotropy` across the scratches.
    #[must_use]
    pub fn ground(alpha: f32, anisotropy: f32) -> Self {
        let level = quantise(alpha, 1000.0, 1000).max(1);
        Self(3 | (level << LEVEL_SHIFT) | Self::anisotropy_bits(anisotropy))
    }

    /// The same surface with the scratch direction fixed at `angle_deg` from the girdle-parallel
    /// direction (a line: taken modulo 180 degrees); the machine is ignored.
    #[must_use]
    pub fn with_custom_angle(self, angle_deg: f32) -> Self {
        let wrapped = angle_deg.rem_euclid(180.0);
        let code = quantise(wrapped, 1023.0 / 180.0, ANGLE_MASK);
        Self((self.0 & !(ANGLE_MASK << ANGLE_SHIFT)) | (code << ANGLE_SHIFT) | CUSTOM_ANGLE_BIT)
    }

    /// The same surface with the sweep collapsed: the scratches take the machine's nominal angle.
    #[must_use]
    pub const fn with_no_sweep(self) -> Self {
        Self(self.0 | NO_SWEEP_BIT)
    }

    /// The same surface with its polish roughness raised to at least `rq_nm` (clamped to the
    /// field's range). An Ideal or Polished surface becomes Polished; a Frosted or Ground surface,
    /// and a roughness already at or above `rq_nm`, stay as they are. The scratch bits are kept.
    #[must_use]
    pub fn with_rq_at_least(self, rq_nm: f32) -> Self {
        let level = quantise(rq_nm, 10.0, LEVEL_MASK);
        if level == 0 || self.is_ground() {
            return self;
        }
        if matches!(self.kind(), SurfaceKind::Polished) && self.level() >= level {
            return self;
        }
        let kept = self.0 & !(KIND_MASK | (LEVEL_MASK << LEVEL_SHIFT));
        Self(kept | 2 | (level << LEVEL_SHIFT))
    }

    fn anisotropy_bits(anisotropy: f32) -> u32 {
        let code = quantise(anisotropy, ANISO_STEPS_PER_UNIT, ANISO_MASK);
        // A ratio of 1 or below is a round lobe: code 0 and the code of ratio 1 decode alike.
        if code <= ANISO_ROUND_CODE {
            0
        } else {
            code << ANISO_SHIFT
        }
    }

    /// The kind of surface.
    #[must_use]
    pub const fn kind(self) -> SurfaceKind {
        match self.0 & KIND_MASK {
            0 => SurfaceKind::Ideal,
            1 => SurfaceKind::Frosted,
            2 => SurfaceKind::Polished,
            _ => SurfaceKind::Ground,
        }
    }

    const fn level(self) -> u32 {
        (self.0 >> LEVEL_SHIFT) & LEVEL_MASK
    }

    /// `true` for a perfect polish.
    #[must_use]
    pub const fn is_ideal(self) -> bool {
        matches!(self.kind(), SurfaceKind::Ideal)
    }

    /// `true` for the kinds the ground (frosted) arm handles: Frosted and Ground.
    #[must_use]
    pub const fn is_ground(self) -> bool {
        matches!(self.kind(), SurfaceKind::Frosted | SurfaceKind::Ground)
    }

    /// `true` for a polish with a roughness above zero, which scatters a halo.
    #[must_use]
    pub const fn has_roughness(self) -> bool {
        matches!(self.kind(), SurfaceKind::Polished) && self.level() > 0
    }

    /// The RMS roughness in nanometres of a Polished surface (`0` for any other kind).
    #[must_use]
    pub fn rq_nm(self) -> f32 {
        if matches!(self.kind(), SurfaceKind::Polished) {
            self.level() as f32 / 10.0
        } else {
            0.0
        }
    }

    /// The GGX roughness of a Frosted or Ground surface (`0` for any other kind); a word without
    /// a roughness code gets [`DEFAULT_GROUND_ALPHA`].
    #[must_use]
    pub fn ground_alpha(self) -> f32 {
        if !self.is_ground() {
            return 0.0;
        }
        match self.level() {
            0 => DEFAULT_GROUND_ALPHA,
            level => level as f32 / 1000.0,
        }
    }

    /// The ratio of the roughness across the scratches to that along them (`1` for a round lobe).
    #[must_use]
    pub fn anisotropy(self) -> f32 {
        let code = (self.0 >> ANISO_SHIFT) & ANISO_MASK;
        (code as f32 / ANISO_STEPS_PER_UNIT).max(1.0)
    }

    /// The fixed scratch angle in degrees, when the Custom Angle bit is set.
    #[must_use]
    pub fn custom_angle_deg(self) -> Option<f32> {
        (self.0 & CUSTOM_ANGLE_BIT != 0)
            .then(|| ((self.0 >> ANGLE_SHIFT) & ANGLE_MASK) as f32 * (180.0 / 1023.0))
    }

    /// `true` when the fan of scratch angles is collapsed.
    #[must_use]
    pub const fn no_sweep(self) -> bool {
        self.0 & NO_SWEEP_BIT != 0
    }

    /// The lobe of the halo of a Polished surface.
    #[must_use]
    pub fn halo_lobe(self) -> GgxLobe {
        GgxLobe::anisotropic(halo_alpha(self.rq_nm()), self.anisotropy())
    }

    /// The lobe of a Frosted or Ground surface.
    #[must_use]
    pub fn ground_lobe(self) -> GgxLobe {
        GgxLobe::anisotropic(self.ground_alpha(), self.anisotropy())
    }
}
