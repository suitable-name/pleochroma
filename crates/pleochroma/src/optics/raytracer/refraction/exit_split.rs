//! Per-channel exit-event Fresnel transmission and the bounded exit-splitting probe --
//! see [`super`]'s "Exit-event spectral splitting" doc comment for the estimator these
//! support.

use super::{
    R_UNPOL_PDF_MAX, R_UNPOL_PDF_MIN,
    context::{ExitCollect, ExitEvent, ExitSplitCtx},
};
use crate::optics::{
    polarization::{MuellerMatrix, StokesVector},
    raytracer::{
        NUM_CHANNELS,
        camera::Ray,
        environment::{sample_environment_channel_at, sample_environment_channels_at},
        intersect_stone::intersect_stone_soa,
    },
    tables::{OpticsTableSet, fresnel_amplitudes},
};
use glam::Vec3;

#[cfg(debug_assertions)]
thread_local! {
    /// Energy `try_split_exit_channel` has dropped on this thread because the exit probe
    /// re-entered the stone. Debug builds only, so a release build pays nothing.
    static TRUNCATED_ENERGY: std::cell::Cell<f64> = const { std::cell::Cell::new(0.0) };
}

/// Returns and resets the energy [`try_split_exit_channel`] truncated on this thread.
///
/// The exit split does not follow a channel that re-enters the stone. On a concave stone that is
/// common (the far wall of a groove is hit by many exits), so the concave white-furnace test
/// reads this to report the loss apart from its gate. The unit is the dropped channel's
/// transmitted intensity (one unit is one fully transmitted channel sample). Always `0.0` in a
/// release build, where the counter does not exist.
#[doc(hidden)]
#[must_use]
pub fn exit_split_truncated_energy_take() -> f64 {
    #[cfg(debug_assertions)]
    {
        TRUNCATED_ENERGY.with(|e| e.replace(0.0))
    }
    #[cfg(not(debug_assertions))]
    {
        0.0
    }
}

/// Scalar inputs of [`compute_channel_transmission`]: channel k's Fresnel transmission at a
/// refract or exit interface, the computation `super::reflect_refract::apply_refract_channel`
/// runs for a matching direction. The split branch reuses it for a diverging channel.
///
/// That function returns the transmitted Stokes state and channel k's unpolarized reflectance
/// `r_unpol_k`, whose factor the split branch applies to `path_pdf[k]`.
pub(super) struct ChannelTransmissionInputs {
    pub(super) n1k: f32,
    pub(super) n2k: f32,
    pub(super) cos_i: f32,
    pub(super) cos_t_k: f32,
    pub(super) r_unpol: f32,
    pub(super) entering_anisotropic: bool,
    pub(super) entry_mode_azimuth2: Option<(f32, f32)>,
    pub(super) incident_stokes_k: StokesVector,
}

#[expect(
    clippy::similar_names,
    reason = "s/p polarisation pairs (t_s_k/t_p_k, r_s_k/r_p_k) are the established Fresnel names"
)]
pub(super) fn compute_channel_transmission(
    inputs: &ChannelTransmissionInputs,
    tables: Option<&OpticsTableSet>,
) -> (StokesVector, f32) {
    let &ChannelTransmissionInputs {
        n1k,
        n2k,
        cos_i,
        cos_t_k,
        r_unpol,
        entering_anisotropic,
        entry_mode_azimuth2,
        incident_stokes_k,
    } = inputs;
    // From the optics lookup tables when they serve this interface (`tables` is `None` by
    // default), else the closed forms this function always used, bit for bit.
    let amplitudes = fresnel_amplitudes(tables, n1k, n2k, cos_i, cos_t_k);
    let t_s_k = amplitudes.t_s;
    let t_p_k = amplitudes.t_p;
    let trans_matrix_k =
        MuellerMatrix::fresnel_transmission(n1k, n2k, cos_i, cos_t_k, t_s_k, t_p_k);
    let incident_k =
        if entering_anisotropic && let Some((cos_2psi_x, sin_2psi_x)) = entry_mode_azimuth2 {
            let i_k = incident_stokes_k.i;
            StokesVector::new(i_k, i_k * cos_2psi_x, i_k * sin_2psi_x, 0.0)
        } else {
            incident_stokes_k
        };
    let transmitted = incident_k
        .apply_matrix(&trans_matrix_k)
        .scale(1.0 / (1.0 - r_unpol));
    let r_s_k = amplitudes.r_s;
    let r_p_k = amplitudes.r_p;
    let r_unpol_k =
        (0.5 * r_p_k.mul_add(r_p_k, r_s_k * r_s_k)).clamp(R_UNPOL_PDF_MIN, R_UNPOL_PDF_MAX);
    (transmitted, r_unpol_k)
}

/// Resolves channel `k`'s own radiance contribution when its Snell-refracted direction
/// at an exit bounce (leaving the gem back into air) diverges from the direction the
/// hero-driven shared path actually took. `path_pdf[k]` is not this function's concern:
/// every non-TIR channel at an exit event gets its own local transmit factor folded
/// into `path_pdf[k]` by the caller before this call, whether or not the split below
/// finds an escaping ray. This function is purely the bounded fan-out mechanism for
/// `split_radiance`, called only for a channel that still carries radiance.
///
/// Callers compute channel k's own Fresnel transmission via
/// [`compute_channel_transmission`] and pass in its intensity, already fully resolved.
/// Only `split_radiance[k]` is written. Bounded to one extra intersection test; it never
/// recurses into the general per-bounce dispatch:
///   - k's own ray escapes the gem (the common case): its contribution is folded
///     directly into `exit.split_radiance[k]` (`transmitted_intensity * env_spectral`,
///     the same product shape `accumulate_miss_radiance` uses for the shared path).
///   - k's own ray instead re-enters the gem (a convex one, or a concave one whose
///     cavity it crossed -- the common case there): tracing further is
///     unbounded work this design declines, so nothing is added to `split_radiance` --
///     a pure energy-loss truncation, exactly like hitting `max_bounces` or a Russian
///     roulette kill partway through a sub-path, not a density/pdf event. Unbiased for
///     the same reason the `sin2_t_k > 1.0` TIR-mismatch case in
///     `apply_refract_channel` already is: channel `k`'s true contribution through
///     that longer sub-path is captured by other render samples, the ones that draw
///     `k` itself as hero.
pub(super) fn try_split_exit_channel(
    exit: &mut ExitSplitCtx<'_>,
    hit_point: Vec3,
    k: usize,
    lambda_k: f32,
    dir_k: Vec3,
    transmitted_intensity: f32,
) {
    let probe = Ray {
        origin: hit_point + dir_k * 1e-4,
        dir: dir_k,
    };
    if intersect_stone_soa(probe, exit.plane_soa, exit.plane_soa.len(), exit.tools).is_some() {
        // Bounded re-entry: decline to trace further (energy-loss truncation only). On a
        // concave stone this is the biggest energy risk in transport, so record it.
        #[cfg(debug_assertions)]
        TRUNCATED_ENERGY.with(|e| e.set(e.get() + f64::from(transmitted_intensity.max(0.0))));
        return;
    }
    let env_spectral = sample_environment_channel_at(
        exit.environment,
        probe.origin,
        dir_k,
        lambda_k,
        exit.studio_rig.as_ref(),
        exit.observer,
    );
    // `split_mis_weight` is `1.0` outside a transmit-out event with a live incoming NEE
    // carry -- see `ExitSplitCtx::split_mis_weight`'s doc comment -- so this reproduces
    // the unweighted addition exactly whenever no competing light-sampling technique is
    // in play.
    exit.split_radiance[k] = f32::mul_add(
        transmitted_intensity.max(0.0) * exit.split_mis_weight,
        env_spectral,
        exit.split_radiance[k],
    );
}

/// The techniques that leave the gem at this event, as a channel bit mask: those whose channel is
/// not totally reflected (`sin2_t_ch[j] <= 1`). A hero that is totally reflected yields no exit
/// leaf.
pub(super) fn transmitting_techniques(sin2_t_ch: &[f32; NUM_CHANNELS]) -> u8 {
    let mut mask = 0u8;
    for (j, &sin2_t) in sin2_t_ch.iter().enumerate() {
        if sin2_t <= 1.0 {
            mask |= 1u8 << j;
        }
    }
    mask
}

/// The spectral-MIS weight of a collected leaf of channel `channel`.
///
/// `N * path_pdf[hero] / sum_{j in family} path_pdf[j]`, where the family is the techniques that
/// are compatible with `channel` at this event (`compat_c`, bit `j` set) and produce the leaf
/// (`producers`, see [`transmitting_techniques`]; `u8::MAX` for an entry reflection, which every
/// technique produces). A technique that is dead (`path_pdf[j] == 0`) drops out of the sum. Over a
/// chain without sampled branches every density is one and the weight is `N / #family`; summed
/// over the heroes of the family it is exactly one, which makes the estimator unbiased. Zero when
/// the hero is not in the family (the channel left the hero's path, and its radiance is zero).
pub(super) fn leaf_mis_weight(
    compat_c: u8,
    path_pdf: &[f32; NUM_CHANNELS],
    hero: usize,
    producers: u8,
) -> f32 {
    let family = compat_c & producers;
    if family & (1u8 << hero) == 0 {
        return 0.0;
    }
    let mut family_pdf = 0.0f32;
    for (j, &pdf) in path_pdf.iter().enumerate() {
        if family & (1u8 << j) != 0 {
            family_pdf += pdf;
        }
    }
    if family_pdf <= 1e-12 {
        return 0.0;
    }
    (NUM_CHANNELS as f32) * path_pdf[hero] / family_pdf
}

/// Puts the densities back to `saved` for every channel that is still alive.
///
/// The collected estimator follows its chain with probability one, so the branch factors a
/// bounce function folded into `path_pdf` are undone; a channel whose density became exactly zero
/// (it cannot transmit where the hero did) stays dead.
pub(super) fn restore_densities(saved: &[f32; NUM_CHANNELS], path_pdf: &mut [f32; NUM_CHANNELS]) {
    for (pdf, &before) in path_pdf.iter_mut().zip(saved) {
        *pdf = if *pdf > 0.0 { before } else { 0.0 };
    }
}

/// The exit leaves of one interior event: per channel its own direction out of the gem and the
/// transmitted intensity, plus what the MIS weight needs.
pub(super) struct ExitLeaves<'a> {
    pub(super) lambdas: &'a [f32; NUM_CHANNELS],
    pub(super) dirs: &'a [Option<Vec3>; NUM_CHANNELS],
    pub(super) intensities: &'a [f32; NUM_CHANNELS],
    pub(super) path_pdf: &'a [f32; NUM_CHANNELS],
    pub(super) hero: usize,
    pub(super) producers: u8,
}

/// Adds every channel's exit leaf to `collect.collected`: its transmitted intensity times the
/// environment along its own direction, times its [`leaf_mis_weight`].
pub(super) fn deposit_exit_leaves(
    event: &ExitEvent<'_, '_>,
    collect: &mut ExitCollect<'_>,
    leaves: &ExitLeaves<'_>,
) {
    if !collect.credit {
        return;
    }
    let exit = &*event.exit;
    for k in 0..NUM_CHANNELS {
        let Some(dir) = leaves.dirs[k] else {
            continue;
        };
        let intensity = leaves.intensities[k].max(0.0);
        if intensity <= 0.0 {
            continue;
        }
        let weight = leaf_mis_weight(
            exit.compat[k],
            leaves.path_pdf,
            leaves.hero,
            leaves.producers,
        );
        if weight <= 0.0 {
            continue;
        }
        let env_spectral = sample_environment_channel_at(
            exit.environment,
            event.hit_point + dir * 1e-4,
            dir,
            leaves.lambdas[k],
            exit.studio_rig.as_ref(),
            exit.observer,
        );
        collect.collected[k] = f32::mul_add(intensity * weight, env_spectral, collect.collected[k]);
    }
}

/// The mirror reflection at an entry: one direction for every channel, and each channel's
/// reflected Stokes state.
pub(super) struct EntryLeaf<'a> {
    pub(super) dir: Vec3,
    pub(super) lambdas: &'a [f32; NUM_CHANNELS],
    pub(super) stokes: &'a [StokesVector; NUM_CHANNELS],
    pub(super) path_pdf: &'a [f32; NUM_CHANNELS],
    pub(super) hero: usize,
}

/// Adds the mirror reflection of an entry to `collect.collected`: each channel's reflected
/// intensity times the environment along the mirror direction, times `collect.entry_leaf_scale`
/// (the surface glare at bounce 0) and its [`leaf_mis_weight`] (every technique produces it).
pub(super) fn deposit_entry_leaf(
    event: &ExitEvent<'_, '_>,
    collect: &mut ExitCollect<'_>,
    leaf: &EntryLeaf<'_>,
) {
    if !collect.credit {
        return;
    }
    let exit = &*event.exit;
    let env_spectral = sample_environment_channels_at(
        exit.environment,
        event.hit_point + leaf.dir * 1e-4,
        leaf.dir,
        leaf.lambdas,
        exit.studio_rig.as_ref(),
        exit.observer,
    );
    let scale = collect.entry_leaf_scale;
    for (k, (collected, &env)) in collect.collected.iter_mut().zip(&env_spectral).enumerate() {
        let weight = leaf_mis_weight(exit.compat[k], leaf.path_pdf, leaf.hero, u8::MAX);
        let intensity = leaf.stokes[k].intensity() * weight * scale;
        *collected = f32::mul_add(intensity, env, *collected);
    }
}

#[cfg(test)]
mod collect_weight_tests {
    use super::*;

    /// With every technique alive and producing the leaf, the weight is one.
    #[test]
    fn full_family_weight_is_one() {
        let pdf = [1.0f32; NUM_CHANNELS];
        for hero in 0..NUM_CHANNELS {
            let w = leaf_mis_weight(u8::MAX, &pdf, hero, u8::MAX);
            assert!((w - 1.0).abs() < 1e-6, "hero {hero}: weight {w}");
        }
    }

    /// Summed over a family's heroes, the weights total `N` (so `sum_j (1/N) * w_j == 1`).
    #[test]
    fn weights_over_a_family_sum_to_the_channel_count() {
        let pdf = [1.0f32; NUM_CHANNELS];
        let family = 0b0001_1110u8;
        let total: f32 = (0..NUM_CHANNELS)
            .filter(|&hero| family & (1u8 << hero) != 0)
            .map(|hero| leaf_mis_weight(family, &pdf, hero, u8::MAX))
            .sum();
        assert!((total - NUM_CHANNELS as f32).abs() < 1e-5, "total {total}");
    }

    /// A technique that is totally reflected here does not produce the leaf: the others share it,
    /// and the reflected hero itself gets none.
    #[test]
    fn a_totally_reflected_technique_leaves_the_family() {
        let pdf = [1.0f32; NUM_CHANNELS];
        let mut sin2_t = [0.5f32; NUM_CHANNELS];
        sin2_t[2] = 1.5;
        let producers = transmitting_techniques(&sin2_t);
        assert_eq!(producers, !(1u8 << 2));
        let w = leaf_mis_weight(u8::MAX, &pdf, 0, producers);
        assert!((w - 8.0 / 7.0).abs() < 1e-6, "weight {w}");
        assert!(leaf_mis_weight(u8::MAX, &pdf, 2, producers).abs() < f32::EPSILON);
    }

    /// A dead technique drops out of the sum, and a hero outside the family gets no weight.
    #[test]
    fn dead_and_foreign_heroes() {
        let mut pdf = [1.0f32; NUM_CHANNELS];
        pdf[1] = 0.0;
        let w = leaf_mis_weight(u8::MAX, &pdf, 0, u8::MAX);
        assert!((w - 8.0 / 7.0).abs() < 1e-6, "weight {w}");
        assert!(leaf_mis_weight(0b0000_0110, &pdf, 0, u8::MAX).abs() < f32::EPSILON);
    }

    /// The densities come back for every live channel; a channel that died stays dead.
    #[test]
    fn restore_densities_keeps_the_dead_dead() {
        let saved = [1.0f32, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 8.0];
        let mut now = [0.5f32; NUM_CHANNELS];
        now[3] = 0.0;
        restore_densities(&saved, &mut now);
        assert_eq!(now, [1.0, 2.0, 3.0, 0.0, 5.0, 6.0, 7.0, 8.0]);
    }
}
