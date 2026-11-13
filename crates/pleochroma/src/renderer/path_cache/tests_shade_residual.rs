//! Phase B beyond one position: the residual completion, the position choice, and whole frames.

use glam::{Vec2, Vec3};

use super::{
    BuildSettings, CacheOptions, DISPERSION_GATE_BG, ExitRecord, PathCacheFrame, PositionRef,
    SampleParts, Shader, TraceKit,
    bounce::EventKind,
    build::dispersion_b_minus_g,
    layout::tile_rect,
    record::FLAG_GLARE,
    shade::{RESIDUAL_FLOOR, is_covered, pick_position},
    signature::{BRANCH_SIG_SEED, branch_step, fold_sig},
    tests_shade::{MAX_BOUNCES, SIZE, Stats, assert_z_stats, centre_ray, draw, smooth_environment},
    tests_support::{Rig, glass},
};
use crate::{
    geometry::tool::StoneGeometry,
    optics::{
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{
            EnvironmentSource, PathEvents, cie_1931_cmf, pixel_rotations, sample_draws,
            surface::SurfaceScene,
            transport::{PreparedMaterial, trace_spectral_ray_prepared},
        },
    },
    renderer::{
        adaptive::ConvergenceMap,
        cpu_frame::{
            trace_live_tiles_surface_cached, trace_pixels_interleaved_surface,
            trace_pixels_interleaved_surface_cached,
        },
        env_map::{EnvironmentMap, rgb_to_spectral_radiance},
        frame_scene::FrameScene,
        whitted::CUTOFF,
    },
};

/// The XYZ a stone in a uniform environment of radiance `level` shows when it loses nothing.
fn furnace_target(level: f32) -> Vec3 {
    let mut target = Vec3::ZERO;
    for step in 0..=(780 - 380) {
        let lambda = 380.0_f32 + step as f32;
        target += cie_1931_cmf(lambda) * rgb_to_spectral_radiance([level; 3], lambda);
    }
    target / crate::color::cie1931::CIE_1931_Y_INTEGRAL_5NM
}

/// Kept energy plus the completed residual equals the light a white, non-absorbing stone receives.
#[test]
fn kept_plus_pruned_energy_is_the_unpruned_energy_through_phase_b() {
    const LEVEL: f32 = 2.5;
    const SAMPLES: u32 = 48;
    // 0.6 percent is lost to the bounce cap at 16 (the path tracer loses it too); the rest is
    // noise of the completion and the half-precision weights.
    const BOUNCES: u32 = 16;
    let rig = Rig::brilliant(glass());
    let map = EnvironmentMap::uniform(1, 1, [LEVEL; 3]);
    let environment = EnvironmentSource::hdr(&map);
    let scene = FrameScene {
        environment,
        ..rig.scene(SIZE, BOUNCES)
    };
    let prepared = PreparedMaterial::new(&rig.material);
    let shader = Shader::new(&scene, &rig.soa, prepared.clone());
    let kit = TraceKit::new(
        &rig.planes,
        &rig.soa,
        &prepared,
        (BOUNCES, CUTOFF),
        DISPERSION_GATE_BG,
    );

    let (mut kept, mut total) = (Stats::default(), Stats::default());
    let mut traced = 0_u32;
    let mut residual_positions = 0_u32;
    for y in 0..SIZE {
        for x in 0..SIZE {
            let ray = centre_ray(&rig, (x, y));
            let output = kit.position(ray);
            residual_positions += u32::from(output.residual >= RESIDUAL_FLOOR);
            let reference = PositionRef {
                records: &output.records,
                residual: output.residual,
            };
            for i in 0..SAMPLES {
                let parts = shader.shade_position(reference, ray, draw(i, u32::from(x == y) + y));
                kept.add(parts.kept);
                total.add(parts.total());
                traced += u32::from(parts.traced);
            }
        }
    }
    assert!(
        residual_positions > 0,
        "some positions drop energy under the cutoff (otherwise nothing is tested)"
    );
    assert!(traced > 0, "the completion traced at least once");
    let target = furnace_target(LEVEL);
    for (component, goal) in target.to_array().into_iter().enumerate() {
        let goal = f64::from(goal);
        let kept_ratio = kept.mean(component) / goal;
        let total_ratio = total.mean(component) / goal;
        assert!(
            kept_ratio <= 1.005,
            "component {component}: the kept part alone is {kept_ratio}"
        );
        assert!(
            (total_ratio - 1.0).abs() < 0.025,
            "component {component}: kept + completion is {total_ratio} of the light received"
        );
    }
}

#[test]
fn the_completion_traces_only_above_the_floor_and_always_at_a_large_residual() {
    let rig = Rig::brilliant(glass());
    let scene = rig.scene(SIZE, MAX_BOUNCES);
    let prepared = PreparedMaterial::new(&rig.material);
    let shader = Shader::new(&scene, &rig.soa, prepared.clone());
    let kit = TraceKit::new(
        &rig.planes,
        &rig.soa,
        &prepared,
        (MAX_BOUNCES, CUTOFF),
        DISPERSION_GATE_BG,
    );
    let ray = centre_ray(&rig, (8, 8));
    let output = kit.position(ray);
    let below = PositionRef {
        records: &output.records,
        residual: 0.5 * RESIDUAL_FLOOR,
    };
    let large = PositionRef {
        records: &output.records,
        residual: 0.5,
    };
    for i in 0..200 {
        assert!(!shader.shade_position(below, ray, draw(i, 3)).traced);
        assert!(
            shader.shade_position(large, ray, draw(i, 3)).traced,
            "a residual of 0.5 always traces (probability min(1, 8 * 0.5))"
        );
    }
}

fn record_with(dir: Vec3, sig: [u32; 2], weight: f32) -> ExitRecord {
    ExitRecord::new(dir, Vec2::ZERO, [weight; 8], 0, fold_sig(sig))
}

fn events_of(dir: Option<Vec3>, sig: [u32; 2]) -> PathEvents {
    PathEvents {
        escaped_dir: dir,
        signature: sig,
        ..PathEvents::default()
    }
}

#[test]
fn a_path_is_covered_by_a_record_of_its_branch_only() {
    let dir = Vec3::new(0.3, 0.8, -0.5).normalize();
    // The twin case: the same facets and events, the eigenmode after the second step differs.
    let first = branch_step(BRANCH_SIG_SEED, 4, EventKind::Transmit, false);
    let ordinary = branch_step(first, 9, EventKind::Reflect, false);
    let extraordinary = branch_step(first, 9, EventKind::Reflect, true);
    let other = branch_step(BRANCH_SIG_SEED, 2, EventKind::Reflect, false);
    let records = [
        record_with(dir, ordinary, 0.5),
        record_with(Vec3::X, other, 0.5),
    ];
    let hero = 560.0;
    assert!(is_covered(&records, hero, &events_of(Some(dir), ordinary)));
    // The same branch covers whatever the exit direction.
    assert!(is_covered(
        &records,
        hero,
        &events_of(Some(Vec3::Y), ordinary)
    ));
    // Another branch does not, even with the same direction and bounce count.
    assert!(!is_covered(
        &records,
        hero,
        &events_of(Some(dir), extraordinary)
    ));
    assert!(!is_covered(
        &records,
        hero,
        &events_of(Some(dir), BRANCH_SIG_SEED)
    ));
    // A path that returned no light has nothing to add.
    assert!(is_covered(&records, hero, &events_of(None, extraordinary)));
    // A record with no energy at the hero wavelength covers nothing.
    let dead = [record_with(dir, ordinary, 0.0)];
    assert!(!is_covered(&dead, hero, &events_of(Some(dir), ordinary)));
}

#[test]
fn glare_records_scale_with_the_surface_glare_of_the_environment() {
    // One glare record, nothing else: the kept part is the environment along it times the weight
    // times the glare scale, so halving the glare halves the kept light.
    let rig = Rig::brilliant(glass());
    let ray = centre_ray(&rig, (8, 8));
    let record = ExitRecord::new(
        Vec3::new(0.2, 0.9, 0.1).normalize(),
        Vec2::ZERO,
        [0.5; 8],
        FLAG_GLARE,
        0,
    );
    let kept_with = |glare: f32| {
        let environment = super::tests_shade::smooth_environment().with_surface_glare(glare);
        let scene = FrameScene {
            environment,
            ..rig.scene(SIZE, MAX_BOUNCES)
        };
        let shader = Shader::new(&scene, &rig.soa, PreparedMaterial::new(&rig.material));
        let reference = PositionRef {
            records: &[record],
            residual: 0.0,
        };
        shader.shade_position(reference, ray, draw(5, 1)).kept
    };
    let full = kept_with(1.0);
    let half = kept_with(0.5);
    assert!(full.y > 0.0);
    assert!(
        (half.y / full.y - 0.5).abs() < 1.0e-3,
        "{half:?} vs {full:?}"
    );
}

#[test]
fn the_position_choice_is_uniform_and_does_not_follow_the_sample_parity() {
    let count = 8;
    let (mut all, mut odd) = ([0_u32; 8], [0_u32; 8]);
    for sample in 0..8000_u32 {
        let position = pick_position(1234, sample, count);
        assert!(position < count);
        all[position] += 1;
        if sample % 2 == 1 {
            odd[position] += 1;
        }
    }
    for position in 0..count {
        assert!(
            (800..=1200).contains(&all[position]),
            "position {position} got {} of 8000",
            all[position]
        );
        assert!(
            (400..=600).contains(&odd[position]),
            "position {position} got {} of the 4000 odd samples",
            odd[position]
        );
    }
    assert_eq!(pick_position(9, 3, 1), 0);
}

fn cached_frame<'s, 'a>(scene: &'s FrameScene<'a>, rig: &'s Rig) -> PathCacheFrame<'s, 'a> {
    PathCacheFrame::new(
        scene,
        &rig.soa,
        (&[], Fluorescence::none(), SurfaceScene::IDEAL),
        CacheOptions::default(),
    )
    .expect("the glass stone is eligible")
}

fn luminance_sum(pixels: &[Vec3]) -> f64 {
    pixels.iter().map(|p| f64::from(p.y)).sum()
}

#[test]
fn a_cached_frame_has_the_same_mean_as_the_traced_frame() {
    let rig = Rig::brilliant(glass());
    let scene = FrameScene {
        environment: super::tests_shade::smooth_environment(),
        ..rig.scene(12, MAX_BOUNCES)
    };
    let extras = (&[][..], Fluorescence::none(), SurfaceScene::IDEAL);
    let cache = cached_frame(&scene, &rig);
    let cached = trace_pixels_interleaved_surface_cached(
        &scene,
        extras,
        &rig.soa,
        (0, 1),
        (0, 96),
        Some(&cache),
    );
    let traced = trace_pixels_interleaved_surface(&scene, extras, &rig.soa, 0, 1, 0, 384);
    let cached_mean = luminance_sum(&cached) / 96.0;
    let traced_mean = luminance_sum(&traced) / 384.0;
    let relative = (cached_mean - traced_mean).abs() / traced_mean;
    assert!(
        relative < 0.04,
        "frame luminance: cached {cached_mean}, traced {traced_mean} ({relative})"
    );
    assert!(cache.built_tiles() > 0);
}

#[test]
fn the_cached_live_pass_agrees_with_the_cached_full_frame_pass_and_its_odd_half() {
    let rig = Rig::brilliant(glass());
    let scene = FrameScene {
        environment: super::tests_shade::smooth_environment(),
        ..rig.scene(12, MAX_BOUNCES)
    };
    let extras = (&[][..], Fluorescence::none(), SurfaceScene::IDEAL);
    let cache = cached_frame(&scene, &rig);
    let live = ConvergenceMap::new(12, 12, 4).live_tiles();
    let pixels = 12 * 12;
    let (mut sum, mut half_b) = (vec![Vec3::ZERO; pixels], vec![Vec3::ZERO; pixels]);
    trace_live_tiles_surface_cached(
        &scene,
        extras,
        &rig.soa,
        &live,
        (0, 6),
        (&mut sum, &mut half_b),
        Some(&cache),
    );

    let full = trace_pixels_interleaved_surface_cached(
        &scene,
        extras,
        &rig.soa,
        (0, 1),
        (0, 6),
        Some(&cache),
    );
    let mut odd = vec![Vec3::ZERO; pixels];
    for sample in (1..6).step_by(2) {
        let one = trace_pixels_interleaved_surface_cached(
            &scene,
            extras,
            &rig.soa,
            (0, 1),
            (sample, 1),
            Some(&cache),
        );
        for (acc, value) in odd.iter_mut().zip(one) {
            *acc += value;
        }
    }
    let bits = |pixels: &[Vec3]| -> Vec<[u32; 3]> {
        pixels
            .iter()
            .map(|p| p.to_array().map(f32::to_bits))
            .collect()
    };
    assert_eq!(bits(&sum), bits(&full));
    assert_eq!(bits(&half_b), bits(&odd));

    // Rendering again from the same cache, and from a fresh one, gives the same bytes.
    let again = trace_pixels_interleaved_surface_cached(
        &scene,
        extras,
        &rig.soa,
        (0, 1),
        (0, 6),
        Some(&cache),
    );
    let fresh = cached_frame(&scene, &rig);
    let other = trace_pixels_interleaved_surface_cached(
        &scene,
        extras,
        &rig.soa,
        (0, 1),
        (0, 6),
        Some(&fresh),
    );
    assert_eq!(bits(&full), bits(&again));
    assert_eq!(bits(&full), bits(&other));
}

/// Cutoff of the paired completion tests: coarse, so most positions complete on every sample
/// (`p = min(1, 8 r)`) and dropped branches sit next to kept ones (mode twins, per-bin
/// neighbours), where a wrong cover decision would show.
const PAIRED_CUTOFF: f32 = 0.02;

/// A pixel whose Y z score reaches this size in the first run is run again on fresh samples.
const SCREEN_Z: f64 = 3.0;

/// The confirmation run of a screened pixel draws this many times the first run's samples.
const CONFIRM_FACTOR: u32 = 16;

/// Running sums of a stream of paired differences, per XYZ component.
#[derive(Default)]
struct Paired {
    n: f64,
    sum: [f64; 3],
    squares: [f64; 3],
}

impl Paired {
    /// Adds one difference.
    fn add(&mut self, d: Vec3) {
        self.n += 1.0;
        for (i, v) in d.to_array().into_iter().enumerate() {
            let v = f64::from(v);
            self.sum[i] += v;
            self.squares[i] = v.mul_add(v, self.squares[i]);
        }
    }

    /// The z score of the mean of component `i` against zero: `None` with fewer than two values or
    /// with no spread and a zero mean, infinite with no spread and a nonzero mean.
    fn z(&self, i: usize) -> Option<f64> {
        if self.n < 2.0 {
            return None;
        }
        let mean = self.sum[i] / self.n;
        let variance =
            mean.mul_add(-mean, self.squares[i] / self.n).max(0.0) * self.n / (self.n - 1.0);
        let spread = (variance / self.n).sqrt();
        if spread > 0.0 {
            Some(mean / spread)
        } else if mean.abs() > 0.0 {
            Some(f64::INFINITY)
        } else {
            None
        }
    }
}

/// What the paired decomposition accumulates over the frame.
#[derive(Default)]
struct PairedRun {
    /// `kept + completion - traced`: zero mean when the cover test is right.
    frame: Paired,
    /// `traced - stratified`: phase A's quadrature, printed only.
    quadrature: Paired,
    compared: u64,
    fired: u64,
    kept_y: f64,
    total_y: f64,
}

impl PairedRun {
    /// Prints the run's summary and asserts the rule of `assert_paired_completion_in`.
    ///
    /// A completion fired, every counted pixel score passes `assert_z_stats` at 5, and the
    /// frame's z of each XYZ component over the first run is below 4.
    fn assert_rule(&self, pixel_zs: &[f64], confirmed_pixels: u32, label: &str) {
        let z_of = |p: &Paired| [0, 1, 2].map(|i| p.z(i));
        println!(
            "{label}: {} samples compared, {:.1} % fired, kept part is {:.1} % of the total Y; {confirmed_pixels} of {} pixels confirmed on fresh samples; frame z (X, Y, Z) {:?}, quadrature z {:?}",
            self.compared,
            100.0 * self.fired as f64 / self.compared.max(1) as f64,
            100.0 * self.kept_y / self.total_y.max(1e-12),
            pixel_zs.len(),
            z_of(&self.frame),
            z_of(&self.quadrature),
        );
        assert!(self.fired > 0, "{label}: no sample fired the completion");
        assert_z_stats(pixel_zs, 5.0, label);
        for (i, z) in z_of(&self.frame).into_iter().enumerate() {
            if let Some(z) = z {
                assert!(z.abs() < 4.0, "{label}: frame z of component {i} is {z}");
            }
        }
    }
}

/// One sample of a paired pixel: the cache's parts, the trace through the position's ray and the
/// trace through the sample's own jittered ray. `None` when the cache has nothing for the pixel.
type PairedSample<'a> = &'a dyn Fn(u32) -> Option<(SampleParts, Vec3, Vec3)>;

/// The Y z score a pixel counts with under the two-stage rule, and whether it was confirmed.
///
/// The first run takes the samples `0..samples` and adds them to `run`. A score below
/// [`SCREEN_Z`] counts as it is. Otherwise the pixel is run again on the next
/// `CONFIRM_FACTOR * samples` sample indices, which the screen did not see, and counts with the
/// Y score of that run alone; one line names `what`, both scores and both means. `None` when the
/// first run has no score.
fn paired_pixel_score(
    sample: PairedSample<'_>,
    samples: u32,
    run: &mut PairedRun,
    what: &str,
) -> Option<(f64, bool)> {
    let mut paired = Paired::default();
    for s in 0..samples {
        let Some((parts, traced, stratified)) = sample(s) else {
            continue;
        };
        paired.add(parts.total() - traced);
        run.frame.add(parts.total() - traced);
        run.quadrature.add(traced - stratified);
        run.compared += 1;
        run.fired += u64::from(parts.traced);
        run.kept_y += f64::from(parts.kept.y);
        run.total_y += f64::from(parts.total().y);
    }
    let z = paired.z(1)?;
    if z.abs() < SCREEN_Z {
        return Some((z, false));
    }
    // The confirmation run: fresh samples only, so the screen does not select its data.
    let mut confirm = Paired::default();
    let (mut cached_y, mut traced_y) = (0.0_f64, 0.0_f64);
    for s in samples..samples.saturating_mul(1 + CONFIRM_FACTOR) {
        let Some((parts, traced, _)) = sample(s) else {
            continue;
        };
        confirm.add(parts.total() - traced);
        cached_y += f64::from(parts.total().y);
        traced_y += f64::from(traced.y);
    }
    let confirmed = confirm.z(1).unwrap_or(0.0);
    println!(
        "{what}: screen z {z:.2} over {samples} samples, confirmation z {confirmed:.2} over {:.0} fresh samples (X, Y, Z: {:?}; mean Y cached {:.6}, traced {:.6})",
        confirm.n,
        [0, 1, 2].map(|i| confirm.z(i)),
        cached_y / confirm.n.max(1.0),
        traced_y / confirm.n.max(1.0),
    );
    Some((confirmed, true))
}

/// What a paired trace needs besides the cache.
struct PairedTrace<'a> {
    rig: &'a Rig,
    geom: StoneGeometry<'a>,
    prepared: PreparedMaterial<'a>,
    environment: EnvironmentSource<'a>,
    max_bounces: u32,
}

impl PairedTrace<'_> {
    /// One ordinary path-tracer sample through `ray` with the draws of `seed` and `hero_rand`.
    fn trace(&self, ray: crate::optics::raytracer::Ray, (seed, hero_rand): (u32, f32)) -> Vec3 {
        trace_spectral_ray_prepared(
            ray,
            self.geom,
            &self.rig.soa,
            &[],
            &self.prepared,
            Fluorescence::none(),
            self.max_bounces,
            self.environment,
            seed,
            hero_rand,
            None,
            SurfaceScene::IDEAL,
        )
    }
}

/// Asserts that, at every cached position of a `size` x `size` frame of `material`, the cache's
/// kept part plus its completion has the mean of the path tracer through the position's ray.
///
/// The identity: `kept + completion - traced` has mean `kept - E[traced * covered]`, because the
/// fire draw is independent of the trace's draws. It is zero exactly when the cover test removes
/// from the traced sample what the records hold. `samples` samples are shaded per pixel at the
/// coarse `PAIRED_CUTOFF`.
///
/// The difference is not low-noise: the kept part sums every record, while the traced sample
/// follows one branch drawn with its probability. A small lamp's branch is drawn only a handful of
/// times per pixel, so a pixel's z is a skewed t-statistic that can be large without any bias. The
/// rule is therefore in two stages. A pixel whose Y z score stays below [`SCREEN_Z`] counts with
/// that score. A pixel that reaches it is run again on [`CONFIRM_FACTOR`] times as many fresh
/// samples (the sample indices after the first run's, so the screen does not select the data),
/// and counts with the score of that run alone: a bias that gives 3 at `samples` gives 12 there,
/// while a rarely drawn branch, drawn that much more often, gives a Gaussian score. Every
/// counted score goes through `assert_z_stats` at 5, and the frame's z of each XYZ component over
/// the first run must stay below 4. The quadrature term (position ray against the sample's own
/// jittered ray) is printed only: it is phase A's.
fn assert_paired_completion_in(
    material: GemMaterial,
    environment: EnvironmentSource<'_>,
    (size, max_bounces, samples): (u32, u32, u32),
    label: &str,
) {
    const TILE: u32 = 8;
    let rig = Rig::brilliant(material);
    let scene = FrameScene {
        environment,
        ..rig.scene(size, max_bounces)
    };
    let options = CacheOptions {
        settings: BuildSettings {
            tile: TILE,
            k_max: 8,
            cutoff: PAIRED_CUTOFF,
            ..BuildSettings::default()
        },
        ..CacheOptions::default()
    };
    let frame = PathCacheFrame::new(
        &scene,
        &rig.soa,
        (&[], Fluorescence::none(), SurfaceScene::IDEAL),
        options,
    )
    .expect("the stone is eligible");
    let paired_trace = PairedTrace {
        rig: &rig,
        geom: StoneGeometry::planes_only(&rig.planes),
        prepared: PreparedMaterial::new(&rig.material),
        environment: scene.environment,
        max_bounces,
    };

    let mut run = PairedRun::default();
    let mut pixel_zs = Vec::new();
    let mut confirmed_pixels = 0_u32;
    for y in 0..size {
        for x in 0..size {
            let index = frame.tile_of((x, y));
            let (x0, y0, x1, _) = tile_rect(size, size, TILE, index);
            let local = (y - y0) as usize * (x1 - x0) as usize + (x - x0) as usize;
            let Some(tile) = frame.tile(index) else {
                continue;
            };
            let count = tile.pixel_positions(local).len();
            if count == 0 {
                continue;
            }
            let pixel = y * size + x;
            let rot = pixel_rotations(pixel);
            let sample = |s: u32| -> Option<(SampleParts, Vec3, Vec3)> {
                let parts = frame.shade_sample((x, y), s, &rot)?;
                let at = sample_draws(
                    pixel,
                    u32::try_from(pick_position(pixel, s, count)).expect("a small index"),
                    &rot,
                );
                let ray_at = |jx: f32, jy: f32| {
                    rig.camera
                        .generate_ray(x as f32, y as f32, size as f32, size as f32, jx, jy)
                };
                let draws = sample_draws(pixel, s, &rot);
                let traced = paired_trace.trace(
                    ray_at(at.jitter_x, at.jitter_y),
                    (draws.seed, draws.hero_rand),
                );
                let stratified = paired_trace.trace(
                    ray_at(draws.jitter_x, draws.jitter_y),
                    (draws.seed, draws.hero_rand),
                );
                Some((parts, traced, stratified))
            };
            let what = format!("{label}: pixel ({x}, {y}) with {count} positions");
            if let Some((z, confirmed)) = paired_pixel_score(&sample, samples, &mut run, &what) {
                pixel_zs.push(z);
                confirmed_pixels += u32::from(confirmed);
            }
        }
    }
    run.assert_rule(&pixel_zs, confirmed_pixels, label);
}

/// [`assert_paired_completion_in`] under the smooth test environment.
fn assert_paired_completion(material: GemMaterial, dimensions: (u32, u32, u32), label: &str) {
    assert_paired_completion_in(material, smooth_environment(), dimensions, label);
}

/// The benchmark sapphire under its Daylight studio: kept part plus completion
/// has the tracer's mean on a colored stone. A transmittance that is off by a percent moves every
/// pixel the same way and shows as a frame z far beyond 4; the small lamps make single pixels
/// heavy-tailed, which the confirmation run of `paired_pixel_score` answers.
#[test]
fn the_completion_is_unbiased_at_every_cached_position_of_a_colored_sapphire() {
    use crate::renderer::test_stats::{bench_daylight, bench_sapphire};
    assert_paired_completion_in(
        bench_sapphire(),
        bench_daylight(),
        (16, 8, 256),
        "bench sapphire, Daylight",
    );
}

/// The colored sapphire under the smooth test environment (see the note on the equivalence
/// test of the same name in `tests_shade`).
#[test]
fn the_completion_is_unbiased_at_every_cached_position_of_a_colored_sapphire_under_smooth_light() {
    use crate::renderer::test_stats::bench_sapphire;
    assert_paired_completion(bench_sapphire(), (16, 8, 256), "bench sapphire, smooth");
}

/// The completion is unbiased at every cached position of a sapphire-like (uniaxial) stone,
/// edge positions included.
#[test]
fn the_completion_is_unbiased_at_every_cached_position_of_a_birefringent_stone() {
    assert_paired_completion(
        GemMaterial::new_custom("cache test sapphire-like", 1.768, 0.0106, 0.008, [0.0; 3]),
        (16, 8, 256),
        "sapphire-like",
    );
}

/// The same for a moissanite-like stone, where every branch is a per-bin record.
#[test]
fn the_completion_is_unbiased_at_every_cached_position_of_a_dispersive_birefringent_stone() {
    let material =
        GemMaterial::new_custom("cache test moissanite-like", 2.65, 0.07, 0.043, [0.0; 3]);
    assert!(
        dispersion_b_minus_g(&material) > DISPERSION_GATE_BG,
        "every branch must be a per-bin record"
    );
    assert_paired_completion(material, (10, 6, 192), "moissanite-like");
}
