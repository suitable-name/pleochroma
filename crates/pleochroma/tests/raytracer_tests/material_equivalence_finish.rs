//! Sapphire/Ruby o-ray-only bit identity and facet-finish (bruted girdle) tests.
//!
//! An all-`Polished` finish matches `trace_spectral_ray` bit for bit. A bruted girdle still
//! conserves energy under the white furnace and changes face-up appearance measurably.

use glam::Vec3;
use pleochroma::{
    geometry::cuts::StandardGemCuts,
    optics::{
        materials::GemMaterial,
        raytracer::{
            Camera, EnvironmentSource, FacetFinish, LightingPreset, Ray, hash_u32,
            trace_spectral_ray, trace_spectral_ray_with_finish,
        },
    },
    renderer::env_map::EnvironmentMap,
};

use crate::fixtures::{bruted_girdle_finishes, furnace_mean_xyz, uniform_furnace_target};

/// Sapphire's and Ruby's face-up render does not depend on their `e_ray` absorption data.
/// With `c_axis` = `Vec3::Y` and this straight-down ray (parallel to `c_axis`), the
/// polarization quadratic form reduces to `alpha_o` at every bounce. The render must
/// therefore match bit for bit a clone with absorption `AbsorptionTensor::isotropic(o_ray)`,
/// across 500 seeds on the full Standard Round Brilliant cut.
#[test]
fn sapphire_and_ruby_face_up_render_is_bit_identical_to_o_ray_only_material() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let ray = Ray {
        origin: Vec3::new(0.0, 2.5, 0.0),
        dir: Vec3::new(0.0, -1.0, 0.0),
    };

    for name in ["Sapphire", "Ruby"] {
        let real = GemMaterial::by_name(name)
            .unwrap_or_else(|| panic!("{name} must be a built-in material"));
        let mut o_ray_only = real.clone();
        o_ray_only.absorption = pleochroma::optics::absorption::AbsorptionTensor::isotropic(
            real.absorption.o_ray.clone(),
        );

        for seed in 0u32..500 {
            let xyz_real = trace_spectral_ray(
                ray,
                &planes,
                &real,
                12,
                LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
                seed,
                (hash_u32(seed) as f32) / 4_294_967_295.0,
                None,
            );
            let xyz_o_only = trace_spectral_ray(
                ray,
                &planes,
                &o_ray_only,
                12,
                LightingPreset::RingLights.studio(1.0, 0.85, 0.95),
                seed,
                (hash_u32(seed) as f32) / 4_294_967_295.0,
                None,
            );
            assert_eq!(
                xyz_real, xyz_o_only,
                "{name}: face-up render (seed={seed}) must be bit-identical to an o-ray-only \
                 clone -- the new e_ray data must never be consulted for this exactly-on-axis ray"
            );
        }
    }
}

// ---------------------------------------------------------------------------------
// Girdle finish (bruted/frosted facets).
// ---------------------------------------------------------------------------------

/// `trace_spectral_ray_with_finish` with an all-`Polished` `facet_finishes` slice (the same
/// length as `planes`) must match `trace_spectral_ray` bit for bit. Existing cuts keep polished
/// girdles unless a finish is set. Covers isotropic and uniaxial materials, so the
/// mode-coupling path is exercised too.
#[test]
fn frosted_finish_all_polished_is_bit_identical_to_trace_spectral_ray() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let all_polished = vec![FacetFinish::Polished; planes.len()];
    let rays = [
        Ray {
            origin: Vec3::new(0.0, 2.5, 0.0),
            dir: Vec3::new(0.0, -1.0, 0.0),
        },
        Ray {
            origin: Vec3::new(0.0, 2.5, 0.0),
            dir: Vec3::new(0.18, -1.0, 0.07).normalize(),
        },
    ];
    for name in ["Diamond", "Zircon", "Sapphire"] {
        let material = GemMaterial::by_name(name).unwrap();
        for ray in rays {
            for seed in [1u32, 42, 9001] {
                let hero_rand = (hash_u32(seed) as f32) / 4_294_967_295.0;
                let env = LightingPreset::RingLights.studio(1.0, 0.85, 0.95);
                let baseline =
                    trace_spectral_ray(ray, &planes, &material, 12, env, seed, hero_rand, None);
                let env2 = LightingPreset::RingLights.studio(1.0, 0.85, 0.95);
                let with_finish = trace_spectral_ray_with_finish(
                    ray,
                    &planes,
                    &all_polished,
                    &material,
                    12,
                    env2,
                    seed,
                    hero_rand,
                    None,
                );
                assert_eq!(
                    baseline.x.to_bits(),
                    with_finish.x.to_bits(),
                    "{name} seed={seed}: all-Polished trace_spectral_ray_with_finish must be \
                     bit-identical to trace_spectral_ray (x component)"
                );
                assert_eq!(
                    baseline.y.to_bits(),
                    with_finish.y.to_bits(),
                    "{name} seed={seed}: y component"
                );
                assert_eq!(
                    baseline.z.to_bits(),
                    with_finish.z.to_bits(),
                    "{name} seed={seed}: z component"
                );
            }
        }
    }
}

/// White-furnace energy conservation with a BRUTED girdle. A colorless, non-dispersive gem in a
/// spatially uniform environment must render at that environment's radiance, since reflectance
/// and transmittance sum to 1 at every interface. This checks that `apply_frosted_bounce`'s
/// reflect/transmit split (the same energy budget as the polished formula, sent diffusely) and
/// its cosine-weighted direction sampling (whose pdf cancels the Lambertian `albedo = 1.0`; see
/// that function's doc comment) conserve energy.
#[test]
fn frosted_girdle_white_furnace_energy_conservation_still_holds() {
    const L0: f32 = 2.5;
    const SAMPLES_PER_PIXEL: u32 = 96;
    // Measured by `furnace_noise_floor` at this budget: about 1.35 % truncation loss at cap 12
    // (paths still inside the gem at the cap are dropped; nothing is lost at cap 256) plus
    // about 0.15 % noise. A branch mis-weighted by a few percent fails this.
    const TOLERANCE: f32 = 0.025;

    let planes = StandardGemCuts::standard_round_brilliant();
    let finishes = bruted_girdle_finishes(planes.len());
    // Same construction as `renderer::gpu::estimator_check::furnace_material`, which is
    // behind the `gpu` feature and so not reused directly.
    let material = GemMaterial::new_custom("CPU furnace probe", 1.5, 0.0, 0.0, [0.0, 0.0, 0.0]);
    let env_map = EnvironmentMap::uniform(1, 1, [L0, L0, L0]);

    let (mean, count) = furnace_mean_xyz(SAMPLES_PER_PIXEL, 0x4657_5230, |ray, seed, hero| {
        trace_spectral_ray_with_finish(
            ray,
            &planes,
            &finishes,
            &material,
            12,
            EnvironmentSource::hdr(&env_map),
            seed,
            hero,
            None,
        )
    });

    // Analytic target: EnvironmentMap::uniform's spectral reconstruction of [L0,L0,L0],
    // integrated against the real CIE 1931 CMF over the visible range.
    let target = uniform_furnace_target(L0);

    let rel_err = |v: f32, t: f32| (v - t).abs() / t.abs().max(1e-6);
    let (ex, ey, ez) = (
        rel_err(mean.x, target.x),
        rel_err(mean.y, target.y),
        rel_err(mean.z, target.z),
    );
    println!(
        "[frosted-girdle furnace] mean={mean:?} target={target:?} rel_err=({ex:.4}, {ey:.4}, {ez:.4}) over {count} samples"
    );
    assert!(
        ex <= TOLERANCE && ey <= TOLERANCE && ez <= TOLERANCE,
        "frosted-girdle furnace should still converge to the uniform environment's own \
         radiance (mean={mean:?}, target={target:?}, rel_err=({ex}, {ey}, {ez}), tolerance={TOLERANCE})"
    );
}

/// A bruted girdle must change the face-up appearance, not merely run without crashing.
///
/// The effect is a redistribution of light: the girdle feeds a soft bright ring into the
/// pavilion, where a specular girdle reaches only a narrow delta direction. Total energy is
/// still conserved (see the furnace test above). Not every viewing angle gets brighter: a
/// steep near-vertical pose can darken, since a diffuse spread can starve a view that sits in
/// a specular glint's narrow cone. The standard face-up framing,
/// `renderer::gpu::estimator_check::test_camera` (`Camera::new(0.35, 0.28, 5.0, 18.0)`),
/// shows a clear increase, so it is the camera used below.
///
/// Averages luminance (Y) over a small grid of pixels and many samples per pixel, for the
/// SAME material/seeds with only the girdle's finish differing.
#[test]
fn frosted_girdle_changes_face_up_appearance_measurably() {
    const SAMPLES_PER_PIXEL: u32 = 96;
    const GRID: usize = 16;

    let planes = StandardGemCuts::standard_round_brilliant();
    let polished = vec![FacetFinish::Polished; planes.len()];
    let frosted = bruted_girdle_finishes(planes.len());
    let material = GemMaterial::by_name("Diamond").expect("Diamond must be a built-in material");
    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let env = || LightingPreset::RingLights.studio(1.0, 0.85, 0.95);

    let mut sum_polished = Vec3::ZERO;
    let mut sum_frosted = Vec3::ZERO;
    let mut count = 0u32;
    for iy in 0..GRID {
        for ix in 0..GRID {
            let ray = camera.generate_ray(ix as f32, iy as f32, GRID as f32, GRID as f32, 0.5, 0.5);
            for s in 0..SAMPLES_PER_PIXEL {
                let pixel_id = u32::try_from(iy * GRID + ix).expect("a pixel id fits in u32");
                let seed = hash_u32(pixel_id ^ hash_u32(s ^ 0x9A7E_C6F1));
                let hero_rand = (hash_u32(seed) as f32) / 4_294_967_295.0;
                sum_polished += trace_spectral_ray_with_finish(
                    ray,
                    &planes,
                    &polished,
                    &material,
                    12,
                    env(),
                    seed,
                    hero_rand,
                    None,
                );
                sum_frosted += trace_spectral_ray_with_finish(
                    ray,
                    &planes,
                    &frosted,
                    &material,
                    12,
                    env(),
                    seed,
                    hero_rand,
                    None,
                );
                count += 1;
            }
        }
    }
    let mean_polished = sum_polished / count as f32;
    let mean_frosted = sum_frosted / count as f32;
    let delta_y = mean_frosted.y - mean_polished.y;
    println!(
        "[frosted-girdle face-up] polished Y={:.5} frosted Y={:.5} delta_y={:.5} ({:.2}%) over {count} samples",
        mean_polished.y,
        mean_frosted.y,
        delta_y,
        100.0 * delta_y / mean_polished.y.max(1e-6)
    );

    assert!(
        delta_y > 0.0,
        "a bruted girdle should make face-up brightness (Y) INCREASE, not decrease or stay \
         flat -- extra diffuse light-gathering paths through the girdle should add energy, \
         not remove it (polished Y={:.5}, frosted Y={:.5})",
        mean_polished.y,
        mean_frosted.y
    );
    let relative_change = delta_y.abs() / mean_polished.y.max(1e-6);
    assert!(
        relative_change > 0.01,
        "the brightness change from a frosted girdle should be clearly measurable (>1%), \
         not noise-level -- got {:.4}% (polished Y={:.5}, frosted Y={:.5})",
        100.0 * relative_change,
        mean_polished.y,
        mean_frosted.y
    );
}
