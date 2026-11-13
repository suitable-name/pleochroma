//! One bounce of the deterministic branch tree: the path tracer's own physics, expanded.
//!
//! The path tracer picks reflect or transmit, and an eigenmode, at random and reweights by the
//! inverse of the probability it picked. Its expected contribution is therefore the sum over the
//! outcomes of the unweighted Stokes-Mueller result, which is what [`expand`] returns: every
//! outcome as a child with the Stokes vector of all eight comb channels, no random draw and no
//! importance weight.
//!
//! Every Fresnel coefficient, Mueller matrix, closed-form uniaxial solve, refraction geometry and
//! mode-selection probability is the path tracer's own function; only the dispatch around them is
//! restated, in the same branch order as `transport::bounce::dispatch_bounce` and
//! `refraction::dispatch::apply_partial_fresnel_bounce`.
//!
//! The per-hit set-up the path tracer runs before a bounce (plane-of-incidence rotation, the
//! normal flip, the interior segment's absorption, the refraction geometry) is [`prepare_bounce`];
//! [`expand_at`] runs it for an explicit hit and expands the result. The Fresnel amplitudes come
//! from the tracer's table-aware helpers, so they follow the optics lookup tables when those are
//! on and are bit-identical to the closed forms when they are off. The energy of an outcome
//! below [`MIN_PROBABILITY`] is not expanded but reported ([`expand_reporting`]).

use glam::Vec3;

use super::{
    build::HeroContext,
    record::{COMB_LEN, DEPTH_LEN},
};
use crate::{
    geometry::GpuFacetPlane,
    optics::{
        birefringence::{BirefringenceParams, assigned_mode_e_field_uniaxial},
        polarization::{MuellerMatrix, StokesVector},
        raytracer::{
            Ray,
            absorption::{
                apply_segment_absorption, channel_absorption_alphas_assigned,
                rotate_stokes_to_plane_of_incidence,
            },
            camera::HitRecord,
            refraction::{
                BounceRefractionGeometry, RayMaterialContext, RayWavelengthCache,
                compute_bounce_refraction_geometry, entry_eigenmode_selection,
                poynting_dir_for_mode, tir_phase_delta,
            },
            shading_normal_near_edge,
            uniaxial_fresnel::{
                UniaxialFrame, azimuth2_in_frame, entry_incidence_frame,
                entry_solve_pair_with_incidence, internal_solve, jones_to_mueller, mode_power,
            },
        },
        tables::{OpticsTableSet, fresnel_amplitudes, reflection_amplitudes},
        zoning::MAX_ZONES,
    },
};

/// Outcomes with a probability below this are not expanded.
pub const MIN_PROBABILITY: f32 = 1.0e-6;

/// Zone slots in every per-zone array: the base zone, then up to the zoning's maximum of shaped
/// zones.
pub const ZONE_SLOTS: usize = MAX_ZONES + 1;

/// Below this the wave normal is parallel to the optic axis and the closed-form solves decline.
const AXIS_PARALLEL_EPS: f32 = 1.0e-6;

/// What produced a child.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventKind {
    /// A partial Fresnel reflection.
    Reflect,
    /// A forced total internal reflection.
    TotalReflect,
    /// A refraction through the surface.
    Transmit,
}

impl EventKind {
    /// A small stable code, mixed into the branch signature.
    #[must_use]
    pub const fn code(self) -> u64 {
        match self {
            Self::Reflect => 1,
            Self::TotalReflect => 2,
            Self::Transmit => 3,
        }
    }
}

/// The part of a branch's state a bounce reads and writes.
#[derive(Clone, Copy, Debug)]
pub struct PathState {
    /// Wave normal.
    pub k: Vec3,
    /// Inside the stone.
    pub inside: bool,
    /// Eigenmode label (extraordinary / mode B).
    pub is_ext: bool,
    /// Stokes vector of every comb channel.
    pub stokes: [StokesVector; COMB_LEN],
}

/// One outcome of a bounce.
#[derive(Clone, Copy, Debug)]
pub struct Child {
    /// The state after the bounce.
    pub state: PathState,
    /// Poynting (ray) direction after the bounce.
    pub s: Vec3,
    /// What happened.
    pub kind: EventKind,
    /// A channel took a different reflect / transmit decision than the hero channel here.
    pub diverged: bool,
}

/// Everything one bounce needs.
pub struct BounceInput<'x, 'a> {
    /// The hero context (material context and wavelength cache).
    pub hero: &'x HeroContext<'a>,
    /// State before the bounce, with the interior segment's absorption already applied.
    pub state: PathState,
    /// Facet normal facing the incoming ray (flipped inside the stone).
    pub normal: Vec3,
    /// Normal of the plane of incidence (the Stokes reference frame).
    pub plane_normal: Vec3,
    /// The path tracer's refraction geometry for this bounce.
    pub(crate) geo: BounceRefractionGeometry,
}

/// The optical depth data of one interior segment, so a caller can evaluate its absorption at
/// wavelengths other than the comb's.
///
/// `lengths_mm` are the per-zone path lengths of the segment in absorption-length units (the
/// `lengths_mm` of the zoned cache, scaled by `absorption_path_scale`); `alphas` are the
/// per-zone, per-comb-channel absorption coefficients at the segment's wave normal and mode. A
/// material without zones puts its whole length in slot 0. An exterior segment, and a scattering
/// material (which extinguishes through its own estimator), have all-zero lengths.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SegmentDepths {
    /// Path length per zone slot, in absorption-length units.
    pub lengths_mm: [f32; ZONE_SLOTS],
    /// Absorption coefficient per zone slot and comb channel.
    pub alphas: [[f32; COMB_LEN]; ZONE_SLOTS],
}

impl SegmentDepths {
    /// No segment: zero lengths, so every optical depth is zero.
    pub const NONE: Self = Self {
        lengths_mm: [0.0; ZONE_SLOTS],
        alphas: [[0.0; COMB_LEN]; ZONE_SLOTS],
    };

    /// The depth data of the straight segment `ray`, `path_len` model units long.
    fn of_segment(
        ctx: &RayMaterialContext<'_>,
        cache: &RayWavelengthCache,
        (k_hat, is_ext): (Vec3, bool),
        ray: Ray,
        path_len: f32,
    ) -> Self {
        let scale = ctx.material.absorption_path_scale;
        if let Some(zoned) = cache.zoned.as_ref() {
            return Self {
                lengths_mm: zoned.lengths_mm(ray.origin, ray.dir, 0.0, path_len, scale),
                alphas: zoned.zone_alphas(ctx, cache, k_hat, is_ext),
            };
        }
        let mut out = Self::NONE;
        out.lengths_mm[0] = path_len * scale;
        out.alphas[0] = channel_absorption_alphas_assigned(ctx, cache, k_hat, is_ext);
        out
    }

    /// The optical depth of the segment per channel for coefficients `alphas` (another comb, or
    /// sub-nodes of one): `sum_z alphas[z][k] * lengths_mm[z]`.
    #[must_use]
    pub fn optical_depths_with(&self, alphas: &[[f32; COMB_LEN]; ZONE_SLOTS]) -> [f32; COMB_LEN] {
        let mut depth = [0.0; COMB_LEN];
        for (zone, &len) in alphas.iter().zip(&self.lengths_mm) {
            for (d, &alpha) in depth.iter_mut().zip(zone) {
                *d = alpha.mul_add(len, *d);
            }
        }
        depth
    }

    /// The optical depth of the segment per comb channel.
    #[must_use]
    pub fn optical_depths(&self) -> [f32; COMB_LEN] {
        self.optical_depths_with(&self.alphas)
    }

    /// The Beer-Lambert transmittance `exp(-depth)` per comb channel.
    #[must_use]
    pub fn transmittance(&self) -> [f32; COMB_LEN] {
        self.optical_depths().map(|depth| (-depth).exp())
    }
}

/// The weights of the three principal absorption spectra in a segment with wave normal `k_hat`
/// in eigenmode `is_extraordinary`, in slots `DEPTH_O`, `DEPTH_E`, `DEPTH_BETA` of `record`.
///
/// The channel absorption is the quadratic form `e . A . e` in a tensor that is diagonal in
/// wavelength-independent axes, so the absorption is `w_o a_o + w_e a_e + w_beta a_beta` with the
/// squared components of the field direction along those axes as weights. An anisotropic
/// material uses the assigned mode's field; otherwise the mean over the two eigen polarizations.
#[must_use]
pub(crate) fn principal_weights(
    ctx: &RayMaterialContext<'_>,
    cache: &RayWavelengthCache,
    k_hat: Vec3,
    is_extraordinary: bool,
) -> [f32; DEPTH_LEN] {
    let c_axis = ctx.c_axis;
    let to_principal = cache.tensor_ch[ctx.hero_idx].axes.transpose();
    let squares = |e_hat: Vec3| {
        let local = to_principal * e_hat;
        local * local
    };
    let weights = if ctx.is_anisotropic {
        let e_mode_hat = cache.hero_indicatrix.map_or_else(
            || {
                let ordinary_hero = cache.n_o_ch[ctx.hero_idx];
                let extraordinary_hero = ctx
                    .material
                    .extraordinary_index_at(ctx.lambdas[ctx.hero_idx], ordinary_hero);
                assigned_mode_e_field_uniaxial(
                    k_hat,
                    c_axis,
                    is_extraordinary,
                    ordinary_hero,
                    extraordinary_hero,
                )
            },
            |ind| ind.assigned_mode_e_field(k_hat, is_extraordinary),
        );
        squares(e_mode_hat)
    } else {
        let (eigen_a, eigen_b) = cache.hero_indicatrix.map_or_else(
            || {
                (
                    BirefringenceParams::ordinary_eigen_polarization(k_hat, c_axis),
                    BirefringenceParams::extraordinary_eigen_polarization(k_hat, c_axis),
                )
            },
            |ind| ind.eigen_polarizations(k_hat),
        );
        (squares(eigen_a) + squares(eigen_b)) * 0.5
    };
    if cache.hero_indicatrix.is_some() && ctx.material.absorption.beta_ray.is_some() {
        [weights.x, weights.z, weights.y]
    } else {
        [weights.x + weights.y, weights.z, 0.0]
    }
}

/// The optical-depth coefficients of the interior segment a bounce ends: the segment's length in
/// absorption-length units times [`principal_weights`].
///
/// `state` is the state before the bounce, `side` the intersection's verdict as in
/// [`prepare_bounce`], `path_len` the segment's length in model units (`hit.t`). All zero outside
/// the stone, for a zoned material (its comb weights keep the zone absorption [`prepare_bounce`]
/// applies, so nothing is counted twice) and for a scattering one (no Beer-Lambert segment; the
/// cache refuses it).
#[must_use]
pub fn segment_principal_depths(
    hero: &HeroContext<'_>,
    state: &PathState,
    side: Option<bool>,
    path_len: f32,
) -> [f32; DEPTH_LEN] {
    let (ctx, cache) = (&hero.ctx, &hero.cache);
    let inside = side.map_or(state.inside, |entering| !entering);
    if !inside || ctx.material.scatters() || cache.zoned.is_some() {
        return [0.0; DEPTH_LEN];
    }
    let scaled = path_len * ctx.material.absorption_path_scale;
    principal_weights(ctx, cache, state.k, state.is_ext).map(|weight| weight * scaled)
}

/// The path tracer's per-hit set-up for one bounce, in the tracer's own order.
///
/// The order is `bounce_body`'s prelude: the Stokes vectors are rotated into the plane of
/// incidence (`prev_plane` is the previous bounce's plane normal), the normal is flipped inside
/// the stone, the interior segment's absorption is applied (`ray` is the segment, `hit.t` its
/// length; skipped for a scattering material), and the refraction geometry is computed.
/// `side` is the intersection's own verdict on entering (`Some(true)`) or leaving (`Some(false)`)
/// the stone and overrides `state.inside` when present.
///
/// Returns the input for [`expand`] and the segment's [`SegmentDepths`].
#[must_use]
pub fn prepare_bounce<'x, 'a>(
    hero: &'x HeroContext<'a>,
    planes: &[GpuFacetPlane],
    state: PathState,
    ray: Ray,
    hit: &HitRecord,
    side: Option<bool>,
    prev_plane: Option<Vec3>,
) -> (BounceInput<'x, 'a>, SegmentDepths) {
    prepare_bounce_with(hero, planes, (state, ray, hit), side, prev_plane, true)
}

/// [`prepare_bounce`] without the [`SegmentDepths`], for callers that never read them (the
/// path cache's own tree build): the same input, bit for bit, without the second per-zone
/// length computation.
#[must_use]
pub fn prepare_bounce_input<'x, 'a>(
    hero: &'x HeroContext<'a>,
    planes: &[GpuFacetPlane],
    state: PathState,
    ray: Ray,
    hit: &HitRecord,
    side: Option<bool>,
    prev_plane: Option<Vec3>,
) -> BounceInput<'x, 'a> {
    prepare_bounce_with(hero, planes, (state, ray, hit), side, prev_plane, false).0
}

/// The body of [`prepare_bounce`]; `want_depths` skips the depth computation when `false`.
fn prepare_bounce_with<'x, 'a>(
    hero: &'x HeroContext<'a>,
    planes: &[GpuFacetPlane],
    (state, ray, hit): (PathState, Ray, &HitRecord),
    side: Option<bool>,
    prev_plane: Option<Vec3>,
    want_depths: bool,
) -> (BounceInput<'x, 'a>, SegmentDepths) {
    let (ctx, cache) = (&hero.ctx, &hero.cache);
    let mut state = state;
    // At close boundaries the restart offset may cross another face: the intersection's own
    // side is authoritative.
    if let Some(entering) = side {
        state.inside = !entering;
    }
    let hit_point = ray.origin + hit.t * ray.dir;
    let mut normal = shading_normal_near_edge(
        planes,
        hit_point,
        hit.facet_idx,
        hit.normal,
        ctx.material.edge_rounding_radius,
    );
    let plane_normal =
        rotate_stokes_to_plane_of_incidence(state.k, normal, prev_plane, &mut state.stokes);
    let mut depths = SegmentDepths::NONE;
    if state.inside {
        normal = -normal;
        if !ctx.material.scatters() {
            apply_segment_absorption(
                ctx,
                cache,
                state.k,
                state.is_ext,
                (ray, hit.t),
                &mut state.stokes,
            );
            if want_depths {
                depths = SegmentDepths::of_segment(ctx, cache, (state.k, state.is_ext), ray, hit.t);
            }
        }
    }
    let geo =
        compute_bounce_refraction_geometry(ctx, cache, normal, state.k, state.inside, state.is_ext);
    let input = BounceInput {
        hero,
        state,
        normal,
        plane_normal,
        geo,
    };
    (input, depths)
}

/// A hit decided by the caller's geometry: the physics never re-decides which facet it is.
#[derive(Clone, Copy, Debug)]
pub struct ExplicitHit {
    /// The hit point.
    pub point: Vec3,
    /// Index of the hit facet in the plane slice.
    pub facet: usize,
    /// Outward normal of the hit facet.
    pub normal: Vec3,
}

/// Everything [`expand_at`] produces for one hit.
#[derive(Clone, Debug)]
pub struct Expansion {
    /// Every outcome, reflections first, then transmissions.
    pub children: Vec<Child>,
    /// Spectral-mean intensity of the outcomes below [`MIN_PROBABILITY`] that were not expanded.
    pub dropped: f32,
    /// The interior segment's depth data (all zero outside the stone).
    pub depths: SegmentDepths,
    /// Normal of the plane of incidence, to hand on as the next bounce's `prev_plane`.
    pub plane_normal: Vec3,
}

/// [`prepare_bounce`] and [`expand_reporting`] for an explicit hit.
///
/// The ray parameter is derived from the segment: `t = (point - ray.origin) . ray.dir`, clamped
/// to be non-negative, so no intersector runs.
#[must_use]
pub fn expand_at(
    hero: &HeroContext<'_>,
    planes: &[GpuFacetPlane],
    state: PathState,
    ray: Ray,
    hit: ExplicitHit,
    side: Option<bool>,
    prev_plane: Option<Vec3>,
) -> Expansion {
    let record = HitRecord {
        t: (hit.point - ray.origin).dot(ray.dir).max(0.0),
        normal: hit.normal,
        facet_idx: hit.facet,
    };
    let (input, depths) = prepare_bounce(hero, planes, state, ray, &record, side, prev_plane);
    let (children, dropped) = expand_reporting(&input);
    Expansion {
        children,
        dropped,
        depths,
        plane_normal: input.plane_normal,
    }
}

fn reflect(k: Vec3, normal: Vec3) -> Vec3 {
    k - 2.0 * k.dot(normal) * normal
}

/// The mean of the comb channels' intensities.
fn mean_intensity(stokes: &[StokesVector; COMB_LEN]) -> f32 {
    stokes.iter().map(StokesVector::intensity).sum::<f32>() / COMB_LEN as f32
}

fn refracted(k: Vec3, normal: Vec3, eta: f32, cos_i: f32) -> Vec3 {
    let sin2_t = (eta * eta * cos_i.mul_add(-cos_i, 1.0)).min(1.0);
    let cos_t = (1.0 - sin2_t).max(0.0).sqrt();
    (eta * k + f32::mul_add(eta, cos_i, -cos_t) * normal).normalize()
}

/// Every outcome of the bounce, in a fixed order: reflections first, then transmissions.
#[must_use]
pub fn expand(b: &BounceInput<'_, '_>) -> Vec<Child> {
    expand_reporting(b).0
}

/// [`expand`], and the spectral-mean intensity of the outcomes it did not expand.
///
/// An outcome with a probability at or below [`MIN_PROBABILITY`] is not a child; its energy (the
/// mean over the comb channels of the intensity it would have carried) is the second value, so
/// the caller can account for it as dropped energy.
#[must_use]
pub fn expand_reporting(b: &BounceInput<'_, '_>) -> (Vec<Child>, f32) {
    let ctx = &b.hero.ctx;
    let mut dropped = 0.0;
    let children = if b.geo.sin2_t > 1.0 {
        total_reflection(b, &mut dropped)
    } else if let Some(frame) = b.geo.uniaxial_frame
        && b.state.k.cross(ctx.c_axis).length_squared() > AXIS_PARALLEL_EPS
    {
        if b.state.inside {
            uniaxial_internal(b, &frame, &mut dropped)
        } else {
            uniaxial_entry(b, &frame)
        }
    } else {
        scalar(b, &mut dropped)
    };
    (children, dropped)
}

/// The reflected child for `stokes`; `walk_off` re-derives the Poynting direction for the mode.
fn reflected(
    b: &BounceInput<'_, '_>,
    stokes: [StokesVector; COMB_LEN],
    kind: EventKind,
    diverged: bool,
    walk_off: bool,
) -> Child {
    let st = &b.state;
    let new_k = reflect(st.k, b.normal);
    let s = if walk_off {
        poynting_dir_for_mode(&b.hero.ctx, &b.geo, new_k, st.inside, st.is_ext)
    } else {
        new_k
    };
    Child {
        state: PathState {
            k: new_k,
            inside: st.inside,
            is_ext: st.is_ext,
            stokes,
        },
        s,
        kind,
        diverged,
    }
}

/// Re-draws the eigenmode label after an internal reflection: ordinary with `p_ordinary`.
///
/// The path tracer relabels without reweighting, so the probabilities scale the branch.
fn with_modes(
    b: &BounceInput<'_, '_>,
    base: Child,
    p_ordinary: f32,
    dropped: &mut f32,
) -> Vec<Child> {
    let ctx = &b.hero.ctx;
    if !(ctx.enable_internal_mode_coupling && ctx.is_anisotropic) {
        return vec![base];
    }
    split_modes(base, p_ordinary, dropped)
}

/// Splits `base` into its ordinary (probability `p_ordinary`) and extraordinary children.
///
/// A mode at or below [`MIN_PROBABILITY`] is not a child; its mean-channel intensity is added to
/// `dropped` (nothing is added for a probability that is not positive).
#[must_use]
pub fn split_modes(base: Child, p_ordinary: f32, dropped: &mut f32) -> Vec<Child> {
    [(false, p_ordinary), (true, 1.0 - p_ordinary)]
        .into_iter()
        .filter_map(|(is_ext, p)| {
            let mut child = base;
            child.state.is_ext = is_ext;
            child.state.stokes = base.state.stokes.map(|s| s.scale(p));
            if p > MIN_PROBABILITY {
                Some(child)
            } else {
                if p > 0.0 {
                    *dropped += mean_intensity(&child.state.stokes);
                }
                None
            }
        })
        .collect()
}

/// The relabeling probability without a closed-form split: the polarization-weighted heuristic.
fn recoupling_probability(
    b: &BounceInput<'_, '_>,
    new_k: Vec3,
    stokes: &[StokesVector; COMB_LEN],
) -> f32 {
    let ctx = &b.hero.ctx;
    let selection = if b.geo.is_biaxial {
        None
    } else {
        entry_eigenmode_selection(ctx.c_axis, b.plane_normal, new_k, stokes[ctx.hero_idx])
    };
    selection.map_or(0.5, |(p, ..)| p)
}

/// The scalar-Fresnel reflection of every channel, and whether a channel's total-internal
/// decision differs from the hero's.
///
/// The amplitudes are the tracer's table-aware `reflection_amplitudes`: `tables` (the render's
/// optics lookup tables, `None` by default) serves them where it can, else the closed form.
fn reflection_stokes(
    geo: &BounceRefractionGeometry,
    stokes: &[StokesVector; COMB_LEN],
    tables: Option<&OpticsTableSet>,
) -> ([StokesVector; COMB_LEN], bool) {
    let hero_total = geo.sin2_t > 1.0;
    let mut out = *stokes;
    let mut diverged = false;
    for (k, slot) in out.iter_mut().enumerate() {
        let (n1k, n2k) = (geo.n1_ch[k], geo.n2_ch[k]);
        let channel_total = geo.sin2_t_ch[k] > 1.0;
        diverged |= channel_total != hero_total;
        let matrix = if channel_total {
            MuellerMatrix::tir_retardation(tir_phase_delta(n1k, geo.cos_i, geo.sin_i))
        } else {
            let cos_t_k = (1.0 - geo.sin2_t_ch[k]).max(0.0).sqrt();
            let (r_s, r_p) = reflection_amplitudes(tables, n1k, n2k, geo.cos_i, cos_t_k);
            MuellerMatrix::fresnel_reflection(r_s, r_p)
        };
        *slot = slot.apply_matrix(&matrix);
    }
    (out, diverged)
}

/// The hero is past its critical angle: a forced reflection (`apply_tir_bounce`).
fn total_reflection(b: &BounceInput<'_, '_>, dropped: &mut f32) -> Vec<Child> {
    let (ctx, geo, st) = (&b.hero.ctx, &b.geo, &b.state);
    let closed_form = geo
        .uniaxial_frame
        .filter(|_| st.k.cross(ctx.c_axis).length_squared() > AXIS_PARALLEL_EPS);
    let (stokes, diverged, exact_p_o) = closed_form.map_or_else(
        || {
            let (stokes, diverged) = reflection_stokes(geo, &st.stokes, ctx_tables(b));
            (stokes, diverged, None)
        },
        |frame| {
            let mut stokes = st.stokes;
            let mut diverged = false;
            let mut exact = None;
            for (k, slot) in stokes.iter_mut().enumerate() {
                let ordinary = geo.n_o_ch[k];
                let extraordinary = ctx
                    .material
                    .extraordinary_index_at(ctx.lambdas[k], ordinary);
                let sol = internal_solve(
                    geo.n1_ch[k],
                    ordinary,
                    extraordinary,
                    ctx.c_axis,
                    &frame,
                    !st.is_ext,
                );
                let power_o = sol.r_o.norm_sqr() * sol.flux_ro;
                let power_e = sol.r_e.norm_sqr() * sol.flux_re;
                let total = ((power_o + power_e) / sol.flux_inc.max(1e-12)).min(1.0);
                *slot = slot.scale(total);
                diverged |= geo.sin2_t_ch[k] <= 1.0;
                if k == ctx.hero_idx {
                    exact = Some(power_o / (power_o + power_e).max(1e-12));
                }
            }
            (stokes, diverged, exact)
        },
    );
    let base = reflected(b, stokes, EventKind::TotalReflect, diverged, true);
    let p_ordinary =
        exact_p_o.unwrap_or_else(|| recoupling_probability(b, base.state.k, &base.state.stokes));
    with_modes(b, base, p_ordinary, dropped)
}

/// Air to uniaxial crystal through the closed-form solve (`apply_uniaxial_entry_bounce`).
fn uniaxial_entry(b: &BounceInput<'_, '_>, frame: &UniaxialFrame) -> Vec<Child> {
    let (ctx, geo, st) = (&b.hero.ctx, &b.geo, &b.state);
    let incidence = entry_incidence_frame(1.0, frame);
    let solutions: Vec<_> = (0..COMB_LEN)
        .map(|k| {
            let ordinary = geo.n_o_ch[k];
            let extraordinary = ctx
                .material
                .extraordinary_index_at(ctx.lambdas[k], ordinary);
            entry_solve_pair_with_incidence(
                &incidence,
                1.0,
                ordinary,
                extraordinary,
                ctx.c_axis,
                frame,
            )
        })
        .collect();

    let mut reflection = st.stokes;
    for (slot, (sol_s, sol_p)) in reflection.iter_mut().zip(&solutions) {
        let matrix = jones_to_mueller(sol_s.r_s, sol_p.r_s, sol_s.r_p, sol_p.r_p);
        *slot = slot.apply_matrix(&matrix);
    }
    let mut children = vec![reflected(b, reflection, EventKind::Reflect, false, false)];

    let incident_flux = 0.5 * frame.cos_i;
    for is_ext in [false, true] {
        let n2_dir = if is_ext { geo.n2 } else { geo.n_o_hero };
        let wave = refracted(st.k, b.normal, geo.n1 / n2_dir, geo.cos_i);
        let ray = if is_ext {
            BirefringenceParams::extraordinary_poynting_dir(
                wave,
                ctx.c_axis,
                geo.n_o_hero,
                geo.n_e_hero,
            )
        } else {
            wave
        };
        let p_axis = wave.cross(frame.s_axis);
        let mut stokes = st.stokes;
        for ((slot, (sol_s, sol_p)), incident) in stokes.iter_mut().zip(&solutions).zip(&st.stokes)
        {
            let (t_s, t_p, flux, axis) = if is_ext {
                (sol_s.t_e, sol_p.t_e, sol_s.flux_e, sol_s.e_hat)
            } else {
                (sol_s.t_o, sol_p.t_o, sol_s.flux_o, sol_s.o_hat)
            };
            let power = mode_power(t_s, t_p, flux, incident_flux, *incident);
            let (cos_2psi, sin_2psi) = azimuth2_in_frame(axis, frame.s_axis, p_axis);
            *slot = StokesVector::new(power, power * cos_2psi, power * sin_2psi, 0.0);
        }
        children.push(Child {
            state: PathState {
                k: wave,
                inside: true,
                is_ext,
                stokes,
            },
            s: ray,
            kind: EventKind::Transmit,
            diverged: false,
        });
    }
    children
}

/// Inside a uniaxial crystal through the closed-form solve (`apply_uniaxial_internal_bounce`).
fn uniaxial_internal(
    b: &BounceInput<'_, '_>,
    frame: &UniaxialFrame,
    dropped: &mut f32,
) -> Vec<Child> {
    let (ctx, geo, st) = (&b.hero.ctx, &b.geo, &b.state);
    let solutions: Vec<_> = (0..COMB_LEN)
        .map(|k| {
            let ordinary = geo.n_o_ch[k];
            let extraordinary = ctx
                .material
                .extraordinary_index_at(ctx.lambdas[k], ordinary);
            internal_solve(
                geo.n1_ch[k],
                ordinary,
                extraordinary,
                ctx.c_axis,
                frame,
                !st.is_ext,
            )
        })
        .collect();

    let mut reflection = st.stokes;
    let mut exact_p_o = 0.5;
    for (k, (slot, sol)) in reflection.iter_mut().zip(&solutions).enumerate() {
        let power_o = sol.r_o.norm_sqr() * sol.flux_ro;
        let power_e = sol.r_e.norm_sqr() * sol.flux_re;
        let total = ((power_o + power_e) / sol.flux_inc.max(1e-12)).min(1.0);
        *slot = slot.scale(total);
        if k == ctx.hero_idx {
            exact_p_o = power_o / (power_o + power_e).max(1e-12);
        }
    }
    let base = reflected(b, reflection, EventKind::Reflect, false, false);
    let mut children = with_modes(b, base, exact_p_o, dropped);

    let wave = refracted(st.k, b.normal, geo.n1 / geo.n2, geo.cos_i);
    let mut stokes = st.stokes;
    let mut diverged = false;
    for (k, (slot, sol)) in stokes.iter_mut().zip(&solutions).enumerate() {
        let sin2_t_k = (geo.n1_ch[k] / geo.n2_ch[k]).powi(2) * geo.cos_i.mul_add(-geo.cos_i, 1.0);
        if sin2_t_k > 1.0 {
            *slot = slot.scale(0.0);
            diverged = true;
            continue;
        }
        let flux_inc = sol.flux_inc.max(1e-12);
        let s_wave = sol.t_s.scale((sol.flux_ts / flux_inc).sqrt());
        let p_wave = sol.t_p.scale((sol.flux_tp / flux_inc).sqrt());
        let i_unit = s_wave.norm_sqr() + p_wave.norm_sqr();
        let q_unit = s_wave.norm_sqr() - p_wave.norm_sqr();
        let cross = s_wave.mul(p_wave.conj());
        let incident = slot.i;
        *slot = StokesVector::new(
            incident * i_unit,
            incident * q_unit,
            incident * 2.0 * cross.re,
            incident * -2.0 * cross.im,
        );
    }
    children.push(Child {
        state: PathState {
            k: wave,
            inside: false,
            is_ext: st.is_ext,
            stokes,
        },
        s: wave,
        kind: EventKind::Transmit,
        diverged,
    });
    children
}

/// Isotropic, biaxial and axis-parallel bounces through the scalar Fresnel machinery.
fn scalar(b: &BounceInput<'_, '_>, dropped: &mut f32) -> Vec<Child> {
    let (ctx, geo, st) = (&b.hero.ctx, &b.geo, &b.state);
    let entering = !st.inside && ctx.is_anisotropic;
    let selection = if entering && !geo.is_biaxial {
        entry_eigenmode_selection(ctx.c_axis, b.plane_normal, st.k, st.stokes[ctx.hero_idx])
    } else {
        None
    };
    let p_ordinary = selection.map_or(0.5, |(p, ..)| p);

    let (stokes, diverged) = reflection_stokes(geo, &st.stokes, ctx_tables(b));
    let base = reflected(b, stokes, EventKind::Reflect, diverged, true);
    let mut children = if st.inside {
        let p = recoupling_probability(b, base.state.k, &base.state.stokes);
        with_modes(b, base, p, dropped)
    } else {
        vec![base]
    };

    let modes = if entering {
        vec![(false, p_ordinary), (true, 1.0 - p_ordinary)]
    } else {
        vec![(st.is_ext, 1.0)]
    };
    for (is_ext, probability) in modes {
        if probability > MIN_PROBABILITY {
            children.push(scalar_transmission(
                b,
                selection,
                entering,
                is_ext,
                probability,
            ));
        } else if probability > 0.0 {
            let lost = scalar_transmission(b, selection, entering, is_ext, probability);
            *dropped += mean_intensity(&lost.state.stokes);
        }
    }
    children
}

/// The render's optics lookup tables, if any.
fn ctx_tables<'b>(b: &'b BounceInput<'_, '_>) -> Option<&'b OpticsTableSet> {
    b.hero.cache.tables.as_deref()
}

/// The refracted child of one eigenmode (`apply_refract_bounce`), weighted by the mode's share.
fn scalar_transmission(
    b: &BounceInput<'_, '_>,
    selection: Option<(f32, f32, f32)>,
    entering: bool,
    is_ext: bool,
    probability: f32,
) -> Child {
    let (ctx, geo, st) = (&b.hero.ctx, &b.geo, &b.state);
    let (wave, ray) =
        if let (true, true, Some(indicatrix)) = (entering, geo.is_biaxial, geo.hero_indicatrix) {
            let n2_dir = if is_ext {
                geo.n_biax_b_hero
            } else {
                geo.n_biax_a_hero
            };
            let wave = refracted(st.k, b.normal, geo.n1 / n2_dir, geo.cos_i);
            (wave, indicatrix.mode_poynting_dir(wave, is_ext))
        } else {
            let n2_dir = if entering && !is_ext {
                geo.n_o_hero
            } else {
                geo.n2
            };
            let wave = refracted(st.k, b.normal, geo.n1 / n2_dir, geo.cos_i);
            let ray = if entering && is_ext {
                BirefringenceParams::extraordinary_poynting_dir(
                    wave,
                    ctx.c_axis,
                    geo.n_o_hero,
                    geo.n_e_hero,
                )
            } else {
                wave
            };
            (wave, ray)
        };

    let azimuth = selection
        .filter(|_| entering)
        .map(|(_, cos_2psi, sin_2psi)| {
            if is_ext {
                (-cos_2psi, -sin_2psi)
            } else {
                (cos_2psi, sin_2psi)
            }
        });
    let mut stokes = st.stokes;
    let mut diverged = false;
    for (k, slot) in stokes.iter_mut().enumerate() {
        let n1k = geo.n1_ch[k];
        let n2k = if entering && !is_ext {
            if geo.is_biaxial {
                geo.n_biax_a_ch[k]
            } else {
                geo.n_o_ch[k]
            }
        } else {
            geo.n2_ch[k]
        };
        let sin2_t_k = (n1k / n2k).powi(2) * geo.cos_i.mul_add(-geo.cos_i, 1.0);
        if sin2_t_k > 1.0 {
            *slot = slot.scale(0.0);
            diverged = true;
            continue;
        }
        let cos_t_k = (1.0 - sin2_t_k).max(0.0).sqrt();
        let amplitudes = fresnel_amplitudes(ctx_tables(b), n1k, n2k, geo.cos_i, cos_t_k);
        let matrix = MuellerMatrix::fresnel_transmission(
            n1k,
            n2k,
            geo.cos_i,
            cos_t_k,
            amplitudes.t_s,
            amplitudes.t_p,
        );
        let incident = match azimuth {
            Some((cos_2psi, sin_2psi)) => {
                StokesVector::new(slot.i, slot.i * cos_2psi, slot.i * sin_2psi, 0.0)
            }
            None => *slot,
        };
        *slot = incident.apply_matrix(&matrix).scale(probability);
    }
    Child {
        state: PathState {
            k: wave,
            inside: !st.inside,
            is_ext: if entering { is_ext } else { st.is_ext },
            stokes,
        },
        s: ray,
        kind: EventKind::Transmit,
        diverged,
    }
}
