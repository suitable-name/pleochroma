//! Spectral-to-tristimulus color conversion.
//!
//! The CIE 1931 color-matching functions, the spectral-MIS combination weight
//! applied at XYZ integration, von Kries white balance, and the final XYZ -> sRGB
//! gamut/gamma mapping.

use super::{
    NUM_CHANNELS,
    environment::{LightingPreset, blackbody_spectrum},
};
use glam::Vec3;

/// CIE 1931 2° Standard Observer Color Matching Functions, linearly interpolated from
/// the tabulated 5 nm CIE 15:2004 table.
///
/// Delegates to [`crate::color::cie1931::cie_1931_cmf`] (the single source of truth for
/// the table); a thin `Vec3`-returning wrapper since the raytracer's hot paths want a
/// `glam::Vec3` rather than `[f32; 3]`.
#[must_use]
pub fn cie_1931_cmf(lambda_nm: f32) -> Vec3 {
    Vec3::from_array(crate::color::cie1931::cie_1931_cmf(lambda_nm))
}

/// Batched, `Vec3`-returning wrapper around [`crate::color::cie1931::cie_1931_cmf_x8`]. It
/// differs from 8 calls to [`cie_1931_cmf`] by ULPs and is used in the hot integration loops.
#[must_use]
fn cie_1931_cmf_x8(lambdas: &[f32; NUM_CHANNELS]) -> [Vec3; NUM_CHANNELS] {
    crate::color::cie1931::cie_1931_cmf_x8(lambdas).map(Vec3::from_array)
}

/// Identity pass-through for a channel's already-unbiased radiance estimate.
///
/// # Why no per-channel reweight
///
/// The 8 channels are N different integrands, not N techniques for one integrand. Veach's
/// balance heuristic cancels a technique's own pdf, so a different integrand's pdf never
/// belongs in the weight. Multiplying `radiance[k]` by `own_pdf / sum_pdf * N` is biased
/// (over 10 % on the two-channel Fresnel test in `color/spectral_mis_tests.rs`). The shared
/// weight [`spectral_mis_weight`] is applied once at XYZ integration instead.
#[inline]
const fn mis_weighted_radiance(radiance: f32) -> f32 {
    radiance
}

/// The spectral-MIS balance-heuristic weight `N * p_hero(x) / sum_k p_k(x)`.
///
/// `p_k(x)` is `path_pdf[k]`: channel k's running density of having produced the realized
/// path `x`, tracked through `trace_spectral_ray`'s bounce loop. This is Veach's balance
/// heuristic for one-sample MIS over N techniques ("which channel drives the shared path"),
/// each chosen with probability `1/N` (the wrapped hero draw makes that hold). The same
/// scalar multiplies every channel's radiance.
///
/// When a companion's refraction diverges from the hero's, its `path_pdf` and radiance drop
/// to 0, the weight tends to `N`, and the sample concentrates on the hero's own color. When
/// every channel agrees, `sum_pdf` is `NUM_CHANNELS * path_pdf[hero_idx]` and the weight is
/// exactly 1.
///
/// A chromatically terminated channel's radiance must also be zeroed, not only its
/// `path_pdf`: see
/// `two_channel_dispersive_termination_monte_carlo_is_unbiased_under_alternating_hero`.
#[inline]
pub(crate) fn spectral_mis_weight(path_pdf: &[f32; 8], hero_idx: usize) -> f32 {
    let sum_pdf: f32 = path_pdf.iter().sum();
    if sum_pdf <= 1e-12 {
        // Should not happen; return the unbiased weight 1 rather than divide 0 by 0.
        return 1.0;
    }
    (path_pdf.len() as f32) * path_pdf[hero_idx] / sum_pdf
}

/// Spectral radiance to CIE XYZ, normalized by the integral of `y_bar` (106.856).
///
/// Each `radiance[k]` is already unbiased on its own. The shared [`spectral_mis_weight`],
/// computed from the accumulated `path_pdf`, multiplies all channels.
// `pub(crate)`: `renderer::gpu::furnace_check`'s CPU reference uses the same building blocks
// (`cie_1931_cmf`, `spectral_mis_weight`) as the GPU port.
pub(crate) fn integrate_channels_to_xyz(
    radiance: &[f32; NUM_CHANNELS],
    lambdas: &[f32; NUM_CHANNELS],
    path_pdf: &[f32; NUM_CHANNELS],
    hero_idx: usize,
) -> Vec3 {
    let mis_weight = spectral_mis_weight(path_pdf, hero_idx);

    let mut xyz = Vec3::ZERO;
    let norm_factor = (400.0 / NUM_CHANNELS as f32) / 106.856;
    let cmfs = cie_1931_cmf_x8(lambdas);
    for k in 0..NUM_CHANNELS {
        let cmf = cmfs[k];
        let weighted_radiance = mis_weighted_radiance(radiance[k]) * mis_weight;
        xyz += cmf * (weighted_radiance * norm_factor);
    }
    xyz
}

/// The plain comb sum of the exit-collection estimator's collected leaves.
///
/// `collected[k]` is channel `k`'s leaf radiance with its spectral-MIS weight already applied
/// where the leaf was deposited (`refraction::exit_split::leaf_mis_weight`), so no weight is
/// formed here: this is [`integrate_channels_to_xyz`] with a weight of one, the same
/// `(400 / N) / 106.856` normalisation and the same CIE tables.
pub(crate) fn integrate_collected_to_xyz(
    collected: &[f32; NUM_CHANNELS],
    lambdas: &[f32; NUM_CHANNELS],
) -> Vec3 {
    let mut xyz = Vec3::ZERO;
    let norm_factor = (400.0 / NUM_CHANNELS as f32) / 106.856;
    let cmfs = cie_1931_cmf_x8(lambdas);
    for k in 0..NUM_CHANNELS {
        xyz += cmfs[k] * (collected[k] * norm_factor);
    }
    xyz
}

/// The beam tracer's spectral integration (beam guide §9.1, §14.4).
///
/// The midpoint rule over the fixed node set, `Σ_k w · v_k · c̄(λ_k)` with
/// `w = (400 / n_lambda) / 106.856` and the same CIE tables as
/// [`integrate_channels_to_xyz`]. `node_values[i]` belongs to node `first_node + i` of
/// `n_lambda`, at its centre `380 + (node + ½) · 400 / n_lambda`.
#[cfg(feature = "beam")]
#[must_use]
pub fn integrate_nodes_to_xyz(node_values: &[f32], first_node: u32, n_lambda: u32) -> Vec3 {
    // Node counts are tiny (at most a few hundred), so the casts to `f32` are exact.
    #[expect(
        clippy::cast_precision_loss,
        reason = "node counts and indices are far below 2^24, exactly representable"
    )]
    let (delta, n_first) = (400.0_f32 / n_lambda.max(1) as f32, first_node as f32);
    let norm_factor = delta / 106.856;
    let mut xyz = Vec3::ZERO;
    for (i, &value) in node_values.iter().enumerate() {
        #[expect(
            clippy::cast_precision_loss,
            reason = "slot indices are below 2^24, exactly representable"
        )]
        let node = n_first + i as f32;
        let lambda_nm = (node + 0.5).mul_add(delta, 380.0);
        xyz += cie_1931_cmf(lambda_nm) * (value * norm_factor);
    }
    xyz
}

/// The per-channel-family generalisation of [`integrate_channels_to_xyz`] for exit-event
/// spectral splitting, where every channel alive at the exit contributes its own radiance.
///
/// Channel `k` is weighted by `N * path_pdf[hero]` over the sum of `path_pdf[j]` for `j` in
/// `compat[k]`: the hero choices under which channel `k` would be alive on this path
/// (see `refraction::ExitSplitCtx::compat`). Summed over that family the weights
/// equal 1, which keeps the estimator unbiased. A single weight normalised over the hero's
/// own family is biased once companions contribute (+7 % Diamond, +12 % Synthetic
/// Moissanite against a single-wavelength reference).
///
/// `path_pdf` keeps accumulating for a chromatically terminated channel: its radiance is
/// zero, but its density still normalises other channels' weights. Reduces to
/// [`integrate_channels_to_xyz`] when every family is the full set.
pub(crate) fn integrate_channels_to_xyz_families(
    radiance: &[f32; NUM_CHANNELS],
    lambdas: &[f32; NUM_CHANNELS],
    path_pdf: &[f32; NUM_CHANNELS],
    hero_idx: usize,
    compat: [u8; NUM_CHANNELS],
) -> Vec3 {
    let mut xyz = Vec3::ZERO;
    let norm_factor = (400.0 / NUM_CHANNELS as f32) / 106.856;
    let cmfs = cie_1931_cmf_x8(lambdas);
    for k in 0..NUM_CHANNELS {
        let family_pdf: f32 = path_pdf
            .iter()
            .enumerate()
            .filter(|&(j, _)| compat[k] & (1u8 << j) != 0)
            .map(|(_, &p)| p)
            .sum();
        // Same "should not happen" fallback as `spectral_mis_weight`.
        let weight_k = if family_pdf <= 1e-12 {
            1.0
        } else {
            (NUM_CHANNELS as f32) * path_pdf[hero_idx] / family_pdf
        };
        let weighted_radiance = mis_weighted_radiance(radiance[k]) * weight_k;
        xyz += cmfs[k] * (weighted_radiance * norm_factor);
    }
    xyz
}

/// Color temperature (Kelvin) of a lighting preset.
///
/// From [`LightingPreset::params`], the same source `sample_studio_environment` lights the
/// scene with. The white balance must come from that illuminant.
// `pub(crate)`: the GPU white-balance self-test needs the exact same preset-to-
// temperature mapping `sample_studio_environment` derives its lighting from.
pub(crate) const fn illuminant_temperature_k(lighting_preset: LightingPreset) -> f32 {
    lighting_preset.params().temp_k
}

/// CIE D65 white point, matching `color::space::ColorSpace::Srgb::white_point_xy()`.
/// [`compute_illuminant_white_balance`] adapts every studio illuminant toward it, so a neutral
/// render stays neutral after the sRGB encode, not just in XYZ.
const D65_WHITE_X: f32 = 0.3127;
const D65_WHITE_Y: f32 = 0.3290;

/// Bradford cone-response matrix (XYZ to LMS, Lam 1985): the basis that von Kries adaptation
/// is diagonal in, as in ICC v4. Row-major, like `color::space::ColorSpace::xyz_to_rgb_matrix`.
/// Scaling X and Z directly would not be von Kries adaptation, because XYZ values are not
/// cone responses and a diagonal scale there shifts hue under a non-D65 illuminant.
const BRADFORD_XYZ_TO_LMS: [[f32; 3]; 3] = [
    [0.8951, 0.2664, -0.1614],
    [-0.7502, 1.7135, 0.0367],
    [0.0389, -0.0685, 1.0296],
];

/// Inverse of [`BRADFORD_XYZ_TO_LMS`] (LMS to XYZ). The product of the two is the identity
/// to about 1e-7, within `f32` rounding.
const BRADFORD_LMS_TO_XYZ: [[f32; 3]; 3] = [
    [0.986_993, -0.147_054, 0.159_963],
    [0.432_305, 0.518_360, 0.049_291],
    [-0.008_529, 0.040_043, 0.968_487],
];

/// Converts CIE XYZ to Bradford-space cone responses (LMS). See
/// [`BRADFORD_XYZ_TO_LMS`]'s doc comment.
#[inline]
fn xyz_to_lms_bradford(xyz: Vec3) -> Vec3 {
    let m = BRADFORD_XYZ_TO_LMS;
    Vec3::new(
        m[0][0].mul_add(xyz.x, m[0][1].mul_add(xyz.y, m[0][2] * xyz.z)),
        m[1][0].mul_add(xyz.x, m[1][1].mul_add(xyz.y, m[1][2] * xyz.z)),
        m[2][0].mul_add(xyz.x, m[2][1].mul_add(xyz.y, m[2][2] * xyz.z)),
    )
}

/// Converts Bradford-space cone responses (LMS) back to CIE XYZ. See
/// [`BRADFORD_LMS_TO_XYZ`]'s doc comment.
#[inline]
fn lms_to_xyz_bradford(lms: Vec3) -> Vec3 {
    let m = BRADFORD_LMS_TO_XYZ;
    Vec3::new(
        m[0][0].mul_add(lms.x, m[0][1].mul_add(lms.y, m[0][2] * lms.z)),
        m[1][0].mul_add(lms.x, m[1][1].mul_add(lms.y, m[1][2] * lms.z)),
        m[2][0].mul_add(lms.x, m[2][1].mul_add(lms.y, m[2][2] * lms.z)),
    )
}

/// Applies a von Kries white-balance scale to `xyz`.
///
/// The scale comes from [`compute_illuminant_white_balance`]. The steps are XYZ to Bradford
/// LMS, scale each cone response, and back to XYZ. The WGSL twin in
/// `shaders/spectral_transport.wgsl` matches within a ULP budget, since WGSL cannot guarantee
/// bit-identical fma and division.
// `pub(crate)`: `renderer::gpu::estimator_check::run_spectral_debug` must apply the scale
// exactly as the megakernel does.
pub(crate) fn apply_von_kries_white_balance(xyz: Vec3, lms_scale: Vec3) -> Vec3 {
    lms_to_xyz_bradford(xyz_to_lms_bradford(xyz) * lms_scale)
}

/// Integrates a blackbody spectrum at `temp_k` against the CIE 1931 CMFs over
/// 380..=780 nm at 1 nm steps to obtain the per-channel von Kries white-balance scale,
/// in Bradford LMS space, that adapts that illuminant's own white point toward
/// [`D65_WHITE_X`]/[`D65_WHITE_Y`]. Applying this scale via
/// [`apply_von_kries_white_balance`] to a rendered XYZ value neutralizes the
/// illuminant's own color cast without altering the light sources' color
/// temperatures or the `blackbody_spectrum` clamp.
///
/// # Diagonalised in LMS, not XYZ
///
/// A diagonal scale of raw X and Z values is not von Kries adaptation: each cone class mixes
/// X, Y and Z, so every color but the white itself shifts hue. Both the source white and the
/// D65 reference white (at equal luminance) are converted with [`xyz_to_lms_bradford`]
/// before taking the per-cone ratio.
// `pub(crate)`: the GPU white-balance self-test (`renderer::gpu::environment_check`)
// reproduces this exact 401-point (380..=780nm, 1nm step) quadrature and compares against it.
pub(crate) fn compute_illuminant_white_balance(temp_k: f32) -> Vec3 {
    let mut xyz_w = Vec3::ZERO;
    for step in 0..=(780 - 380) {
        let lambda = 380.0f32 + step as f32;
        xyz_w += cie_1931_cmf(lambda) * blackbody_spectrum(lambda, temp_k);
    }

    let target_y = xyz_w.y.max(1e-6);
    let xyz_target = Vec3::new(
        (D65_WHITE_X / D65_WHITE_Y) * target_y,
        target_y,
        ((1.0 - D65_WHITE_X - D65_WHITE_Y) / D65_WHITE_Y) * target_y,
    );

    let lms_source = xyz_to_lms_bradford(xyz_w);
    let lms_target = xyz_to_lms_bradford(xyz_target);

    Vec3::new(
        if lms_source.x > 1e-6 {
            lms_target.x / lms_source.x
        } else {
            1.0
        },
        if lms_source.y > 1e-6 {
            lms_target.y / lms_source.y
        } else {
            1.0
        },
        if lms_source.z > 1e-6 {
            lms_target.z / lms_source.z
        } else {
            1.0
        },
    )
}

/// The cached per-channel von Kries white-balance scale for a lighting preset, computed on
/// first use.
///
/// Called per ray sample from many render threads, so it must not take a shared lock. Each
/// preset has its own `OnceLock<Vec3>` static, selected with a `match`.
///
/// Only presets with [`LightingPreset::uses_white_balance`] (the Planckian ones) are adapted.
/// The D65 presets get the identity: that white is already the sRGB white, and a 6500 K
/// Planckian scale would push neutrals green. The UV lamps have no white point to adapt from.
pub(super) fn illuminant_white_balance(lighting_preset: LightingPreset) -> Vec3 {
    static INCANDESCENT: std::sync::OnceLock<Vec3> = std::sync::OnceLock::new();
    static RING_LIGHTS: std::sync::OnceLock<Vec3> = std::sync::OnceLock::new();
    static DARK_SPOTLIGHT: std::sync::OnceLock<Vec3> = std::sync::OnceLock::new();
    static LIGHT_TENT: std::sync::OnceLock<Vec3> = std::sync::OnceLock::new();
    static ILLUMINANT_A: std::sync::OnceLock<Vec3> = std::sync::OnceLock::new();

    if !lighting_preset.uses_white_balance() {
        return Vec3::ONE;
    }
    let cell = match lighting_preset {
        LightingPreset::Incandescent => &INCANDESCENT,
        LightingPreset::RingLights => &RING_LIGHTS,
        LightingPreset::DarkSpotlight => &DARK_SPOTLIGHT,
        LightingPreset::LightTent => &LIGHT_TENT,
        LightingPreset::IlluminantA => &ILLUMINANT_A,
        LightingPreset::Daylight
        | LightingPreset::IsoHemisphere
        | LightingPreset::DaylightDome
        | LightingPreset::DaylightSun
        | LightingPreset::Aset
        | LightingPreset::ShopLights
        | LightingPreset::WindowDaylight
        | LightingPreset::WhiteTray
        | LightingPreset::UvLamp365
        | LightingPreset::UvLamp395 => {
            return Vec3::ONE;
        }
    };
    *cell
        .get_or_init(|| compute_illuminant_white_balance(illuminant_temperature_k(lighting_preset)))
}

/// ACES filmic tone-map curve for a scalar luminance.
///
/// It is applied to luminance only: a per-RGB-channel curve shifts hue on saturated colors.
#[must_use]
#[expect(
    clippy::many_single_char_names,
    reason = "ACES filmic tonemap (Narkowicz 2015) fit constants, named a..e to match \
              the canonical y*(a*y+b) / (y*(c*y+d)+e) formula as published everywhere \
              it's referenced; renaming them would only obscure the connection to the \
              reference formula for anyone checking this against the source"
)]
pub fn aces_tonemap(y: f32) -> f32 {
    let a = 2.51f32;
    let b = 0.03f32;
    let c = 2.43f32;
    let d = 0.59f32;
    let e = 0.14f32;
    (y * a.mul_add(y, b)) / y.mul_add(c.mul_add(y, d), e)
}

/// Converts a CIE XYZ radiance sample to encoded 8-bit RGBA in an arbitrary wide-gamut
/// [`crate::color::ColorSpace`].
///
/// Space-aware chromaticity-preserving gamut mapping (radially compressing
/// out-of-gamut colors toward the space's own white point in CIE xyY at constant
/// luminance, rather than desaturating/hue-shifting via naive per-channel clamping),
/// ACES filmic tone mapping applied to luminance only, and finally that space's own
/// transfer function. See `crate::color::space` and `crate::color::gamut` for the
/// implementation this delegates to.
///
/// Every call site goes through [`xyz_to_srgb_gamma`]; `space` is the parameter for other
/// output spaces.
#[must_use]
pub fn xyz_to_rgb_in_space(xyz: Vec3, space: crate::color::ColorSpace) -> [u8; 4] {
    space.encode(xyz, crate::color::ToneMap::AcesFilmic { exposure: 1.0 })
}

/// Converts CIE XYZ to sRGB via chromaticity-preserving gamut mapping (Procedure 3).
///
/// Thin wrapper around [`xyz_to_rgb_in_space`] for [`crate::color::ColorSpace::Srgb`], which
/// uses the piecewise sRGB transfer curve, not a flat `1/2.2` gamma.
///
/// The two curves differ most in deep shadow (about 8.5 of 255 levels near linear x 0.002)
/// and by under about 1.5 levels for 0.1 to 0.9. The regression test is
/// `tests/color_tests.rs::
/// srgb_encode_matches_xyz_to_srgb_gamma_reference_within_the_known_gamma_curve_difference`.
#[must_use]
pub fn xyz_to_srgb_gamma(xyz: Vec3) -> [u8; 4] {
    xyz_to_rgb_in_space(xyz, crate::color::ColorSpace::Srgb)
}

#[cfg(test)]
mod spectral_mis_tests;
#[cfg(test)]
mod white_balance_cache_tests;
