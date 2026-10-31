//! Spectral delta tracking through a Bragg domain structure (opal, labradorite).
//!
//! The reflection coefficient `sigma_B(x, w, lambda)` of a [`BraggStructure`] varies in space (the
//! domain map) and with the wavelength (a narrow band around the Bragg wavelength of the domain
//! the ray is in). Distance sampling by inversion is impossible, so the free flight is sampled by
//! delta tracking (Kutz et al. 2017) with the hero wavelength:
//!
//! * a majorant `sigma_bar >= sigma_B,k(x)` for every channel and place (the structure's
//!   [`BraggStructure::majorant`]) drives tentative collisions along the segment;
//! * at a tentative collision at `x`, the hero accepts a real reflection with probability
//!   `sigma_B,hero(x) / sigma_bar`;
//! * each channel `k` owes the ratio of its own collision probability to the hero's: a real
//!   reflection carries `sigma_B,k / sigma_B,hero`, a null collision
//!   `(1 - sigma_B,k / sigma_bar) / (1 - sigma_B,hero / sigma_bar)`. Both are non-negative
//!   because `sigma_bar` is a true majorant.
//!
//! The product of those ratios over a tentative-collision sequence is the ratio of the sequence's
//! probability under channel `k`'s own technique to the hero's, which is exactly what the spectral
//! multiple importance sampling needs in `path_pdf`: `path_pdf[k]` is multiplied by the same
//! ratio (the hero's own factor is one).
//!
//! # Composition with the haze and the families
//!
//! The homogeneous part of the medium (the absorption, the Henyey-Greenstein haze and the silk of
//! [`super::oriented`]) keeps its own estimator. The two are independent Poisson processes, so
//! they superpose: the homogeneous free flight `t_h` is drawn first (the same draw
//! [`maybe_scatter_oriented`] makes), tracking runs over `[0, min(t_h, hit_t))`, and whichever
//! event comes first wins. A reflection at `t < min(t_h, hit_t)` weighs each channel by
//! `exp(-(c_k - c_hero) t)` for the homogeneous extinction `c_k` times its tracking ratio. When
//! tracking finds nothing, [`maybe_scatter_oriented`] decides the homogeneous event as before and
//! the product of the null-collision ratios multiplies the result.
//!
//! In a zoned host the homogeneous part is the zoned estimator
//! ([`super::oriented_zoned::maybe_scatter_oriented_zoned`]); the zones change only the absorption,
//! so the same composition holds with the zoned optical depth in place of the linear one
//! ([`bragg_reflect_weights`], [`super::oriented_zoned::maybe_scatter_oriented_zoned_bragg`]).
//!
//! A reflection is a specular event with a mosaic lobe: it takes no scattering-point next-event
//! estimate and no phase-function MIS partner. It depolarizes, like every scattering event here.
//!
//! # Randoms
//!
//! Tentative collision `i` draws its distance and its decision from [`BRAGG_STEP_STREAM`] and
//! [`BRAGG_ACCEPT_STREAM`] at index `i`; the mosaic perturbation uses [`BRAGG_MOSAIC_U_STREAM`]
//! and [`BRAGG_MOSAIC_V_STREAM`]. At most [`MAX_TENTATIVE_COLLISIONS`] collisions are followed; a
//! segment that would need more is treated as passed (the cap is far above the expected count
//! `sigma_bar * L` of any preset).

use super::{
    super::{
        NUM_CHANNELS,
        camera::Ray,
        sampling::{DISTANCE_SAMPLE_STREAM, hash_u32},
    },
    oriented::{OrientedMedium, OrientedScatter, maybe_scatter_oriented},
};
use crate::optics::{
    inclusions::{
        bragg::{BraggStructure, bragg_reflect},
        domains::{Domain, domain_at},
    },
    polarization::StokesVector,
};
use glam::Vec3;

/// The RNG stream of the tentative-collision distances.
pub(crate) const BRAGG_STEP_STREAM: u32 = 0xB5AD_4ECE;
/// The RNG stream of the accept-or-null decisions.
pub(crate) const BRAGG_ACCEPT_STREAM: u32 = 0xE3D2_8F51;
/// The RNG stream of the mosaic perturbation's tilt.
pub(crate) const BRAGG_MOSAIC_U_STREAM: u32 = 0x1D8E_4E27;
/// The RNG stream of the mosaic perturbation's azimuth.
pub(crate) const BRAGG_MOSAIC_V_STREAM: u32 = 0x8CC7_0208;

/// Most tentative collisions followed along one segment.
pub(crate) const MAX_TENTATIVE_COLLISIONS: u32 = 1024;

/// The unit random of `stream` at `(rng_seed, bounce)` and tentative-collision index `index`,
/// limited to `[0, 1)`.
pub(crate) fn draw(rng_seed: u32, bounce: u32, stream: u32, index: u32) -> f32 {
    let salt = hash_u32(index.wrapping_mul(0x9E37_79B9).wrapping_add(stream));
    let u = (hash_u32(rng_seed ^ hash_u32(bounce ^ salt)) as f32) / 4_294_967_295.0;
    u.min(1.0 - 1e-6)
}

/// What tracking found along a segment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum BraggEvent {
    /// The hero accepted a real reflection `t_model` along the segment, in `domain`.
    Reflect {
        /// Distance along the segment, model units.
        t_model: f32,
        /// The domain that reflects.
        domain: Domain,
    },
    /// No reflection before the end of the segment.
    Pass,
}

/// The result of tracking one segment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct BraggTrack {
    /// The event.
    pub(crate) event: BraggEvent,
    /// Per channel, the product of the null-collision ratios before the event (all ones for the
    /// hero). A reflection's own ratio is not included.
    pub(crate) null_weight: [f32; NUM_CHANNELS],
}

/// The segment tracked and the hero comb that drives it.
#[derive(Clone, Copy)]
pub(crate) struct TrackSegment<'a> {
    /// Where the segment starts, model units.
    pub(crate) origin: Vec3,
    /// The unit propagation direction.
    pub(crate) dir: Vec3,
    /// Where tracking stops, model units along `dir`.
    pub(crate) t_max_model: f32,
    /// The ray's eight wavelengths, nanometres.
    pub(crate) lambdas: &'a [f32; NUM_CHANNELS],
    /// Which channel drives the shared path.
    pub(crate) hero_idx: usize,
    /// The RNG stream identity.
    pub(crate) rng_seed: u32,
    /// The RNG stream identity.
    pub(crate) bounce: u32,
}

/// Tracks `segment` through `structure`: the first real reflection the hero accepts, and the
/// null-collision ratios of every channel before it.
#[must_use]
pub(crate) fn track_bragg(structure: &BraggStructure, segment: &TrackSegment<'_>) -> BraggTrack {
    let sigma_bar = structure.majorant();
    let hero = segment.hero_idx;
    let mut weight = [1.0f32; NUM_CHANNELS];
    let mut t = 0.0f32;
    for i in 0..MAX_TENTATIVE_COLLISIONS {
        let step = draw(segment.rng_seed, segment.bounce, BRAGG_STEP_STREAM, i);
        t += -(1.0 - step).ln() / sigma_bar;
        if t >= segment.t_max_model {
            break;
        }
        let domain = domain_at(structure, segment.origin + segment.dir * t);
        let cos_theta = segment.dir.dot(domain.normal).abs();
        let sigma: [f32; NUM_CHANNELS] =
            std::array::from_fn(|k| structure.sigma(&domain, cos_theta, segment.lambdas[k]));
        let p_hero = (sigma[hero] / sigma_bar).clamp(0.0, 1.0);
        if draw(segment.rng_seed, segment.bounce, BRAGG_ACCEPT_STREAM, i) < p_hero {
            weight[hero] = 1.0;
            // The real reflection's own ratio, `sigma_k / sigma_hero`.
            for k in 0..NUM_CHANNELS {
                if k != hero {
                    weight[k] *= sigma[k] / sigma[hero];
                }
            }
            return BraggTrack {
                event: BraggEvent::Reflect { t_model: t, domain },
                null_weight: weight,
            };
        }
        let null_hero = (1.0 - p_hero).max(1e-6);
        for k in 0..NUM_CHANNELS {
            weight[k] *= (1.0 - sigma[k] / sigma_bar).max(0.0) / null_hero;
        }
        weight[hero] = 1.0;
    }
    BraggTrack {
        event: BraggEvent::Pass,
        null_weight: weight,
    }
}

/// The per-channel weight of a Bragg reflection in a host whose optical depth is not linear in
/// the distance (a zoned host): `exp(-(depth_k - depth_hero))` times the tracking ratio
/// `null_weight[k]`.
///
/// `depth[k]` is channel `k`'s optical depth (absorption plus inclusion extinction, per
/// absorption-length unit already multiplied by the path length) from the segment start to the
/// reflection, which the zoned estimator reads from `ZonedCache::lengths_mm` and
/// `optical_depths_spectral`. For one covering zone the difference is
/// `((alpha_k + sigma_k) - (alpha_hero + sigma_hero)) * t_model * path_scale`, the argument of
/// the homogeneous arm of [`maybe_scatter_oriented_bragg`]. That arm keeps its own spelling of
/// the argument (a product of the rate difference and the distance, not a difference of
/// products), so it does not call this function: the two agree to rounding, not bitwise.
#[must_use]
pub(crate) fn bragg_reflect_weights(
    depth: &[f32; NUM_CHANNELS],
    hero: usize,
    null_weight: &[f32; NUM_CHANNELS],
) -> [f32; NUM_CHANNELS] {
    let mut args = [0f32; NUM_CHANNELS];
    for k in 0..NUM_CHANNELS {
        args[k] = -(depth[k] - depth[hero]);
    }
    let survival = crate::simd::exp_f32x8(args);
    std::array::from_fn(|k| survival[k] * null_weight[k])
}

/// What [`maybe_scatter_oriented_bragg`] found.
pub(crate) struct BraggScatter {
    /// The scattering event, in the same shape as [`maybe_scatter_oriented`]'s.
    pub(crate) scatter: OrientedScatter,
    /// Whether the event is a Bragg reflection (no scattering-point NEE, no phase MIS) rather
    /// than a homogeneous scatter.
    pub(crate) bragg: bool,
}

/// [`maybe_scatter_oriented`] for a medium that also holds a Bragg domain structure.
///
/// `Some` if an event fired strictly before `hit_t`; `None` if the path survived to the facet
/// (`stokes` and `path_pdf` then carry the survival weight, null collisions included). `ray` is
/// the segment's start and direction, which tracking needs and the homogeneous estimator does
/// not. See the module documentation for the composition.
#[expect(
    clippy::too_many_arguments,
    reason = "maybe_scatter_oriented's own per-ray inputs plus the structure and the ray origin"
)]
pub(crate) fn maybe_scatter_oriented_bragg(
    medium: &OrientedMedium<'_>,
    structure: &BraggStructure,
    alphas: &[f32; NUM_CHANNELS],
    ray: Ray,
    hit_t: f32,
    rng_seed: u32,
    bounce: u32,
    stokes: &mut [StokesVector; NUM_CHANNELS],
    path_pdf: &mut [f32; NUM_CHANNELS],
) -> Option<BraggScatter> {
    let hero = medium.hero_idx;
    let path_scale = medium.path_scale;
    // The homogeneous extinction per channel, per absorption-length unit, and the hero's free
    // flight with the very draw `maybe_scatter_oriented` makes.
    let sigma_s: [f32; NUM_CHANNELS] =
        std::array::from_fn(|k| medium.sigma_s(ray.dir, medium.lambdas[k]) / path_scale);
    let sigma_t_hero = (alphas[hero] + sigma_s[hero]).max(1e-12);
    let dist_rand =
        (hash_u32(rng_seed ^ hash_u32(bounce ^ DISTANCE_SAMPLE_STREAM)) as f32) / 4_294_967_295.0;
    let t_homogeneous_model = -((1.0 - dist_rand).max(1e-7).ln()) / sigma_t_hero / path_scale;

    let track = track_bragg(
        structure,
        &TrackSegment {
            origin: ray.origin,
            dir: ray.dir,
            t_max_model: t_homogeneous_model.min(hit_t),
            lambdas: medium.lambdas,
            hero_idx: hero,
            rng_seed,
            bounce,
        },
    );

    match track.event {
        BraggEvent::Reflect { t_model, domain } => {
            // The homogeneous medium's survival to the reflection, relative to the hero's.
            let t_abs = t_model * path_scale;
            let mut args = [0f32; NUM_CHANNELS];
            for k in 0..NUM_CHANNELS {
                args[k] = -((alphas[k] + sigma_s[k]) - (alphas[hero] + sigma_s[hero])) * t_abs;
            }
            let survival = crate::simd::exp_f32x8(args);
            for k in 0..NUM_CHANNELS {
                let w = survival[k] * track.null_weight[k];
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
            let event = maybe_scatter_oriented(
                medium, alphas, ray.dir, hit_t, rng_seed, bounce, stokes, path_pdf,
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
