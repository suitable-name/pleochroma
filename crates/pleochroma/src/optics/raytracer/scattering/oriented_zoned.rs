//! Oriented inclusions in a zoned host.
//!
//! [`super::oriented::maybe_scatter_oriented`] for a material whose absorption varies by zone
//! (`GemMaterial::zoning`). Along one straight segment the direction `w` is constant, so the
//! inclusions' coefficient `sigma_s,k(w)` is constant per channel while the host's absorption
//! `alpha_z,k` changes from zone to zone: the optical depth is piecewise linear and the hero
//! channel's is inverted for the free-flight sample, exactly as
//! [`super::hg::maybe_scatter_or_extinguish_zoned`] does for the plain haze, with the per-channel
//! scattering coefficient in place of the scalar.
//!
//! Scattering-point next-event estimation is off for a zoned medium (the shadow ray's
//! transmittance would need the zone lengths of its own path); the caller disables it, which
//! keeps the phase-sampled continuation at full weight and the estimator unbiased.
//!
//! A zoned host that also holds a Bragg domain structure (play-of-color, schiller) takes
//! [`maybe_scatter_oriented_zoned_bragg`]: the zones change only the host's absorption, the
//! structure's reflection coefficient does not depend on them, so the composition of
//! [`super::delta_tracking`] holds with the zoned optical depth in place of the homogeneous one.

use super::{
    super::{
        NUM_CHANNELS,
        camera::Ray,
        sampling::{DISTANCE_SAMPLE_STREAM, PHASE_DIR_U_STREAM, PHASE_DIR_V_STREAM},
        zoned::{ZoneAlphas, ZonedCache, local_extinction_spectral, optical_depths_spectral},
    },
    delta_tracking::{
        BRAGG_MOSAIC_U_STREAM, BRAGG_MOSAIC_V_STREAM, BraggEvent, BraggScatter, TrackSegment,
        bragg_reflect_weights, draw, track_bragg,
    },
    oriented::{ORIENTED_COMPONENT_STREAM, OrientedMedium, OrientedScatter, stream_draw},
};
use crate::optics::{
    inclusions::bragg::{BraggStructure, bragg_reflect},
    polarization::StokesVector,
};
use glam::Vec3;

/// The hero channel's free flight through a zoned segment by inversion of its piecewise-linear
/// optical depth: `Some(t_free)` (model units) when `target` falls inside the segment's total
/// hero depth `depth_full_hero`, `None` when the hero survives to `hit_t`.
///
/// `depth_to(t)` is the per-channel optical depth of the first `t` model units. The 48-step
/// bisection is the one [`maybe_scatter_oriented_zoned`] and
/// [`maybe_scatter_oriented_zoned_bragg`] share, so both land on the same flight for the same
/// draw.
fn zoned_hero_free_flight(
    depth_to: &impl Fn(f32) -> [f32; NUM_CHANNELS],
    hero: usize,
    hit_t: f32,
    target: f32,
    depth_full_hero: f32,
) -> Option<f32> {
    if target < depth_full_hero {
        // Invert the hero's piecewise-linear optical depth.
        let (mut lo, mut hi) = (0.0f32, hit_t);
        for _ in 0..48 {
            let mid = f32::midpoint(lo, hi);
            if depth_to(mid)[hero] < target {
                lo = mid;
            } else {
                hi = mid;
            }
            if hi - lo <= f32::EPSILON * hi {
                break;
            }
        }
        Some(f32::midpoint(lo, hi))
    } else {
        None
    }
}

/// The per-bounce extinction/scatter estimator for oriented inclusions in a zoned host.
///
/// `ray` is the segment's ray (model units) and `hit_t` its length; `zone_alphas` are the
/// per-zone per-channel absorption coefficients of `ZonedCache::zone_alphas`. The decisions,
/// weights and return value are those of [`super::oriented::maybe_scatter_oriented`]: `Some` if
/// an event fired strictly before `hit_t`, `None` if the path survived to the facet (`stokes`
/// and `path_pdf` then carry the survival weight).
///
/// With one zone covering the segment this reduces to `maybe_scatter_oriented` up to the
/// bisection's rounding.
#[expect(
    clippy::too_many_arguments,
    reason = "maybe_scatter_oriented's own per-ray inputs plus the zone tables and the ray"
)]
pub(in super::super) fn maybe_scatter_oriented_zoned(
    medium: &OrientedMedium<'_>,
    zoned: &ZonedCache,
    zone_alphas: &ZoneAlphas,
    ray: Ray,
    hit_t: f32,
    rng_seed: u32,
    bounce: u32,
    stokes: &mut [StokesVector; NUM_CHANNELS],
    path_pdf: &mut [f32; NUM_CHANNELS],
) -> Option<OrientedScatter> {
    let hero = medium.hero_idx;
    let path_scale = medium.path_scale;
    // Per absorption-length unit, like the zone alphas.
    let sigma_s: [f32; NUM_CHANNELS] =
        std::array::from_fn(|k| medium.sigma_s(ray.dir, medium.lambdas[k]) / path_scale);
    let depth_to = |t: f32| -> [f32; NUM_CHANNELS] {
        let lengths = zoned.lengths_mm(ray.origin, ray.dir, 0.0, t, path_scale);
        optical_depths_spectral(zone_alphas, &lengths, &sigma_s)
    };

    let dist_rand = stream_draw(rng_seed, bounce, DISTANCE_SAMPLE_STREAM);
    let one_minus_u = (1.0 - dist_rand).max(1e-7);
    let target = -(one_minus_u.ln());

    let depth_full = depth_to(hit_t);
    if let Some(t_free) = zoned_hero_free_flight(&depth_to, hero, hit_t, target, depth_full[hero]) {
        let sigma_t_local =
            local_extinction_spectral(zoned, zone_alphas, ray, t_free, hit_t, path_scale, &sigma_s);
        let pdf_hero = (sigma_t_local[hero] * one_minus_u).max(1e-12);
        let depth_free = depth_to(t_free);
        let mut tr_args = [0f32; NUM_CHANNELS];
        for (a, d) in tr_args.iter_mut().zip(&depth_free) {
            *a = -d;
        }
        let tr = crate::simd::exp_f32x8(tr_args);
        for k in 0..NUM_CHANNELS {
            let weight = tr[k] * sigma_s[k] / pdf_hero;
            stokes[k] = StokesVector::unpolarized(stokes[k].intensity() * weight);
            path_pdf[k] *= sigma_t_local[k] * tr[k];
        }
        let u_c = stream_draw(rng_seed, bounce, ORIENTED_COMPONENT_STREAM);
        let u1 = stream_draw(rng_seed, bounce, PHASE_DIR_U_STREAM);
        let u2 = stream_draw(rng_seed, bounce, PHASE_DIR_V_STREAM);
        let new_dir = medium.sample_direction(ray.dir, u_c, u1, u2);
        let (hero_phase, per_channel) = medium.channel_phases(ray.dir, new_dir);
        let phase_ratio: [f32; NUM_CHANNELS] = std::array::from_fn(|k| {
            if hero_phase > 1e-12 {
                per_channel[k] / hero_phase
            } else {
                1.0
            }
        });
        Some(OrientedScatter {
            t_free_model: t_free,
            new_dir,
            phase_ratio,
            hero_phase,
        })
    } else {
        let mut survive_args = [0f32; NUM_CHANNELS];
        for (a, d) in survive_args.iter_mut().zip(&depth_full) {
            *a = -d;
        }
        let survive = crate::simd::exp_f32x8(survive_args);
        let survive_hero = survive[hero];
        for k in 0..NUM_CHANNELS {
            stokes[k] = stokes[k].scale(survive[k] / survive_hero.max(1e-30));
            path_pdf[k] *= survive[k];
        }
        None
    }
}

/// [`maybe_scatter_oriented_zoned`] for a zoned host that also holds a Bragg domain structure.
///
/// The zones change only the host's absorption `alpha_z,k`; the structure's reflection
/// coefficient and the inclusions' `sigma_s,k(w)` do not depend on them, so the processes
/// superpose exactly as in [`super::delta_tracking::maybe_scatter_oriented_bragg`] with the
/// zoned optical depth in place of the homogeneous one: the hero's zoned free flight `t_h`
/// (the very draw and bisection of [`maybe_scatter_oriented_zoned`]) bounds the tracking, and
/// whichever event comes first wins.
///
/// * A reflection at `t_model < min(t_h, hit_t)` weighs channel `k` by
///   `exp(-(depth_k(t_model) - depth_hero(t_model)))` times its tracking ratio
///   ([`bragg_reflect_weights`]), depolarizes, and carries no phase ratio (`bragg` is set: no
///   scattering-point NEE, no phase MIS partner).
/// * With nothing found, [`maybe_scatter_oriented_zoned`] decides the zoned event as before
///   (it re-draws the same `u` and lands on the same `t_h`) and the product of the null-collision
///   ratios multiplies the result.
#[expect(
    clippy::too_many_arguments,
    reason = "maybe_scatter_oriented_zoned's own inputs plus the structure"
)]
pub(in super::super) fn maybe_scatter_oriented_zoned_bragg(
    medium: &OrientedMedium<'_>,
    structure: &BraggStructure,
    zoned: &ZonedCache,
    zone_alphas: &ZoneAlphas,
    ray: Ray,
    hit_t: f32,
    rng_seed: u32,
    bounce: u32,
    stokes: &mut [StokesVector; NUM_CHANNELS],
    path_pdf: &mut [f32; NUM_CHANNELS],
) -> Option<BraggScatter> {
    let hero = medium.hero_idx;
    let path_scale = medium.path_scale;
    // Per absorption-length unit, like the zone alphas.
    let sigma_s: [f32; NUM_CHANNELS] =
        std::array::from_fn(|k| medium.sigma_s(ray.dir, medium.lambdas[k]) / path_scale);
    let depth_to = |t: f32| -> [f32; NUM_CHANNELS] {
        let lengths = zoned.lengths_mm(ray.origin, ray.dir, 0.0, t, path_scale);
        optical_depths_spectral(zone_alphas, &lengths, &sigma_s)
    };

    let dist_rand = stream_draw(rng_seed, bounce, DISTANCE_SAMPLE_STREAM);
    let one_minus_u = (1.0 - dist_rand).max(1e-7);
    let target = -(one_minus_u.ln());

    let depth_full = depth_to(hit_t);
    let t_homogeneous = zoned_hero_free_flight(&depth_to, hero, hit_t, target, depth_full[hero]);

    let track = track_bragg(
        structure,
        &TrackSegment {
            origin: ray.origin,
            dir: ray.dir,
            t_max_model: t_homogeneous.unwrap_or(hit_t).min(hit_t),
            lambdas: medium.lambdas,
            hero_idx: hero,
            rng_seed,
            bounce,
        },
    );

    match track.event {
        BraggEvent::Reflect { t_model, domain } => {
            // The zoned medium's survival to the reflection, relative to the hero's, times the
            // tracking ratio.
            let depth = depth_to(t_model);
            let weights = bragg_reflect_weights(&depth, hero, &track.null_weight);
            for k in 0..NUM_CHANNELS {
                let w = weights[k];
                stokes[k] = StokesVector::unpolarized(stokes[k].intensity() * w);
                path_pdf[k] *= w;
            }
            let u1 = draw(rng_seed, bounce, BRAGG_MOSAIC_U_STREAM, 0);
            let u2 = draw(rng_seed, bounce, BRAGG_MOSAIC_V_STREAM, 0);
            let mut new_dir = bragg_reflect(ray.dir, domain.normal, structure.mosaic, u1, u2);
            if new_dir == Vec3::ZERO {
                new_dir = ray.dir;
            }
            Some(BraggScatter {
                scatter: OrientedScatter {
                    t_free_model: t_model,
                    new_dir,
                    phase_ratio: [1.0; NUM_CHANNELS],
                    hero_phase: 0.0,
                },
                bragg: true,
            })
        }
        BraggEvent::Pass => {
            let event = maybe_scatter_oriented_zoned(
                medium,
                zoned,
                zone_alphas,
                ray,
                hit_t,
                rng_seed,
                bounce,
                stokes,
                path_pdf,
            );
            for k in 0..NUM_CHANNELS {
                stokes[k] = stokes[k].scale(track.null_weight[k]);
                path_pdf[k] *= track.null_weight[k];
            }
            event.map(|scatter| BraggScatter {
                scatter,
                bragg: false,
            })
        }
    }
}
