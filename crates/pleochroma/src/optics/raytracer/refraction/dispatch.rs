//! [`apply_partial_fresnel_bounce`], the top-level per-bounce dispatch: draws the
//! reflect-vs-transmit decision for the shared hero-driven path and routes to whichever
//! closed-form uniaxial path, or scalar-Fresnel fallback, applies.

use super::{
    R_UNPOL_SELECT_MAX, R_UNPOL_SELECT_MIN,
    context::{
        BounceContext, BounceRay, BounceState, ExitCollect, ExitEvent, PathModeState,
        RayMaterialContext, RngDraw, UniaxialBounceContext,
    },
    exit_split::{
        ChannelTransmissionInputs, EntryLeaf, ExitLeaves, compute_channel_transmission,
        deposit_entry_leaf, deposit_exit_leaves, restore_densities, transmitting_techniques,
    },
    geometry::{BounceRefractionGeometry, poynting_dir_for_mode},
    reflect_refract::{RefractSelection, apply_partial_reflect_bounce, apply_refract_bounce},
    uniaxial_entry::{apply_uniaxial_entry_bounce, entry_eigenmode_selection},
    uniaxial_internal::{
        HeroInternalSolve, apply_uniaxial_internal_bounce,
        apply_uniaxial_internal_reflect_channels, solve_hero_internal,
        uniaxial_exit_leaf_intensities,
    },
};
use crate::optics::{
    polarization::StokesVector,
    raytracer::{
        NUM_CHANNELS,
        sampling::{BIREFRINGENT_SPLIT_STREAM, FRESNEL_BRANCH_STREAM, discrete_draw},
    },
    tables::{OpticsTableSet, reflection_amplitudes},
};
use glam::Vec3;

/// The result of [`apply_partial_fresnel_bounce`]: the new wave normal `k` and Poynting direction
/// `S`, the new `inside_gem` state, the `is_extraordinary` update (if any), and the exact ordinary
/// probability `p_o` of a closed-form uniaxial internal event (`None` otherwise).
///
/// A named alias keeps the `Option` return type of [`try_dispatch_uniaxial_bounce`] under
/// clippy's `type_complexity` limit.
type FresnelBounceResult = (Vec3, Vec3, bool, Option<bool>, Option<f32>);

/// Dispatches the two closed-form uniaxial paths of [`apply_partial_fresnel_bounce`]: air to
/// crystal entry, and internal exit or o/e coupling. Kept out of that function for its line-count
/// lint. Returns `None` when the general (isotropic, biaxial or degenerate) machinery must run.
///
/// A path is taken only when the wave normal is not parallel to the optic axis
/// (`k_hat.cross(c_axis).length_squared() > 1e-6`). There the closed forms are singular, and both
/// eigenmodes collapse to one isotropic response at `n_o` (`effective_extraordinary_index` gives
/// `n_o` at zero critical angle), so the scalar path at `n_o` is exact there. Hero total internal
/// reflection is `apply_tir_bounce`'s, dispatched from `transport::dispatch_bounce`.
fn try_dispatch_uniaxial_bounce(
    bctx: &BounceContext<'_, '_>,
    ray: BounceRay,
    mode_state: PathModeState,
    entering_anisotropic: bool,
    rng: RngDraw,
    state: &mut BounceState<'_>,
    exit_event: &mut ExitEvent<'_, '_>,
) -> Option<FresnelBounceResult> {
    let (ctx, geo) = (bctx.ctx, bctx.geo);
    let BounceRay { k_hat, .. } = ray;
    let PathModeState {
        inside_gem,
        is_extraordinary,
        ..
    } = mode_state;

    // Same non-degenerate test as the internal branch below. A wave normal parallel to
    // `c_axis` makes the entry system singular, so this declines (`None`) and the general path
    // takes the event, as the GPU's `uniaxial_nondegenerate` gate does.
    if entering_anisotropic
        && let Some(frame) = geo.uniaxial_frame
        && k_hat.cross(ctx.c_axis).length_squared() > 1e-6
    {
        let ubctx = UniaxialBounceContext {
            ctx,
            geo,
            frame: &frame,
        };
        let (new_k, new_s, new_inside_gem, is_extraordinary_update) =
            apply_uniaxial_entry_bounce(&ubctx, ray, rng, state, exit_event, None);
        return Some((new_k, new_s, new_inside_gem, is_extraordinary_update, None));
    }
    if inside_gem
        && let Some(frame) = geo.uniaxial_frame
        && k_hat.cross(ctx.c_axis).length_squared() > 1e-6
    {
        let ubctx = UniaxialBounceContext {
            ctx,
            geo,
            frame: &frame,
        };
        return Some(apply_uniaxial_internal_bounce(
            &ubctx,
            is_extraordinary,
            ray,
            rng,
            state,
            exit_event,
        ));
    }
    None
}

/// The inputs [`resolve_entry_mode_selection`] reads, bundled to keep its signature
/// short.
struct EntryModeSelectionInputs<'m, 'b> {
    ctx: &'b RayMaterialContext<'m>,
    geo: &'b BounceRefractionGeometry,
    entering_anisotropic: bool,
    current_plane_normal: Vec3,
    k_hat: Vec3,
    hero_stokes: StokesVector,
    is_extraordinary: bool,
    rng_seed: u32,
    bounce: u32,
}

/// The polarization-weighted eigenmode selection [`apply_partial_fresnel_bounce`]
/// makes at an anisotropic entry -- split out purely to keep that function's own body
/// under the workspace line-count lint. Returns `(use_extraordinary,
/// entry_mode_azimuth2)`.
///
/// With no `entry_selection` (`p_o == 0.5`) this is the plain 50/50 split: the same hash,
/// threshold and sense, extraordinary iff `mode_split_rand < 0.5`;
/// weighted by polarization otherwise (extraordinary drawn with probability
/// `1 - p_o`, ordinary with probability `p_o`, as `p_o`'s own doc comment defines
/// it). For a non-entering bounce (internal TIR/reflect/refract, or any
/// isotropic-material refraction) this keeps whatever mode the path already
/// carries -- exactly the `entering_anisotropic` guard's else branch.
///
/// The chosen mode's own doubled polarization azimuth is for `apply_refract_channel`'s
/// eigenmode projection -- `None` when `entry_selection` itself was `None`, in which
/// case that projection is skipped entirely (see its own doc comment). Extraordinary
/// is perpendicular to ordinary (`psi_e == psi_o + 90 deg`), so its doubled azimuth
/// is simply the negation of the ordinary one (`cos(2*psi_o + 180deg) ==
/// -cos(2*psi_o)`, likewise for `sin`).
fn resolve_entry_mode_selection(
    inputs: &EntryModeSelectionInputs<'_, '_>,
) -> (bool, Option<(f32, f32)>) {
    let &EntryModeSelectionInputs {
        ctx,
        geo,
        entering_anisotropic,
        current_plane_normal,
        k_hat,
        hero_stokes,
        is_extraordinary,
        rng_seed,
        bounce,
    } = inputs;
    let entry_selection = if entering_anisotropic && !geo.is_biaxial {
        entry_eigenmode_selection(ctx.c_axis, current_plane_normal, k_hat, hero_stokes)
    } else {
        None
    };
    let p_o = entry_selection.map_or(0.5, |(p, ..)| p);
    let mode_split_rand =
        (discrete_draw(rng_seed, bounce, BIREFRINGENT_SPLIT_STREAM) as f32) / 4_294_967_295.0;
    let use_extraordinary = if entering_anisotropic {
        mode_split_rand < (1.0 - p_o)
    } else {
        is_extraordinary
    };
    let entry_mode_azimuth2 = entry_selection.map(|(_, cos_2psi_o, sin_2psi_o)| {
        if use_extraordinary {
            (-cos_2psi_o, -sin_2psi_o)
        } else {
            (cos_2psi_o, sin_2psi_o)
        }
    });
    (use_extraordinary, entry_mode_azimuth2)
}

/// The reflect-vs-transmit SELECTION probability [`apply_partial_fresnel_bounce`]
/// draws against -- split out purely to keep that function's own body under the
/// workspace line-count lint.
///
/// Use the SELECTED mode's own index for n2 at THIS interface -- `geo.n2` (mode
/// B/extraordinary) unchanged when extraordinary is selected, not entering an
/// anisotropic material at all, or the material is biaxial (`geo.n_o_hero` is a
/// uniaxial-only quantity, not mode A -- see `entry_eigenmode_selection`'s doc
/// comment), `geo.n_o_hero` when the ordinary mode is selected instead. `geo.n1`/
/// `geo.cos_i` are unaffected: `n1 == 1.0` (air) on every path this function can
/// reach (`entering_anisotropic` requires `!inside_gem`).
///
/// `sin2_t` reuses `geo.sin2_t` unless the ordinary mode was selected: a re-derived value is
/// not bit-identical to `geo.sin2_t`. When the ordinary mode is selected it is recomputed at
/// `n_o_hero` with the same expression shape `compute_bounce_refraction_geometry` uses.
///
/// The reflect/transmit SELECTION probability's clamp (`min`/`max`) is distinct from
/// the per-channel `r_unpol_k` clamps elsewhere in this file (which only scale
/// `path_pdf`, never divide `stokes` directly). `[0.02, 0.98]` rather than
/// `[1e-4, 1-1e-4]` caps the `1/r_unpol`/`1/(1-r_unpol)` divisions
/// (`apply_partial_reflect_bounce`/`apply_refract_channel`) at 50x instead of
/// 10,000x at grazing incidence, where the unclamped raw value genuinely approaches 0
/// or 1 -- still unbiased (the contribution is weighted by the TRUE `R`/`T` divided by
/// this same `p`), just far less firefly-prone.
fn compute_entry_reflect_probability(
    geo: &BounceRefractionGeometry,
    entering_anisotropic: bool,
    use_extraordinary: bool,
    min: f32,
    max: f32,
    tables: Option<&OpticsTableSet>,
) -> f32 {
    let ordinary_selected = entering_anisotropic && !geo.is_biaxial && !use_extraordinary;
    let n2 = if ordinary_selected {
        geo.n_o_hero
    } else {
        geo.n2
    };
    let sin2_t = if ordinary_selected {
        let eta = geo.n1 / n2;
        eta * eta * geo.cos_i.mul_add(-geo.cos_i, 1.0)
    } else {
        geo.sin2_t
    };
    let cos_t = (1.0 - sin2_t).max(0.0).sqrt();
    let (r_s, r_p) = reflection_amplitudes(tables, geo.n1, n2, geo.cos_i, cos_t);

    let r_unpol_raw = 0.5 * r_p.mul_add(r_p, r_s * r_s);
    r_unpol_raw.clamp(min, max)
}

/// Partial Fresnel reflection or refraction of the shared hero-driven path.
///
/// Decides reflect vs. transmit from the hero's `r_unpol`, drawn from a hash of `(rng_seed,
/// bounce)` with its own salt, so the draw is decorrelated from the Russian-roulette draw. Then
/// applies the sampled event via [`apply_partial_reflect_bounce`] or [`apply_refract_bounce`].
/// Returns the new wave normal `k'`, the new Poynting direction `S'` (a reflection's `k'` is
/// converted with [`poynting_dir_for_mode`] using the mode still in effect, since reflection never
/// changes `is_extraordinary`), the new `inside_gem` state, and the `is_extraordinary` update, if
/// any (see [`super::reflect_refract::RefractBounceOutcome`]).
///
/// At an air->crystal entry into an anisotropic material, which eigenmode
/// (ordinary/mode-A vs extraordinary/mode-B) this path's single geometric transmission
/// event represents is decided HERE -- before the reflect-vs-transmit draw below,
/// rather than inside `apply_refract_bounce`'s own transmit branch.
/// Two reasons this has to happen up here:
///   - The SELECTION itself is weighted by the incident polarization's projection onto
///     each eigenmode's own axis ([`entry_eigenmode_selection`]), not a blanket 50/50 --
///     and that weighting has to happen exactly once per bounce, shared by both
///     branches below: a beam already aligned with the ordinary axis should be MORE
///     likely to reflect at the ordinary index too, not just more likely to transmit
///     as ordinary conditional on transmitting.
///   - The REFLECT branch's own Fresnel coefficients should be evaluated at the SAME
///     mode's index the transmit branch (`apply_refract_channel`) uses for this
///     channel. For a uniaxial material they are ([`compute_entry_reflect_probability`]
///     substitutes `n_o_hero` when the ordinary mode is selected), so `R + T == 1`
///     for whichever mode this draw selects; always
///     using mode B (extraordinary)'s index for the reflect/transmit decision would
///     instead leave `R + T != 1` by `O(delta_n / n)` whenever the ordinary mode is
///     selected.
///
/// A biaxial material has no uniaxial "ordinary" eigenmode to weight against, so both
/// the selection and the index correction are gated on `!geo.is_biaxial`: a biaxial
/// entry always uses a blanket 50/50 split. Mode B's index (`geo.n2`) drives the
/// reflect/transmit decision while transmission uses the index of the mode the split
/// selects, so energy is conserved only on average there: with `R_A`/`R_B` the
/// unpolarized Fresnel reflectances at the two modes' indices, `R + T` is
/// `R_B + (1 - R_sel)`, whose expectation over the 50/50 selection is
/// `1 + 0.5 * (R_B - R_A)`, not 1.
///
/// Non-degenerate uniaxial entries and internal events do not reach the code below
/// whenever a closed-form path applies: [`try_dispatch_uniaxial_bounce`] takes them
/// first.
pub(in crate::optics::raytracer) fn apply_partial_fresnel_bounce(
    bctx: &BounceContext<'_, '_>,
    ray: BounceRay,
    mode_state: PathModeState,
    rng: RngDraw,
    state: &mut BounceState<'_>,
    exit_event: &mut ExitEvent<'_, '_>,
) -> (Vec3, Vec3, bool, Option<bool>, Option<f32>) {
    let (ctx, geo) = (bctx.ctx, bctx.geo);
    let BounceRay { k_hat, normal } = ray;
    let PathModeState {
        current_plane_normal,
        inside_gem,
        is_extraordinary,
    } = mode_state;
    let RngDraw { rng_seed, bounce } = rng;

    let entering_anisotropic = !inside_gem && ctx.is_anisotropic;
    if let Some(result) = try_dispatch_uniaxial_bounce(
        bctx,
        ray,
        mode_state,
        entering_anisotropic,
        rng,
        state,
        exit_event,
    ) {
        return result;
    }

    let (use_extraordinary, entry_mode_azimuth2) =
        resolve_entry_mode_selection(&EntryModeSelectionInputs {
            ctx,
            geo,
            entering_anisotropic,
            current_plane_normal,
            k_hat,
            hero_stokes: state.stokes[ctx.hero_idx],
            is_extraordinary,
            rng_seed,
            bounce,
        });

    let tables = bctx.cache.tables.as_deref();
    let r_unpol = compute_entry_reflect_probability(
        geo,
        entering_anisotropic,
        use_extraordinary,
        R_UNPOL_SELECT_MIN,
        R_UNPOL_SELECT_MAX,
        tables,
    );
    let rng_bounce =
        (discrete_draw(rng_seed, bounce, FRESNEL_BRANCH_STREAM) as f32) / 4_294_967_295.0;

    if rng_bounce < r_unpol {
        let new_k = apply_partial_reflect_bounce(
            geo,
            r_unpol,
            k_hat,
            normal,
            state.stokes,
            state.path_pdf,
            tables,
        );
        let new_s = poynting_dir_for_mode(ctx, geo, new_k, inside_gem, is_extraordinary);
        (new_k, new_s, inside_gem, None, None)
    } else {
        let selection = RefractSelection {
            use_extraordinary,
            entry_mode_azimuth2,
        };
        let outcome =
            apply_refract_bounce(bctx, r_unpol, ray, inside_gem, selection, state, exit_event);
        (
            outcome.new_k,
            outcome.new_s,
            !inside_gem,
            outcome.is_extraordinary_update,
            None,
        )
    }
}

/// [`apply_partial_fresnel_bounce`] for the exit-collection estimator
/// ([`crate::optics::raytracer::Estimator::ExitCollection`]): no reflect-vs-transmit draw.
///
/// Both outcomes of the event are taken, each with its physical weight and nothing divided by a
/// selection probability:
///
/// - **Interior event** (the hero is not totally reflected; that case is `apply_tir_bounce`'s):
///   every channel that can transmit adds its own exit leaf, `T_k` times the environment along its
///   own refracted direction, at once ([`deposit_exit_leaves`]); the path continues along the
///   reflection with `R_k`.
/// - **Entry event** (outside to inside): every channel adds the mirror reflection, `R_k` times
///   the environment along the mirror direction; the path continues inside with `T_k`. The
///   eigenmode draw of an anisotropic entry is the one of [`apply_partial_fresnel_bounce`].
///
/// The branch factors the reused bounce functions fold into `path_pdf` are undone afterwards
/// ([`restore_densities`]): the chain is followed with probability one, so the spectral-MIS weight
/// lives in the leaf ([`super::exit_split::leaf_mis_weight`]), not in `path_pdf`.
///
/// Only a convex stone (no tools) is traced this way, so no leaf can re-enter the stone: a stone
/// with tools runs the branching estimator on every engine. The tuple is
/// [`apply_partial_fresnel_bounce`]'s, with `inside_gem` flipped at an entry and kept at an
/// interior reflection.
pub(in crate::optics::raytracer) fn apply_collect_bounce(
    bctx: &BounceContext<'_, '_>,
    ray: BounceRay,
    mode_state: PathModeState,
    rng: RngDraw,
    state: &mut BounceState<'_>,
    exit_event: &mut ExitEvent<'_, '_>,
    collect: &mut ExitCollect<'_>,
) -> FresnelBounceResult {
    debug_assert!(
        exit_event.exit.tools.is_empty(),
        "exit collection runs only for a stone without tools"
    );
    if mode_state.inside_gem {
        collect_interior_event(bctx, ray, mode_state, state, (exit_event, collect))
    } else {
        collect_entry_event(bctx, ray, mode_state, rng, state, (exit_event, collect))
    }
}

/// Channel k's own refracted wave direction out of the gem at an interior event, `None` where k is
/// totally reflected.
fn exit_leaf_dirs(geo: &BounceRefractionGeometry, ray: BounceRay) -> [Option<Vec3>; NUM_CHANNELS] {
    let BounceRay { k_hat, normal } = ray;
    std::array::from_fn(|k| {
        let sin2_t_k = geo.sin2_t_ch[k];
        if sin2_t_k > 1.0 {
            return None;
        }
        let cos_t_k = (1.0 - sin2_t_k).max(0.0).sqrt();
        let eta_k = geo.n1_ch[k] / geo.n2_ch[k];
        Some((eta_k * k_hat + f32::mul_add(eta_k, geo.cos_i, -cos_t_k) * normal).normalize())
    })
}

/// Each channel's transmitted intensity out of the gem at an interior event of the scalar-Fresnel
/// machinery (isotropic, biaxial, or the degenerate uniaxial limit), nothing divided out.
///
/// The per-channel Fresnel transmission `apply_refract_channel` runs for a matching channel,
/// called with `r_unpol = 0` so its `1 / (1 - r_unpol)` is one.
fn scalar_exit_leaf_intensities(
    bctx: &BounceContext<'_, '_>,
    stokes: &[StokesVector; NUM_CHANNELS],
) -> [f32; NUM_CHANNELS] {
    let geo = bctx.geo;
    let tables = bctx.cache.tables.as_deref();
    std::array::from_fn(|k| {
        if geo.sin2_t_ch[k] > 1.0 {
            return 0.0;
        }
        let (transmitted, _) = compute_channel_transmission(
            &ChannelTransmissionInputs {
                n1k: geo.n1_ch[k],
                n2k: geo.n2_ch[k],
                cos_i: geo.cos_i,
                cos_t_k: (1.0 - geo.sin2_t_ch[k]).max(0.0).sqrt(),
                r_unpol: 0.0,
                entering_anisotropic: false,
                entry_mode_azimuth2: None,
                incident_stokes_k: stokes[k],
            },
            tables,
        );
        transmitted.intensity()
    })
}

/// Adds the interior exit leaves `intensities` along `dirs` to `collect`.
fn deposit_interior_leaves(
    bctx: &BounceContext<'_, '_>,
    state: &BounceState<'_>,
    (exit_event, collect): (&ExitEvent<'_, '_>, &mut ExitCollect<'_>),
    dirs: &[Option<Vec3>; NUM_CHANNELS],
    intensities: &[f32; NUM_CHANNELS],
) {
    deposit_exit_leaves(
        exit_event,
        collect,
        &ExitLeaves {
            lambdas: &bctx.ctx.lambdas,
            dirs,
            intensities,
            path_pdf: state.path_pdf,
            hero: bctx.ctx.hero_idx,
            producers: transmitting_techniques(&bctx.geo.sin2_t_ch),
        },
    );
}

/// The interior event of [`apply_collect_bounce`]: the exit leaves, then the undivided reflection.
fn collect_interior_event(
    bctx: &BounceContext<'_, '_>,
    ray: BounceRay,
    mode_state: PathModeState,
    state: &mut BounceState<'_>,
    (exit_event, collect): (&mut ExitEvent<'_, '_>, &mut ExitCollect<'_>),
) -> FresnelBounceResult {
    let (ctx, geo) = (bctx.ctx, bctx.geo);
    let BounceRay { k_hat, normal } = ray;
    let dirs = exit_leaf_dirs(geo, ray);
    let saved = *state.path_pdf;
    if let Some(frame) = geo.uniaxial_frame
        && k_hat.cross(ctx.c_axis).length_squared() > 1e-6
    {
        // Closed-form uniaxial internal event (same guard as `try_dispatch_uniaxial_bounce`).
        let ubctx = UniaxialBounceContext {
            ctx,
            geo,
            frame: &frame,
        };
        let (hero_solve, p_o_exact) = solve_hero_internal(&ubctx, mode_state.is_extraordinary);
        let intensities = uniaxial_exit_leaf_intensities(&ubctx, &hero_solve, state.stokes);
        deposit_interior_leaves(bctx, state, (exit_event, collect), &dirs, &intensities);
        let undivided = HeroInternalSolve {
            r_branch: 1.0,
            ..hero_solve
        };
        apply_uniaxial_internal_reflect_channels(&ubctx, &undivided, state);
        restore_densities(&saved, state.path_pdf);
        let new_k = k_hat - 2.0 * k_hat.dot(normal) * normal;
        return (new_k, new_k, true, None, Some(p_o_exact));
    }
    let intensities = scalar_exit_leaf_intensities(bctx, state.stokes);
    deposit_interior_leaves(bctx, state, (exit_event, collect), &dirs, &intensities);
    let tables = bctx.cache.tables.as_deref();
    let new_k = apply_partial_reflect_bounce(
        geo,
        1.0,
        k_hat,
        normal,
        state.stokes,
        state.path_pdf,
        tables,
    );
    restore_densities(&saved, state.path_pdf);
    let new_s = poynting_dir_for_mode(ctx, geo, new_k, true, mode_state.is_extraordinary);
    (new_k, new_s, true, None, None)
}

/// The entry event of [`apply_collect_bounce`]: the mirror leaf, then the undivided transmission.
fn collect_entry_event(
    bctx: &BounceContext<'_, '_>,
    ray: BounceRay,
    mode_state: PathModeState,
    rng: RngDraw,
    state: &mut BounceState<'_>,
    (exit_event, collect): (&mut ExitEvent<'_, '_>, &mut ExitCollect<'_>),
) -> FresnelBounceResult {
    let (ctx, geo) = (bctx.ctx, bctx.geo);
    let BounceRay { k_hat, normal } = ray;
    let mirror_dir = k_hat - 2.0 * k_hat.dot(normal) * normal;
    let entering_anisotropic = ctx.is_anisotropic;
    if entering_anisotropic
        && let Some(frame) = geo.uniaxial_frame
        && k_hat.cross(ctx.c_axis).length_squared() > 1e-6
    {
        // The closed-form uniaxial entry collects its own mirror leaf.
        let ubctx = UniaxialBounceContext {
            ctx,
            geo,
            frame: &frame,
        };
        let (new_k, new_s, new_inside_gem, update) =
            apply_uniaxial_entry_bounce(&ubctx, ray, rng, state, exit_event, Some(collect));
        return (new_k, new_s, new_inside_gem, update, None);
    }
    let tables = bctx.cache.tables.as_deref();
    let mut leaf_stokes = *state.stokes;
    let mut leaf_pdf = *state.path_pdf;
    apply_partial_reflect_bounce(
        geo,
        1.0,
        k_hat,
        normal,
        &mut leaf_stokes,
        &mut leaf_pdf,
        tables,
    );
    deposit_entry_leaf(
        exit_event,
        collect,
        &EntryLeaf {
            dir: mirror_dir,
            lambdas: &ctx.lambdas,
            stokes: &leaf_stokes,
            path_pdf: state.path_pdf,
            hero: ctx.hero_idx,
        },
    );
    let (use_extraordinary, entry_mode_azimuth2) =
        resolve_entry_mode_selection(&EntryModeSelectionInputs {
            ctx,
            geo,
            entering_anisotropic,
            current_plane_normal: mode_state.current_plane_normal,
            k_hat,
            hero_stokes: state.stokes[ctx.hero_idx],
            is_extraordinary: mode_state.is_extraordinary,
            rng_seed: rng.rng_seed,
            bounce: rng.bounce,
        });
    let selection = RefractSelection {
        use_extraordinary,
        entry_mode_azimuth2,
    };
    let saved = *state.path_pdf;
    let outcome = apply_refract_bounce(bctx, 0.0, ray, false, selection, state, exit_event);
    restore_densities(&saved, state.path_pdf);
    (
        outcome.new_k,
        outcome.new_s,
        true,
        outcome.is_extraordinary_update,
        None,
    )
}
