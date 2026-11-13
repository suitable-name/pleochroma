//! Per-bounce facet dispatch and Russian-roulette/mode-coupling machinery.
//!
//! Accumulating environment radiance for an escaped ray, dispatching one bounce's
//! TIR/partial-reflect/refract/frosted event, the survival-weighted Russian-roulette
//! termination draw, and the stochastic o<->e (uniaxial) / mode-A<->mode-B (biaxial)
//! re-coupling at an internal reflection.

use super::super::{
    NUM_CHANNELS,
    camera::{FacetFinish, Ray},
    environment::{environment_nee_pdf, sample_environment_channels_at},
    refraction::{
        BounceContext, BounceRay, BounceRefractionGeometry, BounceState, ExitEvent, ExitSplitCtx,
        PathModeState, RayMaterialContext, RayWavelengthCache, RngDraw, apply_collect_bounce,
        apply_partial_fresnel_bounce, apply_tir_bounce, context::ExitCollect,
        entry_eigenmode_selection, poynting_dir_for_mode,
    },
    sampling::{MODE_COUPLING_STREAM, RUSSIAN_ROULETTE_STREAM, discrete_draw},
    scattering::{NeeContext, balance_heuristic},
    surface::{GroundParams, apply_ground_bounce},
};
use crate::optics::polarization::StokesVector;
use glam::Vec3;

/// Adds the environment radiance seen by a ray that left the gemstone or missed it. The
/// escaping ray's origin (the camera origin at bounce 0) locates the illuminated paper for a
/// downward escape.
///
/// The `StudioRig` is built once per trace in the `exit` context, since building it per escape
/// is too costly. All channels look along the same `ray_dir`, so the wavelength-independent
/// lighting is evaluated once and each channel applies only its own spectral power.
///
/// `exit.observer` is the unit direction from the stone towards the eye, for the lit lighting
/// models' head shadow (see `environment::sample_studio_environment_observed`).
///
/// `mis_weight` scales every channel uniformly. `1.0` leaves the bare environment value bit
/// for bit and is passed whenever NEE is off. A value below `1.0` is the balance-heuristic
/// weight of a phase- or BSDF-sampled continuation that follows an NEE-eligible scatter (see
/// `trace_spectral_ray_inner`'s call site).
pub(super) fn accumulate_miss_radiance(
    exit: &ExitSplitCtx<'_>,
    ray_origin: Vec3,
    ray_dir: Vec3,
    lambdas: &[f32; NUM_CHANNELS],
    stokes: &[StokesVector; NUM_CHANNELS],
    mis_weight: f32,
    radiance: &mut [f32; NUM_CHANNELS],
) {
    let env_spectral = sample_environment_channels_at(
        exit.environment,
        ray_origin,
        ray_dir,
        lambdas,
        exit.studio_rig.as_ref(),
        exit.observer,
    );
    for k in 0..NUM_CHANNELS {
        // `StokesVector::intensity` clamps `I` to >= 0 before it reaches the
        // environment sample, matching `spectral_transport.wgsl`'s equivalent clamp at
        // its miss/environment-lookup site -- negative `I` is unphysical either side.
        radiance[k] = (stokes[k].intensity() * mis_weight).mul_add(env_spectral[k], radiance[k]);
    }
}

/// Dispatches one bounce's event: TIR, partial reflect or refract, or for a
/// `FacetFinish::Frosted` facet the ground lobe of `apply_ground_bounce`. It then applies
/// internal mode re-coupling if the event was an internal reflection.
///
/// `was_internal_reflection` is `true` for a TIR bounce, which is only reachable from inside
/// the gem (`n1 > n2` for the hero channel), and `false` for any event that transmits, since
/// those always flip `inside_gem`.
///
/// `current_k` is the wave normal `k` (see the `refraction` module docs, wave normal versus
/// Poynting direction). It is read as the incoming `k` and updated in place to the outgoing
/// `k'`, alongside `current_ray.dir` (`S'`). A `Frosted` bounce sets `k' == S'`: a diffuse
/// bounce depolarizes like a Henyey-Greenstein scatter, so no wave-normal distinction remains.
///
/// Returns the `pending_light_mis` carry: `Some` for a `Frosted` NEE-eligible outcome (see the
/// "Sign convention for NEE eligibility" section of `apply_frosted_bounce`) or for a polished
/// transmit out that passes an incoming carry through. The caller stores it like the
/// Henyey-Greenstein carry of `try_scatter_step`.
///
/// `collect` is the exit-collection estimator's state (see [`ExitCollect`]). With
/// `collect.active == false` the dispatch is the branching one. With it, a polished event is
/// traced by `apply_collect_bounce` (the transmitted leaf is collected, the reflection is
/// followed undivided), and a forced total internal reflection keeps its branch factors out of
/// `path_pdf`. Collection is active only for a stone without tools, so a collected leaf never
/// re-enters the stone.
#[expect(
    clippy::too_many_arguments,
    reason = "bundles every value fixed for the trace (ctx) or the bounce (cache, geo) \
              into context structs; the rest are this bounce's scalar inputs, the RNG \
              stream identity, and the per-ray state a bounce can mutate (stokes, \
              path_pdf, current_ray, current_k, inside_gem, is_extraordinary) -- \
              collapsing those into one struct would hide which fields each bounce kind \
              touches behind an opaque &mut; `exit` is the per-trace `ExitSplitCtx` \
              (see its doc comment), used only by the apply_partial_fresnel_bounce \
              branch -- the frosted/TIR branches never reach an exit event; `nee`/ \
              `lambdas`/`radiance` are the HDR-map NEE inputs/output, used only by \
              the frosted branch, mirroring try_scatter_step's identical addition for \
              the Henyey-Greenstein path; `incoming_light_mis` is the light-sample MIS \
              carry, \
              threaded through so the partial-Fresnel arm's transmit-out case can return \
              it unchanged rather than silently dropping it"
)]
pub(super) fn dispatch_bounce(
    ctx: &RayMaterialContext,
    cache: &RayWavelengthCache,
    geo: &BounceRefractionGeometry,
    hit_point: Vec3,
    normal: Vec3,
    current_plane_normal: Vec3,
    (finish, ground): (FacetFinish, GroundParams),
    rng_seed: u32,
    bounce: u32,
    stokes: &mut [StokesVector; NUM_CHANNELS],
    path_pdf: &mut [f32; NUM_CHANNELS],
    exit: &mut ExitSplitCtx<'_>,
    current_ray: &mut Ray,
    current_k: &mut Vec3,
    inside_gem: &mut bool,
    is_extraordinary: &mut bool,
    nee: NeeContext<'_>,
    lambdas: &[f32; NUM_CHANNELS],
    radiance: &mut [f32; NUM_CHANNELS],
    incoming_light_mis: Option<(f32, Vec3)>,
    collect: &mut ExitCollect<'_>,
) -> Option<(f32, Vec3)> {
    // `*inside_gem` read below is the PRE-event value, since it is only written after the
    // event. `dispatch_polished_bounce` reads the same value through `mode_state.inside_gem`.
    let (new_k, new_s, new_inside_gem, is_extraordinary_update, exact_p_o, bounce_light_mis) =
        if finish == FacetFinish::Frosted {
            let (new_dir, new_inside_gem, is_extraordinary_update, frosted_light_mis) =
                apply_ground_bounce(
                    ctx,
                    geo,
                    normal,
                    (*current_k, hit_point),
                    ground,
                    *inside_gem,
                    (rng_seed, bounce),
                    stokes,
                    path_pdf,
                    nee,
                    lambdas,
                    radiance,
                );
            (
                new_dir,
                new_dir,
                new_inside_gem,
                is_extraordinary_update,
                None,
                // The frosted carry pairs with `new_dir`, already the exterior propagation
                // direction, so it equals `current_ray.dir` at a later escape. The incoming
                // carry is dropped: this bounce's own carry replaces it.
                frosted_light_mis.map(|phase_pdf| (phase_pdf, new_dir)),
            )
        } else if geo.sin2_t > 1.0 {
            let densities_before = *path_pdf;
            let (k_prime, exact_p_o) = apply_tir_bounce(
                ctx,
                geo,
                *is_extraordinary,
                *current_k,
                normal,
                stokes,
                path_pdf,
            );
            if collect.active {
                // The collected chain is followed with probability one: no branch factor.
                *path_pdf = densities_before;
            }
            let s_prime = poynting_dir_for_mode(ctx, geo, k_prime, *inside_gem, *is_extraordinary);
            // TIR is an internal reflection, so the incoming carry has no competing NEE
            // sample and is dropped.
            (k_prime, s_prime, *inside_gem, None, exact_p_o, None)
        } else {
            let bctx = BounceContext { ctx, cache, geo };
            let ray = BounceRay {
                k_hat: *current_k,
                normal,
            };
            let mode_state = PathModeState {
                current_plane_normal,
                inside_gem: *inside_gem,
                is_extraordinary: *is_extraordinary,
            };
            let rng = RngDraw { rng_seed, bounce };
            let mut state = BounceState { stokes, path_pdf };
            let mut exit_event = ExitEvent { exit, hit_point };
            dispatch_polished_bounce(
                &bctx,
                ray,
                mode_state,
                rng,
                &mut state,
                &mut exit_event,
                (nee, incoming_light_mis, collect),
            )
        };
    let was_internal_reflection =
        *inside_gem && is_extraordinary_update.is_none() && new_inside_gem == *inside_gem;
    current_ray.dir = new_s;
    current_ray.origin = hit_point + new_s * 1e-4;
    *current_k = new_k;
    *inside_gem = new_inside_gem;
    if let Some(updated) = is_extraordinary_update {
        *is_extraordinary = updated;
    }
    // `current_k` and `stokes` are the post-event state the re-coupling weighs by
    // polarization. `exact_p_o` is the closed-form `R_o/(R_o+R_e)` split of an internal
    // reflection; it is `None` for a frosted bounce, a biaxial material or a transmit, where
    // the fallback heuristic applies.
    maybe_apply_internal_mode_coupling(
        ctx,
        geo.is_biaxial,
        current_plane_normal,
        *current_k,
        stokes,
        was_internal_reflection,
        exact_p_o,
        rng_seed,
        bounce,
        is_extraordinary,
    );
    bounce_light_mis
}

/// The return type of [`dispatch_polished_bounce`]: the five-tuple of
/// `apply_partial_fresnel_bounce` plus the transmit-out `bounce_light_mis` carry. Named so that
/// clippy's `type_complexity` lint stays quiet.
type PolishedBounceResult = (
    Vec3,
    Vec3,
    bool,
    Option<bool>,
    Option<f32>,
    Option<(f32, Vec3)>,
);

/// The non-frosted, non-forced-TIR arm of [`dispatch_bounce`]: the partial Fresnel dispatch
/// (scalar or closed-form uniaxial), or [`apply_collect_bounce`] when collection is active.
/// The context structs `apply_partial_fresnel_bounce` takes are built here, with one tuple for
/// the values it does not need (`nee`, the incoming light-MIS carry and `collect`). That keeps
/// the argument count under clippy's limit.
///
/// With `collect.active` the event is traced by `apply_collect_bounce` and returns. NEE is off
/// whenever collection is active, so a collected event has no light-sample MIS weight and
/// returns no carry.
///
/// `mode_state.inside_gem` is the PRE-event value. The return is the five-tuple of
/// `apply_partial_fresnel_bounce` plus the transmit-out carry (`None` for every other outcome).
fn dispatch_polished_bounce(
    bctx: &BounceContext<'_, '_>,
    ray: BounceRay,
    mode_state: PathModeState,
    rng: RngDraw,
    state: &mut BounceState<'_>,
    exit_event: &mut ExitEvent<'_, '_>,
    nee_carry_collect: (NeeContext<'_>, Option<(f32, Vec3)>, &mut ExitCollect<'_>),
) -> PolishedBounceResult {
    let (nee, incoming_light_mis, collect) = nee_carry_collect;
    if collect.active {
        let (k_prime, s_prime, new_inside_gem, is_extraordinary_update, exact_p_o) =
            apply_collect_bounce(bctx, ray, mode_state, rng, state, exit_event, collect);
        return (
            k_prime,
            s_prime,
            new_inside_gem,
            is_extraordinary_update,
            exact_p_o,
            None,
        );
    }
    let pre_bounce_inside_gem = mode_state.inside_gem;
    // Only a transmit out of the gem can reach `try_split_exit_channel` with a live carry (see
    // `ExitSplitCtx::split_mis_weight`). The partial-Fresnel dispatch below decides whether the
    // hero transmits, but a transmit from inside always leaves the gem. So the weight can be
    // computed from `pre_bounce_inside_gem` before the reflect/transmit draw, and it is exact.
    // It is the same weight `handle_miss` gives a hero escape: the balance heuristic with
    // `environment_nee_pdf` at the carried INTERIOR direction.
    exit_event.exit.split_mis_weight = if pre_bounce_inside_gem {
        incoming_light_mis.map_or(1.0, |(phase_pdf, interior_dir)| {
            balance_heuristic(
                phase_pdf,
                environment_nee_pdf(nee.environment, interior_dir),
            )
        })
    } else {
        1.0
    };
    let (k_prime, s_prime, new_inside_gem, is_extraordinary_update, exact_p_o) =
        apply_partial_fresnel_bounce(bctx, ray, mode_state, rng, state, exit_event);
    // Reset immediately after dispatch -- `split_mis_weight` must never leak into
    // a later bounce's own (unrelated) split contributions.
    exit_event.exit.split_mis_weight = 1.0;
    // A pending NEE carry survives only a genuine transmit out: the ray started inside, the
    // outcome is exterior, and there is no entry-mode update. A reflection never changes
    // `inside_gem`, and an air->crystal entry transmit never has a carry pending.
    let transmit_out_of_gem =
        pre_bounce_inside_gem && !new_inside_gem && is_extraordinary_update.is_none();
    let bounce_light_mis = if transmit_out_of_gem {
        incoming_light_mis
    } else {
        None
    };
    (
        k_prime,
        s_prime,
        new_inside_gem,
        is_extraordinary_update,
        exact_p_o,
        bounce_light_mis,
    )
}

/// Russian roulette with weighted survival. A hard cutoff on dim paths biases the estimator
/// dark, worst on long internal bounce trains inside high-index stones. Instead the path
/// survives with probability `q` (the largest channel intensity, clamped), and on survival
/// every Stokes vector is divided by `q`, so `E[survive] * (1/q) = 1` and the estimator stays
/// unbiased. Returns `false` if the path terminates, `true` if it survives (Stokes vectors
/// rescaled in place).
///
/// `split_radiance` takes the same `1/q` rescale. A channel that split off at an earlier
/// bounce stages its contribution there, and that is committed to `radiance` only if the hero
/// path survives every later draw, so it must be rescaled with them.
///
/// `pub(in super::super)`: `scattering`'s `try_scatter_step` calls this for its own trailing
/// roulette draw, from outside `transport`.
pub(in super::super) fn apply_russian_roulette(
    bounce: u32,
    rng_seed: u32,
    stokes: &mut [StokesVector; NUM_CHANNELS],
    split_radiance: &mut [f32; NUM_CHANNELS],
) -> bool {
    let max_intensity = stokes.iter().fold(0.0f32, |a, s| a.max(s.intensity()));
    let q = max_intensity.clamp(0.05, 1.0);
    let rr_rand =
        (discrete_draw(rng_seed, bounce, RUSSIAN_ROULETTE_STREAM) as f32) / 4_294_967_295.0;
    if rr_rand > q {
        return false;
    }
    for s in &mut *stokes {
        *s = s.scale(1.0 / q);
    }
    for r in &mut *split_radiance {
        *r /= q;
    }
    true
}

/// The weight below which the exit-collection estimator plays Russian roulette.
///
/// Above it the largest channel intensity is a physical weight that the chain carries as it is.
const COLLECT_ROULETTE_FLOOR: f32 = 0.02;

/// Russian roulette for the exit-collection estimator.
///
/// [`apply_russian_roulette`] would fire on most collected paths: their largest intensity is a
/// physical weight, and a white-noise draw on it gives back the variance the collection removed.
/// Here nothing happens while the largest intensity is at least [`COLLECT_ROULETTE_FLOOR`].
/// Below it the path survives with probability `intensity / floor`, and every Stokes vector is
/// divided by that probability, which keeps the estimator unbiased. The draw uses the roulette
/// stream. Returns `false` if the path terminates.
///
/// `split_radiance` is rescaled as in [`apply_russian_roulette`]; the collected leaves are final
/// and are not rescaled.
pub(super) fn apply_collect_roulette(
    bounce: u32,
    rng_seed: u32,
    stokes: &mut [StokesVector; NUM_CHANNELS],
    split_radiance: &mut [f32; NUM_CHANNELS],
) -> bool {
    let max_intensity = stokes.iter().fold(0.0f32, |a, s| a.max(s.intensity()));
    if max_intensity >= COLLECT_ROULETTE_FLOOR {
        return true;
    }
    let q = max_intensity / COLLECT_ROULETTE_FLOOR;
    let rr_rand =
        (discrete_draw(rng_seed, bounce, RUSSIAN_ROULETTE_STREAM) as f32) / 4_294_967_295.0;
    if q <= 0.0 || rr_rand > q {
        return false;
    }
    for s in &mut *stokes {
        *s = s.scale(1.0 / q);
    }
    for r in &mut *split_radiance {
        *r /= q;
    }
    true
}

/// Stochastic o<->e (uniaxial) / mode-A<->mode-B (biaxial) re-coupling at an internal
/// reflection inside an anisotropic crystal.
///
/// # What this models
///
/// A path takes its eigenmode once, at the air->crystal entry. A uniaxial eigenbasis is
/// defined relative to the local wave normal, so a facet with another orientation has another
/// basis, and light partly converts between the two labels at every internal bounce. That
/// conversion contributes to the doubling of high-birefringence stones (zircon, moissanite).
/// This function models it as a fresh draw of which mode's index governs the path from here.
/// The draw uses the exact closed-form share when the caller has one, the polarization-weighted
/// `entry_eigenmode_selection` share for a uniaxial reflection without one, and 50/50 for
/// biaxial materials. It is not a per-bounce probability from the angle between the old and new
/// eigenbases, which would need anisotropic Fresnel coefficients at an anisotropic interface. A
/// long TIR train therefore mixes o-like and e-like propagation instead of freezing at its
/// entry mode.
///
/// # Why no path weight
///
/// The path keeps one Stokes vector and one `path_pdf`. This function only re-labels which
/// eigenmode governs the next bounce, so it applies no `1/p` weight, and the selection
/// probability is a modelling choice rather than an importance-sampling density. The exact
/// split `R_o/(R_o+R_e)` is the hero channel's share, from this bounce's own `internal_solve`.
/// The companion channels use it too, although their own shares differ slightly
/// (`docs/physics.md` section 2.4). Biaxial materials have no closed-form uniaxial solve and
/// keep 50/50.
///
/// `path_pdf` is not multiplied by `0.5` per draw. Across a long internal-bounce train (tens to
/// about 100 reflections) `0.5^n` underflows to zero, which would zero the path's spectral MIS
/// weight.
///
/// Returns the freshly-selected `is_extraordinary`. The caller applies it only when the internal
/// reflection happened while `inside_gem && is_anisotropic`.
///
/// `pub(super)`: `transport::tests::mode_coupling_tests` checks this directly against the exact
/// closed-form split.
#[expect(
    clippy::too_many_arguments,
    reason = "thin re-labeling draw: ctx/is_biaxial/current_plane_normal/new_k/stokes \
              are this function's own inputs (see its doc comment), exact_p_o is the \
              closed-form override of the same probability, plus the RNG stream identity"
)]
pub(super) fn apply_internal_mode_coupling(
    ctx: &RayMaterialContext,
    is_biaxial: bool,
    current_plane_normal: Vec3,
    new_k: Vec3,
    stokes: &[StokesVector; NUM_CHANNELS],
    exact_p_o: Option<f32>,
    rng_seed: u32,
    bounce: u32,
) -> bool {
    let p_o = exact_p_o.unwrap_or_else(|| {
        if is_biaxial {
            None
        } else {
            entry_eigenmode_selection(
                ctx.c_axis,
                current_plane_normal,
                new_k,
                stokes[ctx.hero_idx],
            )
        }
        .map_or(0.5, |(p, ..)| p)
    });
    let split_rand =
        (discrete_draw(rng_seed, bounce, MODE_COUPLING_STREAM) as f32) / 4_294_967_295.0;
    split_rand >= p_o
}

/// Applies [`apply_internal_mode_coupling`] to `is_extraordinary` only when the bounce that
/// just ran was an internal reflection inside an anisotropic crystal; a no-op otherwise.
/// `was_internal_reflection` is `true` for a TIR bounce, and for a partial Fresnel bounce
/// that neither transmits nor changes `inside_gem`.
#[expect(
    clippy::too_many_arguments,
    reason = "thin dispatcher for apply_internal_mode_coupling's own inputs plus the \
              was_internal_reflection gate and RNG stream identity"
)]
fn maybe_apply_internal_mode_coupling(
    ctx: &RayMaterialContext,
    is_biaxial: bool,
    current_plane_normal: Vec3,
    new_k: Vec3,
    stokes: &[StokesVector; NUM_CHANNELS],
    was_internal_reflection: bool,
    exact_p_o: Option<f32>,
    rng_seed: u32,
    bounce: u32,
    is_extraordinary: &mut bool,
) {
    if ctx.enable_internal_mode_coupling && ctx.is_anisotropic && was_internal_reflection {
        *is_extraordinary = apply_internal_mode_coupling(
            ctx,
            is_biaxial,
            current_plane_normal,
            new_k,
            stokes,
            exact_p_o,
            rng_seed,
            bounce,
        );
    }
}
