//! Bit-exact golden values for `trace_spectral_ray`, `sample_studio_environment` and
//! `evaluate_gem_optical_metrics`, over a small diamond and sapphire case table.

use glam::Vec3;
use pleochroma::{
    color::metrics::evaluate_gem_optical_metrics,
    geometry::cuts::StandardGemCuts,
    optics::{
        materials::GemMaterial,
        raytracer::{LightingPreset, Ray, hash_u32, trace_spectral_ray},
    },
};
use std::hint::black_box;

/// One case of `studio_rig_trace_output_is_pinned_to_current_values`: ray, rng seed, lighting
/// preset, light yaw and pitch, then the expected diamond and sapphire XYZ bits.
type GoldenCase = (Ray, u32, LightingPreset, f32, f32, [u32; 3], [u32; 3]);

/// Pins the output bit for bit (`f32::to_bits()`) across diamond and sapphire rays, presets
/// and light poses. Any drift means the estimator or these metrics changed.
///
/// The tables record the code's current output. They are drift detectors, not independently
/// derived physics. Re-pin a table only after an intended change to the quantity it covers,
/// and name that change in the commit.
#[test]
#[expect(
    clippy::too_many_lines,
    reason = "a bit-exact golden-value regression test; splitting the case table from \
              its assertions risks the exact kind of accidental drift this test exists \
              to catch"
)]
fn studio_rig_trace_output_is_pinned_to_current_values() {
    use pleochroma::optics::raytracer::sample_studio_environment;
    let planes = StandardGemCuts::standard_round_brilliant();
    let diamond = GemMaterial::diamond();
    let ruby = GemMaterial::sapphire();

    // The diamond column (cubic) is unaffected by anisotropic-only changes. The ruby column
    // is really `GemMaterial::sapphire()` (uniaxial) and moves with the entry, internal and
    // exit birefringence handling.
    let cases: [GoldenCase; 4] = [
        (
            Ray {
                origin: Vec3::new(0.0, 2.5, 0.0),
                dir: Vec3::new(0.0, -1.0, 0.0),
            },
            1337,
            LightingPreset::RingLights,
            0.85,
            0.95,
            [0x3c69_75d5, 0x3c73_f69d, 0x3c8a_018e],
            // This ray hits the crown at exactly normal incidence with Sapphire's default
            // `c_axis == Vec3::Y`: the degenerate limit where the wave normal is parallel to
            // the optic axis and both eigenmodes collapse to one isotropic response at `n_o`.
            [0x3ae4_cb15, 0x3a8a_b0e1, 0x3bec_7e4b],
        ),
        (
            Ray {
                origin: Vec3::new(0.0, 2.5, 0.0),
                dir: Vec3::new(0.0, -1.0, 0.0),
            },
            42,
            LightingPreset::Incandescent,
            0.0,
            1.2,
            // The bits are for the optimised test profile: the studio sampler rounds a few ULP
            // differently from an unoptimised build (`[profile.test.package.pleochroma]`).
            [0x3ff8_597e, 0x4000_e393, 0x4010_b5b0],
            // Same degenerate normal-incidence geometry along `c_axis` as the seed=1337 case.
            // The degenerate uniaxial entry goes through the general partial-Fresnel path, as
            // on the GPU. Sapphire uses its measured extraordinary-index curve n_e(lambda).
            [0x3d99_78ab, 0x3ccd_1d47, 0x3ed6_3784],
        ),
        (
            Ray {
                origin: Vec3::new(0.3, 2.5, 0.1),
                dir: Vec3::new(-0.05, -1.0, 0.02).normalize(),
            },
            9001,
            LightingPreset::DarkSpotlight,
            2.1,
            0.4,
            // This oblique ray draws the EXTRAORDINARY eigenmode at the air-to-crystal entry and
            // reaches a real dispersive crystal-to-air exit with a live companion channel. It is
            // the case that exercises exit-event spectral splitting, unlike the other three.
            [0x3d22_0c97, 0x3d2b_789c, 0x3d45_c546],
            // A channel past its critical angle keeps its Stokes vector and path_pdf rather than
            // being scaled by the solved reflectance (1 up to rounding; `apply_tir_bounce`).
            [0x3a8a_b475, 0x39a9_05b7, 0x3bae_bfbb],
        ),
        (
            Ray {
                origin: Vec3::new(0.0, 10.0, 0.0),
                dir: Vec3::new(0.0, -1.0, 0.0),
            },
            7,
            LightingPreset::Daylight,
            std::f32::consts::PI,
            0.3,
            // The only case in this table using `LightingPreset::Daylight`, which
            // samples the tabulated CIE D65 measured spectrum rather than a smooth
            // 6500K Planckian.
            [0x3ccc_d14a, 0x3cdc_72bc, 0x3ce4_6a02],
            // Same degenerate normal-incidence geometry along `c_axis` as the seed=1337 case.
            [0x3a62_10a0, 0x3999_9429, 0x3b97_e2be],
        ),
    ];

    // Every row is traced before anything is compared, so one failing run prints the
    // whole table (all rows that drifted, not only the first).
    let mut got = Vec::with_capacity(cases.len());
    let mut want = Vec::with_capacity(cases.len());
    for (ray, seed, preset, lyaw, lpitch, expected_diamond, expected_ruby) in cases {
        let xyz_d = trace_spectral_ray(
            ray,
            &planes,
            &diamond,
            12,
            preset.studio(1.0, lyaw, lpitch),
            seed,
            (hash_u32(seed) as f32) / 4_294_967_295.0,
            None,
        );
        let xyz_r = trace_spectral_ray(
            ray,
            &planes,
            &ruby,
            12,
            preset.studio(1.0, lyaw, lpitch),
            seed,
            (hash_u32(seed) as f32) / 4_294_967_295.0,
            None,
        );
        got.push((
            seed,
            preset,
            [xyz_d.x.to_bits(), xyz_d.y.to_bits(), xyz_d.z.to_bits()],
            [xyz_r.x.to_bits(), xyz_r.y.to_bits(), xyz_r.z.to_bits()],
        ));
        want.push((seed, preset, expected_diamond, expected_ruby));
    }
    assert_eq!(
        got, want,
        "diamond / sapphire trace drifted from the pinned baseline (seed, preset, diamond \
         bits, sapphire bits); re-pin only after an intended physics change"
    );

    // sample_studio_environment direct sampling across a grid of directions.
    let env_dirs = [
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::new(0.5, 0.5, 0.5).normalize(),
        Vec3::new(-0.3, 0.2, 0.9).normalize(),
        Vec3::new(1.0, 0.0, 0.0),
    ];
    // Pinned for the optimised test profile, as the case table above.
    let env_expected: [[u32; 3]; 4] = [
        [0x3d95_c282, 0x3db0_45c3, 0x3dac_2599],
        [0x4160_feaa, 0x4184_69d1, 0x4181_5071],
        [0x3c9a_2cbb, 0x3cb5_7813, 0x3cb1_38c6],
        [0x3c91_97fc, 0x3cab_5e6d, 0x3ca7_5ba5],
    ];
    // The inputs go through `black_box` so that no build can fold this call at compile
    // time: a release build with LTO otherwise evaluates it in double precision and
    // lands hundreds of ULP from the run-time result the renderer produces.
    for (d, expected) in env_dirs.into_iter().zip(env_expected) {
        for (&lambda, &expected_bits) in [450.0f32, 550.0, 650.0].iter().zip(expected.iter()) {
            let v = sample_studio_environment(
                black_box(d),
                black_box(lambda),
                black_box(LightingPreset::RingLights),
                black_box(1.0),
                black_box(0.85),
                black_box(0.95),
            );
            assert_eq!(
                v.to_bits(),
                expected_bits,
                "sample_studio_environment drifted for dir {d:?}, lambda {lambda}"
            );
        }
    }

    // evaluate_gem_optical_metrics golden values at a face-up pose; any change to the
    // metrics moves them.
    let m = evaluate_gem_optical_metrics(
        &planes,
        &diamond,
        0.0,
        0.45,
        LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
    );
    assert_eq!(
        [
            m.brilliance_pct.to_bits(),
            m.fire_index.to_bits(),
            m.scintillation_pct.to_bits(),
            m.windowing_pct.to_bits(),
            m.extinction_pct.to_bits(),
        ],
        [
            0x418f_4899,
            0x40b5_ef5a,
            0x421f_d86a,
            0x4213_c2dd,
            0x4234_98d6
        ],
        "evaluate_gem_optical_metrics drifted from the pinned baseline; re-pin only after an intended physics change"
    );
}
