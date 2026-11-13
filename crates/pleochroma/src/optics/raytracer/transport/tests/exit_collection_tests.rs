//! The exit-collection estimator ([`crate::optics::raytracer::Estimator::ExitCollection`]).
//!
//! - `exit_collection_keeps_the_mean_within_a_percent`: collected against branching, on the same
//!   rays, for five materials at 8 and 32 bounces.
//! - `exit_collection_white_furnace_returns_the_environment`: a colorless stone in a uniform
//!   environment returns that environment, which proves the spectral-MIS weight of a leaf.
//! - `branching_estimator_is_bit_identical_to_the_unprepared_entry`: the switch off changes
//!   nothing, and the prepared entry is wired to the switch.
//! - `a_stone_with_tools_declines_exit_collection_bit_for_bit`: a stone with tools runs the
//!   branching estimator on every engine, so asking the processor for collection changes no bit.
//! - `the_cpu_and_the_gpu_decline_the_same_scenes` (the `gpu` feature): the processor and
//!   `GpuTransportParams::with_estimator` agree on which scenes collect.
//! - `exit_collection_go_no_go_table`: the variance and efficiency table, with a row per
//!   estimator (ignored: run with `--ignored --nocapture`).

use super::super::{
    super::{
        camera::{Camera, DEFAULT_FOV_DEG, Ray},
        environment::EnvironmentSource,
        estimator::Estimator,
        intersect::{build_plane_soa, intersect_polyhedron},
        sampling::{PixelRotations, SampleDraws, hash_u32, pixel_rotations, sample_draws},
        surface::{FacetSurface, SurfaceScene},
    },
    PathTermination, PreparedMaterial,
    inner::trace_spectral_ray_inner_geom_events_surface_prepared,
    trace_spectral_ray_prepared, trace_spectral_ray_with_finish_soa_geom,
};
use crate::{
    geometry::{
        cuts::StandardGemCuts,
        plane::GpuFacetPlane,
        tool::{StoneGeometry, ToolPrimitive},
    },
    optics::{LightingPreset, fluorescence::Fluorescence, materials::GemMaterial},
    renderer::env_map::EnvironmentMap,
};
use glam::Vec3;
use std::time::Instant;

/// A stone, its environment and the bounce cap of one test scene.
struct Stone<'a> {
    planes: &'a [GpuFacetPlane],
    plane_soa: &'a crate::simd::PlanesSoA32,
    material: &'a GemMaterial,
    environment: EnvironmentSource<'a>,
    max_bounces: u32,
}

/// What a traced sample reports besides its XYZ: the loop iterations it ran.
struct Traced {
    xyz: Vec3,
    segments: u32,
}

/// Traces one sample through the shared bounce loop with an explicit estimator request.
///
/// `nee` is passed explicitly (an HDR map would switch it on by itself), `collect` asks for exit
/// collection. The iteration count is the termination bounce plus one, or the cap on a `HitCap`.
fn trace(stone: &Stone<'_>, ray: Ray, ids: (u32, f32), nee: bool, collect: bool) -> Traced {
    let (seed, hero_rand) = ids;
    let mut termination = (stone.max_bounces, PathTermination::HitCap);
    let xyz = trace_spectral_ray_inner_geom_events_surface_prepared(
        ray,
        stone.planes,
        &[],
        stone.plane_soa,
        &[],
        stone.material,
        Fluorescence::none(),
        stone.max_bounces,
        stone.environment,
        seed,
        hero_rand,
        None,
        true,
        true,
        nee,
        (
            (Some(&mut termination), None),
            SurfaceScene::IDEAL,
            None,
            None,
            collect,
        ),
    );
    let segments = match termination.1 {
        PathTermination::HitCap => termination.0,
        _ => termination.0 + 1,
    };
    Traced { xyz, segments }
}

/// The seed and hero draw of sample `i` of a salted stream.
fn hashed_ids(salt: u32, i: u32) -> (u32, f32) {
    let seed = hash_u32(salt ^ hash_u32(i));
    (seed, (hash_u32(seed) as f32) / 4_294_967_295.0)
}

/// Mean and standard error of the mean of `v`.
fn mean_and_se(v: &[f64]) -> (f64, f64) {
    let n = v.len() as f64;
    let mean = v.iter().sum::<f64>() / n;
    let var = v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / (n - 1.0);
    (mean, (var / n).sqrt())
}

/// One XYZ component of `samples`, widened to `f64`.
fn component(samples: &[Vec3], axis: usize) -> Vec<f64> {
    samples.iter().map(|xyz| f64::from(xyz[axis])).collect()
}

/// Collected against branching, on the same rays: the XYZ means must agree within one percent plus
/// three and a half standard errors of their difference (the tolerance of the exit-splitting test:
/// both estimators ride a companion on the hero's path while its direction is within
/// `DIRECTION_MATCH_COS_TOL`, and they lean on that approximation differently).
///
/// Five materials, a studio rig (ring lights, so no next-event estimation), 8 and 32 bounces. The
/// two estimators truncate alike (an event earns its leaf only if the branching estimator could
/// have seen its escape within the cap), so the 8-bounce means must agree too.
#[test]
fn exit_collection_keeps_the_mean_within_a_percent() {
    const BRANCHING_SAMPLES: u32 = 12_000;
    const COLLECTED_SAMPLES: u32 = 3_000;
    let planes = StandardGemCuts::standard_round_brilliant();
    let plane_soa = build_plane_soa(&planes);
    let environment = LightingPreset::RingLights.studio(1.0, 0.85, 0.95);
    let rays = [
        Ray {
            origin: Vec3::new(0.0, 2.5, 0.0),
            dir: Vec3::new(0.18, -1.0, 0.07).normalize(),
        },
        Ray {
            origin: Vec3::new(0.3, 2.5, -0.2),
            dir: Vec3::new(-0.1, -1.0, 0.2).normalize(),
        },
    ];
    for name in [
        "Diamond",
        "Synthetic Moissanite",
        "Zircon",
        "Sapphire",
        "Rutile",
    ] {
        let material = GemMaterial::by_name(name)
            .unwrap_or_else(|| panic!("{name} must be a built-in material"));
        for max_bounces in [8u32, 32] {
            let stone = Stone {
                planes: &planes,
                plane_soa: &plane_soa,
                material: &material,
                environment,
                max_bounces,
            };
            for (ray_idx, &ray) in (0u32..).zip(rays.iter()) {
                let salt = 0xEC00 + 17 * ray_idx + max_bounces;
                let branching: Vec<Vec3> = (0..BRANCHING_SAMPLES)
                    .map(|i| trace(&stone, ray, hashed_ids(salt, i), false, false).xyz)
                    .collect();
                let collected: Vec<Vec3> = (0..COLLECTED_SAMPLES)
                    .map(|i| trace(&stone, ray, hashed_ids(salt ^ 0x5555, i), false, true).xyz)
                    .collect();
                for (axis, tag) in ["X", "Y", "Z"].iter().enumerate() {
                    let (mean_b, se_b) = mean_and_se(&component(&branching, axis));
                    let (mean_c, se_c) = mean_and_se(&component(&collected, axis));
                    let se = se_b.hypot(se_c);
                    println!(
                        "exit collection -- {name}, {max_bounces} bounces, ray {ray_idx}, {tag}: \
                         branching {mean_b:.5}, collected {mean_c:.5}, diff {:+.3e} (se {se:.1e})",
                        mean_c - mean_b
                    );
                    assert!(
                        (mean_c - mean_b).abs() <= 3.5f64.mul_add(se, 0.01 * mean_b.abs()),
                        "{name}, {max_bounces} bounces, ray {ray_idx}: the collected {tag} mean \
                         {mean_c:.5} differs from the branching mean {mean_b:.5} by more than one \
                         percent plus 3.5 standard errors ({se:.1e})"
                    );
                }
            }
        }
    }
}

/// The analytic XYZ of a uniform environment of radiance `l0`: the CMF-weighted spectral
/// reconstruction over 380..=780 nm, normalized by the CIE 1931 Y integral.
fn uniform_target(l0: f32) -> Vec3 {
    let mut target = Vec3::ZERO;
    for step in 0..=(780 - 380) {
        let lambda = 380.0f32 + step as f32;
        let spec = crate::renderer::env_map::rgb_to_spectral_radiance([l0, l0, l0], lambda);
        target += crate::optics::raytracer::cie_1931_cmf(lambda) * spec;
    }
    target / crate::color::cie1931::CIE_1931_Y_INTEGRAL_5NM
}

/// A colorless stone in a uniform environment returns that environment.
///
/// With every weight one the collected estimator is exact only if the leaf weights sum to one over
/// each channel's family: a wrong family count, a missed total-reflection exclusion or a leaf that
/// is added twice shows up here as a brightness error, not as noise. Three stones: non-dispersive,
/// dispersive (the families split at the entry) and dispersive plus uniaxial (the closed-form
/// entry and internal solves). The roulette's floor is unbiased, and at 64 bounces the truncation
/// loss is gone, so the tolerance is two percent plus four standard errors.
#[test]
fn exit_collection_white_furnace_returns_the_environment() {
    const L0: f32 = 2.5;
    const GRID: usize = 12;
    const SAMPLES_PER_PIXEL: u32 = 32;
    let planes = StandardGemCuts::standard_round_brilliant();
    let plane_soa = build_plane_soa(&planes);
    let env_map = crate::renderer::env_map::EnvironmentMap::uniform(1, 1, [L0, L0, L0]);
    let environment = EnvironmentSource::hdr(&env_map);
    let target = uniform_target(L0);
    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    for (label, dispersion, birefringence) in [
        ("non-dispersive", 0.0f32, 0.0f32),
        ("dispersive", 0.05, 0.0),
        ("dispersive and uniaxial", 0.05, 0.03),
    ] {
        let material = GemMaterial::new_custom(
            "EC furnace probe",
            1.76,
            dispersion,
            birefringence,
            [0.0; 3],
        );
        let stone = Stone {
            planes: &planes,
            plane_soa: &plane_soa,
            material: &material,
            environment,
            max_bounces: 64,
        };
        let mut samples = Vec::new();
        for iy in 0..GRID {
            for ix in 0..GRID {
                let ray =
                    camera.generate_ray(ix as f32, iy as f32, GRID as f32, GRID as f32, 0.5, 0.5);
                for s in 0..SAMPLES_PER_PIXEL {
                    let pixel =
                        u32::try_from(iy * GRID + ix).expect("the grid has fewer than 2^32 pixels");
                    let ids = hashed_ids(0xF0A4_0000 ^ pixel, s);
                    // The map would switch next-event estimation on by itself; it stays off here.
                    samples.push(trace(&stone, ray, ids, false, true).xyz);
                }
            }
        }
        for (axis, tag) in ["X", "Y", "Z"].iter().enumerate() {
            let (mean, se) = mean_and_se(&component(&samples, axis));
            let truth = f64::from(target[axis]);
            println!(
                "exit collection furnace -- {label}, {tag}: mean {mean:.5}, target {truth:.5} \
                 (se {se:.1e})"
            );
            assert!(
                (mean - truth).abs() <= 4.0f64.mul_add(se, 0.02 * truth.abs()),
                "{label}: the collected {tag} mean {mean:.5} is not the environment {truth:.5} \
                 (standard error {se:.1e}): the leaf weights do not sum to one"
            );
        }
    }
}

/// `Branching` is the trace bit for bit, and the prepared entry is wired to the switch.
///
/// For an isotropic stone (Diamond) and a uniaxial one (Zircon) under a studio rig: the prepared
/// entry with `Estimator::Branching` equals the entry that takes no prepared material, on every
/// ray. With `Estimator::ExitCollection` it equals the shared bounce loop asked for collection
/// (so the switch reaches the loop), and it differs from branching on at least one ray (so the
/// request is not ignored).
#[test]
fn branching_estimator_is_bit_identical_to_the_unprepared_entry() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let plane_soa = build_plane_soa(&planes);
    let environment = LightingPreset::RingLights.studio(1.0, 0.85, 0.95);
    let geom = StoneGeometry {
        planes: &planes,
        tools: &[],
    };
    let bits = |v: Vec3| v.to_array().map(f32::to_bits);
    for name in ["Diamond", "Zircon"] {
        let material = GemMaterial::by_name(name)
            .unwrap_or_else(|| panic!("{name} must be a built-in material"));
        let branching = PreparedMaterial::new(&material).with_estimator(Estimator::Branching);
        let collecting = PreparedMaterial::new(&material).with_estimator(Estimator::ExitCollection);
        assert_eq!(branching.estimator(), Estimator::Branching);
        assert_eq!(collecting.estimator(), Estimator::ExitCollection);
        let stone = Stone {
            planes: &planes,
            plane_soa: &plane_soa,
            material: &material,
            environment,
            max_bounces: 12,
        };
        let mut collection_changed_a_ray = false;
        for i in 0..24u32 {
            let ids = hashed_ids(0x000B_171D, i);
            let ray = Ray {
                origin: Vec3::new(0.02f32.mul_add(i as f32, -0.2), 2.5, 0.01 * i as f32),
                dir: Vec3::new(0.01f32.mul_add(-(i as f32), 0.18), -1.0, 0.07).normalize(),
            };
            let prepared = |p: &PreparedMaterial<'_>| {
                trace_spectral_ray_prepared(
                    ray,
                    geom,
                    &plane_soa,
                    &[],
                    p,
                    Fluorescence::none(),
                    12,
                    environment,
                    ids.0,
                    ids.1,
                    None,
                    SurfaceScene::IDEAL,
                )
            };
            let unprepared = trace_spectral_ray_with_finish_soa_geom(
                ray,
                geom,
                &plane_soa,
                &[],
                &material,
                Fluorescence::none(),
                12,
                environment,
                ids.0,
                ids.1,
                None,
            );
            let off = prepared(&branching);
            assert_eq!(
                bits(off),
                bits(unprepared),
                "{name}, ray {i}: the branching estimator moved a bit"
            );
            let on = prepared(&collecting);
            assert_eq!(
                bits(on),
                bits(trace(&stone, ray, ids, false, true).xyz),
                "{name}, ray {i}: the prepared entry did not reach the collecting loop"
            );
            collection_changed_a_ray |= bits(on) != bits(off);
        }
        assert!(
            collection_changed_a_ray,
            "{name}: asking for exit collection changed no ray: the switch is ignored"
        );
    }
}

/// One class of test scene: a brilliant with or without a dimple cut into its table, with an ideal
/// or a rough facet, lit by a studio rig or an HDR map.
#[derive(Clone, Copy, Debug)]
struct SceneClass {
    with_tools: bool,
    rough: bool,
    hdr: bool,
}

#[cfg(feature = "gpu")]
impl SceneClass {
    /// The eight classes.
    fn all() -> impl Iterator<Item = Self> {
        (0..8u8).map(|n| Self {
            with_tools: n & 1 != 0,
            rough: n & 2 != 0,
            hdr: n & 4 != 0,
        })
    }
}

/// A small two-tone sky: bright above, dim and tinted below.
fn two_tone_sky() -> EnvironmentMap {
    let pixels = (0..8)
        .map(|n| {
            if n < 4 {
                [0.9, 1.0, 1.2]
            } else {
                [0.12, 0.08, 0.05]
            }
        })
        .collect();
    EnvironmentMap::from_rgb(4, 2, pixels).expect("self-consistent by construction")
}

/// Ray `i` of the decision tests: oblique rays across the table and the dimple.
fn table_ray(i: u32) -> Ray {
    Ray {
        origin: Vec3::new(
            0.02f32.mul_add(i as f32, -0.1),
            2.5,
            0.01f32.mul_add(i as f32, 0.05),
        ),
        dir: Vec3::new(0.01f32.mul_add(-(i as f32), 0.18), -1.0, 0.07).normalize(),
    }
}

/// The surface scene of `class`: the one `ground` facet when it is rough, else ideal.
fn class_surface(class: SceneClass, ground: &[FacetSurface]) -> SurfaceScene<'_> {
    SurfaceScene {
        facet_surfaces: if class.rough { ground } else { &[] },
        ..SurfaceScene::IDEAL
    }
}

/// The bits of 32 prepared traces of a Diamond brilliant of `class`, `estimator` requested.
fn class_traces(class: SceneClass, estimator: Estimator) -> Vec<[u32; 3]> {
    let planes = StandardGemCuts::standard_round_brilliant();
    let plane_soa = build_plane_soa(&planes);
    let dimple = [ToolPrimitive::ball(Vec3::new(0.25, 0.32, 0.1), 0.22)];
    let tools: &[ToolPrimitive] = if class.with_tools { &dimple } else { &[] };
    let geom = StoneGeometry {
        planes: &planes,
        tools,
    };
    let material = GemMaterial::by_name("Diamond").expect("Diamond is a built-in material");
    let prepared = PreparedMaterial::new(&material).with_estimator(estimator);
    let sky = two_tone_sky();
    let environment = if class.hdr {
        EnvironmentSource::hdr_at(&sky, 1.0)
    } else {
        LightingPreset::RingLights.studio(1.0, 0.85, 0.95)
    };
    let ground = [FacetSurface::ground(0.3, 1.0)];
    let surface = class_surface(class, &ground);
    (0..32u32)
        .map(|i| {
            let ids = hashed_ids(0x00C0_11EC, i);
            trace_spectral_ray_prepared(
                table_ray(i),
                geom,
                &plane_soa,
                &[],
                &prepared,
                Fluorescence::none(),
                12,
                environment,
                ids.0,
                ids.1,
                None,
                surface,
            )
            .to_array()
            .map(f32::to_bits)
        })
        .collect()
}

/// A stone with tools runs the branching estimator on every engine: asking the processor tracer
/// for exit collection changes no bit of any trace of it, as for every other declined scene. The
/// tools do change the picture, so the premise (the dimple is traced) holds.
#[test]
fn a_stone_with_tools_declines_exit_collection_bit_for_bit() {
    let concave = SceneClass {
        with_tools: true,
        rough: false,
        hdr: false,
    };
    assert_eq!(
        class_traces(concave, Estimator::ExitCollection),
        class_traces(concave, Estimator::Branching),
        "the processor tracer collected over a stone with tools"
    );
    let convex = SceneClass {
        with_tools: false,
        ..concave
    };
    assert_ne!(
        class_traces(concave, Estimator::Branching),
        class_traces(convex, Estimator::Branching),
        "the dimple changed no trace"
    );
}

/// The processor tracer and the GPU's `with_estimator` collect for exactly the same scenes: the
/// classes {convex, with tools} x {ideal surface, rough} x {studio rig, HDR map}. Only the convex,
/// ideal, studio-rig scene collects, so a hybrid frame never mixes the two estimators.
#[cfg(feature = "gpu")]
#[test]
fn the_cpu_and_the_gpu_decline_the_same_scenes() {
    use crate::renderer::buffers::{GpuSurfaceScene, GpuTransportParams, transport_env_mode};
    let ground = [FacetSurface::ground(0.3, 1.0)];
    for class in SceneClass::all() {
        let ideal_surface =
            GpuSurfaceScene::from_scene(&class_surface(class, &ground)).is_ideal_for_cpu();
        let env_mode = if class.hdr {
            transport_env_mode::HDR_MAP
        } else {
            transport_env_mode::STUDIO_RIG
        };
        let gpu =
            GpuTransportParams::new(1, 8, 0, env_mode, 0.0, 6500.0, 1.0, 1.0, 0.0, 0.0, [1.0; 3])
                .with_tool_count(u32::from(class.with_tools))
                .with_estimator(Estimator::ExitCollection, ideal_surface);
        let gpu_collects = gpu.estimator() == Estimator::ExitCollection;
        let cpu_collects = class_traces(class, Estimator::ExitCollection)
            != class_traces(class, Estimator::Branching);
        assert_eq!(
            cpu_collects, gpu_collects,
            "{class:?}: the engines disagree"
        );
        assert_eq!(
            cpu_collects,
            !class.with_tools && !class.rough && !class.hdr,
            "{class:?}: only the convex, ideal, studio-rig scene collects"
        );
    }
}

/// One estimator configuration of the go / no-go table.
///
/// `draws` is the sampler of the pixel jitter, the hero wavelength and the per-bounce seed; the
/// production sampler is [`sample_draws`]. Another sampler is another configuration with its own
/// `draws`.
struct TableConfig {
    label: &'static str,
    collect: bool,
    draws: fn(u32, u32, &PixelRotations) -> SampleDraws,
}

/// The size of the table: how many pixels, how many runs per pixel, and the sample counts `N`.
struct TableSize<'a> {
    pixels: usize,
    runs: u32,
    counts: &'a [u32],
}

/// Samples per simulated GPU warp: the megakernel traces consecutive samples of a pixel in
/// neighbouring invocations, and a warp of 32 runs as long as its longest path.
const WARP: u32 = 32;

/// One cell of the table: one configuration at one `N`.
#[derive(Clone, Debug)]
struct TableCell {
    /// `N` times the variance (over the runs) of the pixel mean of Y, averaged over the pixels.
    var_x_n: f64,
    /// The same per pixel, in pixel order (for the median of the per-pixel ratios).
    pixel_var_x_n: Vec<f64>,
    /// Mean loop iterations per sample.
    segments: f64,
    /// Mean, over groups of [`WARP`] consecutive samples, of the longest path of the group in
    /// loop iterations; NaN when `N` is below [`WARP`].
    warp_max: f64,
    /// Mean trace time per sample, in microseconds.
    micros: f64,
}

/// The stone pixels of the s1 benchmark scene: the camera of `s1_glints.job.json` (top view, roll
/// 18 degrees, distance 3.6257, 512 px) over a deterministic grid, background skipped, `count` of
/// them evenly spread through the hits. Each is `(pixel id, x, y)`.
fn stone_pixels(planes: &[GpuFacetPlane], count: usize) -> (Camera, Vec<(u32, f32, f32)>) {
    const SIZE: f32 = 512.0;
    let camera = Camera::new(
        0.0,
        std::f32::consts::FRAC_PI_2,
        3.625_719_5,
        DEFAULT_FOV_DEG,
    )
    .with_roll(0.314_159_27);
    let mut hits = Vec::new();
    for py in (4..512).step_by(8) {
        for px in (4..512).step_by(8) {
            let ray = camera.generate_ray(px as f32 + 0.5, py as f32 + 0.5, SIZE, SIZE, 0.0, 0.0);
            if intersect_polyhedron(ray, planes).is_some() {
                hits.push((py * 512 + px, px as f32, py as f32));
            }
        }
    }
    assert!(hits.len() >= count, "the grid found too few stone pixels");
    let picked = (0..count).map(|i| hits[i * hits.len() / count]).collect();
    (camera, picked)
}

/// The pixel ids of the 512 px table frame end below this. Run `r` of a table pixel draws with
/// the id `pixel + r * RUN_PIXEL_STRIDE`, an id no frame pixel and no other run has.
const RUN_PIXEL_STRIDE: u32 = 512 * 512;

/// Builds the table: for every configuration and every `N`, `var x N` of Y, the mean iterations
/// and the mean time per sample.
///
/// Each run of a pixel traces the samples `0..max(counts)` of a pixel id of its own
/// (`pixel + r * RUN_PIXEL_STRIDE`) through the pixel's place in the picture, so the runs
/// are independent randomisations of the production sampler, and the mean after the first `N`
/// samples is the run's estimate at `N`: exactly what a render at `N` samples computes. The scene
/// is the s1 benchmark camera and light pose with a standard round brilliant of Synthetic
/// Moissanite under Dark Spotlight, 32 bounces. A cell also holds the per-pixel variances and the
/// mean longest path of each group of 32 consecutive samples. Returns `cells[config][count]`.
fn go_no_go_table(configs: &[TableConfig], size: &TableSize<'_>) -> Vec<Vec<TableCell>> {
    let planes = StandardGemCuts::standard_round_brilliant();
    let plane_soa = build_plane_soa(&planes);
    let material = GemMaterial::by_name("Synthetic Moissanite")
        .expect("Synthetic Moissanite must be a built-in material");
    let stone = Stone {
        planes: &planes,
        plane_soa: &plane_soa,
        material: &material,
        environment: LightingPreset::DarkSpotlight.studio(1.0, 0.85, 0.95),
        max_bounces: 32,
    };
    let (camera, pixels) = stone_pixels(&planes, size.pixels);
    let n_max = *size.counts.iter().max().expect("at least one count");
    let mut cells = Vec::new();
    for config in configs {
        // [count][pixel * runs + run] mean Y; and the totals over every sample of the cell.
        let mut means = vec![Vec::new(); size.counts.len()];
        let mut segments = vec![0.0f64; size.counts.len()];
        let mut micros = vec![0.0f64; size.counts.len()];
        let mut warps = vec![0.0f64; size.counts.len()];
        for &(pixel, x, y) in &pixels {
            for run in 0..size.runs {
                // A run is the samples `0..n_max` of a pixel id of its own at this place of the
                // picture: its own Cranley-Patterson rotations and seeds, as
                // another pixel of a render would have. Consecutive stretches of ONE pixel's
                // sequence are not independent in the base-2 jitter (stretch `r * n_max ..`
                // repeats the jitter of stretch 0 within `r * 2^-15` of a pixel).
                let run_pixel = pixel + run * RUN_PIXEL_STRIDE;
                let rot = pixel_rotations(run_pixel);
                let mut sum_y = 0.0f64;
                let mut run_segments = 0.0f64;
                let mut run_micros = 0.0f64;
                let mut group_max = 0u32;
                let mut warp_sum = 0.0f64;
                for i in 0..n_max {
                    let draws = (config.draws)(run_pixel, i, &rot);
                    let ray = camera.generate_ray(
                        x + 0.5,
                        y + 0.5,
                        512.0,
                        512.0,
                        draws.jitter_x,
                        draws.jitter_y,
                    );
                    let started = Instant::now();
                    let traced = trace(
                        &stone,
                        ray,
                        (draws.seed, draws.hero_rand),
                        false,
                        config.collect,
                    );
                    run_micros = started.elapsed().as_secs_f64().mul_add(1e6, run_micros);
                    run_segments += f64::from(traced.segments);
                    sum_y += f64::from(traced.xyz.y);
                    group_max = group_max.max(traced.segments);
                    if (i + 1).is_multiple_of(WARP) {
                        warp_sum += f64::from(group_max);
                        group_max = 0;
                    }
                    for (c, &count) in size.counts.iter().enumerate() {
                        if i + 1 == count {
                            means[c].push(sum_y / f64::from(count));
                            segments[c] += run_segments;
                            micros[c] += run_micros;
                            warps[c] += warp_sum;
                        }
                    }
                }
            }
        }
        let runs_per_cell = (pixels.len() as f64) * f64::from(size.runs);
        let row = size
            .counts
            .iter()
            .enumerate()
            .map(|(c, &count)| {
                let pixel_var_x_n: Vec<f64> = group_variances(&means[c], size.runs as usize)
                    .into_iter()
                    .map(|variance| variance * f64::from(count))
                    .collect();
                TableCell {
                    var_x_n: pixel_var_x_n.iter().sum::<f64>() / pixel_var_x_n.len() as f64,
                    pixel_var_x_n,
                    segments: segments[c] / (runs_per_cell * f64::from(count)),
                    warp_max: warps[c] / (runs_per_cell * f64::from(count / WARP)),
                    micros: micros[c] / (runs_per_cell * f64::from(count)),
                }
            })
            .collect();
        cells.push(row);
    }
    cells
}

/// The sample variance of each group of `runs` consecutive means (one group per pixel).
fn group_variances(means: &[f64], runs: usize) -> Vec<f64> {
    means
        .chunks(runs)
        .map(|group| {
            let mean = group.iter().sum::<f64>() / group.len() as f64;
            group.iter().map(|m| (m - mean).powi(2)).sum::<f64>() / (group.len() as f64 - 1.0)
        })
        .collect()
}

/// The median over the pixels of `a / b`, pixel by pixel (a pixel whose `b` is zero is left
/// out; NaN when none is left).
fn median_ratio(a: &[f64], b: &[f64]) -> f64 {
    let mut ratios: Vec<f64> = a
        .iter()
        .zip(b)
        .filter(|&(_, &denominator)| denominator > 0.0)
        .map(|(numerator, denominator)| numerator / denominator)
        .collect();
    if ratios.is_empty() {
        return f64::NAN;
    }
    ratios.sort_by(f64::total_cmp);
    ratios[ratios.len() / 2]
}

/// Megakernel cost model: a sample costs `GPU_FIXED_NS` plus `GPU_NS_PER_WARP_ITERATION` for
/// every loop iteration its warp runs. The constants are fitted to a measured bounce sweep of
/// Synthetic Moissanite in the s1 scene on an A100 (27.6, 43.0, 58.1, 69.1 and 72.6 ns per
/// sample at 8, 16, 32, 64 and 128 bounces). The s1 branching warps run about 24 of 32
/// iterations, while the standard round brilliant of this table runs shorter warps, so the
/// modelled ratio here is specific to this stone.
const GPU_FIXED_NS: f64 = 12.0;
/// See [`GPU_FIXED_NS`].
const GPU_NS_PER_WARP_ITERATION: f64 = 1.94;

/// The modelled megakernel time per sample of `cell`, in nanoseconds, with every iteration
/// `iteration_cost` times as heavy as a branching one.
fn modelled_gpu_ns(cell: &TableCell, iteration_cost: f64) -> f64 {
    (GPU_NS_PER_WARP_ITERATION * iteration_cost).mul_add(cell.warp_max, GPU_FIXED_NS)
}

/// What `other` buys against `base` at one `N`: the variance ratio, the CPU time ratio and the
/// modelled megakernel time ratio (the CPU's cost per iteration, never below one, times the
/// longest path of a warp).
fn against(base: &TableCell, other: &TableCell) -> (f64, f64, f64) {
    let iteration_cost = ((other.micros / other.segments) / (base.micros / base.segments)).max(1.0);
    (
        base.var_x_n / other.var_x_n,
        other.micros / base.micros,
        modelled_gpu_ns(other, iteration_cost) / modelled_gpu_ns(base, 1.0),
    )
}

/// The verdict of the table: `GO` when the modelled megakernel efficiency (variance ratio over
/// time ratio) of the second configuration against the first at the largest `N` is at least 3,
/// `STOP` below 1.5, `UNCLEAR` otherwise (also when it is not a number).
fn verdict(gpu_efficiency: f64) -> &'static str {
    if gpu_efficiency >= 3.0 {
        "GO"
    } else if gpu_efficiency < 1.5 {
        "STOP"
    } else {
        "UNCLEAR"
    }
}

/// Prints the table, the efficiency lines and the verdict line. The first configuration is the
/// baseline of the ratio column; the verdict compares the second configuration (exit collection)
/// with the first (branching) at the largest `N`.
fn print_table(configs: &[TableConfig], size: &TableSize<'_>, cells: &[Vec<TableCell>]) {
    println!("exit collection go / no-go table (Synthetic Moissanite, Dark Spotlight, s1 camera)");
    for (c, &count) in size.counts.iter().enumerate() {
        println!("N = {count}");
        for (config, row) in configs.iter().zip(cells) {
            let cell = &row[c];
            println!(
                "  {:<18} var x N of Y {:>11.4e}   ratio vs {} {:>6.1} (median pixel {:>6.1})   \
                 iterations/sample {:>6.2}   longest of {WARP} {:>6.2}   time/sample {:>8.1} us",
                config.label,
                cell.var_x_n,
                configs[0].label,
                cells[0][c].var_x_n / cell.var_x_n,
                median_ratio(&cells[0][c].pixel_var_x_n, &cell.pixel_var_x_n),
                cell.segments,
                cell.warp_max,
                cell.micros,
            );
        }
    }
    if cells.len() < 2 {
        return;
    }
    println!(
        "efficiency = variance ratio / time ratio (above 1: the first needs less time for the \
         same noise); the GPU time is modelled"
    );
    for (c, &count) in size.counts.iter().enumerate() {
        let (variance, cpu_time, gpu_time) = against(&cells[0][c], &cells[1][c]);
        println!(
            "  N = {count:>6}  {} against {}: variance ratio {variance:>6.2}, CPU time ratio \
             {cpu_time:>5.2}, CPU efficiency {:>6.2}, GPU time ratio {gpu_time:>5.2}, GPU \
             efficiency {:>6.2}",
            configs[1].label,
            configs[0].label,
            variance / cpu_time,
            variance / gpu_time,
        );
    }
    let Some(last) = size.counts.len().checked_sub(1) else {
        return;
    };
    let (variance, _, gpu_time) = against(&cells[0][last], &cells[1][last]);
    let gpu_efficiency = variance / gpu_time;
    println!(
        "verdict: {} (modelled GPU efficiency of {} against {} at N = {} is {gpu_efficiency:.2}; \
         GO >= 3, STOP < 1.5)",
        verdict(gpu_efficiency),
        configs[1].label,
        configs[0].label,
        size.counts[last],
    );
}

/// The two configurations of the table: branching and exit collection, both with the hashed
/// discrete draws and the production sampler.
///
/// The first is the baseline of the ratio column; the pair is the go / no-go pair that
/// [`print_table`] judges with [`verdict`].
fn production_configs() -> [TableConfig; 2] {
    [
        TableConfig {
            label: "Branching+Hash",
            collect: false,
            draws: sample_draws,
        },
        TableConfig {
            label: "Collect+Hash",
            collect: true,
            draws: sample_draws,
        },
    ]
}

/// The table plumbing at a tiny size: finite positive cells for every configuration. Runs in a
/// default test pass; the real table is [`exit_collection_go_no_go_table`].
#[test]
fn exit_collection_table_smoke() {
    let configs = production_configs();
    let size = TableSize {
        pixels: 2,
        runs: 2,
        counts: &[4, 8],
    };
    let cells = go_no_go_table(&configs, &size);
    assert_eq!(cells.len(), 2);
    for row in &cells {
        assert_eq!(row.len(), 2);
        for cell in row {
            assert!(
                cell.var_x_n.is_finite() && cell.var_x_n >= 0.0,
                "variance cell {cell:?} is not a finite non-negative number"
            );
            assert_eq!(cell.pixel_var_x_n.len(), 2, "one variance per pixel");
            assert!(cell.segments >= 1.0 && cell.micros > 0.0, "cell {cell:?}");
        }
    }
    // The printer must not panic on a table too small for a warp (its longest-path cells are NaN).
    print_table(&configs, &size, &cells);
    assert_eq!(verdict(f64::NAN), "UNCLEAR");
    assert_eq!(verdict(3.0), "GO");
    assert_eq!(verdict(1.49), "STOP");
}

/// The go / no-go table of the exit-collection estimator.
///
/// 32 fixed stone pixels of the s1 scene (a standard round brilliant of Synthetic Moissanite under
/// Dark Spotlight, the s1 camera and light pose), 8 independent runs of 16384 samples per
/// pixel, for branching and exit collection, at `N` = 64, 256, 1024, 4096 and 16384. Prints
/// `var x N` of Y, the ratio against the first row, the mean loop iterations, the longest path of
/// each group of 32 samples and the mean time per sample, then the efficiency lines (variance
/// ratio over time ratio, CPU measured and GPU modelled) and the verdict (exit collection against
/// branching, by modelled GPU efficiency at the largest N). About eight million traces: about
/// five minutes, so it is ignored in the default run.
#[test]
#[ignore = "prints the go/no-go table of research ALG section 6.2; run with --ignored --nocapture"]
fn exit_collection_go_no_go_table() {
    let configs = production_configs();
    let size = TableSize {
        pixels: 32,
        runs: 8,
        counts: &[64, 256, 1024, 4096, 16384],
    };
    let cells = go_no_go_table(&configs, &size);
    print_table(&configs, &size, &cells);
}
