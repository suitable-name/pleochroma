//! Tests of the zoned Bragg estimator (`maybe_scatter_oriented_zoned_bragg`): a zoned host that
//! also holds a Bragg domain structure (play-of-color, schiller). With no reflecting domain it is
//! the zoned estimator bit for bit; with one covering zone it is the homogeneous Bragg estimator;
//! a reflection weighs each channel by its zoned depth; and the survival transmittance is the
//! analytic one through the zones and the structure.

use super::{
    super::{
        NUM_CHANNELS,
        absorption::channel_absorption_alphas_assigned,
        camera::{Camera, Ray},
        environment::LightingPreset,
        refraction::{RayMaterialContext, build_ray_wavelength_cache},
        sampling::hash_u32,
        transport::trace_spectral_ray,
        zoned::{ZoneAlphas, ZonedCache, optical_depths_spectral},
    },
    delta_tracking::{
        BRAGG_MOSAIC_U_STREAM, BRAGG_MOSAIC_V_STREAM, BraggEvent, BraggScatter, TrackSegment, draw,
        maybe_scatter_oriented_bragg, track_bragg,
    },
    oriented::OrientedMedium,
    oriented_zoned::{maybe_scatter_oriented_zoned, maybe_scatter_oriented_zoned_bragg},
};
use crate::{
    geometry::cuts::StandardGemCuts,
    optics::{
        absorption::{AbsorptionBand, AbsorptionTensor},
        inclusions::{
            FamilyKind, InclusionSet, ResolvedFamily,
            bragg::{BraggStructure, StructureSpec, bragg_reflect},
            domains::domain_at,
        },
        materials::GemMaterial,
        polarization::StokesVector,
        zoning::{Zone, ZoneAbsorption, ZoneFrame, ZoneShape, ZonedAbsorption},
    },
};
use glam::{DVec3, Vec3};

const LAMBDAS: [f32; NUM_CHANNELS] = [420.0, 460.0, 500.0, 540.0, 580.0, 620.0, 660.0, 700.0];
const SCALE: f32 = 3.0;

fn band(center_nm: f32, width_nm: f32, peak_per_mm: f32) -> ZoneAbsorption {
    ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(vec![AbsorptionBand::new(
        center_nm,
        width_nm,
        peak_per_mm,
    )]))
}

/// The bicolour zoning: the base zone absorbs red, the half space `x > 0` absorbs blue.
fn bicolour_zoning() -> ZonedAbsorption {
    ZonedAbsorption {
        frame: ZoneFrame::IDENTITY,
        base: band(620.0, 50.0, 0.25),
        zones: vec![Zone {
            shape: ZoneShape::HalfSpace {
                normal: DVec3::X,
                offset: 0.0,
            },
            absorption: band(450.0, 40.0, 0.30),
        }],
        boundary_softness_mm: 0.0,
        follows_axis: false,
    }
}

fn probe(name: &str) -> GemMaterial {
    GemMaterial::new_custom(name, 1.76, 0.0, 0.0, [0.0, 0.0, 0.0])
}

/// A two-zone host (the bicolour) at [`SCALE`].
fn two_zone_host() -> GemMaterial {
    probe("two zones")
        .with_zoning(bicolour_zoning())
        .with_absorption_path_scale(SCALE)
}

/// A host whose base zone and one half-space zone carry the same `tensor`: one tensor everywhere.
fn identical_zones(tensor: &AbsorptionTensor, scale: f32) -> GemMaterial {
    probe("identical zones")
        .with_zoning(ZonedAbsorption {
            frame: ZoneFrame::IDENTITY,
            base: ZoneAbsorption::per_mm(tensor.clone()),
            zones: vec![Zone {
                shape: ZoneShape::HalfSpace {
                    normal: DVec3::X,
                    offset: -1.0e6,
                },
                absorption: ZoneAbsorption::per_mm(tensor.clone()),
            }],
            boundary_softness_mm: 0.0,
            follows_axis: false,
        })
        .with_absorption_path_scale(scale)
}

fn band_tensor() -> AbsorptionTensor {
    AbsorptionTensor::isotropic(vec![AbsorptionBand::new(540.0, 60.0, 0.10)])
}

fn silk() -> InclusionSet {
    InclusionSet::new(&[ResolvedFamily {
        kind: FamilyKind::Needle,
        dir: Vec3::Y,
        sigma: 0.8,
        p: 2,
        lobe: 0.2,
        rho: 0.3,
    }])
}

const fn context(material: &GemMaterial) -> RayMaterialContext<'_> {
    RayMaterialContext {
        material,
        lambdas: LAMBDAS,
        hero_idx: 0,
        c_axis: Vec3::Y,
        is_anisotropic: false,
        enable_internal_mode_coupling: true,
    }
}

/// Runs `f` with the zoned cache and the per-zone absorption of `material` for a ray of wave
/// normal `k_hat`.
fn with_zoned<R>(
    material: &GemMaterial,
    k_hat: Vec3,
    f: impl FnOnce(&ZonedCache, &ZoneAlphas) -> R,
) -> R {
    let ctx = context(material);
    let cache = build_ray_wavelength_cache(&ctx);
    let zoned = cache.zoned.as_ref().expect("a zoned cache");
    let alphas = zoned.zone_alphas(&ctx, &cache, k_hat, false);
    f(zoned, &alphas)
}

fn fresh_state() -> ([StokesVector; NUM_CHANNELS], [f32; NUM_CHANNELS]) {
    (
        [StokesVector::unpolarized(1.0); NUM_CHANNELS],
        [1.0; NUM_CHANNELS],
    )
}

fn seeds(i: u32) -> u32 {
    i.wrapping_mul(0x9E37_79B9).wrapping_add(0x1234_5677)
}

/// Every domain identical: one normal, one Bragg wavelength, all active.
fn uniform_structure(
    normal: Vec3,
    lambda_nm: f32,
    cell: f32,
    r0: f32,
    mosaic: f32,
) -> BraggStructure {
    BraggStructure::new(
        7,
        cell,
        (lambda_nm, lambda_nm),
        0.06,
        1.0,
        r0,
        mosaic,
        normal,
    )
}

/// An opal-like structure: many small domains with random normals and colors.
fn varied_structure() -> BraggStructure {
    BraggStructure::new(11, 0.25, (450.0, 650.0), 0.3, 1.0, 0.95, 0.3, Vec3::ZERO)
}

/// `a` and `b` agree within the relative `tol` (with an absolute floor of `tol * floor`).
fn close(a: f32, b: f32, tol: f32, floor: f32) -> bool {
    (a - b).abs() <= tol * a.abs().max(floor)
}

/// With no reflecting domain (coverage zero, so every tentative collision is a null one of
/// weight exactly one) the zoned Bragg estimator is the zoned estimator, bit for bit: the same
/// event, the same Stokes vectors and path pdfs.
#[test]
fn no_reflecting_domain_is_bit_identical_to_the_zoned_estimator() {
    let structure = BraggStructure::new(5, 0.2, (500.0, 600.0), 0.05, 0.0, 0.5, 0.0, Vec3::ZERO);
    let set = silk().with_structure(structure);
    let host = two_zone_host();
    let k_hat = Vec3::new(1.0, 0.2, 0.3).normalize();
    let ray = Ray {
        origin: Vec3::new(-0.2, 0.1, -0.9),
        dir: k_hat,
    };
    let hit_t = 1.8;
    let medium = OrientedMedium {
        set: &set,
        sigma_hg_model: 0.1,
        g: 0.3,
        lambdas: &LAMBDAS,
        hero_idx: 0,
        path_scale: SCALE,
    };
    let (mut scattered, mut survived) = (0, 0);
    with_zoned(&host, k_hat, |zoned, alphas| {
        for bounce in 0..3u32 {
            for rng_seed in 0..64u32 {
                let ((mut stokes_a, mut pdf_a), (mut stokes_b, mut pdf_b)) =
                    (fresh_state(), fresh_state());
                let plain = maybe_scatter_oriented_zoned(
                    &medium,
                    zoned,
                    alphas,
                    ray,
                    hit_t,
                    rng_seed,
                    bounce,
                    &mut stokes_a,
                    &mut pdf_a,
                );
                let with_structure = maybe_scatter_oriented_zoned_bragg(
                    &medium,
                    &structure,
                    zoned,
                    alphas,
                    ray,
                    hit_t,
                    rng_seed,
                    bounce,
                    &mut stokes_b,
                    &mut pdf_b,
                );
                assert_eq!(
                    plain.is_some(),
                    with_structure.is_some(),
                    "seed {rng_seed}, bounce {bounce}"
                );
                if let (Some(a), Some(b)) = (&plain, &with_structure) {
                    scattered += 1;
                    assert!(!b.bragg);
                    assert_eq!(a.t_free_model.to_bits(), b.scatter.t_free_model.to_bits());
                    assert_eq!(a.new_dir, b.scatter.new_dir);
                    assert_eq!(a.phase_ratio, b.scatter.phase_ratio);
                    assert_eq!(a.hero_phase.to_bits(), b.scatter.hero_phase.to_bits());
                } else {
                    survived += 1;
                }
                assert_eq!(stokes_a, stokes_b, "seed {rng_seed}, bounce {bounce}");
                assert_eq!(pdf_a, pdf_b, "seed {rng_seed}, bounce {bounce}");
            }
        }
    });
    assert!(
        scattered > 0 && survived > 0,
        "both branches must be exercised ({scattered} scattered, {survived} survived)"
    );
}

/// The homogeneous and the zoned estimator took the same event: the same free flight, new
/// direction and per-channel phase ratio, up to the bisection's rounding.
fn assert_events_match(rng_seed: u32, h: &BraggScatter, z: &BraggScatter) {
    assert!(
        close(h.scatter.t_free_model, z.scatter.t_free_model, 1e-4, 1e-3),
        "seed {rng_seed}: {} vs {}",
        h.scatter.t_free_model,
        z.scatter.t_free_model
    );
    assert!(
        (h.scatter.new_dir - z.scatter.new_dir).length() <= 1e-5,
        "seed {rng_seed}: direction"
    );
    for k in 0..NUM_CHANNELS {
        assert!(
            (h.scatter.phase_ratio[k] - z.scatter.phase_ratio[k]).abs() <= 1e-4,
            "seed {rng_seed}, channel {k}: phase ratio"
        );
    }
}

/// The per-channel Stokes intensities and path pdfs of the homogeneous (first) and the zoned
/// (second) estimator agree within the relative `tol`.
fn assert_weights_match(
    rng_seed: u32,
    tol: f32,
    homogeneous: (&[StokesVector; NUM_CHANNELS], &[f32; NUM_CHANNELS]),
    zoned: (&[StokesVector; NUM_CHANNELS], &[f32; NUM_CHANNELS]),
) {
    for k in 0..NUM_CHANNELS {
        let (a, b) = (homogeneous.0[k].intensity(), zoned.0[k].intensity());
        assert!(
            close(a, b, tol, 1e-3),
            "seed {rng_seed}, channel {k}: weight {a} vs {b}"
        );
        let (a, b) = (homogeneous.1[k], zoned.1[k]);
        assert!(
            close(a, b, tol, 1e-3),
            "seed {rng_seed}, channel {k}: path pdf {a} vs {b}"
        );
    }
}

/// One tensor everywhere: the zoned Bragg estimator makes the homogeneous Bragg estimator's
/// decisions and weights, up to the bisection's rounding.
#[test]
fn a_single_covering_zone_matches_the_homogeneous_bragg_estimator() {
    let tensor = band_tensor();
    let plain = probe("plain")
        .with_chromophore_absorption(tensor.clone())
        .with_absorption_path_scale(SCALE);
    let covering = identical_zones(&tensor, SCALE);
    let structure = varied_structure();
    let set = silk().with_structure(structure);
    let k_hat = Vec3::new(1.0, 0.2, 0.3).normalize();
    let ctx_plain = context(&plain);
    let cache_plain = build_ray_wavelength_cache(&ctx_plain);
    let alphas = channel_absorption_alphas_assigned(&ctx_plain, &cache_plain, k_hat, false);
    let medium = OrientedMedium {
        set: &set,
        sigma_hg_model: 0.1,
        g: 0.3,
        lambdas: &LAMBDAS,
        hero_idx: 0,
        path_scale: SCALE,
    };
    let ray = Ray {
        origin: Vec3::new(-0.2, 0.1, -0.9),
        dir: k_hat,
    };
    let hit_t = 1.8;

    let kind = |event: &Option<BraggScatter>| event.as_ref().map(|found| found.bragg);
    let trials = 400u32;
    let (mut skipped, mut reflected, mut homogeneous_events) = (0u32, 0u32, 0u32);
    with_zoned(&covering, k_hat, |zoned, zone_alphas| {
        for rng_seed in 0..trials {
            let ((mut stokes_h, mut pdf_h), (mut stokes_z, mut pdf_z)) =
                (fresh_state(), fresh_state());
            let homogeneous = maybe_scatter_oriented_bragg(
                &medium,
                &structure,
                &alphas,
                ray,
                hit_t,
                rng_seed,
                0,
                &mut stokes_h,
                &mut pdf_h,
            );
            let zoned_event = maybe_scatter_oriented_zoned_bragg(
                &medium,
                &structure,
                zoned,
                zone_alphas,
                ray,
                hit_t,
                rng_seed,
                0,
                &mut stokes_z,
                &mut pdf_z,
            );
            if kind(&homogeneous) != kind(&zoned_event) {
                // The bisected free flight moved a tentative collision across the boundary.
                skipped += 1;
                continue;
            }
            let bragg = kind(&homogeneous) == Some(true);
            if let (Some(h), Some(z)) = (&homogeneous, &zoned_event) {
                assert_events_match(rng_seed, h, z);
                if bragg {
                    reflected += 1;
                } else {
                    homogeneous_events += 1;
                }
            }
            // A reflection's weights are the analytic ones to rounding; the homogeneous events
            // and the survival share the looser tolerance of the plain zoned estimator's test.
            let tol = if bragg { 1e-4 } else { 2e-3 };
            assert_weights_match(rng_seed, tol, (&stokes_h, &pdf_h), (&stokes_z, &pdf_z));
        }
    });
    assert!(
        skipped * 50 < trials,
        "{skipped} of {trials} seeds changed event kind (at most 2 percent allowed)"
    );
    assert!(
        reflected > 0 && homogeneous_events > 0,
        "both event kinds must be exercised ({reflected} reflections, {homogeneous_events} scatters)"
    );
}

/// In a two-zone host a reflection weighs channel `k` by `exp(-(depth_k - depth_hero))` of its
/// zoned depth to the reflection, times its tracking weight, recomputed here independently.
#[test]
fn a_reflection_in_a_two_zone_host_weighs_each_channel_by_its_zoned_depth() {
    let host = two_zone_host();
    let dir = Vec3::new(0.6, 0.0, 0.8).normalize();
    let cos = dir.dot(Vec3::Z).abs();
    // The hero (channel 0, 420 nm) sits at the peak of the first order at this angle, so it
    // reflects early; the others weigh in by their own (much smaller) coefficient.
    let structure = uniform_structure(Vec3::Z, LAMBDAS[0] / cos, 0.1, 0.95, 0.0);
    let set = InclusionSet::EMPTY.with_structure(structure);
    let medium = OrientedMedium {
        set: &set,
        sigma_hg_model: 0.0,
        g: 0.0,
        lambdas: &LAMBDAS,
        hero_idx: 0,
        path_scale: SCALE,
    };
    // Starts just inside the base half, so most reflections come after the zone boundary.
    let ray = Ray {
        origin: Vec3::new(-0.01, 0.0, -0.5),
        dir,
    };
    let hit_t = 1.8;
    let bounce = 1u32;
    let sigma_s = [0.0f32; NUM_CHANNELS];
    let (mut checked, mut crossing) = (0, 0);
    with_zoned(&host, dir, |zoned, zone_alphas| {
        for seed in 0..256u32 {
            let (mut stokes, mut path_pdf) = fresh_state();
            let Some(found) = maybe_scatter_oriented_zoned_bragg(
                &medium,
                &structure,
                zoned,
                zone_alphas,
                ray,
                hit_t,
                seed,
                bounce,
                &mut stokes,
                &mut path_pdf,
            ) else {
                continue;
            };
            if !found.bragg {
                continue;
            }
            checked += 1;
            let t_free = found.scatter.t_free_model;
            let lengths = zoned.lengths_mm(ray.origin, ray.dir, 0.0, t_free, SCALE);
            if lengths[0] > 0.0 && lengths[1] > 0.0 {
                crossing += 1;
            }
            let depth = optical_depths_spectral(zone_alphas, &lengths, &sigma_s);

            // The tracking weights, from the tracker itself: the collisions before the first
            // accepted one do not depend on where tracking stops.
            let track = track_bragg(
                &structure,
                &TrackSegment {
                    origin: ray.origin,
                    dir,
                    t_max_model: hit_t,
                    lambdas: &LAMBDAS,
                    hero_idx: 0,
                    rng_seed: seed,
                    bounce,
                },
            );
            let BraggEvent::Reflect { t_model, domain } = track.event else {
                panic!("seed {seed}: the estimator reflected but the tracker did not");
            };
            assert_eq!(t_model.to_bits(), t_free.to_bits(), "seed {seed}");

            for k in 0..NUM_CHANNELS {
                let survival = (-f64::from(depth[k] - depth[0])).exp();
                let w = survival * f64::from(track.null_weight[k]);
                for (what, got) in [
                    ("intensity", f64::from(stokes[k].intensity())),
                    ("path pdf", f64::from(path_pdf[k])),
                ] {
                    assert!(
                        (got - w).abs() <= w.abs().mul_add(1e-5, 1e-30),
                        "seed {seed}, channel {k}: {what} {got} vs {w}"
                    );
                }
                assert!(
                    stokes[k].q == 0.0 && stokes[k].u == 0.0 && stokes[k].v == 0.0,
                    "seed {seed}, channel {k}: a reflection depolarizes"
                );
            }
            assert_eq!(found.scatter.phase_ratio, [1.0; NUM_CHANNELS]);
            assert_eq!(found.scatter.hero_phase, 0.0);

            let u1 = draw(seed, bounce, BRAGG_MOSAIC_U_STREAM, 0);
            let u2 = draw(seed, bounce, BRAGG_MOSAIC_V_STREAM, 0);
            let mut expected = bragg_reflect(dir, domain.normal, structure.mosaic, u1, u2);
            if expected == Vec3::ZERO {
                expected = dir;
            }
            assert_eq!(found.scatter.new_dir, expected, "seed {seed}");
        }
    });
    assert!(checked >= 8, "only {checked} of 256 seeds reflected");
    assert!(
        crossing > 0,
        "no reflection after the zone boundary among {checked}: the test would not see the zones"
    );
}

/// The mean and the standard error of the mean of `n` samples from their sum and sum of squares.
#[expect(
    clippy::suboptimal_flops,
    clippy::suspicious_operation_groupings,
    reason = "test arithmetic written as the formula"
)]
fn mean_and_se(sum: f64, sum_sq: f64, n: f64) -> (f64, f64) {
    let mean = sum / n;
    let var = (sum_sq / n - mean * mean).max(0.0);
    (mean, (var / n).sqrt())
}

/// The unbiasedness of the survival weight: through a two-zone host with a uniform structure of
/// known reflection coefficient, the mean over paths of the Stokes intensity of those that reach
/// the end of the segment is `exp(-depth_k - sigma_bragg,k L)` in every channel.
#[test]
fn survival_is_unbiased_through_a_zoned_host_with_a_uniform_structure() {
    let host = two_zone_host();
    let dir = Vec3::new(0.7, 0.1, 0.7).normalize();
    let origin = Vec3::new(-0.4, 0.1, -0.3);
    let length = 1.2f32;
    // lambda_B = 640 cos(45 deg) = 450 nm: the hero (420) sits on the shoulder of the band, the
    // next channels near its peak, the red channels far outside it.
    let structure = uniform_structure(Vec3::Z, 640.0, 0.5, 0.8, 0.0);
    let set = InclusionSet::EMPTY.with_structure(structure);
    let medium = OrientedMedium {
        set: &set,
        sigma_hg_model: 0.0,
        g: 0.0,
        lambdas: &LAMBDAS,
        hero_idx: 0,
        path_scale: SCALE,
    };
    let ray = Ray { origin, dir };
    let domain = domain_at(&structure, origin);
    let cos = dir.dot(Vec3::Z).abs();
    let sigma_b: [f32; NUM_CHANNELS] =
        std::array::from_fn(|k| structure.sigma(&domain, cos, LAMBDAS[k]));
    assert!(
        sigma_b.iter().filter(|s| **s * length > 0.3).count() >= 2,
        "the band must cover at least two channels for the check to mean anything: {sigma_b:?}"
    );

    let trials = 20_000u32;
    let mut sum = [0.0f64; NUM_CHANNELS];
    let mut sum_sq = [0.0f64; NUM_CHANNELS];
    let depth = with_zoned(&host, dir, |zoned, zone_alphas| {
        for i in 0..trials {
            let (mut stokes, mut path_pdf) = fresh_state();
            let event = maybe_scatter_oriented_zoned_bragg(
                &medium,
                &structure,
                zoned,
                zone_alphas,
                ray,
                length,
                seeds(i),
                1,
                &mut stokes,
                &mut path_pdf,
            );
            if event.is_none() {
                for k in 0..NUM_CHANNELS {
                    let w = f64::from(stokes[k].intensity());
                    sum[k] += w;
                    sum_sq[k] = w.mul_add(w, sum_sq[k]);
                }
            }
        }
        let lengths = zoned.lengths_mm(origin, dir, 0.0, length, SCALE);
        assert!(
            lengths[0] > 0.0 && lengths[1] > 0.0,
            "the segment must cross the zone boundary: {lengths:?}"
        );
        optical_depths_spectral(zone_alphas, &lengths, &[0.0; NUM_CHANNELS])
    });
    for k in 0..NUM_CHANNELS {
        let (mean, se) = mean_and_se(sum[k], sum_sq[k], f64::from(trials));
        let exact = f64::from(sigma_b[k])
            .mul_add(-f64::from(length), -f64::from(depth[k]))
            .exp();
        assert!(
            (mean - exact).abs() <= se.mul_add(4.0, 2e-3),
            "channel {k} ({} nm): estimated {mean} (se {se}), exact {exact}",
            LAMBDAS[k]
        );
    }
}

/// The opal on a two-zone host, with or without its default structure.
fn zoned_opal(structured: bool) -> GemMaterial {
    let name = "Precious Opal (White)";
    let opal = GemMaterial::by_name(name)
        .expect("\"Precious Opal (White)\" is a built-in material")
        .with_zoning(bicolour_zoning())
        .with_absorption_path_scale(SCALE)
        .with_structure_spec(
            &StructureSpec::default_for(name).expect("the opal has a default structure"),
        );
    if structured {
        opal
    } else {
        opal.with_inclusions(InclusionSet::EMPTY)
    }
}

/// The summed radiance of a grid of rays through the whole tracer (the studio rig has no NEE),
/// asserting every one is finite.
fn total_radiance(material: &GemMaterial) -> Vec3 {
    const GRID: usize = 10;
    const SAMPLES: u32 = 4;
    let planes = StandardGemCuts::standard_round_brilliant();
    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let mut sum = Vec3::ZERO;
    for iy in 0..GRID {
        for ix in 0..GRID {
            let ray = camera.generate_ray(ix as f32, iy as f32, GRID as f32, GRID as f32, 0.5, 0.5);
            for s in 0..SAMPLES {
                let pixel_id = u32::try_from(iy * GRID + ix).expect("a small grid");
                let seed = hash_u32(pixel_id ^ hash_u32(s ^ 0x0A_A1));
                let radiance = trace_spectral_ray(
                    ray,
                    &planes,
                    material,
                    16,
                    LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
                    seed,
                    (hash_u32(seed) as f32) / 4_294_967_295.0,
                    None,
                );
                assert!(
                    radiance.is_finite(),
                    "pixel {pixel_id}, sample {s}: {radiance:?}"
                );
                sum += radiance;
            }
        }
    }
    sum
}

/// The whole tracer renders a zoned opal: finite everywhere, and the structure shows (before the
/// zoned Bragg estimator it was inert in a zoned host).
#[test]
fn the_tracer_renders_a_zoned_opal_finite_and_different() {
    let structured = zoned_opal(true);
    assert!(
        structured
            .oriented_inclusions()
            .is_some_and(|set| set.structure().is_some())
            && structured.zoning.is_some()
    );
    let with = total_radiance(&structured);
    let without = total_radiance(&zoned_opal(false));
    let rel = ((with - without).abs() / without.abs().max(Vec3::splat(1e-6))).max_element();
    assert!(
        rel > 1e-3,
        "the structure must change the zoned opal's render: {with:?} vs {without:?} (relative {rel})"
    );
}
