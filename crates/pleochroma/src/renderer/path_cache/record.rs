//! The exit record: one way light can leave the stone for a cached sub-pixel position.
//!
//! A record stores the exit direction at the reference wavelength (octahedral, two `u16`), the
//! direction's derivative with respect to wavelength (two `f16`), the absorption-free spectral
//! weight at the eight comb wavelengths (eight `f16`) and three optical-depth coefficients (three
//! `f32` as bits). It is 44 bytes, `Pod`, and read by the GPU as eleven `u32` words.
//!
//! # Absorption is not in the comb
//!
//! A branch's Beer-Lambert factor is `exp(-tau(lambda))` with
//! `tau(lambda) = D_o a_o(lambda) + D_e a_e(lambda) + D_beta a_beta(lambda)`: the material's
//! principal absorption spectra times three coefficients that depend on the branch only (the
//! path length of every interior segment times the squared components of its eigenmode's field
//! along the principal axes, see `bounce::segment_principal_depths`). An absorption band is
//! narrower than the 50 nm comb, so the comb holds only the smooth Fresnel/Stokes weight and
//! phase B multiplies by `exp(-tau)` at each sample's own wavelength.

use glam::{Vec2, Vec3};

/// Number of comb wavelengths a record stores a weight for.
pub const COMB_LEN: usize = 8;

/// The fixed wavelength comb (nm): the centres of eight 50 nm bins covering 385 to 785 nm.
pub const COMB_NM: [f32; COMB_LEN] = [410.0, 460.0, 510.0, 560.0, 610.0, 660.0, 710.0, 760.0];

/// Comb spacing in nm.
pub const COMB_STEP_NM: f32 = 50.0;

/// Reference wavelength (nm) of the stored direction.
pub const REFERENCE_NM: f32 = 560.0;

/// Index of the reference wavelength in [`COMB_NM`].
pub const REFERENCE_SLOT: usize = 3;

/// Wavelength span (nm) the stored direction derivative is expressed per.
pub const DERIVATIVE_SPAN_NM: f32 = 100.0;

/// Number of optical-depth coefficients of a record: one per principal absorption spectrum.
pub const DEPTH_LEN: usize = 3;

/// Slot of the ordinary (`o_ray`, biaxial alpha) coefficient in [`ExitRecord::depths`].
pub const DEPTH_O: usize = 0;
/// Slot of the extraordinary (`e_ray`, biaxial gamma) coefficient.
pub const DEPTH_E: usize = 1;
/// Slot of the `beta_ray` coefficient (zero unless the material is biaxial with a beta spectrum).
pub const DEPTH_BETA: usize = 2;

/// Lower edge (nm) of the first comb bin.
const BIN_ORIGIN_NM: f32 = 385.0;

/// The record holds one wavelength bin only: `weight` is non-zero in slot [`ExitRecord::bin`].
pub const FLAG_PER_BIN: u32 = 1 << 0;
/// The branch ended in the extraordinary (uniaxial) or second (biaxial) eigenmode.
pub const FLAG_EXTRAORDINARY: u32 = 1 << 1;
/// Every interior reflection of the branch was a total internal reflection.
pub const FLAG_TIR_DOMINATED: u32 = 1 << 2;
/// The first-surface reflection of the camera ray, which the lighting's surface glare scales.
pub const FLAG_GLARE: u32 = 1 << 3;
/// The camera ray missed the stone; the record is the direct view of the environment.
pub const FLAG_DIRECT: u32 = 1 << 4;
/// Bit position of the 3-bit wavelength bin of a `PER_BIN` record.
pub const BIN_SHIFT: u32 = 8;
/// Bit position of the 8-bit interior bounce count.
pub const BOUNCE_SHIFT: u32 = 16;

/// One way light can leave the stone for a cached sub-pixel position.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ExitRecord {
    /// Exit direction at 560 nm, octahedral-encoded (2 x u16).
    pub dir_oct: [u16; 2],
    /// d(octahedral position)/d(lambda) per 100 nm (2 x f16 as u16); zero for `PER_BIN` records.
    pub ddir_dlambda: [u16; 2],
    /// Absorption-free spectral weight at the [`COMB_NM`] wavelengths (8 x f16 as u16): the Stokes
    /// intensity the branch carries for unpolarized camera light, before Beer-Lambert absorption.
    pub weight: [u16; COMB_LEN],
    /// `FLAG_*` bits, the wavelength bin and the interior bounce count.
    pub flags: u32,
    /// The folded branch signature of the branch (`signature::fold_sig`): the cover test compares it
    /// with the traced path's.
    pub signature: u32,
    /// The optical-depth coefficients (`f32` bits) in absorption-length units, slots [`DEPTH_O`],
    /// [`DEPTH_E`], [`DEPTH_BETA`]: the branch's optical depth at a wavelength is their dot product
    /// with the material's principal absorption coefficients there. All zero for a branch that
    /// crossed no absorbing material and for every record of a zoned material, whose comb weights
    /// keep their absorption (a scattering material is not cached).
    pub depth_bits: [u32; DEPTH_LEN],
}

const _: () = assert!(size_of::<ExitRecord>() == 44);

/// Narrows a value already masked to 16 bits.
fn low16(value: u32) -> u16 {
    // A masked value always fits; the fallback is unreachable.
    u16::try_from(value & 0xffff).unwrap_or(0)
}

/// Converts an `f32` to IEEE half-precision bits, rounding to nearest even.
#[must_use]
pub fn f32_to_f16_bits(value: f32) -> u16 {
    let bits = value.to_bits();
    let sign = low16(bits >> 16) & 0x8000;
    let exponent = i32::try_from((bits >> 23) & 0xff).unwrap_or(0);
    let mantissa = bits & 0x007f_ffff;
    if exponent == 0xff {
        let payload = if mantissa == 0 { 0 } else { 0x0200 };
        return sign | 0x7c00 | payload;
    }
    let unbiased = exponent - 127;
    if unbiased > 15 {
        return sign | 0x7c00;
    }
    if unbiased >= -14 {
        let half_exponent = u32::try_from(unbiased + 15).unwrap_or(0);
        let mut half = (half_exponent << 10) | (mantissa >> 13);
        let round_bits = mantissa & 0x1fff;
        if round_bits > 0x1000 || (round_bits == 0x1000 && (half & 1) == 1) {
            half += 1;
        }
        return sign | low16(half);
    }
    if unbiased >= -25 {
        let full = mantissa | 0x0080_0000;
        let shift = u32::try_from(-unbiased - 1).unwrap_or(24);
        let mut half = full >> shift;
        let remainder = full & ((1_u32 << shift) - 1);
        let halfway = 1_u32 << (shift - 1);
        if remainder > halfway || (remainder == halfway && (half & 1) == 1) {
            half += 1;
        }
        return sign | low16(half);
    }
    sign
}

/// Converts IEEE half-precision bits to an `f32`.
#[must_use]
pub fn f16_bits_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 == 0 { 1.0 } else { -1.0 };
    let exponent = i32::from((bits >> 10) & 0x1f);
    let mantissa = f32::from(bits & 0x03ff);
    let magnitude = match exponent {
        0 => mantissa * 2.0_f32.powi(-24),
        0x1f => {
            if bits.trailing_zeros() >= 10 {
                f32::INFINITY
            } else {
                f32::NAN
            }
        }
        _ => (1.0 + mantissa / 1024.0) * 2.0_f32.powi(exponent - 15),
    };
    sign * magnitude
}

/// The index a whole-number `f32` in `0..=7` names.
const fn whole_index(value: f32) -> usize {
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "callers pass a whole number clamped to 0..=7"
    )]
    let index = value.clamp(0.0, 7.0) as usize;
    index
}

/// Quantizes a value in `[0, 1]` to 16 bits.
fn quantize_unorm16(value: f32) -> u16 {
    let scaled = (value.clamp(0.0, 1.0) * 65535.0).round();
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "the value is clamped to 0..=65535 just above"
    )]
    let bits = scaled as u16;
    bits
}

/// Octahedral position (in `[-1, 1]^2`) of a direction.
#[must_use]
pub fn oct_encode(dir: Vec3) -> Vec2 {
    let unit = dir.normalize_or_zero();
    let unit = if unit.length_squared() < 0.5 {
        Vec3::Z
    } else {
        unit
    };
    let l1 = unit.x.abs() + unit.y.abs() + unit.z.abs();
    let p = Vec2::new(unit.x, unit.y) / l1;
    if unit.z < 0.0 {
        Vec2::new(
            (1.0 - p.y.abs()).copysign(p.x),
            (1.0 - p.x.abs()).copysign(p.y),
        )
    } else {
        p
    }
}

/// Unit direction of an octahedral position.
#[must_use]
pub fn oct_decode(p: Vec2) -> Vec3 {
    let z = 1.0 - p.x.abs() - p.y.abs();
    let v = if z < 0.0 {
        Vec3::new(
            (1.0 - p.y.abs()).copysign(p.x),
            (1.0 - p.x.abs()).copysign(p.y),
            z,
        )
    } else {
        Vec3::new(p.x, p.y, z)
    };
    v.normalize_or_zero()
}

/// Quantizes an octahedral position to two `u16`.
#[must_use]
pub fn oct_pack(p: Vec2) -> [u16; 2] {
    [
        quantize_unorm16(p.x.mul_add(0.5, 0.5)),
        quantize_unorm16(p.y.mul_add(0.5, 0.5)),
    ]
}

/// Inverse of [`oct_pack`].
#[must_use]
pub fn oct_unpack(bits: [u16; 2]) -> Vec2 {
    Vec2::new(
        (f32::from(bits[0]) / 65535.0).mul_add(2.0, -1.0),
        (f32::from(bits[1]) / 65535.0).mul_add(2.0, -1.0),
    )
}

/// Index of the comb bin that contains `lambda_nm` (clamped to the comb).
#[must_use]
pub fn bin_of(lambda_nm: f32) -> usize {
    let index = ((lambda_nm - BIN_ORIGIN_NM) / COMB_STEP_NM).floor();
    if index.is_nan() {
        0
    } else {
        whole_index(index)
    }
}

impl ExitRecord {
    /// Builds a record without absorption from its unquantized parts.
    ///
    /// `slope` is the derivative of the octahedral position per 100 nm and `weight` the
    /// absorption-free comb weight. [`Self::with_depth`] adds the optical-depth coefficients.
    #[must_use]
    pub fn new(
        dir: Vec3,
        slope: Vec2,
        weight: [f32; COMB_LEN],
        flags: u32,
        signature: u32,
    ) -> Self {
        Self {
            dir_oct: oct_pack(oct_encode(dir)),
            ddir_dlambda: [f32_to_f16_bits(slope.x), f32_to_f16_bits(slope.y)],
            weight: weight.map(f32_to_f16_bits),
            flags,
            signature,
            depth_bits: [0; DEPTH_LEN],
        }
    }

    /// The record with the optical-depth coefficients `depth` (a coefficient that is not a
    /// positive finite number is stored as zero).
    #[must_use]
    pub const fn with_depth(mut self, depth: [f32; DEPTH_LEN]) -> Self {
        let mut slot = 0;
        while slot < DEPTH_LEN {
            let value = depth[slot];
            self.depth_bits[slot] = if value.is_finite() && value > 0.0 {
                value.to_bits()
            } else {
                0
            };
            slot += 1;
        }
        self
    }

    /// The three optical-depth coefficients.
    #[must_use]
    pub const fn depths(&self) -> [f32; DEPTH_LEN] {
        [
            f32::from_bits(self.depth_bits[0]),
            f32::from_bits(self.depth_bits[1]),
            f32::from_bits(self.depth_bits[2]),
        ]
    }

    /// `true` when the branch crossed absorbing material (a coefficient is not zero).
    #[must_use]
    pub const fn has_depth(&self) -> bool {
        (self.depth_bits[0] | self.depth_bits[1] | self.depth_bits[2]) != 0
    }

    /// The branch's optical depth for the principal absorption coefficients `alphas` of one
    /// wavelength (slots [`DEPTH_O`], [`DEPTH_E`], [`DEPTH_BETA`]).
    ///
    /// The GPU twin is `pcb_optical_depth` of `shaders/path_cache/phase_b.wgsl`.
    #[must_use]
    pub fn optical_depth(&self, alphas: [f32; DEPTH_LEN]) -> f32 {
        let depth = self.depths();
        depth[0].mul_add(alphas[0], depth[1].mul_add(alphas[1], depth[2] * alphas[2]))
    }

    /// The exit direction at the reference wavelength.
    #[must_use]
    pub fn direction(&self) -> Vec3 {
        oct_decode(oct_unpack(self.dir_oct))
    }

    /// The exit direction at `lambda_nm`: the reference direction plus the derivative term.
    #[must_use]
    pub fn direction_at(&self, lambda_nm: f32) -> Vec3 {
        let ddir = Vec2::new(
            f16_bits_to_f32(self.ddir_dlambda[0]),
            f16_bits_to_f32(self.ddir_dlambda[1]),
        );
        let shift = (lambda_nm - REFERENCE_NM) / DERIVATIVE_SPAN_NM;
        oct_decode(oct_unpack(self.dir_oct) + ddir * shift)
    }

    /// The eight stored absorption-free weights as `f32`.
    #[must_use]
    pub fn weights(&self) -> [f32; COMB_LEN] {
        self.weight.map(f16_bits_to_f32)
    }

    /// `true` when the record holds one wavelength bin only.
    #[must_use]
    pub const fn is_per_bin(&self) -> bool {
        self.flags & FLAG_PER_BIN != 0
    }

    /// The wavelength bin of a `PER_BIN` record.
    #[must_use]
    pub const fn bin(&self) -> usize {
        ((self.flags >> BIN_SHIFT) & 7) as usize
    }

    /// The absorption-free spectral weight at `lambda_nm`.
    ///
    /// A merged record interpolates linearly between the comb wavelengths (clamped at the ends).
    /// A `PER_BIN` record is its bin's value inside the bin and zero outside. The weight a sample
    /// sees is this times `exp(-optical_depth)` at its wavelength (`shade::Shader`); the cover
    /// test reads this value alone.
    #[must_use]
    pub fn weight_at(&self, lambda_nm: f32) -> f32 {
        let weights = self.weights();
        if self.is_per_bin() {
            let bin = self.bin();
            return if bin_of(lambda_nm) == bin {
                weights[bin]
            } else {
                0.0
            };
        }
        let position = ((lambda_nm - COMB_NM[0]) / COMB_STEP_NM).clamp(0.0, 7.0);
        let lower_whole = position.floor().min(6.0);
        let fraction = position - lower_whole;
        let lower = whole_index(lower_whole);
        weights[lower].mul_add(1.0 - fraction, weights[lower + 1] * fraction)
    }

    /// `true` when the camera ray missed the stone and the record is the direct view.
    #[must_use]
    pub const fn is_direct(&self) -> bool {
        self.flags & FLAG_DIRECT != 0
    }

    /// The interior bounce count the record's branch took (saturating at 255).
    #[must_use]
    pub const fn bounces(&self) -> u32 {
        (self.flags >> BOUNCE_SHIFT) & 0xff
    }
}
