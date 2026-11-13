//! The closed-form uniaxial air->crystal entry bounce -- [`apply_uniaxial_entry_bounce`]
//! and its reflect/transmit per-channel loops -- plus [`entry_eigenmode_selection`], the
//! polarization-weighted ordinary/extraordinary eigenmode draw every anisotropic entry
//! (uniaxial or biaxial scalar-Fresnel) shares.

use super::{
    ExactExtra, R_UNPOL_PDF_MAX, R_UNPOL_PDF_MIN, R_UNPOL_SELECT_MAX, R_UNPOL_SELECT_MIN,
    context::{
        BounceRay, BounceState, ExitCollect, ExitEvent, ExitSplitCtx, RngDraw,
        UniaxialBounceContext, narrow_compat,
    },
    direction_matches,
    exit_split::{EntryLeaf, deposit_entry_leaf, restore_densities},
};
use crate::optics::{
    birefringence::BirefringenceParams,
    polarization::StokesVector,
    raytracer::{
        NUM_CHANNELS,
        sampling::{BIREFRINGENT_SPLIT_STREAM, FRESNEL_BRANCH_STREAM, discrete_draw},
        uniaxial_fresnel,
    },
};
use glam::Vec3;

/// Polarization-weighted probability that the uniaxial ordinary eigenmode is the
/// physically correct label for this bounce's shared hero-driven path, together with
/// that eigenmode's own doubled polarization azimuth (`cos(2*psi_o)`, `sin(2*psi_o)`)
/// expressed in the same `(s, p)` Stokes frame `current_plane_normal` establishes
/// (`s_axis == current_plane_normal`, `p_axis == k_hat x s_axis`, matching
/// `MuellerMatrix::fresnel_reflection`/`fresnel_transmission`).
///
/// `p_o = 1/2 * (1 + DoP_linear * cos(2*(psi - psi_o)))`, where `psi` is the incident
/// Stokes state's own polarization azimuth and `DoP_linear = sqrt(Q^2+U^2)/I` -- the
/// physical energy fraction Malus's law puts into the ordinary eigenmode for a
/// partially linearly polarized beam (the unpolarized remainder splits 50/50; the
/// polarized remainder follows `cos^2(psi-psi_o)`). Circular polarization does not
/// bias the split between two linear eigenmodes, so `V` plays no role. Expanded
/// directly in `Q`/`U` rather than via `psi`/`DoP_linear` intermediates, which also
/// makes `p_o` reduce to exactly 0.5 for unpolarized light with no separate branch.
/// `cos(2*psi_o)`/`sin(2*psi_o)` come from `o_hat`'s components in the local
/// `(s_hat, p_hat)` basis via the double-angle identities `cos(2x) = s^2 - p^2`,
/// `sin(2x) = 2*s*p`, avoiding an `atan2`/half-angle round-trip.
///
/// Returns `None` (fall back to a flat 50/50 draw, no eigenmode projection) whenever
/// there isn't enough signal to weight the draw meaningfully: `i` negligibly small, a
/// degenerate plane of incidence (`current_plane_normal` near zero, near-normal
/// incidence), or negligible linear polarization (`q`/`u` both near zero). Callers also
/// skip this entirely for a biaxial material -- `ordinary_eigen_polarization` is
/// uniaxial-only -- keeping a biaxial entry at a blanket 50/50 unconditionally.
#[must_use]
pub fn entry_eigenmode_selection(
    c_axis: Vec3,
    current_plane_normal: Vec3,
    k_hat: Vec3,
    hero_stokes: StokesVector,
) -> Option<(f32, f32, f32)> {
    if hero_stokes.i <= 1e-7 || current_plane_normal.length_squared() <= 1e-6 {
        return None;
    }
    let (q, u) = (hero_stokes.q, hero_stokes.u);
    if q.mul_add(q, u * u) <= 1e-12 {
        return None;
    }
    let o_hat = BirefringenceParams::ordinary_eigen_polarization(k_hat, c_axis);
    // `current_plane_normal` is already unit and exactly perpendicular to `k_hat`
    // (`k_hat.cross(normal)`, normalized) -- no re-orthogonalization needed.
    let s_hat = current_plane_normal;
    let p_hat = k_hat.cross(s_hat);
    let s_comp = o_hat.dot(s_hat);
    let p_comp = o_hat.dot(p_hat);
    let cos_2psi_o = s_comp.mul_add(s_comp, -(p_comp * p_comp));
    let sin_2psi_o = 2.0 * s_comp * p_comp;
    let p_o = (0.5 + 0.5 * q.mul_add(cos_2psi_o, u * sin_2psi_o) / hero_stokes.i).clamp(0.0, 1.0);
    Some((p_o, cos_2psi_o, sin_2psi_o))
}

/// The isotropic-air -> uniaxial-crystal entry bounce, via the closed-form `uniaxial_fresnel`
/// solver, for `entering_anisotropic && !geo.is_biaxial`. Called from
/// `apply_partial_fresnel_bounce`.
///
/// The transmitted direction comes from the Snell and walk-off geometry of
/// `compute_bounce_refraction_geometry`; only the Fresnel amplitudes and polarization are
/// solved here.
///
/// # Reflect vs. transmit, and mode selection
///
/// `r_branch` is the hero's own unpolarized-average total reflectance from the full
/// coupled Jones solution (`jones_to_mueller(...).col(0)[0]`) -- used only to draw the
/// reflect/transmit branch, never to weight the actual contribution. If transmitting,
/// `p_o_hero` is the hero's true Poynting-weighted ordinary-mode power fraction
/// (`uniaxial_fresnel::mode_power`, fed the hero's actual Stokes state): it already
/// incorporates both the incident polarization's alignment with each mode's eigenaxis
/// and that mode's own transmission efficiency in one physically exact quantity.
/// Reduces to 50/50 only where the geometry actually makes it symmetric (e.g. optic
/// axis along the surface normal) -- see `mode_power`'s own doc comment.
///
/// Each channel's deposited transmitted intensity is `P_mode(stokes[k]) /
/// p_mode_hero_frac` (standard importance-sampling `target / p_sample`, unbiased for
/// any valid `p_sample`), polarized along the drawn mode's own eigenaxis (`o_hat`/
/// `e_hat`, always real -- an entry interface's transmitted side never evanesces).
///
/// # Exit collection
///
/// With `collect` the event draws no reflect-vs-transmit branch: the mirror reflection is
/// collected as a leaf at once and the path takes the transmit branch undivided
/// ([`apply_uniaxial_entry_collect`]). The mode draw stays as it is. `None` is the branching
/// estimator, bit for bit.
#[expect(
    clippy::similar_names,
    reason = "n_o/n_e are the established ordinary/extraordinary index names; sol_s_hero/sol_p_hero are the established s/p polarisation solution names; p_o_raw/p_e_raw are the established ordinary/extraordinary mode-power names"
)]
pub(super) fn apply_uniaxial_entry_bounce(
    ubctx: &UniaxialBounceContext<'_, '_>,
    ray: BounceRay,
    rng: RngDraw,
    state: &mut BounceState<'_>,
    exit_event: &mut ExitEvent<'_, '_>,
    collect: Option<&mut ExitCollect<'_>>,
) -> (Vec3, Vec3, bool, Option<bool>) {
    let (ctx, geo, frame) = (ubctx.ctx, ubctx.geo, ubctx.frame);
    let BounceRay { k_hat, normal } = ray;
    let RngDraw { rng_seed, bounce } = rng;

    let hero = ctx.hero_idx;
    let c_axis = ctx.c_axis;
    let n_o_hero = geo.n_o_ch[hero];
    let n_e_hero = ctx
        .material
        .extraordinary_index_at(ctx.lambdas[hero], n_o_hero);

    // `k_hat` here always satisfies `k_hat.cross(c_axis).length_squared() > 1e-6`: the
    // dispatcher (`dispatch::try_dispatch_uniaxial_bounce`) sends a wave normal parallel to
    // the optic axis, where the closed-form system is singular, to the scalar fallback in
    // `apply_partial_fresnel_bounce`, the same path the GPU takes.
    //
    // Built once per bounce and shared by the hero's branch-decision solve and every
    // channel's solve. `n1 == 1.0` at an air->crystal entry, so it is the same for all;
    // see `EntryIncidenceFrame`.
    let inc_frame = uniaxial_fresnel::entry_incidence_frame(1.0, frame);
    let (sol_s_hero, sol_p_hero) = uniaxial_fresnel::entry_solve_pair_with_incidence(
        &inc_frame, 1.0, n_o_hero, n_e_hero, c_axis, frame,
    );
    let reflect_mueller_hero = uniaxial_fresnel::jones_to_mueller(
        sol_s_hero.r_s,
        sol_p_hero.r_s,
        sol_s_hero.r_p,
        sol_p_hero.r_p,
    );
    let r_branch = reflect_mueller_hero.col(0)[0].clamp(R_UNPOL_SELECT_MIN, R_UNPOL_SELECT_MAX);

    // Amplitude-1 isotropic-mode intrinsic flux (`poynting_z`'s own 0.5 time-average
    // factor included) -- shared by every channel at entry (`n1 == 1.0` for air,
    // `frame.cos_i` is the hero-driven shared geometry every channel's own solve also
    // uses), so computed once here rather than re-derived per `mode_power` call.
    let inc_flux = 0.5 * frame.cos_i;
    let p_o_raw = uniaxial_fresnel::mode_power(
        sol_s_hero.t_o,
        sol_p_hero.t_o,
        sol_s_hero.flux_o,
        inc_flux,
        state.stokes[hero],
    );
    let p_e_raw = uniaxial_fresnel::mode_power(
        sol_s_hero.t_e,
        sol_p_hero.t_e,
        sol_s_hero.flux_e,
        inc_flux,
        state.stokes[hero],
    );
    let p_o_hero =
        (p_o_raw / (p_o_raw + p_e_raw).max(1e-12)).clamp(R_UNPOL_SELECT_MIN, R_UNPOL_SELECT_MAX);
    let mode_split_rand =
        (discrete_draw(rng_seed, bounce, BIREFRINGENT_SPLIT_STREAM) as f32) / 4_294_967_295.0;
    let use_extraordinary = mode_split_rand < (1.0 - p_o_hero);
    let p_mode_hero_frac = if use_extraordinary {
        1.0 - p_o_hero
    } else {
        p_o_hero
    };

    // Geometry: bit-identical to apply_refract_bounce's own non-biaxial branch.
    let n2_hero_dir = if use_extraordinary {
        geo.n2
    } else {
        geo.n_o_hero
    };
    let eta_dir = geo.n1 / n2_hero_dir;
    let sin2_t_dir = (eta_dir * eta_dir * geo.cos_i.mul_add(-geo.cos_i, 1.0)).min(1.0);
    let cos_t_dir = (1.0 - sin2_t_dir).max(0.0).sqrt();
    let refr_wave_dir =
        (eta_dir * k_hat + f32::mul_add(eta_dir, geo.cos_i, -cos_t_dir) * normal).normalize();
    let final_refr_dir = if use_extraordinary {
        BirefringenceParams::extraordinary_poynting_dir(
            refr_wave_dir,
            c_axis,
            geo.n_o_hero,
            geo.n_e_hero,
        )
    } else {
        refr_wave_dir
    };
    // `o_hat`/`e_hat` are only guaranteed perpendicular to `refr_wave_dir` (the
    // transmitted wave normal), not to `k_hat` (the incident one) -- `frame.p_axis` is
    // built from `k_hat`, so projecting a mode direction onto it directly would lose
    // information whenever Snell's law bends the ray. `s_axis` is shared between the
    // incident and transmitted sides (Snell coplanarity), so
    // `p_axis_transmitted = refr_wave_dir x s_axis` is the correct partner axis, and
    // `refr_wave_dir` is exactly the axis `current_k` becomes for the next bounce's
    // own rotation.
    let p_axis_transmitted = refr_wave_dir.cross(frame.s_axis);

    let mode = EntryTransmitMode {
        use_extraordinary,
        final_refr_dir,
        p_axis_transmitted,
        p_mode_hero_frac,
        inc_flux,
        r_branch,
    };
    if let Some(collect) = collect {
        apply_uniaxial_entry_collect(ubctx, &inc_frame, ray, &mode, state, (exit_event, collect));
        return (refr_wave_dir, final_refr_dir, true, Some(use_extraordinary));
    }

    let rng_bounce =
        (discrete_draw(rng_seed, bounce, FRESNEL_BRANCH_STREAM) as f32) / 4_294_967_295.0;

    if rng_bounce < r_branch {
        apply_uniaxial_entry_reflect_channels(ubctx, &inc_frame, r_branch, state);
        let new_k = k_hat - 2.0 * k_hat.dot(normal) * normal;
        (new_k, new_k, false, None)
    } else {
        apply_uniaxial_entry_transmit_channels(
            ubctx,
            &inc_frame,
            ray,
            &mode,
            state,
            exit_event.exit,
        );
        (refr_wave_dir, final_refr_dir, true, Some(use_extraordinary))
    }
}

/// The exit-collection entry event: the mirror reflection becomes a leaf, the transmit branch
/// continues undivided.
///
/// The reflect loop runs on a copy of the Stokes states with `r_branch = 1`, so its result is each
/// channel's own `R_k`-weighted state with nothing divided out; [`deposit_entry_leaf`] adds that
/// times the environment along the mirror direction (every technique produces the reflection, so
/// the MIS weight is the plain family weight). The transmit loop then runs on the real state with
/// `r_branch = 0`, which is the same loop with `1 / (1 - r_branch) == 1`; the branch factors it
/// folded into `path_pdf` are undone ([`restore_densities`]) because this estimator follows its
/// chain with probability one. Collection runs only for a stone without tools, so the mirror leaf
/// cannot re-enter the stone.
fn apply_uniaxial_entry_collect(
    ubctx: &UniaxialBounceContext<'_, '_>,
    inc_frame: &uniaxial_fresnel::EntryIncidenceFrame,
    ray: BounceRay,
    mode: &EntryTransmitMode,
    state: &mut BounceState<'_>,
    (exit_event, collect): (&mut ExitEvent<'_, '_>, &mut ExitCollect<'_>),
) {
    let ctx = ubctx.ctx;
    let BounceRay { k_hat, normal } = ray;
    let mut leaf_stokes = *state.stokes;
    let mut leaf_pdf = *state.path_pdf;
    apply_uniaxial_entry_reflect_channels(
        ubctx,
        inc_frame,
        1.0,
        &mut BounceState {
            stokes: &mut leaf_stokes,
            path_pdf: &mut leaf_pdf,
        },
    );
    deposit_entry_leaf(
        exit_event,
        collect,
        &EntryLeaf {
            dir: k_hat - 2.0 * k_hat.dot(normal) * normal,
            lambdas: &ctx.lambdas,
            stokes: &leaf_stokes,
            path_pdf: state.path_pdf,
            hero: ctx.hero_idx,
        },
    );
    let saved = *state.path_pdf;
    let undivided = EntryTransmitMode {
        r_branch: 0.0,
        ..*mode
    };
    apply_uniaxial_entry_transmit_channels(
        ubctx,
        inc_frame,
        ray,
        &undivided,
        state,
        exit_event.exit,
    );
    restore_densities(&saved, state.path_pdf);
}

/// The reflect branch's per-channel loop, split out of [`apply_uniaxial_entry_bounce`] for
/// its function-length lint. `inc_frame` is the
/// caller's already-built [`EntryIncidenceFrame`] -- see that type's own doc comment.
#[expect(
    clippy::similar_names,
    reason = "n_o/n_e and sol_s_k/sol_p_k are the established ordinary/extraordinary and s/p names"
)]
fn apply_uniaxial_entry_reflect_channels(
    ubctx: &UniaxialBounceContext<'_, '_>,
    inc_frame: &uniaxial_fresnel::EntryIncidenceFrame,
    r_branch: f32,
    state: &mut BounceState<'_>,
) {
    let ctx = ubctx.ctx;
    let geo = ubctx.geo;
    let frame = ubctx.frame;
    let c_axis = ctx.c_axis;
    let stokes = &mut *state.stokes;
    let path_pdf = &mut *state.path_pdf;
    for k in 0..NUM_CHANNELS {
        let n_o_k = geo.n_o_ch[k];
        let n_e_k = ctx.material.extraordinary_index_at(ctx.lambdas[k], n_o_k);
        let (sol_s_k, sol_p_k) = uniaxial_fresnel::entry_solve_pair_with_incidence(
            inc_frame, 1.0, n_o_k, n_e_k, c_axis, frame,
        );
        let mueller_k =
            uniaxial_fresnel::jones_to_mueller(sol_s_k.r_s, sol_p_k.r_s, sol_s_k.r_p, sol_p_k.r_p);
        let r_unpol_k = mueller_k.col(0)[0].clamp(R_UNPOL_PDF_MIN, R_UNPOL_PDF_MAX);
        stokes[k] = stokes[k].apply_matrix(&mueller_k).scale(1.0 / r_branch);
        path_pdf[k] *= r_unpol_k;
    }
}

/// Unpolarized entry-transmit density `1 - R_unpol` for one channel's `(n_o, n_e)` pair,
/// clamped to the shared PDF bounds.
#[expect(
    clippy::similar_names,
    reason = "sol_s_k/sol_p_k are the established s/p polarisation solution names"
)]
fn entry_transmit_unpol_density(
    inc_frame: &uniaxial_fresnel::EntryIncidenceFrame,
    n_o_k: f32,
    n_e_k: f32,
    c_axis: Vec3,
    frame: &uniaxial_fresnel::UniaxialFrame,
) -> f32 {
    let (sol_s_k, sol_p_k) = uniaxial_fresnel::entry_solve_pair_with_incidence(
        inc_frame, 1.0, n_o_k, n_e_k, c_axis, frame,
    );
    let mueller_k =
        uniaxial_fresnel::jones_to_mueller(sol_s_k.r_s, sol_p_k.r_s, sol_s_k.r_p, sol_p_k.r_p);
    (1.0 - mueller_k.col(0)[0]).clamp(R_UNPOL_PDF_MIN, R_UNPOL_PDF_MAX)
}

/// The resolved transmit-branch outcome of [`apply_uniaxial_entry_bounce`], passed to
/// [`apply_uniaxial_entry_transmit_channels`], whose per-channel loop does the
/// chromatic-termination geometry check and the closed-form mode-power deposit.
#[derive(Clone, Copy)]
struct EntryTransmitMode {
    use_extraordinary: bool,
    final_refr_dir: Vec3,
    p_axis_transmitted: Vec3,
    p_mode_hero_frac: f32,
    inc_flux: f32,
    r_branch: f32,
}

#[expect(
    clippy::similar_names,
    reason = "n_o/n_e are the established ordinary/extraordinary index names; sol_s_k/sol_p_k are the established s/p polarisation solution names; t_s_row/t_p_row are the established s/p polarisation transmission names"
)]
fn apply_uniaxial_entry_transmit_channels(
    ubctx: &UniaxialBounceContext<'_, '_>,
    inc_frame: &uniaxial_fresnel::EntryIncidenceFrame,
    ray: BounceRay,
    mode: &EntryTransmitMode,
    state: &mut BounceState<'_>,
    exit: &mut ExitSplitCtx<'_>,
) {
    let (ctx, geo, frame) = (ubctx.ctx, ubctx.geo, ubctx.frame);
    let c_axis = ctx.c_axis;
    let BounceRay { k_hat, normal } = ray;
    let &EntryTransmitMode {
        use_extraordinary,
        final_refr_dir,
        p_axis_transmitted,
        p_mode_hero_frac,
        inc_flux,
        r_branch,
    } = mode;
    let (stokes, path_pdf) = (&mut *state.stokes, &mut *state.path_pdf);

    let mut dirs = [None; NUM_CHANNELS];
    let mut hero_match = [false; NUM_CHANNELS];
    for k in 0..NUM_CHANNELS {
        let n_o_k = geo.n_o_ch[k];
        // Chromatic-termination geometry: apply_refract_channel's own formula,
        // reusing geo.n2_ch[k] (the channel's own effective extraordinary index) for
        // the direction check only.
        let n2k_dir = if use_extraordinary {
            geo.n2_ch[k]
        } else {
            n_o_k
        };
        let sin2_t_k = (geo.n1 / n2k_dir).powi(2) * geo.cos_i.mul_add(-geo.cos_i, 1.0);
        if sin2_t_k > 1.0 {
            stokes[k] = stokes[k].scale(0.0);
            path_pdf[k] = 0.0;
            continue;
        }
        let cos_t_k = (1.0 - sin2_t_k).max(0.0).sqrt();
        let eta_dir_k = geo.n1 / n2k_dir;
        let refr_wave_dir_k =
            (eta_dir_k * k_hat + f32::mul_add(eta_dir_k, geo.cos_i, -cos_t_k) * normal).normalize();
        let final_dir_k = if use_extraordinary {
            let n_e_eff_k = ctx.material.extraordinary_index_at(ctx.lambdas[k], n_o_k);
            BirefringenceParams::extraordinary_poynting_dir(
                refr_wave_dir_k,
                c_axis,
                n_o_k,
                n_e_eff_k,
            )
        } else {
            refr_wave_dir_k
        };
        dirs[k] = Some(final_dir_k);
        let hero = ctx.hero_idx;
        let n2_hero_dir = if use_extraordinary {
            geo.n2_ch[hero]
        } else {
            geo.n_o_ch[hero]
        };
        let direction_matches = direction_matches(
            exit.direction_match,
            final_dir_k,
            final_refr_dir,
            k == hero,
            [geo.n1, n2k_dir, n_o_k],
            [geo.n1, n2_hero_dir, geo.n_o_ch[hero]],
            ExactExtra {
                material: ctx.material,
                lambdas: (ctx.lambdas[k], ctx.lambdas[hero]),
                uniaxial_e: use_extraordinary,
                biaxial: false,
            },
        );
        hero_match[k] = direction_matches;
        if !direction_matches {
            stokes[k] = stokes[k].scale(0.0);
            if exit.enabled {
                // Technique k stays a live member of every compatible channel's MIS
                // family -- keep its own entry-transmit density accumulating (the same
                // `t_unpol_k` the matching case below folds in); only its radiance
                // ends here. See `apply_refract_channel`'s scalar counterpart.
                let n_e_k = ctx.material.extraordinary_index_at(ctx.lambdas[k], n_o_k);
                path_pdf[k] *= entry_transmit_unpol_density(inc_frame, n_o_k, n_e_k, c_axis, frame);
            } else {
                path_pdf[k] = 0.0;
            }
            continue;
        }

        let n_e_k = ctx.material.extraordinary_index_at(ctx.lambdas[k], n_o_k);
        let (sol_s_k, sol_p_k) = uniaxial_fresnel::entry_solve_pair_with_incidence(
            inc_frame, 1.0, n_o_k, n_e_k, c_axis, frame,
        );
        let (t_s_row, t_p_row, flux_k, mode_dir) = if use_extraordinary {
            (sol_s_k.t_e, sol_p_k.t_e, sol_s_k.flux_e, sol_s_k.e_hat)
        } else {
            (sol_s_k.t_o, sol_p_k.t_o, sol_s_k.flux_o, sol_s_k.o_hat)
        };
        let p_mode_k = uniaxial_fresnel::mode_power(t_s_row, t_p_row, flux_k, inc_flux, stokes[k]);
        let deposit_i = p_mode_k / p_mode_hero_frac;
        let (cos_2psi, sin_2psi) =
            uniaxial_fresnel::azimuth2_in_frame(mode_dir, frame.s_axis, p_axis_transmitted);
        stokes[k] = StokesVector::new(deposit_i, deposit_i * cos_2psi, deposit_i * sin_2psi, 0.0)
            .scale(1.0 / (1.0 - r_branch));

        let mueller_k =
            uniaxial_fresnel::jones_to_mueller(sol_s_k.r_s, sol_p_k.r_s, sol_s_k.r_p, sol_p_k.r_p);
        let t_unpol_k = (1.0 - mueller_k.col(0)[0]).clamp(R_UNPOL_PDF_MIN, R_UNPOL_PDF_MAX);
        path_pdf[k] *= t_unpol_k;
    }
    // The uniaxial entry is an interior dispersive event -- narrow every channel's MIS
    // family, see `narrow_compat`'s doc comment.
    if exit.enabled {
        narrow_compat(&mut exit.compat, &dirs, ctx.hero_idx, hero_match);
    }
}
