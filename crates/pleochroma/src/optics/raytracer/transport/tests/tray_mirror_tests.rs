//! The Tray Reflection through the spectral tracer: a glossy plane under the stone.
//!
//! The cases that need no stone put the camera ray far to the side of one (the stone sits at the
//! origin, the rays at `x = 50`), so the only geometry is the tray plane at `y = -1`. Every
//! expected value is built from traces of the same seed without a mirror -- the radiance seen
//! at the plane is `(1 - F) * tray + F * sky(reflected)`, and the estimator is linear in it --
//! with the head shadow off so the observer direction (which differs between the traces) plays
//! no part. The slow statistical check is `#[ignore]`d.

use super::{
    super::{
        super::{
            NUM_CHANNELS,
            camera::Ray,
            environment::{EnvironmentSource, LightingPreset, TrayMirror},
            intersect::{build_plane_soa, intersect_polyhedron},
            refraction::{
                BounceContext, BounceRay, BounceState, ExitEvent, ExitSplitCtx, PathModeState,
                RngDraw, apply_partial_fresnel_bounce, build_ray_wavelength_cache,
                compute_bounce_refraction_geometry, exit_leaves_via_floor,
            },
            sampling::hash_u32,
        },
        PathTermination,
        inner::{build_ray_material_context, trace_spectral_ray_inner},
        trace_spectral_ray_with_finish_soa, wrapped_hero_wavelengths,
    },
    exit_splitting_tests::paired_mean_diff,
};
use crate::{
    geometry::cuts::StandardGemCuts,
    optics::{materials::GemMaterial, polarization::StokesVector, studio_rig::StudioRig},
};
use glam::Vec3;

/// The tray plane of the stone-free cases.
const PLANE_Y: f32 = -1.0;

/// The studio rig with a lit tray and no head shadow.
fn plain_env() -> EnvironmentSource<'static> {
    LightingPreset::RingLights
        .studio(1.0, 0.0, 0.85)
        .with_backdrop(0.23)
        .with_head_shadow(0.0)
}

/// A camera ray that meets the stone side-on, on the pavilion main facet at azimuth 0 (the
/// facet passes through `x = 0.5, y = -0.45`). Its first-surface reflection heads down onto the
/// tray, and its refracted part leaves the pavilion downward as well, so the tray and Rule M
/// are both reached. (A ray down through the table returns through the crown: a brilliant's
/// pavilion reflects it totally.)
fn pavilion_ray() -> Ray {
    Ray {
        origin: Vec3::new(3.0, -0.45, 0.0),
        dir: Vec3::NEG_X,
    }
}

/// The seeded hero draw every trace of these tests uses.
fn hero_rand(seed: u32) -> f32 {
    (hash_u32(seed) as f32) / 4_294_967_295.0
}

/// One path of `ray` under `env` through a round brilliant Diamond (12 bounces).
fn trace(env: EnvironmentSource<'_>, ray: Ray, seed: u32) -> Vec3 {
    let planes = StandardGemCuts::standard_round_brilliant();
    let plane_soa = build_plane_soa(&planes);
    let diamond = GemMaterial::diamond();
    trace_spectral_ray_with_finish_soa(
        ray,
        &planes,
        &plane_soa,
        &[],
        &diamond,
        12,
        env,
        seed,
        hero_rand(seed),
        None,
    )
}

/// [`trace`] with the termination record: the bounce index and the reason the path ended with.
fn terminated(env: EnvironmentSource<'_>, ray: Ray, seed: u32) -> (u32, PathTermination) {
    let planes = StandardGemCuts::standard_round_brilliant();
    let plane_soa = build_plane_soa(&planes);
    let diamond = GemMaterial::diamond();
    let mut termination = (u32::MAX, PathTermination::HitCap);
    trace_spectral_ray_inner(
        ray,
        &planes,
        &plane_soa,
        &[],
        &diamond,
        12,
        env,
        seed,
        hero_rand(seed),
        None,
        true,
        true,
        false,
        Some(&mut termination),
    );
    termination
}

/// `a` and `b` agree to a relative `1e-4` of the larger component, plus a tiny absolute floor.
fn assert_close(a: Vec3, b: Vec3, label: &str) {
    let scale = a.abs().max_element().max(b.abs().max_element());
    let error = (a - b).abs().max_element();
    assert!(
        error <= 1e-4f32.mul_add(scale, 1e-7),
        "{label}: {a} against {b} (error {error}, scale {scale})"
    );
}

/// (a) No mirror: `with_tray_mirror(None)` -- and a reflectance of zero, which builds no mirror at
/// all -- trace bit for bit like the environment that never heard of the field, on a ray whose
/// reflection and exits reach the tray ([`pavilion_ray`]). So does a mirror attached to a rig
/// without a lit tray (`backdrop = 0`), which reports no mirror.
#[test]
fn no_mirror_traces_bit_identically() {
    let ray = pavilion_ray();
    let base = LightingPreset::RingLights
        .studio(1.0, 0.85, 0.95)
        .with_backdrop(0.23);
    assert!(
        TrayMirror::new(PLANE_Y, 0.0).is_none(),
        "R0 = 0 is no mirror"
    );
    let unlit = LightingPreset::RingLights
        .studio(1.0, 0.85, 0.95)
        .with_backdrop(0.0);
    let hidden = unlit.with_tray_mirror(TrayMirror::new(PLANE_Y, 0.3));
    assert!(
        hidden.tray_mirror().is_none(),
        "a mirror needs a lit tray (backdrop > 0)"
    );
    for seed in 0..48u32 {
        let unlit_bits = trace(unlit, ray, 4000 + seed).to_array().map(f32::to_bits);
        let hidden_bits = trace(hidden, ray, 4000 + seed).to_array().map(f32::to_bits);
        assert_eq!(
            hidden_bits, unlit_bits,
            "seed {seed}: mirror without a tray"
        );
        let want = trace(base, ray, 4000 + seed).to_array().map(f32::to_bits);
        let none = trace(base.with_tray_mirror(None), ray, 4000 + seed)
            .to_array()
            .map(f32::to_bits);
        let zero = trace(
            base.with_tray_mirror(TrayMirror::new(PLANE_Y, 0.0)),
            ray,
            4000 + seed,
        )
        .to_array()
        .map(f32::to_bits);
        assert_eq!(none, want, "seed {seed}: explicit None");
        assert_eq!(zero, want, "seed {seed}: zero reflectance");
    }
}

/// (b) No stone, the camera straight down onto a perfect mirror: the pixel is the environment
/// straight up (the matte share `1 - F` is zero, the reflected ray escapes upward), and it is
/// not what the matte tray alone would show. The path takes one bounce to reach the floor and
/// escapes at the next, so it ends as `Escaped` at bounce 1 (a floor bounce uses one bounce).
#[test]
fn straight_down_onto_a_perfect_mirror_sees_the_sky() {
    let mirror = TrayMirror::new(PLANE_Y, 1.0).expect("R0 = 1 is a mirror");
    let mirrored = plain_env().with_tray_mirror(Some(mirror));
    let down = Ray {
        origin: Vec3::new(50.0, 3.0, 0.0),
        dir: Vec3::NEG_Y,
    };
    let up = Ray {
        origin: Vec3::new(50.0, PLANE_Y + 1e-4, 0.0),
        dir: Vec3::Y,
    };
    let mut any_differs_from_tray = false;
    for seed in 0..8u32 {
        assert_eq!(
            terminated(mirrored, down, seed),
            (1, PathTermination::Escaped),
            "seed {seed}: one floor bounce, then the escape"
        );
        let got = trace(mirrored, down, seed);
        assert_close(got, trace(plain_env(), up, seed), "mirror against sky");
        any_differs_from_tray |= (got - trace(plain_env(), down, seed)).abs().max_element() > 1e-6;
    }
    assert!(
        any_differs_from_tray,
        "the mirror must show something other than the matte tray"
    );
}

/// (c) No stone, `R0 = 0.04`, an oblique view: `(1 - F) * tray + F * sky(reflected)` with the
/// Schlick `F` of the viewing angle.
#[test]
fn oblique_low_reflectance_matches_the_formula() {
    let mirror = TrayMirror::new(PLANE_Y, 0.04).expect("R0 = 0.04 is a mirror");
    let mirrored = plain_env().with_tray_mirror(Some(mirror));
    let ray = Ray {
        origin: Vec3::new(50.0, 3.0, 0.0),
        dir: Vec3::new(0.4, -1.0, 0.25).normalize(),
    };
    let floor = mirror.hit(ray.origin, ray.dir).expect("meets the tray");
    assert!(
        floor.fresnel > 0.04 && floor.fresnel < 1.0,
        "oblique Schlick reflectance {} must lie between R0 and 1",
        floor.fresnel
    );
    let reflected = Ray {
        origin: floor.restart(),
        dir: floor.reflected,
    };
    for seed in 0..8u32 {
        let got = trace(mirrored, ray, seed);
        let tray = trace(plain_env(), ray, seed);
        let sky = trace(plain_env(), reflected, seed);
        let expected = tray * (1.0 - floor.fresnel) + sky * floor.fresnel;
        assert_close(got, expected, "oblique mirror against the formula");
    }
}

/// The exit of a ray from a dispersive `material` through a facet it meets at 15 degrees, with
/// exit splitting on and the tray `mirror` attached (`downward`: the ray leaves along `-y`, onto
/// the tray; else along `+y`, away from it): the per-channel Stokes intensity and `path_pdf`
/// after the first seed that transmits. A channel off the hero's direction ends with zero
/// intensity either way; its `path_pdf` tells whether it was split or terminated. An anisotropic
/// `material` must take the closed-form uniaxial exit (`uniaxial_fresnel/internal.rs`), which
/// the function asserts.
fn exit_event_after_transmit(
    material: &GemMaterial,
    mirror: Option<TrayMirror>,
    downward: bool,
) -> ([f32; NUM_CHANNELS], [f32; NUM_CHANNELS]) {
    let angle = 15.0f32.to_radians();
    // The normal faces against the ray's travel (as at the entry facet of the slab tests).
    let (normal, k_hat) = if downward {
        (Vec3::Y, Vec3::new(angle.sin(), -angle.cos(), 0.0))
    } else {
        (Vec3::NEG_Y, Vec3::new(angle.sin(), angle.cos(), 0.0))
    };
    let lambdas: [f32; NUM_CHANNELS] = wrapped_hero_wavelengths(0.0);
    let mat_ctx = build_ray_material_context(material, lambdas, 0, false);
    let cache = build_ray_wavelength_cache(&mat_ctx);
    let geo = compute_bounce_refraction_geometry(&mat_ctx, &cache, normal, k_hat, true, false);
    assert!(
        geo.sin2_t <= 1.0,
        "the 15 degree exit must not be total reflection"
    );
    if mat_ctx.is_anisotropic {
        assert!(
            geo.uniaxial_frame.is_some() && k_hat.cross(mat_ctx.c_axis).length_squared() > 1e-6,
            "an anisotropic exit must reach the closed-form uniaxial event"
        );
    }
    // A stone away from the hit point: the exit probes of the upward case find nothing to re-enter.
    let planes = StandardGemCuts::standard_round_brilliant();
    let plane_soa = build_plane_soa(&planes);
    let environment = plain_env().with_tray_mirror(mirror);
    let plane_of_incidence = k_hat.cross(normal).normalize_or_zero();
    for seed in 0..2_000u32 {
        let mut stokes = [StokesVector::unpolarized(1.0); NUM_CHANNELS];
        let mut path_pdf = [1.0f32; NUM_CHANNELS];
        let mut split_radiance = [0.0f32; NUM_CHANNELS];
        let mut exit_ctx = ExitSplitCtx {
            plane_soa: &plane_soa,
            tools: &[],
            environment,
            studio_rig: Some(StudioRig::new(0.0, 0.85)),
            observer: Vec3::Y,
            split_radiance: &mut split_radiance,
            enabled: true,
            compat: [u8::MAX; NUM_CHANNELS],
            split_mis_weight: 1.0,
            direction_match: crate::optics::raytracer::refraction::DirectionMatch::default(),
        };
        let bctx = BounceContext {
            ctx: &mat_ctx,
            cache: &cache,
            geo: &geo,
        };
        let mut state = BounceState {
            stokes: &mut stokes,
            path_pdf: &mut path_pdf,
        };
        let mut event = ExitEvent {
            exit: &mut exit_ctx,
            hit_point: Vec3::new(50.0, 0.0, 0.0),
        };
        let (_, _, inside_gem, _, _) = apply_partial_fresnel_bounce(
            &bctx,
            BounceRay { k_hat, normal },
            PathModeState {
                current_plane_normal: plane_of_incidence,
                inside_gem: true,
                is_extraordinary: false,
            },
            RngDraw {
                rng_seed: seed,
                bounce: 1,
            },
            &mut state,
            &mut event,
        );
        if !inside_gem {
            return (stokes.map(|s| s.i), path_pdf);
        }
    }
    panic!("no seed in 0..2000 transmitted at the exit facet");
}

/// The downward exit of Rule M for `material`: with the tray attached every channel off the hero's
/// direction is plainly terminated (`path_pdf = 0`) and the hero keeps its path; without the tray
/// the same channels are split, keeping their transmit-factor density.
fn assert_downward_exit_terminates_mismatched_channels(material: &GemMaterial) {
    let mirror = TrayMirror::new(PLANE_Y, 0.3);

    // Downward exit onto the tray: every mismatched channel is plainly terminated.
    let (stokes, pdf) = exit_event_after_transmit(material, mirror, true);
    let mismatched: Vec<usize> = (1..NUM_CHANNELS).filter(|&k| stokes[k] <= 0.0).collect();
    assert!(
        !mismatched.is_empty(),
        "a dispersive exit must leave some channel off the hero's direction"
    );
    assert!(pdf[0] > 0.0 && stokes[0] > 0.0, "the hero keeps its path");
    for &k in &mismatched {
        assert_eq!(
            pdf[k].to_bits(),
            0.0f32.to_bits(),
            "channel {k} over the tray"
        );
    }

    // The same exit without a tray: the mismatched channels are split (density kept).
    let (stokes_open, pdf_open) = exit_event_after_transmit(material, None, true);
    for &k in &mismatched {
        assert!(
            stokes_open[k] <= 0.0 && pdf_open[k] > 0.0,
            "channel {k} without a tray is split, not terminated"
        );
    }
}

/// (d) Rule M: with the tray attached, a channel whose own direction (or the hero's) meets the
/// tray is terminated with `path_pdf = 0`; one that does not is split, keeping its
/// transmit-factor density.
#[test]
fn rule_m_terminates_channels_whose_exit_meets_the_floor() {
    let diamond = GemMaterial::diamond();
    assert_downward_exit_terminates_mismatched_channels(&diamond);

    let mirror = TrayMirror::new(PLANE_Y, 0.3);
    // An upward exit with the tray attached never meets it: bit-identical to no tray.
    let (stokes_up, pdf_up) = exit_event_after_transmit(&diamond, mirror, false);
    let (stokes_up_open, pdf_up_open) = exit_event_after_transmit(&diamond, None, false);
    assert_eq!(
        stokes_up.map(f32::to_bits),
        stokes_up_open.map(f32::to_bits)
    );
    assert_eq!(pdf_up.map(f32::to_bits), pdf_up_open.map(f32::to_bits));
    assert!(
        (1..NUM_CHANNELS).any(|k| stokes_up[k] <= 0.0 && pdf_up[k] > 0.0),
        "an upward exit splits its mismatched channels"
    );
}

/// (d) for a uniaxial stone: a Sapphire exit through a downward facet takes the closed-form
/// uniaxial exit event, whose own Rule M site terminates the mismatched channels the same way.
#[test]
fn rule_m_terminates_uniaxial_channels_whose_exit_meets_the_floor() {
    let sapphire = GemMaterial::sapphire();
    let mat_ctx = build_ray_material_context(&sapphire, wrapped_hero_wavelengths(0.0), 0, false);
    assert!(mat_ctx.is_anisotropic, "Sapphire is birefringent");
    assert_downward_exit_terminates_mismatched_channels(&sapphire);
}

/// The decision of Rule M on its own: no mirror never declines; a mirror declines when the hero's
/// direction or the channel's meets it, and not when both point away.
#[test]
fn rule_m_decision_table() {
    let mirror = TrayMirror::new(PLANE_Y, 0.5);
    let hit_point = Vec3::new(50.0, 0.0, 0.0);
    let down = Vec3::new(0.2, -1.0, 0.1).normalize();
    let up = Vec3::new(0.2, 1.0, 0.1).normalize();
    assert!(!exit_leaves_via_floor(None, hit_point, down, down));
    assert!(exit_leaves_via_floor(mirror, hit_point, down, down));
    assert!(exit_leaves_via_floor(mirror, hit_point, down, up));
    assert!(exit_leaves_via_floor(mirror, hit_point, up, down));
    assert!(!exit_leaves_via_floor(mirror, hit_point, up, up));
}

/// (f) A stone over a perfect mirror shows other light through the pavilion than over
/// `R0 = 0.04`, and the exit-splitting estimator (Rule M at work) agrees with the plain chromatic
/// termination within noise on the mirrored scene.
///
/// Whether the mirror brightens the pixel depends on what it reflects (here the sky above the
/// stone against the lit paper), so the first check asks only that it changes the light beyond
/// noise. A ray down through the table returns through the crown and never reaches the tray,
/// so the ray meets the pavilion side-on ([`pavilion_ray`]).
#[test]
#[ignore = "statistical, a few seconds; run with --ignored"]
fn a_stone_over_a_mirror_changes_its_light_and_both_estimators_agree() {
    const SAMPLES: u32 = 6_000;
    let planes = StandardGemCuts::standard_round_brilliant();
    let plane_soa = build_plane_soa(&planes);
    let diamond = GemMaterial::diamond();
    // The culet sits on the axis: the lowest point of the stone is where a ray from below meets it.
    let culet = intersect_polyhedron(
        Ray {
            origin: Vec3::new(0.0, -10.0, 0.0),
            dir: Vec3::Y,
        },
        &planes,
    )
    .expect("the axis meets the stone");
    let plane_y = -10.0 + culet.t - 1e-3;
    let ray = pavilion_ray();
    let base = LightingPreset::RingLights
        .studio(1.0, 0.85, 0.95)
        .with_backdrop(0.23);
    let run = |reflectance: f32, split: bool| -> Vec<f64> {
        let env = base.with_tray_mirror(TrayMirror::new(plane_y, reflectance));
        (0..SAMPLES)
            .map(|i| {
                let seed = 20_000 + i;
                let xyz = trace_spectral_ray_inner(
                    ray,
                    &planes,
                    &plane_soa,
                    &[],
                    &diamond,
                    12,
                    env,
                    seed,
                    hero_rand(seed),
                    None,
                    true,
                    split,
                    false,
                    None,
                );
                f64::from(xyz.y)
            })
            .collect()
    };
    let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len() as f64;

    let mirror_on = run(1.0, true);
    let glass_on = run(0.04, true);
    let (change, change_error) = paired_mean_diff(&mirror_on, &glass_on);
    assert!(
        change.abs() > 4.0 * change_error,
        "the mirror must change the light through the pavilion: {} against {} ({change} +- \
         {change_error})",
        mean(&mirror_on),
        mean(&glass_on)
    );

    let mirror_off = run(1.0, false);
    let (diff, standard_error) = paired_mean_diff(&mirror_on, &mirror_off);
    assert!(
        diff.abs() <= 4.0f64.mul_add(standard_error, 0.01 * mean(&mirror_off)),
        "exit splitting disagrees with plain termination over the mirror: {diff} +- \
         {standard_error}"
    );
}
