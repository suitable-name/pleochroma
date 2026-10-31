//! Scattering off oriented inclusions (silk, needles, platelets).
//!
//! The estimator of [`super::hg::maybe_scatter_or_extinguish`] with the scattering coefficient
//! made direction- and wavelength-dependent. Along one straight segment the direction `w` is
//! constant and the host homogeneous, so `sigma_t,k = alpha_k + sigma_s,k(w)` is constant per
//! channel and the same exponential free-path sampler applies, driven by the hero channel.
//!
//! # The medium
//!
//! `sigma_s,k(w) = sigma_HG + sum_f sigma_f(w, lambda_k)`: the plain Henyey-Greenstein haze
//! (achromatic, `GemMaterial::scattering_sigma_s`) plus the families of the material's
//! [`InclusionSet`]. A needle extinguishes `~ sin(theta)` (`theta` the angle to its axis), a
//! platelet `~ |w . m|`, random needles `pi / 4` on average; each family is scaled by
//! `(550 nm / lambda)^p`.
//!
//! # One direction, a mixture of channels
//!
//! The event picks a component `c` (the haze or one family) with probability
//! `sigma_c(w, lambda_hero) / sigma_s(w, lambda_hero)` and samples the scattered direction
//! from that component's phase function; the direction is shared by every channel. When the
//! channels' mixtures differ (families with different exponents, or a chromatic family beside
//! the achromatic haze) each channel `k` owes the ratio of its own mixture phase
//! `sum_c (sigma_c,k / sigma_s,k) p_c(w')` to the hero's at the sampled direction. The
//! estimator returns that ratio rather than applying it: the scattering-point next-event
//! estimate that follows must use the weight WITHOUT it (it evaluates the phase at the light
//! direction, not at the sampled one), and the caller multiplies it in afterwards.
//!
//! # Randoms
//!
//! Distance and direction reuse [`DISTANCE_SAMPLE_STREAM`], [`PHASE_DIR_U_STREAM`] and
//! [`PHASE_DIR_V_STREAM`]; the component choice (which also selects the axis of a random
//! needle, after remapping) has its own [`ORIENTED_COMPONENT_STREAM`].
//!
//! # Random needles
//!
//! Rutilated quartz has needles of every orientation. Their phase function is the mixture,
//! over a fixed set of [`RANDOM_NEEDLE_AXES`] axes spread evenly over a hemisphere, of the
//! needle cone of each axis weighted by its extinction `sin(theta)`. The same discrete mixture
//! is sampled and evaluated, so the two agree exactly; the quantisation is the finite number
//! of needles a real stone has.

use super::{
    super::{
        NUM_CHANNELS,
        absorption::channel_absorption_alphas_assigned,
        camera::{FacetFinish, Ray},
        color::{integrate_channels_to_xyz, integrate_channels_to_xyz_families},
        environment::EnvironmentSource,
        refraction::{RayMaterialContext, RayWavelengthCache},
        sampling::{DISTANCE_SAMPLE_STREAM, PHASE_DIR_U_STREAM, PHASE_DIR_V_STREAM, hash_u32},
        transport::apply_russian_roulette,
    },
    NeeContext,
    delta_tracking::maybe_scatter_oriented_bragg,
    hg::{ScatterStepOutcome, henyey_greenstein_phase, sample_henyey_greenstein_direction},
    hg_nee::{ShadowAlphas, nee_contribution_oriented_scatter},
    oriented_zoned::{maybe_scatter_oriented_zoned, maybe_scatter_oriented_zoned_bragg},
};
use crate::optics::{
    inclusions::{
        FamilyKind, InclusionSet, MAX_FAMILIES, ResolvedFamily,
        needle::{needle_pdf, needle_sample},
        platelet::{platelet_pdf, platelet_sample},
        spectral::spectral_factor,
    },
    materials::GemMaterial,
    polarization::StokesVector,
};
use glam::Vec3;
use std::{f32::consts::FRAC_PI_4, sync::LazyLock};

/// The RNG stream of the component choice. The WGSL twin (`11_oriented_scattering.wgsl`)
/// carries the same value.
pub(crate) const ORIENTED_COMPONENT_STREAM: u32 = 0x7F4A_7C15;

/// How many needle axes stand in for "every orientation" (rutilated quartz).
pub(crate) const RANDOM_NEEDLE_AXES: usize = 64;

/// Components of the mixture: the haze first, then the families.
pub(crate) const COMPONENTS: usize = 1 + MAX_FAMILIES;

/// The golden angle, radians.
const GOLDEN_ANGLE: f32 = 2.399_963_2;

/// The fixed needle axes of a random-needle family: a Fibonacci spiral over the upper
/// hemisphere (an axis and its negative are the same line).
static RANDOM_AXES: LazyLock<[Vec3; RANDOM_NEEDLE_AXES]> = LazyLock::new(|| {
    std::array::from_fn(|i| {
        let z = (i as f32 + 0.5) / RANDOM_NEEDLE_AXES as f32;
        // Separate multiply and subtract, as in the WGSL twins (`11_oriented_scattering.wgsl`,
        // `12_bragg_domains.wgsl`); a fused `mul_add` would round differently. `allow`, not
        // `expect`: older clippy does not raise it.
        #[allow(clippy::suboptimal_flops)]
        let r = (1.0 - z * z).max(0.0).sqrt();
        let (sin, cos) = (i as f32 * GOLDEN_ANGLE).sin_cos();
        Vec3::new(r * cos, r * sin, z)
    })
});

/// A scattering medium with oriented inclusions, for one ray (one hero comb).
#[derive(Clone, Copy)]
pub(crate) struct OrientedMedium<'a> {
    /// The families.
    pub(crate) set: &'a InclusionSet,
    /// The plain haze `sigma_s`, per model unit (`GemMaterial::scattering_sigma_s`).
    pub(crate) sigma_hg_model: f32,
    /// The haze asymmetry (`GemMaterial::scattering_g`).
    pub(crate) g: f32,
    /// The ray's eight wavelengths, nanometres.
    pub(crate) lambdas: &'a [f32; NUM_CHANNELS],
    /// Which channel drives the shared geometric path.
    pub(crate) hero_idx: usize,
    /// `GemMaterial::absorption_path_scale`.
    pub(crate) path_scale: f32,
}

/// `x` rounded into `[0, 1)` for use as a remapped random.
fn unit(x: f32) -> f32 {
    x.clamp(0.0, 1.0 - 1e-6)
}

/// The unit random of the RNG stream `stream` at `(rng_seed, bounce)`, exactly as the other
/// streams of this tree draw theirs.
pub(super) fn stream_draw(rng_seed: u32, bounce: u32, stream: u32) -> f32 {
    (hash_u32(rng_seed ^ hash_u32(bounce ^ stream)) as f32) / 4_294_967_295.0
}

/// The extinction of one family for the propagation direction `w` at `lambda_nm`, per model
/// unit.
#[expect(
    clippy::suboptimal_flops,
    reason = "the WGSL twin and the pinned goldens use this exact arithmetic"
)]
fn family_extinction(family: &ResolvedFamily, w: Vec3, lambda_nm: f32) -> f32 {
    let shape = match family.kind {
        FamilyKind::Needle => {
            let along = w.dot(family.dir);
            (1.0 - along * along).max(0.0).sqrt()
        }
        FamilyKind::Platelet => w.dot(family.dir).abs(),
        FamilyKind::RandomNeedle => FRAC_PI_4,
    };
    family.sigma * spectral_factor(lambda_nm, family.p) * shape
}

/// The weights of the random-needle axes for the incoming direction (each axis's `sin(theta)`)
/// and their sum.
#[expect(
    clippy::suboptimal_flops,
    reason = "the WGSL twin and the pinned goldens use this exact arithmetic"
)]
fn random_needle_weights(w_i: Vec3) -> ([f32; RANDOM_NEEDLE_AXES], f32) {
    let axes = &*RANDOM_AXES;
    let weights: [f32; RANDOM_NEEDLE_AXES] = std::array::from_fn(|j| {
        let along = w_i.dot(axes[j]);
        (1.0 - along * along).max(0.0).sqrt()
    });
    let total = weights.iter().sum();
    (weights, total)
}

/// The random-needle phase function: the discrete mixture of the needle cones.
pub(crate) fn random_needle_phase(w_i: Vec3, beta: f32, rho: f32, w_o: Vec3) -> f32 {
    let (weights, total) = random_needle_weights(w_i);
    if total <= 0.0 {
        return 0.0;
    }
    let sum: f32 = RANDOM_AXES
        .iter()
        .zip(&weights)
        .map(|(axis, weight)| weight * needle_pdf(w_i, *axis, beta, rho, w_o))
        .sum();
    sum / total
}

/// Draws from the random-needle mixture: `u_axis` picks the axis, `u1`/`u2` the cone sample.
pub(crate) fn random_needle_sample(
    w_i: Vec3,
    beta: f32,
    rho: f32,
    u_axis: f32,
    u1: f32,
    u2: f32,
) -> Vec3 {
    let (weights, total) = random_needle_weights(w_i);
    let target = u_axis * total;
    let mut acc = 0.0f32;
    let mut chosen = RANDOM_NEEDLE_AXES - 1;
    for (j, weight) in weights.iter().enumerate() {
        acc += weight;
        if target < acc {
            chosen = j;
            break;
        }
    }
    needle_sample(w_i, RANDOM_AXES[chosen], beta, rho, u1, u2)
}

/// The phase function of one family, per steradian.
fn family_phase(family: &ResolvedFamily, w_i: Vec3, w_o: Vec3) -> f32 {
    match family.kind {
        FamilyKind::Needle => needle_pdf(w_i, family.dir, family.lobe, family.rho, w_o),
        FamilyKind::Platelet => platelet_pdf(w_i, family.dir, family.lobe, w_o),
        FamilyKind::RandomNeedle => random_needle_phase(w_i, family.lobe, family.rho, w_o),
    }
}

/// The normalised mixture phase for component extinctions `sig` and phases `phases`.
fn mixture(sig: &[f32; COMPONENTS], phases: &[f32; COMPONENTS]) -> f32 {
    let total: f32 = sig.iter().sum();
    if total <= 0.0 {
        return 0.0;
    }
    let weighted: f32 = sig.iter().zip(phases).map(|(s, p)| s * p).sum();
    weighted / total
}

impl OrientedMedium<'_> {
    /// Per-component extinction for direction `w` at `lambda_nm`, per model unit: the haze at
    /// index 0, the families after it (unused slots are zero).
    pub(crate) fn extinctions(&self, w: Vec3, lambda_nm: f32) -> [f32; COMPONENTS] {
        let mut sig = [0.0f32; COMPONENTS];
        sig[0] = self.sigma_hg_model.max(0.0);
        for (slot, family) in sig[1..].iter_mut().zip(self.set.families()) {
            *slot = family_extinction(family, w, lambda_nm);
        }
        sig
    }

    /// The total scattering coefficient for direction `w` at `lambda_nm`, per model unit.
    pub(crate) fn sigma_s(&self, w: Vec3, lambda_nm: f32) -> f32 {
        self.extinctions(w, lambda_nm).iter().sum()
    }

    /// The phase function of every component for the turn `w_i -> w_o`.
    pub(crate) fn phases(&self, w_i: Vec3, w_o: Vec3) -> [f32; COMPONENTS] {
        let mut phases = [0.0f32; COMPONENTS];
        phases[0] = if self.sigma_hg_model > 0.0 {
            henyey_greenstein_phase(w_i.dot(w_o), self.g)
        } else {
            0.0
        };
        for (slot, family) in phases[1..].iter_mut().zip(self.set.families()) {
            *slot = family_phase(family, w_i, w_o);
        }
        phases
    }

    /// The mixture phase function at one wavelength.
    #[cfg(test)]
    pub(crate) fn phase(&self, lambda_nm: f32, w_i: Vec3, w_o: Vec3) -> f32 {
        mixture(&self.extinctions(w_i, lambda_nm), &self.phases(w_i, w_o))
    }

    /// The mixture phase of every channel (each with its own wavelength's weights) and the
    /// hero's, for the turn `w_i -> w_o`.
    pub(crate) fn channel_phases(&self, w_i: Vec3, w_o: Vec3) -> (f32, [f32; NUM_CHANNELS]) {
        let phases = self.phases(w_i, w_o);
        let per_channel: [f32; NUM_CHANNELS] =
            std::array::from_fn(|k| mixture(&self.extinctions(w_i, self.lambdas[k]), &phases));
        (per_channel[self.hero_idx], per_channel)
    }

    /// Draws the scattered direction from the mixture at the hero wavelength: `u_c` picks the
    /// component, `u1`/`u2` drive its sampler.
    pub(crate) fn sample_direction(&self, w_i: Vec3, u_c: f32, u1: f32, u2: f32) -> Vec3 {
        let sig = self.extinctions(w_i, self.lambdas[self.hero_idx]);
        let total: f32 = sig.iter().sum();
        if total <= 0.0 {
            // No component can scatter here: fall back to the haze's own distribution.
            return sample_henyey_greenstein_direction(u1, u2, self.g, w_i);
        }
        let target = u_c * total;
        let mut acc = 0.0f32;
        let mut chosen = 0usize;
        let mut local = 0.0f32;
        for (c, s) in sig.iter().enumerate() {
            if *s <= 0.0 {
                continue;
            }
            chosen = c;
            local = unit((target - acc) / s);
            acc += s;
            if target < acc {
                break;
            }
        }
        if chosen == 0 {
            return sample_henyey_greenstein_direction(u1, u2, self.g, w_i);
        }
        let family = &self.set.families()[chosen - 1];
        match family.kind {
            FamilyKind::Needle => needle_sample(w_i, family.dir, family.lobe, family.rho, u1, u2),
            FamilyKind::Platelet => platelet_sample(w_i, family.dir, family.lobe, u1, u2),
            FamilyKind::RandomNeedle => {
                random_needle_sample(w_i, family.lobe, family.rho, local, u1, u2)
            }
        }
    }
}

/// What [`maybe_scatter_oriented`] found when an event fired.
pub(crate) struct OrientedScatter {
    /// Where along the old direction the event is, model units.
    pub(crate) t_free_model: f32,
    /// The sampled direction.
    pub(crate) new_dir: Vec3,
    /// Per channel, the ratio of its own mixture phase to the hero's at `new_dir`; the caller
    /// multiplies it into the Stokes intensities AFTER the scattering-point NEE.
    pub(crate) phase_ratio: [f32; NUM_CHANNELS],
    /// The hero's mixture phase at `new_dir`: the density the direction was sampled with.
    pub(crate) hero_phase: f32,
}

/// The per-bounce extinction/scatter estimator for a medium with oriented inclusions.
///
/// The same decisions as [`super::hg::maybe_scatter_or_extinguish`] (read its doc comment) with
/// the per-channel coefficient `sigma_s,k(w)` in place of the scalar, and the mixture sampler in
/// place of Henyey-Greenstein. `Some` if an event fired strictly before `hit_t`; `None` if the
/// path survived to the facet (`stokes` and `path_pdf` then carry the survival weight).
///
/// `pub(crate)`: `renderer::gpu::transport_check`'s Tier 2 check of the WGSL twin calls it.
#[expect(
    clippy::too_many_arguments,
    reason = "mirrors maybe_scatter_or_extinguish's own per-ray inputs one for one so the WGSL \
              translation keeps its direct correspondence"
)]
pub(crate) fn maybe_scatter_oriented(
    medium: &OrientedMedium<'_>,
    alphas: &[f32; NUM_CHANNELS],
    ray_dir: Vec3,
    hit_t: f32,
    rng_seed: u32,
    bounce: u32,
    stokes: &mut [StokesVector; NUM_CHANNELS],
    path_pdf: &mut [f32; NUM_CHANNELS],
) -> Option<OrientedScatter> {
    let hero = medium.hero_idx;
    let path_scale = medium.path_scale;
    // Per absorption-length unit, like `alphas`.
    let sigma_s: [f32; NUM_CHANNELS] =
        std::array::from_fn(|k| medium.sigma_s(ray_dir, medium.lambdas[k]) / path_scale);
    let sigma_t_hero = (alphas[hero] + sigma_s[hero]).max(1e-12);

    let dist_rand = stream_draw(rng_seed, bounce, DISTANCE_SAMPLE_STREAM);
    let one_minus_u = (1.0 - dist_rand).max(1e-7);
    let t_free = -(one_minus_u.ln()) / sigma_t_hero;
    let hit_t_scaled = hit_t * path_scale;

    if t_free < hit_t_scaled {
        let pdf_hero = sigma_t_hero * one_minus_u;
        let mut tr_args = [0f32; NUM_CHANNELS];
        for k in 0..NUM_CHANNELS {
            tr_args[k] = -(alphas[k] + sigma_s[k]) * t_free;
        }
        let tr = crate::simd::exp_f32x8(tr_args);
        for k in 0..NUM_CHANNELS {
            let weight = tr[k] * sigma_s[k] / pdf_hero;
            stokes[k] = StokesVector::unpolarized(stokes[k].intensity() * weight);
            path_pdf[k] *= (alphas[k] + sigma_s[k]) * tr[k];
        }
        let u_c = stream_draw(rng_seed, bounce, ORIENTED_COMPONENT_STREAM);
        let u1 = stream_draw(rng_seed, bounce, PHASE_DIR_U_STREAM);
        let u2 = stream_draw(rng_seed, bounce, PHASE_DIR_V_STREAM);
        let new_dir = medium.sample_direction(ray_dir, u_c, u1, u2);
        let (hero_phase, per_channel) = medium.channel_phases(ray_dir, new_dir);
        let phase_ratio: [f32; NUM_CHANNELS] = std::array::from_fn(|k| {
            if hero_phase > 1e-12 {
                per_channel[k] / hero_phase
            } else {
                1.0
            }
        });
        Some(OrientedScatter {
            t_free_model: t_free / path_scale,
            new_dir,
            phase_ratio,
            hero_phase,
        })
    } else {
        let mut survive_args = [0f32; NUM_CHANNELS];
        for k in 0..NUM_CHANNELS {
            survive_args[k] = -(alphas[k] + sigma_s[k]) * hit_t_scaled;
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

/// The phase density and direction a scatter hands to the next bounce as its MIS partner, or
/// `None` when the density is not positive.
///
/// A density of 0 (an ideal-cone lobe, a Bragg reflection) is no partner: carrying it would make
/// the escape weight `balance_heuristic(0, light)` zero and drop the lobe's energy. The GPU
/// (`08b_extend_scatter.wgsl`, `phase_pdf_this_check > 0.0`) takes weight 1 there.
pub(super) fn mis_phase_carry(hero_phase: f32, new_dir: Vec3) -> Option<(f32, Vec3)> {
    (hero_phase > 0.0).then_some((hero_phase, new_dir))
}

/// [`super::hg::try_scatter_step`] for a material with oriented inclusions: the scatter-or-
/// survive decision, the scattering-point next-event estimate and Russian roulette, with the
/// mixture phase function in place of Henyey-Greenstein. `hg::try_scatter_step` dispatches
/// here; the parameters and the return value are its own.
#[expect(
    clippy::too_many_arguments,
    reason = "the argument list is try_scatter_step's own, which dispatches here"
)]
#[expect(
    clippy::too_many_lines,
    reason = "one scatter step: sample, weight, split and NEE in sequence"
)]
pub(in super::super) fn try_scatter_step_oriented(
    set: &InclusionSet,
    mat_ctx: &RayMaterialContext,
    cache: &RayWavelengthCache,
    material: &GemMaterial,
    is_extraordinary: bool,
    current_ray: &mut Ray,
    current_k: &mut Vec3,
    hit_t: f32,
    rng_seed: u32,
    bounce: u32,
    stokes: &mut [StokesVector; NUM_CHANNELS],
    path_pdf: &mut [f32; NUM_CHANNELS],
    split_radiance: &mut [f32; NUM_CHANNELS],
    nee: NeeContext<'_>,
    lambdas: &[f32; NUM_CHANNELS],
    nee_xyz: &mut Vec3,
    enable_exit_splitting: bool,
    compat: [u8; NUM_CHANNELS],
    facet_finishes: &[FacetFinish],
) -> ScatterStepOutcome {
    let alphas = channel_absorption_alphas_assigned(mat_ctx, cache, *current_k, is_extraordinary);
    let medium = OrientedMedium {
        set,
        sigma_hg_model: material.scattering_sigma_s,
        g: material.scattering_g,
        lambdas,
        hero_idx: mat_ctx.hero_idx,
        path_scale: material.absorption_path_scale,
    };
    // A zoned host keeps its absorption per zone along the segment, and the shadow ray's
    // transmittance would need its own zone lengths: scattering-point NEE is off, as for the
    // zoned plain haze.
    let zoned_alphas = cache
        .zoned
        .as_ref()
        .map(|zoned| zoned.zone_alphas(mat_ctx, cache, *current_k, is_extraordinary));
    let nee = if zoned_alphas.is_some() {
        NeeContext {
            enabled: false,
            ..nee
        }
    } else {
        nee
    };
    // A Bragg reflection (opal, labradorite) is a specular event with a mosaic lobe: no
    // scattering-point NEE, no phase MIS partner (see `delta_tracking`).
    let mut bragg_event = false;
    let event = match (cache.zoned.as_ref(), zoned_alphas.as_ref()) {
        // A zoned host keeps its zones' absorption per segment; with a Bragg structure the
        // structure reflects on top of it (`maybe_scatter_oriented_zoned_bragg`).
        (Some(zoned), Some(alphas)) => match set.structure() {
            Some(structure) => maybe_scatter_oriented_zoned_bragg(
                &medium,
                structure,
                zoned,
                alphas,
                *current_ray,
                hit_t,
                rng_seed,
                bounce,
                stokes,
                path_pdf,
            )
            .map(|found| {
                bragg_event = found.bragg;
                found.scatter
            }),
            None => maybe_scatter_oriented_zoned(
                &medium,
                zoned,
                alphas,
                *current_ray,
                hit_t,
                rng_seed,
                bounce,
                stokes,
                path_pdf,
            ),
        },
        _ => match set.structure() {
            Some(structure) => maybe_scatter_oriented_bragg(
                &medium,
                structure,
                &alphas,
                *current_ray,
                hit_t,
                rng_seed,
                bounce,
                stokes,
                path_pdf,
            )
            .map(|found| {
                bragg_event = found.bragg;
                found.scatter
            }),
            None => maybe_scatter_oriented(
                &medium,
                &alphas,
                current_ray.dir,
                hit_t,
                rng_seed,
                bounce,
                stokes,
                path_pdf,
            ),
        },
    };
    let Some(scatter) = event else {
        return ScatterStepOutcome::ReachedBoundary;
    };
    let nee = if bragg_event {
        NeeContext {
            enabled: false,
            ..nee
        }
    } else {
        nee
    };
    let old_dir = current_ray.dir;
    let scatter_point = current_ray.origin + scatter.t_free_model * old_dir;

    // The direct-light sample, with the weight as it stands BEFORE the phase ratio (see the
    // module doc), integrated to XYZ with this event's own `path_pdf`/`compat`.
    let mut nee_deposit = [0.0f32; NUM_CHANNELS];
    // Silk is anisotropic in its own right, and an anisotropic crystal's absorption also depends
    // on the direction: the shadow ray re-evaluates both at the shadow direction.
    let shadow_alphas = if mat_ctx.is_anisotropic {
        ShadowAlphas::AssignedMode {
            ctx: mat_ctx,
            cache,
            is_extraordinary,
        }
    } else {
        ShadowAlphas::Fixed(&alphas)
    };
    nee_contribution_oriented_scatter(
        nee,
        cache.n_o_ch[mat_ctx.hero_idx],
        scatter_point,
        old_dir,
        &medium,
        rng_seed,
        bounce,
        stokes,
        &mut nee_deposit,
        shadow_alphas,
        facet_finishes,
    );
    *nee_xyz += if enable_exit_splitting {
        integrate_channels_to_xyz_families(
            &nee_deposit,
            lambdas,
            path_pdf,
            mat_ctx.hero_idx,
            compat,
        )
    } else {
        integrate_channels_to_xyz(&nee_deposit, lambdas, path_pdf, mat_ctx.hero_idx)
    };

    // Now the sampled direction's own phase ratio goes into the continuation's weight.
    for (stokes_k, ratio) in stokes.iter_mut().zip(scatter.phase_ratio.iter()) {
        *stokes_k = stokes_k.scale(*ratio);
    }
    current_ray.origin = scatter_point;
    current_ray.dir = scatter.new_dir;
    *current_k = scatter.new_dir;
    let phase_pdf_for_mis =
        if nee.enabled && matches!(nee.environment, EnvironmentSource::HdrMap(_)) {
            mis_phase_carry(scatter.hero_phase, scatter.new_dir)
        } else {
            None
        };
    if bounce > 4 && !apply_russian_roulette(bounce, rng_seed, stokes, split_radiance) {
        return ScatterStepOutcome::ScatteredAndTerminated;
    }
    ScatterStepOutcome::ScatteredAndSurvived(phase_pdf_for_mis)
}
