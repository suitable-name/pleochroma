//! Per-ray/per-sample/per-bounce context types shared by every bounce-dispatch function
//! in this module tree.
//!
//! Plus [`narrow_compat`], the MIS-family narrowing step those functions share at an
//! interior dispersive event.

use super::{directions_match, geometry::BounceRefractionGeometry};
use crate::optics::{
    birefringence::{AbsorptionTensor3, BiaxialIndicatrix},
    materials::GemMaterial,
    polarization::StokesVector,
    raytracer::{NUM_CHANNELS, environment::EnvironmentSource, uniaxial_fresnel::UniaxialFrame},
    studio_rig::StudioRig,
};
use glam::Vec3;

/// Per-ray context fixed across every bounce of `trace_spectral_ray`'s main loop: the material,
/// the 8 hero-wavelength comb, the slot that drives the shared geometric path, the optical c-axis,
/// and whether the material is anisotropic. A struct so that
/// [`super::geometry::compute_bounce_refraction_geometry`] stays under clippy's
/// `too_many_arguments` limit.
pub(crate) struct RayMaterialContext<'a> {
    pub(crate) material: &'a GemMaterial,
    pub(crate) lambdas: [f32; NUM_CHANNELS],
    pub(crate) hero_idx: usize,
    pub(crate) c_axis: Vec3,
    pub(crate) is_anisotropic: bool,
    /// Whether `maybe_apply_internal_mode_coupling` is active for this path. See
    /// `trace_spectral_ray_inner`'s `enable_internal_mode_coupling` parameter.
    pub(crate) enable_internal_mode_coupling: bool,
}

/// Per-sample cache of quantities that depend only on the fixed 8-wavelength comb
/// (`ctx.lambdas`) and the material, built once by
/// [`super::wavelength_cache::build_ray_wavelength_cache`] and read every bounce. Separate from
/// [`RayMaterialContext`], which GPU self-test call sites build by struct literal.
pub(crate) struct RayWavelengthCache {
    /// `material.dispersion.evaluate(lambdas[k])` per channel. See
    /// [`super::geometry::per_channel_uniaxial_indices`] for the per-bounce `n_eff_ch`
    /// half that reads it back.
    pub(crate) n_o_ch: [f32; NUM_CHANNELS],
    /// `material.biaxial_indicatrix(lambdas[hero_idx])`. Always exactly
    /// `biaxial_ch[hero_idx]`; kept as its own field so call sites that only need the
    /// hero's own indicatrix don't have to index `biaxial_ch` themselves.
    pub(crate) hero_indicatrix: Option<BiaxialIndicatrix>,
    /// `material.biaxial_indicatrix(lambdas[k])` per channel.
    pub(crate) biaxial_ch: [Option<BiaxialIndicatrix>; NUM_CHANNELS],
    /// Per-channel [`AbsorptionTensor3`] (uniaxial or biaxial, matching whether
    /// `hero_indicatrix.is_some()`), built from this channel's own `alpha_o`/`alpha_e`/
    /// `alpha_beta` (each `spectral_absorption` at `lambdas[k]`) and the material's
    /// fixed `c_axis`. `channel_absorption_alphas` calls
    /// `birefringence::effective_pleochroic_alpha` against this cached tensor rather
    /// than rebuilding one (the axis frame + `Mat3`) from scratch every bounce.
    pub(crate) tensor_ch: [AbsorptionTensor3; NUM_CHANNELS],
    /// The zoned-absorption tables (kernel plus per-zone tensors); `Some` exactly when
    /// `material.zoning` is. See `raytracer::zoned`.
    pub(crate) zoned: Option<crate::optics::raytracer::zoned::ZonedCache>,
    /// The optics lookup tables of this render (`optics::tables`); `None` (the default) leaves
    /// every Fresnel and absorption site on its closed form, bit for bit.
    pub(crate) tables: Option<std::sync::Arc<crate::optics::tables::OpticsTableSet>>,
}

/// Per-trace context for exit-event spectral splitting -- see `refraction`'s top-of-file
/// "Exit-event spectral splitting" doc comment for the estimator this supports. The
/// fixed parts (`plane_soa`, `environment`, `studio_rig`, `enabled`) are bundled the
/// same way [`RayMaterialContext`]/[`RayWavelengthCache`] are; the two per-trace
/// accumulators (`split_radiance`, `compat`) are threaded as `&mut` through every
/// bounce-dispatch function exactly like `stokes`/`path_pdf` are. `radiance` itself
/// stays outside, threaded separately.
pub(in crate::optics::raytracer) struct ExitSplitCtx<'a> {
    /// The same intersection arena `trace_spectral_ray_inner`'s own bounce loop already
    /// built once per trace -- reused here for [`super::exit_split::try_split_exit_channel`]'s
    /// bounded "does channel k's own exit ray re-enter the gem" probe, never rebuilt.
    pub(in crate::optics::raytracer) plane_soa: &'a crate::simd::PlanesSoA32,
    /// Tools subtracted from the polyhedron `plane_soa` holds (empty when planar), so the
    /// re-entry probe sees the same stone the bounce loop traces.
    pub(in crate::optics::raytracer) tools: &'a [crate::geometry::tool::ToolPrimitive],
    pub(in crate::optics::raytracer) environment: EnvironmentSource<'a>,
    /// Built once per trace by `trace_spectral_ray_inner` and shared: the exit-split
    /// probes here and `accumulate_miss_radiance`'s escape lookup both borrow this one
    /// instance, since the rig depends only on the light pose, which is constant across
    /// an entire ray. `None` for [`EnvironmentSource::HdrMap`], which
    /// `sample_environment_channel` ignores.
    pub(in crate::optics::raytracer) studio_rig: Option<StudioRig>,
    /// Unit direction from the stone towards the eye -- the reverse of the pixel's
    /// primary ray -- for the lit lighting models' head shadow; see
    /// `environment::sample_studio_environment_observed`.
    pub(in crate::optics::raytracer) observer: Vec3,
    /// A staging accumulator, separate from `trace_spectral_ray_inner`'s own `radiance`. Every
    /// split channel's contribution lands here first, and it is folded into `radiance` only if
    /// the shared path terminates via [`PathTermination::Escaped`], as
    /// `accumulate_miss_radiance` does for a matching channel. Any other termination (Russian
    /// roulette, scatter absorption, `max_bounces`) discards it, so a split channel is not
    /// credited where a matching one is not.
    ///
    /// A bounce off the glossy tray (Tray Reflection) is also an environment lookup of the
    /// shared path: it commits this staging area into `radiance` and flushes both to the
    /// integrated `nee_xyz` (`PathState::flush_radiance_to_nee`), after which it is empty
    /// again. At an exit, a channel whose own direction, or the hero's, meets the tray stages
    /// nothing here (see [`super::exit_leaves_via_floor`]).
    pub(in crate::optics::raytracer) split_radiance: &'a mut [f32; NUM_CHANNELS],
    /// `false` gives chromatic termination without splitting, bit for bit at every exit event;
    /// `true` (the default of every public entry point) enables splitting. Threaded only as far
    /// as `trace_spectral_ray_inner`'s parameter, never exposed publicly.
    pub(in crate::optics::raytracer) enabled: bool,
    /// Pairwise spectral-compatibility mask -- bit `j` of `compat[c]` is set while
    /// channels `c` and `j` have refracted within `DIRECTION_MATCH_COS_TOL` of each
    /// other at every interior dispersive event so far (all bits set before the first
    /// one; `compat[c]` always keeps its own bit). `compat[c]` is channel c's MIS
    /// *family*: the set of techniques under which channel c would have stayed alive on
    /// this same geometric path, and therefore the set its balance-heuristic weight
    /// must be normalised over -- see this module's top-of-file doc comment. Narrowed
    /// by [`narrow_compat`]; read by `color::integrate_channels_to_xyz_families`. Only
    /// meaningful while `enabled`.
    pub(in crate::optics::raytracer) compat: [u8; NUM_CHANNELS],
    /// The balance-heuristic MIS weight [`super::exit_split::try_split_exit_channel`] applies to
    /// every channel it adds this bounce; `1.0` outside a transmit-out event with a live incoming
    /// NEE carry. Set by `transport::bounce::dispatch_bounce` before the dispatch, which may turn
    /// out to be such an event, and reset to `1.0` after it. A dispatch that reflects never reads
    /// it, since only the transmit branch reaches `try_split_exit_channel`.
    pub(in crate::optics::raytracer) split_mis_weight: f32,
    /// How each channel's refracted direction is matched against the hero's at the
    /// dispersive events; `DirectionMatch::default()` (the tolerance test) in production, and
    /// `Exact` for the beam tracer's reference tracer.
    pub(in crate::optics::raytracer) direction_match: super::DirectionMatch,
}

const _: () = assert!(
    NUM_CHANNELS <= 8,
    "ExitSplitCtx::compat packs one bit per channel into a u8"
);

/// The per-bounce state of the exit-collection estimator
/// ([`crate::optics::raytracer::Estimator`]).
///
/// A separate struct rather than fields of [`ExitSplitCtx`] (which a number of tests build with a
/// struct literal): the bounce loop builds one per facet event and hands it to
/// `transport::bounce::dispatch_bounce`. With `active == false` nothing here is read or written
/// and the event is the branching estimator's, bit for bit.
///
/// A collected leaf is a transmitted exit (or, at an entry, the mirror reflection) that is added
/// to [`Self::collected`] at once, with its spectral-MIS weight already applied, instead of
/// being carried on a branch; see `refraction::dispatch::apply_collect_bounce`. `collected` is
/// final: Russian roulette never rescales it, and the end of the trace integrates it with the
/// plain comb sum.
pub(in crate::optics::raytracer) struct ExitCollect<'a> {
    /// Whether this event is traced by exit collection.
    pub(in crate::optics::raytracer) active: bool,
    /// Whether a leaf of this event counts. The branching estimator only sees the escape of a
    /// branch in the next loop iteration, so an event of the last iteration earns nothing there;
    /// the collected leaf follows the same convention and the two estimators truncate alike.
    pub(in crate::optics::raytracer) credit: bool,
    /// Scale of the mirror reflection at an entry: the surface-glare factor at bounce 0, else 1.
    pub(in crate::optics::raytracer) entry_leaf_scale: f32,
    /// The weighted leaf radiance per channel.
    pub(in crate::optics::raytracer) collected: &'a mut [f32; NUM_CHANNELS],
}

/// The three read-only references (fixed per trace, per sample and per bounce) that every
/// isotropic and biaxial bounce-dispatch function needs. Each field is the caller's own reference.
pub(in crate::optics::raytracer) struct BounceContext<'m, 'b> {
    pub(in crate::optics::raytracer) ctx: &'b RayMaterialContext<'m>,
    pub(in crate::optics::raytracer) cache: &'b RayWavelengthCache,
    pub(in crate::optics::raytracer) geo: &'b BounceRefractionGeometry,
}

/// The uniaxial-closed-form counterpart of [`BounceContext`]: every uniaxial
/// bounce-dispatch function shares exactly `ctx`/`geo`/`frame` (never `cache` -- the
/// closed-form `uniaxial_fresnel` solve reads indices straight off `ctx`/`geo` instead
/// of the biaxial-indicatrix cache).
pub(super) struct UniaxialBounceContext<'m, 'b> {
    pub(super) ctx: &'b RayMaterialContext<'m>,
    pub(super) geo: &'b BounceRefractionGeometry,
    pub(super) frame: &'b UniaxialFrame,
}

/// The wave normal `k` and facet normal at one bounce -- always consumed together
/// (Snell refraction/reflection both act on this pair). See this module's top-of-file
/// doc comment for the `k` (wave normal) vs `S` (Poynting direction) distinction.
#[derive(Clone, Copy)]
pub(in crate::optics::raytracer) struct BounceRay {
    pub(in crate::optics::raytracer) k_hat: Vec3,
    pub(in crate::optics::raytracer) normal: Vec3,
}

/// The mutable per-channel Stokes/path-pdf accumulators every bounce-dispatch function
/// reads and writes -- bundled so a `&mut` of this one struct replaces threading both
/// arrays as separate parameters. Always passed as `&mut BounceState<'_>` (never by
/// value) so a caller can reborrow it across a per-channel loop's repeated calls.
pub(in crate::optics::raytracer) struct BounceState<'a> {
    pub(in crate::optics::raytracer) stokes: &'a mut [StokesVector; NUM_CHANNELS],
    pub(in crate::optics::raytracer) path_pdf: &'a mut [f32; NUM_CHANNELS],
}

/// The per-bounce exit-event pair: [`ExitSplitCtx`]'s per-trace accumulator plus this
/// bounce's own hit point, needed together by [`super::exit_split::try_split_exit_channel`]'s
/// bounded re-entry probe. Always passed as `&mut ExitEvent<'_, '_>` for the same
/// reborrowing reason as [`BounceState`].
pub(in crate::optics::raytracer) struct ExitEvent<'a, 'b> {
    pub(in crate::optics::raytracer) exit: &'a mut ExitSplitCtx<'b>,
    pub(in crate::optics::raytracer) hit_point: Vec3,
}

/// This bounce's RNG identity -- the same `(rng_seed, bounce)` pair every stochastic
/// branch decision in the bounce loop hashes against its own stream salt.
#[derive(Clone, Copy)]
pub(in crate::optics::raytracer) struct RngDraw {
    pub(in crate::optics::raytracer) rng_seed: u32,
    pub(in crate::optics::raytracer) bounce: u32,
}

/// [`super::dispatch::apply_partial_fresnel_bounce`]'s own per-bounce path-mode state:
/// the current Stokes plane-of-incidence frame plus the two mode flags (`inside_gem`,
/// `is_extraordinary`) that decide which branch every downstream helper takes.
#[derive(Clone, Copy)]
pub(in crate::optics::raytracer) struct PathModeState {
    pub(in crate::optics::raytracer) current_plane_normal: Vec3,
    pub(in crate::optics::raytracer) inside_gem: bool,
    pub(in crate::optics::raytracer) is_extraordinary: bool,
}

/// Narrows [`ExitSplitCtx::compat`] after one interior dispersive event (an entry
/// refraction, or any other event where a channel's own refracted direction may
/// diverge from another's). `dirs[k]` is channel k's own direction out of this event
/// (`None` for a channel that cannot transmit here at all, whose `path_pdf` is already
/// zero); `hero_match[k]` is the same `direction_matches` verdict the calling loop used
/// to decide chromatic termination of k against the hero, reused verbatim for every
/// pair involving the hero so the mask and the zeroing can never disagree; every other
/// pair is tested with the same tolerance directly.
///
/// Exit events never narrow the mask: every live channel resolves its own exit
/// direction deterministically, so every technique in a channel's family produces that
/// channel's exit path.
pub(super) fn narrow_compat(
    compat: &mut [u8; NUM_CHANNELS],
    dirs: &[Option<Vec3>; NUM_CHANNELS],
    hero: usize,
    hero_match: [bool; NUM_CHANNELS],
) {
    for a in 0..NUM_CHANNELS {
        for b in (a + 1)..NUM_CHANNELS {
            let matches = if a == hero {
                hero_match[b]
            } else if b == hero {
                hero_match[a]
            } else if let (Some(da), Some(db)) = (dirs[a], dirs[b]) {
                directions_match(da, db)
            } else {
                true
            };
            if !matches {
                compat[a] &= !(1u8 << b);
                compat[b] &= !(1u8 << a);
            }
        }
    }
}
