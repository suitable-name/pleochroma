//! Pleochroic and biaxial-indicatrix absorption wiring tests.
//!
//! Every built-in material stays finite through the pleochroic absorption block. Sapphire's
//! pleochroism attenuates end to end, Tanzanite's biaxial eigenmodes diverge from the uniaxial
//! fallback off-axis, non-biaxial output stays pinned, and the three biaxial built-ins differ
//! measurably from the uniaxial approximation.

use glam::Vec3;
use pleochroma::{
    geometry::cuts::StandardGemCuts,
    optics::{
        birefringence::{AbsorptionTensor3, BirefringenceParams, effective_pleochroic_alpha},
        materials::GemMaterial,
        raytracer::{LightingPreset, Ray, hash_u32, trace_spectral_ray},
    },
};

/// Every built-in material, including the biaxial Alexandrite, Topaz and Tanzanite, must trace
/// through `trace_spectral_ray` to finite, non-negative XYZ over several seeds and an oblique
/// ray. This catches a NaN or Inf in the per-channel pleochroic quadratic form of
/// `trace_spectral_ray`'s absorption block, which every anisotropic material passes through.
#[test]
fn all_builtin_materials_render_finite_through_pleochroic_absorption() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.22, -1.0, -0.13).normalize(),
    };

    for material in GemMaterial::all_materials() {
        for seed in [1u32, 2, 42, 4242] {
            let xyz = trace_spectral_ray(
                ray,
                &planes,
                &material,
                12,
                LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
                seed,
                (hash_u32(seed) as f32) / 4_294_967_295.0,
                None,
            );
            assert!(
                xyz.is_finite(),
                "{}: trace_spectral_ray must produce finite output (seed={seed}), got {xyz:?}",
                material.name
            );
            assert!(
                xyz.x >= 0.0 && xyz.y >= 0.0 && xyz.z >= 0.0,
                "{}: trace_spectral_ray must produce non-negative XYZ (seed={seed}), got {xyz:?}",
                material.name
            );
        }
    }
}

/// The pleochroic absorption block must engage for a real uniaxial built-in (Sapphire) traced
/// end to end: light through the body must be attenuated, not zeroed or bypassed. This
/// exercises the call site (argument order, `current_plane_normal`, per-channel Stokes vectors)
/// that the unit tests in `birefringence` cannot reach.
#[test]
fn sapphire_pleochroic_absorption_still_attenuates_end_to_end() {
    const SAMPLES: u32 = 64;

    let planes = StandardGemCuts::standard_round_brilliant();
    let sapphire = GemMaterial::sapphire();
    let mut sapphire_colorless = sapphire.clone();
    sapphire_colorless.absorption =
        pleochroma::optics::absorption::AbsorptionTensor::isotropic(vec![]);

    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.15, -1.0, 0.09).normalize(),
    };

    let mut sum_absorbing = Vec3::ZERO;
    let mut sum_colorless = Vec3::ZERO;
    for i in 0..SAMPLES {
        let seed = hash_u32(0xC0FF_EE00 ^ hash_u32(i));
        sum_absorbing += trace_spectral_ray(
            ray,
            &planes,
            &sapphire,
            12,
            LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
            seed,
            (hash_u32(seed) as f32) / 4_294_967_295.0,
            None,
        );
        sum_colorless += trace_spectral_ray(
            ray,
            &planes,
            &sapphire_colorless,
            12,
            LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
            seed,
            (hash_u32(seed) as f32) / 4_294_967_295.0,
            None,
        );
    }
    let luma_absorbing = sum_absorbing.y;
    let luma_colorless = sum_colorless.y;

    assert!(
        luma_absorbing < luma_colorless * 0.99,
        "sapphire's real absorption bands should measurably dim the averaged output relative to an \
         otherwise-identical colorless material (absorbing luma={luma_absorbing}, colorless luma={luma_colorless})"
    );
}

/// Tanzanite's trichroism (three colors down three axes) follows from it being biaxial. The
/// absorption block takes its two eigen-polarizations from `GemMaterial::biaxial_indicatrix`,
/// not from the uniaxial ordinary/extraordinary approximation, which separates only two colors.
/// At a generic, non-axis-aligned direction the biaxial eigenmodes must give a measurably
/// different pleochroic coefficient than the uniaxial fallback, so the biaxial data is used.
#[test]
fn tanzanite_biaxial_eigenmodes_diverge_from_uniaxial_fallback_off_axis() {
    let tanzanite = GemMaterial::by_name("Tanzanite").unwrap();
    let lambda = 550.0f32;
    // Synthetic, strongly dichroic coefficients are used instead of Tanzanite's own three
    // fitted band sets so that this test isolates the eigen-DIRECTION wiring from the
    // material's data.
    let (alpha_o, alpha_e) = (1.0f32, 6.0f32);
    let tensor = AbsorptionTensor3::uniaxial(alpha_o, alpha_e, tanzanite.c_axis);

    let propagation_dir = Vec3::new(0.5, 0.4, 0.6).normalize(); // generic, off every principal axis
    let indicatrix = tanzanite
        .biaxial_indicatrix(lambda)
        .expect("tanzanite must expose a biaxial indicatrix");
    let (biaxial_a, biaxial_b) = indicatrix.eigen_polarizations(propagation_dir);
    let uniaxial_a =
        BirefringenceParams::ordinary_eigen_polarization(propagation_dir, tanzanite.c_axis);
    let uniaxial_b =
        BirefringenceParams::extraordinary_eigen_polarization(propagation_dir, tanzanite.c_axis);

    // Fully polarized light aligned with each pair's own first eigenmode: if the two
    // eigenmode pairs were the same direction, these two coefficients would match.
    let alpha_biaxial = effective_pleochroic_alpha(&tensor, biaxial_a, biaxial_a, biaxial_b, 1.0);
    let alpha_uniaxial =
        effective_pleochroic_alpha(&tensor, uniaxial_a, uniaxial_a, uniaxial_b, 1.0);

    assert!(
        (alpha_biaxial - alpha_uniaxial).abs() > 1e-3,
        "tanzanite's true biaxial eigenmodes should give a measurably different pleochroic \
         coefficient than the uniaxial fallback at an off-axis direction (biaxial={alpha_biaxial}, uniaxial={alpha_uniaxial})"
    );
}

/// Non-biaxial materials (cubic or uniaxial) must render bit for bit as pinned, so the biaxial
/// refraction wiring cannot change them. The pins are literal `f32` bit patterns (`to_bits()`).
/// They cover two cubic materials (Diamond, Cubic Zirconia) and three uniaxial ones with
/// different birefringence signs: Sapphire and Ruby (both trigonal corundum, opposite
/// pleochroism) and Synthetic Moissanite (hexagonal, the strongest birefringence in the
/// catalogue).
#[test]
fn non_biaxial_materials_render_is_pinned_to_current_post_chapter_04_values() {
    const SEED: u32 = 1;

    let planes = StandardGemCuts::standard_round_brilliant();
    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.18, -1.0, 0.07).normalize(),
    };

    // (material name, expected x/y/z as f32 bit patterns). Diamond/Cubic Zirconia are
    // cubic (isotropic) and must stay bit-identical across any anisotropic-only
    // physics change; Sapphire/Ruby/Synthetic Moissanite are genuinely birefringent and
    // are expected to move whenever the uniaxial Fresnel/absorption/exit-splitting
    // machinery changes.
    //
    // The golden reflects three things:
    // (1) `color::cie1931` uses the tabulated CIE 1931 2-degree observer, not the Wyman,
    //     Sloan and Shirley Gaussian fit. This moves every isotropic row.
    // (2) `raytracer::absorption` takes pleochroic absorption from the geometric assigned-mode
    //     alpha (`birefringence::assigned_mode_alpha`), not the Stokes degree-of-polarization
    //     heuristic.
    // (3) The Mueller frame-rotation mirror fix (`polarization.rs`) sets the oblique uniaxial
    //     rows: corundum absorbs the assigned mode's true E-field, so the mirrored oblique paths
    //     are brighter than an unmirrored frame would give. Other candidates were ruled out by
    //     toggle or proof: mode re-coupling, the TIR guard, exit splitting, the observer, the
    //     backdrop, the RNG streams and the studio lighting.
    //
    // The values are drift detectors captured from the code's output, not verified physics. An
    // independent check would need a single-wavelength Beer-Lambert case: one channel through a
    // non-dispersive slab of known length, compared with `exp(-alpha * length)` for the assigned
    // mode's coefficient.
    //
    // The bits are for the optimised test profile (`[profile.test.package.pleochroma]`): the
    // studio sampler rounds a few ULP differently in unoptimised builds.
    let golden: &[(&str, u32, u32, u32)] = &[
        ("Diamond", 0x3F1D_28BE, 0x3F1B_8872, 0x3F1F_C317),
        ("Cubic Zirconia", 0x3FDA_E16D, 0x3FD1_A43F, 0x4018_44C4),
        // Per material, the x, y and z bits of the same ray and seed.
        ("Sapphire", 0x3B2E_8ABA, 0x3AD9_7227, 0x3C4E_DA1F),
        ("Ruby", 0x3BDF_D6CD, 0x3B51_5532, 0x38F5_3B04),
        (
            "Synthetic Moissanite",
            0x3E9B_3B7B,
            0x3E98_9807,
            0x3ED9_EC75,
        ),
    ];

    // Every material is traced before anything is compared, so one failing run prints the
    // whole table (all rows that drifted, not only the first).
    let mut got = Vec::with_capacity(golden.len());
    for &(name, _, _, _) in golden {
        let material = GemMaterial::by_name(name)
            .unwrap_or_else(|| panic!("{name} must be a built-in material"));
        let xyz = trace_spectral_ray(
            ray,
            &planes,
            &material,
            12,
            LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
            SEED,
            (hash_u32(SEED) as f32) / 4_294_967_295.0,
            None,
        );
        got.push((name, xyz.x.to_bits(), xyz.y.to_bits(), xyz.z.to_bits()));
    }
    assert_eq!(
        got.as_slice(),
        golden,
        "a material's x/y/z bit pattern changed (name, x, y, z bits); re-pin only after an \
         intended physics change"
    );
}

/// The biaxial refraction wiring must be consulted for Alexandrite, Topaz and Tanzanite, not
/// left as dead code. Each real render is compared with a clone whose `biaxial_delta_beta_alpha`
/// is `None`, which uses the uniaxial ordinary/extraordinary approximation (`c_axis` as the optic
/// axis, `birefringence_delta` as `n_gamma - n_o`) instead of the biaxial Fresnel solve. Paired
/// samples share their seeds, and an off-axis ray keeps the wave normal oblique to every
/// principal axis. Comparing a material with itself gives exactly zero, so any nonzero
/// difference is signal.
///
/// The three are not equally biaxial. Alexandrite and Topaz have `n_beta` close to `n_alpha` (see
/// their entries in `materials::GemMaterial::all_materials`), so they are nearly uniaxial. The
/// effect at this ray is about 4e-5 for Alexandrite and 1e-5 for Topaz, against about 1e-2 for
/// Tanzanite. The 1e-6 threshold sits an order of magnitude below the smallest of these and well
/// above f32 noise.
#[test]
fn biaxial_materials_measurably_differ_from_uniaxial_fallback_in_refraction() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.18, -1.0, 0.07).normalize(),
    };
    let samples = 64u32;

    for (name, min_diff) in [("Alexandrite", 1e-6), ("Topaz", 1e-6), ("Tanzanite", 1e-6)] {
        let biaxial = GemMaterial::by_name(name)
            .unwrap_or_else(|| panic!("{name} must be a built-in material"));
        assert!(
            biaxial.biaxial_delta_beta_alpha.is_some(),
            "{name} must be a genuinely biaxial built-in"
        );

        let mut uniaxial_fallback = biaxial.clone();
        uniaxial_fallback.biaxial_delta_beta_alpha = None;

        let mut sum_biaxial = Vec3::ZERO;
        let mut sum_fallback = Vec3::ZERO;
        for i in 0..samples {
            let seed = i.wrapping_mul(7919).wrapping_add(13);
            sum_biaxial += trace_spectral_ray(
                ray,
                &planes,
                &biaxial,
                12,
                LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
                seed,
                (hash_u32(seed) as f32) / 4_294_967_295.0,
                None,
            );
            sum_fallback += trace_spectral_ray(
                ray,
                &planes,
                &uniaxial_fallback,
                12,
                LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
                seed,
                (hash_u32(seed) as f32) / 4_294_967_295.0,
                None,
            );
        }
        let avg_biaxial = sum_biaxial / samples as f32;
        let avg_fallback = sum_fallback / samples as f32;
        let diff = (avg_biaxial - avg_fallback).length();

        assert!(
            diff > min_diff,
            "{name}: true biaxial refraction should render measurably differently (averaged over {samples} samples) \
             than the uniaxial ordinary/extraordinary fallback -- got diff={diff} (threshold={min_diff}, \
             biaxial={avg_biaxial:?}, fallback={avg_fallback:?})"
        );
    }
}
