//! Birefringence for beams: uniaxial o/e children with fitted apexes, the optic-axis guard,
//! topology splits, and the biaxial resolved modes (beam guide §10).
//!
//! # The uniaxial retracer (§7.5, §10.1)
//!
//! [`uniaxial_ray`] is the exact ray of a branch prefix through a uniaxial stone. Every step is
//! solved the path tracer's way, in `f32` and with the tracer's own functions
//! (`theta_c_for_bounce`, [`BirefringenceParams::effective_extraordinary_index`],
//! [`BirefringenceParams::extraordinary_poynting_dir`]), so the retracer's `S` and `k` equal the
//! tracer's bit for bit at the same incoming wave:
//!
//! - **entry**: the mode named by the step word's bit is refracted with `n_o` (the o-ray, Snell) or
//!   with `n_e(θ)` at the wave normal's own angle (the e-ray, the `theta_c` fixed point), and the
//!   e-ray's Poynting direction leaves the plane of incidence;
//! - **internal reflection**: `k' = reflect(k)`; `S'` is `k'` unless the hero is past its critical
//!   angle or `k` is parallel to the optic axis, where the tracer re-derives the walk-off of the
//!   **pre-relabel** mode (`reflected(.., walk_off)` in `path_cache::bounce`); the step word's bit
//!   is the mode after the relabel and does not change `S'`;
//! - **exit**: Snell with the leaving mode's index, `S' = k'`.
//!
//! Geometry (intersections, partition, apex fits) follows `S`; angles, Fresnel and the Stokes frame
//! follow `k` (§10.1).

use glam::{DVec2, DVec3, Vec3};

use super::{
    beam::{Beam, BeamMode, BranchPrefix, MAX_PREFIX, OPTIC_AXIS_GUARD_RAD, SpectralNode},
    entry::fit_apex,
    retrace::{
        PrefixRay, RetraceError, RetraceKit, camera_direction, is_total_reflection, step_code,
        step_facet,
    },
};
use crate::optics::{
    birefringence::{BiaxialIndicatrix, BirefringenceParams},
    materials::{CrystalSystem, GemMaterial},
    raytracer::{
        NUM_CHANNELS,
        refraction::{RayMaterialContext, theta_c_for_bounce},
    },
};

/// Below this `|k × c|²` the wave normal counts as parallel to the optic axis and `expand` falls
/// back to the scalar physics. Must equal `AXIS_PARALLEL_EPS` in `path_cache::bounce`.
pub const AXIS_PARALLEL_EPS: f32 = 1.0e-6;

/// The event code of a transmission in a branch step word.
const TRANSMIT_CODE: u32 = 3;

/// The refractive index of the air outside the stone, as `f32` like the tracer's states.
const AIR_INDEX: f32 = 1.0;

/// `true` when the tracer treats `material` as anisotropic (`build_ray_material_context`'s rule).
#[must_use]
pub fn is_anisotropic(material: &GemMaterial) -> bool {
    material.crystal_system != CrystalSystem::Cubic && material.birefringence_delta.abs() > 1e-4
}

/// `true` when the anisotropy is genuinely biaxial (three principal indices).
#[must_use]
pub const fn is_biaxial(material: &GemMaterial) -> bool {
    material.biaxial_delta_beta_alpha.is_some()
}

/// `true` for an ordinary uniaxial birefringent material: the o and e eigenmodes of §10.1.
#[must_use]
pub fn is_uniaxial(material: &GemMaterial) -> bool {
    is_anisotropic(material) && !is_biaxial(material)
}

/// The mode of a ray inside (or outside) `material` as a [`BeamMode`].
#[must_use]
pub fn mode_of(material: &GemMaterial, inside: bool, is_ext: bool) -> BeamMode {
    if !inside || !is_anisotropic(material) {
        BeamMode::Isotropic
    } else if is_biaxial(material) {
        if is_ext {
            BeamMode::BiaxialB
        } else {
            BeamMode::BiaxialA
        }
    } else if is_ext {
        BeamMode::Extraordinary
    } else {
        BeamMode::Ordinary
    }
}

/// One wave of a birefringent medium: the wave normal `k`, the Poynting (ray) direction `S` and the
/// eigenmode label (§10.1).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Wave {
    /// The wave normal.
    pub k: Vec3,
    /// The Poynting direction, along which the geometry runs.
    pub s: Vec3,
    /// The eigenmode label (extraordinary).
    pub is_ext: bool,
}

/// The path tracer's per-ray context at one wavelength, as far as the direction solves read it.
const fn direction_context(material: &GemMaterial, lambda_nm: f32) -> RayMaterialContext<'_> {
    RayMaterialContext {
        material,
        lambdas: [lambda_nm; NUM_CHANNELS],
        hero_idx: 0,
        c_axis: material.c_axis,
        is_anisotropic: true,
        enable_internal_mode_coupling: true,
    }
}

/// The ordinary and extraordinary (principal) indices at `lambda_nm`, as the tracer's hero has them.
fn principal_indices(material: &GemMaterial, lambda_nm: f32) -> (f32, f32) {
    let n_o = material.dispersion.evaluate(lambda_nm);
    (n_o, material.extraordinary_index_at(lambda_nm, n_o))
}

/// The index of the mode inside the crystal at wave normal `k` (the tracer's `n_medium_hero`).
///
/// This is the tracer's value for `inside_gem`: `n_o` for the ordinary mode, `n_e(θ)` with
/// `θ = angle(k, c)` for the extraordinary. A biaxial stone gives the fast (mode A) or slow
/// (mode B) root of Fresnel's equation at `k`.
#[must_use]
pub fn mode_index(material: &GemMaterial, lambda_nm: f32, k: Vec3, is_ext: bool) -> f32 {
    if is_biaxial(material) {
        return biaxial_mode_index(material, lambda_nm, k, is_ext);
    }
    let (n_o, n_e) = principal_indices(material, lambda_nm);
    if !is_ext {
        return n_o;
    }
    let theta = k.dot(material.c_axis).clamp(-1.0, 1.0).abs().acos();
    BirefringenceParams::effective_extraordinary_index(n_o, n_e, theta)
}

/// Snell's law in `f32` exactly as `path_cache::bounce::refracted` has it.
fn refracted(k: Vec3, normal: Vec3, eta: f32, cos_i: f32) -> Vec3 {
    let sin2_t = (eta * eta * cos_i.mul_add(-cos_i, 1.0)).min(1.0);
    let cos_t = (1.0 - sin2_t).max(0.0).sqrt();
    (eta * k + f32::mul_add(eta, cos_i, -cos_t) * normal).normalize()
}

/// `sin²θt` of a mode with index `n1` leaving into air: the tracer's `geo.sin2_t`.
fn sin2_leaving(n1: f32, cos_i: f32) -> f32 {
    let eta = n1 / AIR_INDEX;
    eta * eta * cos_i.mul_add(-cos_i, 1.0)
}

/// The cosine of incidence of wave normal `k` on a facet with the incoming-facing `normal`.
fn incidence_cosine(k: Vec3, normal: Vec3) -> f32 {
    (-k).dot(normal).clamp(0.0, 1.0)
}

/// `true` when a ray of mode `is_ext` with wave normal `k` meets a facet of unit outward normal
/// `normal` past the critical angle (the tracer's hero decision `geo.sin2_t > 1`).
#[must_use]
pub fn is_total_reflection_mode(
    material: &GemMaterial,
    lambda_nm: f32,
    wave: (Vec3, bool),
    normal: Vec3,
) -> bool {
    let (k, is_ext) = wave;
    let cos_i = k.dot(normal).abs().clamp(0.0, 1.0);
    sin2_leaving(mode_index(material, lambda_nm, k, is_ext), cos_i) > 1.0
}

/// The refraction of the camera-side wave `k` into the crystal as mode `is_ext` (§10.1).
///
/// `normal` is the unit facet normal facing the incoming ray. The o-ray follows Snell's law with
/// `n_o`; the e-ray's index comes from the tracer's `theta_c` fixed point and its Poynting direction
/// from `extraordinary_poynting_dir`. `None` when the wave cannot enter (past the critical angle).
#[must_use]
pub fn entry_wave(
    material: &GemMaterial,
    lambda_nm: f32,
    k: Vec3,
    normal: Vec3,
    is_ext: bool,
) -> Option<Wave> {
    let (n_o, n_e) = principal_indices(material, lambda_nm);
    let cos_i = incidence_cosine(k, normal);
    let n2 = if is_ext {
        let ctx = direction_context(material, lambda_nm);
        let theta = theta_c_for_bounce(&ctx, normal, k, cos_i, false, false, n_o);
        BirefringenceParams::effective_extraordinary_index(n_o, n_e, theta)
    } else {
        n_o
    };
    let eta = 1.0 / n2;
    if eta * eta * cos_i.mul_add(-cos_i, 1.0) > 1.0 {
        return None;
    }
    let wave = refracted(k, normal, eta, cos_i);
    let s = if is_ext {
        BirefringenceParams::extraordinary_poynting_dir(wave, material.c_axis, n_o, n_e)
    } else {
        wave
    };
    Some(Wave { k: wave, s, is_ext })
}

/// The transmission of `wave` out of the crystal into air (§10.1): Snell with the leaving mode's
/// index, no walk-off outside. `normal` faces the incoming ray. `None` past the critical angle.
#[must_use]
pub fn exit_wave(material: &GemMaterial, lambda_nm: f32, wave: Wave, normal: Vec3) -> Option<Wave> {
    let n1 = mode_index(material, lambda_nm, wave.k, wave.is_ext);
    let cos_i = incidence_cosine(wave.k, normal);
    if sin2_leaving(n1, cos_i) > 1.0 {
        return None;
    }
    let k = refracted(wave.k, normal, n1 / AIR_INDEX, cos_i);
    Some(Wave {
        k,
        s: k,
        is_ext: wave.is_ext,
    })
}

/// The reflection of `wave` at a facet, labelled `new_ext` afterwards (§10.1).
///
/// `normal` faces the incoming ray and `inside` tells which side the wave is on. The wave normal is
/// mirrored; the Poynting direction is the mirror image `k'` too, except inside the crystal when the
/// hero is totally reflected or `k` is parallel to the axis: there the tracer re-derives the
/// walk-off of the **pre-relabel** mode (`wave.is_ext`), so a relabelled pair of children shares one
/// `S` (§10.1, the shared-`s` pin of §16.1).
#[must_use]
pub fn reflect_wave(
    material: &GemMaterial,
    lambda_nm: f32,
    wave: Wave,
    normal: Vec3,
    (inside, new_ext): (bool, bool),
) -> Wave {
    let k = wave.k - 2.0 * wave.k.dot(normal) * normal;
    let walk_off = inside
        && (wave.k.cross(material.c_axis).length_squared() <= AXIS_PARALLEL_EPS
            || is_total_reflection_mode(material, lambda_nm, (wave.k, wave.is_ext), normal));
    let s = if walk_off && wave.is_ext {
        let (n_o, n_e) = principal_indices(material, lambda_nm);
        BirefringenceParams::extraordinary_poynting_dir(k, material.c_axis, n_o, n_e)
    } else {
        k
    };
    Wave {
        k,
        s,
        is_ext: new_ext,
    }
}

/// One step of a prefix through a uniaxial stone: the next wave, or `None` when the leaving wave is
/// past its critical angle.
fn step_wave(
    material: &GemMaterial,
    lambda_nm: f32,
    wave: Wave,
    (inside, word): (bool, u32),
    normal: Vec3,
) -> Option<Wave> {
    let mode_bit = word & 1 == 1;
    let biaxial = is_biaxial(material);
    if step_code(word) == TRANSMIT_CODE {
        if inside {
            exit_wave(material, lambda_nm, wave, normal)
        } else if biaxial {
            biaxial_entry_wave(material, lambda_nm, wave.k, normal, mode_bit)
        } else {
            entry_wave(material, lambda_nm, wave.k, normal, mode_bit)
        }
    } else if biaxial {
        Some(biaxial_reflect_wave(
            material,
            lambda_nm,
            wave,
            normal,
            (inside, inside && mode_bit),
        ))
    } else {
        Some(reflect_wave(
            material,
            lambda_nm,
            wave,
            normal,
            (inside, inside && mode_bit),
        ))
    }
}

/// The exact ray of `prefix` through `image_point` at `lambda_nm` in a uniaxial stone (§7.5).
///
/// The geometry follows the Poynting direction `S`; the wave normal and the final mode are returned
/// with it ([`PrefixRay::k`], [`PrefixRay::is_ext`]). An empty prefix is the camera ray.
///
/// # Errors
///
/// See [`RetraceError`]; a leaving or internally refracted wave past its critical angle is
/// `TotalReflection`.
pub fn uniaxial_ray(
    image_point: DVec2,
    lambda_nm: f32,
    prefix: &BranchPrefix,
    kit: &RetraceKit<'_>,
) -> Result<PrefixRay, RetraceError> {
    birefringent_ray(image_point, lambda_nm, prefix, kit)
}

/// The loop shared by [`uniaxial_ray`] and [`biaxial_ray`]; step solves depend on the material.
fn birefringent_ray(
    image_point: DVec2,
    lambda_nm: f32,
    prefix: &BranchPrefix,
    kit: &RetraceKit<'_>,
) -> Result<PrefixRay, RetraceError> {
    let material = kit.material;
    let first = camera_direction(kit.camera, kit.size, image_point).normalize();
    let mut origin = kit.camera.origin.as_dvec3();
    let (mut s, mut k) = (first, first);
    let (mut inside, mut is_ext) = (false, false);
    let mut out = PrefixRay {
        origin,
        dir: s,
        k,
        inside,
        is_ext,
        hits: [DVec3::ZERO; MAX_PREFIX],
        lengths: [0.0; MAX_PREFIX],
        len: prefix.len,
    };
    for (i, &word) in prefix.steps().iter().enumerate() {
        let step = u8::try_from(i).unwrap_or(u8::MAX);
        let facet = kit
            .stone
            .facet_of_original(step_facet(word))
            .ok_or(RetraceError::UnknownFacet { step })?;
        let (n_out, m) = kit.stone.plane(facet);
        let t = (m - n_out.dot(origin)) / n_out.dot(s);
        if !(t > 0.0 && t.is_finite()) {
            return Err(RetraceError::MissedPlane { step });
        }
        let hit = origin + s * t;
        out.hits[i] = hit;
        out.lengths[i] = t;
        if step_code(word) != TRANSMIT_CODE && !inside && i > 0 {
            return Err(RetraceError::Inconsistent { step });
        }
        let unit_out = n_out.normalize();
        let normal = (if inside { -unit_out } else { unit_out }).as_vec3();
        let wave = Wave {
            k: k.as_vec3(),
            s: s.as_vec3(),
            is_ext,
        };
        let next = step_wave(material, lambda_nm, wave, (inside, word), normal)
            .ok_or(RetraceError::TotalReflection { step })?;
        if step_code(word) == TRANSMIT_CODE {
            inside = !inside;
        }
        (origin, k, s, is_ext) = (hit, next.k.as_dvec3(), next.s.as_dvec3(), next.is_ext);
    }
    out.origin = origin;
    out.dir = s;
    out.k = k;
    out.inside = inside;
    out.is_ext = is_ext;
    Ok(out)
}

/// `true` when the exact ray `ray` meets `facet` past the critical angle at `lambda_nm`.
///
/// For a birefringent stone the test is the mode's own (`k`, `n_e(θ)` for the uniaxial e-ray, the
/// fast or slow root for a biaxial one); otherwise it is the isotropic [`is_total_reflection`] of the
/// ray direction, unchanged.
#[must_use]
pub fn ray_is_total_reflection(
    ray: &PrefixRay,
    facet: u32,
    lambda_nm: f32,
    kit: &RetraceKit<'_>,
) -> bool {
    if is_anisotropic(kit.material) {
        state_is_total_reflection((ray.k.as_vec3(), ray.is_ext), facet, lambda_nm, kit)
    } else {
        is_total_reflection(ray.dir, facet, lambda_nm, kit)
    }
}

/// [`ray_is_total_reflection`] for a state's own `(k, is_ext)` on a birefringent stone.
#[must_use]
pub fn state_is_total_reflection(
    wave: (Vec3, bool),
    facet: u32,
    lambda_nm: f32,
    kit: &RetraceKit<'_>,
) -> bool {
    let (n_out, _) = kit.stone.plane(facet);
    is_total_reflection_mode(kit.material, lambda_nm, wave, n_out.normalize().as_vec3())
}

/// `true` when the wave normal `k` lies within [`OPTIC_AXIS_GUARD_RAD`] of the optic axis `c_axis`
/// (either sense), where `expand` switches to its scalar physics (§10.2).
#[must_use]
pub fn near_optic_axis(k: Vec3, c_axis: Vec3) -> bool {
    let (k, c) = (k.as_dvec3(), c_axis.as_dvec3());
    k.cross(c).length().atan2(k.dot(c).abs()) <= OPTIC_AXIS_GUARD_RAD
}

/// `true` when a uniaxial `beam` has a sample point whose wave normal is inside the optic-axis guard:
/// it is not subdivided further and its energy goes to the residual (§10.2).
#[must_use]
pub fn beam_near_optic_axis(beam: &Beam, material: &GemMaterial) -> bool {
    is_uniaxial(material)
        && beam
            .state
            .points
            .iter()
            .any(|state| near_optic_axis(state.k, material.c_axis))
}

/// The largest angle between a ray and the line from `apex` to its hit; infinite when a hit sits on
/// the apex.
fn fit_error(hits: &[DVec3], dirs: &[DVec3], apex: DVec3) -> f64 {
    hits.iter()
        .zip(dirs)
        .map(|(&p, &d)| {
            (p - apex)
                .try_normalize()
                .map_or(f64::INFINITY, |v| d.cross(v).length().atan2(d.dot(v)))
        })
        .fold(0.0, f64::max)
}

/// The apex that the lines `hits[i] + t · dirs[i]` pass closest to (§8.3 procedure), or `None` when
/// the fit fails or its direction error exceeds `tolerance_rad`: the caller subdivides.
///
/// The e-ray's reflected pencil is not a mirror image of its apex (§10.1), so a reflected e-ray beam
/// refits its apex from the exact `S` of its sample points with the entry's own [`fit_apex`].
#[must_use]
pub fn refit_apex(hits: &[DVec3], dirs: &[DVec3], tolerance_rad: f64) -> Option<DVec3> {
    let apex = fit_apex(hits, dirs)?;
    let error = fit_error(hits, dirs, apex);
    (error.is_finite() && error <= tolerance_rad).then_some(apex)
}

// ---- biaxial (§10.3) ----

/// `true` for a genuinely biaxial anisotropic material: the resolved modes A and B of §10.3.
#[must_use]
pub fn is_biaxial_crystal(material: &GemMaterial) -> bool {
    is_anisotropic(material) && is_biaxial(material)
}

/// The index of biaxial mode A (`is_ext == false`, the fast root) or B (the slow root) at wave
/// normal `k`: the tracer's `n_biax_a_ch` / `n_biax_b_ch` while inside the crystal.
fn biaxial_mode_index(material: &GemMaterial, lambda_nm: f32, k: Vec3, is_ext: bool) -> f32 {
    material.biaxial_indicatrix(lambda_nm).map_or_else(
        || material.dispersion.evaluate(lambda_nm),
        |indicatrix| {
            let (slow, fast) = indicatrix.wave_indices(k);
            if is_ext { slow } else { fast }
        },
    )
}

/// The refraction of the camera-side wave `k` into a biaxial crystal as resolved mode `is_ext`
/// (`false` is mode A, `true` mode B), with the tracer's own solves (§10.3).
///
/// `normal` is the unit facet normal facing the incoming ray. The mode's wave normal and index come
/// from `BiaxialIndicatrix::resolve_entry_mode` (the two-step fixed point seeded with the ordinary
/// dispersion curve, as `compute_bounce_refraction_geometry` does) and its Poynting direction from
/// `BiaxialIndicatrix::mode_poynting_dir`; both modes walk off. `None` when the wave cannot enter.
#[must_use]
pub fn biaxial_entry_wave(
    material: &GemMaterial,
    lambda_nm: f32,
    k: Vec3,
    normal: Vec3,
    is_ext: bool,
) -> Option<Wave> {
    let indicatrix = material.biaxial_indicatrix(lambda_nm)?;
    let seed = material.dispersion.evaluate(lambda_nm);
    let cos_i = incidence_cosine(k, normal);
    let (_, direction) = indicatrix.resolve_entry_mode(k, normal, cos_i, seed, is_ext);
    let (slow, fast) = indicatrix.wave_indices(direction);
    let n2 = if is_ext { slow } else { fast };
    let eta = AIR_INDEX / n2;
    if eta * eta * cos_i.mul_add(-cos_i, 1.0) > 1.0 {
        return None;
    }
    let wave = refracted(k, normal, eta, cos_i);
    Some(Wave {
        k: wave,
        s: indicatrix.mode_poynting_dir(wave, is_ext),
        is_ext,
    })
}

/// The reflection of `wave` at a facet of a biaxial crystal, labelled `new_ext` afterwards (§10.3).
///
/// The wave normal is mirrored. Inside the crystal the Poynting direction is re-derived for the
/// **pre-relabel** mode `wave.is_ext` (both biaxial modes walk off, and the tracer's scalar bounce
/// always re-derives it, `poynting_dir_for_mode`), so the relabelled pair of children shares one
/// `S`; outside it is the mirror image `k'`.
#[must_use]
pub fn biaxial_reflect_wave(
    material: &GemMaterial,
    lambda_nm: f32,
    wave: Wave,
    normal: Vec3,
    (inside, new_ext): (bool, bool),
) -> Wave {
    let k = wave.k - 2.0 * wave.k.dot(normal) * normal;
    let s = match material.biaxial_indicatrix(lambda_nm) {
        Some(indicatrix) if inside => indicatrix.mode_poynting_dir(k, wave.is_ext),
        _ => k,
    };
    Wave {
        k,
        s,
        is_ext: new_ext,
    }
}

/// The exact ray of `prefix` through `image_point` at `lambda_nm` in a biaxial stone (§7.5, §10.3).
///
/// The loop of [`uniaxial_ray`] with the biaxial entry and reflection solves: the step word's mode
/// bit names resolved mode A (`0`) or B (`1`). The geometry follows `S`; the wave normal and the
/// final mode are returned with it ([`PrefixRay::k`], [`PrefixRay::is_ext`]).
///
/// # Errors
///
/// See [`RetraceError`]; a leaving wave past its critical angle is `TotalReflection`.
pub fn biaxial_ray(
    image_point: DVec2,
    lambda_nm: f32,
    prefix: &BranchPrefix,
    kit: &RetraceKit<'_>,
) -> Result<PrefixRay, RetraceError> {
    birefringent_ray(image_point, lambda_nm, prefix, kit)
}

/// The two optic axes (binormals) of `indicatrix` as unit wave normals, or `None` for an isotropic
/// indicatrix.
///
/// They lie in the `alpha`-`gamma` plane at the direction cosines `a² = (b_α − b_β) / (b_α − b_γ)`
/// (`b = 1/n²`): there both roots of Fresnel's equation coincide and conical refraction occurs.
/// The opposite senses are the same lines.
#[must_use]
pub fn biaxial_optic_axes(indicatrix: &BiaxialIndicatrix) -> Option<[Vec3; 2]> {
    let b = |n: f32| 1.0 / f64::from(n).powi(2);
    let (b_alpha, b_beta, b_gamma) = (
        b(indicatrix.n_alpha),
        b(indicatrix.n_beta),
        b(indicatrix.n_gamma),
    );
    let spread = b_alpha - b_gamma;
    if spread <= 0.0 || !spread.is_finite() {
        return None;
    }
    let along_alpha_sq = ((b_alpha - b_beta) / spread).clamp(0.0, 1.0);
    let (a, g) = (along_alpha_sq.sqrt(), (1.0 - along_alpha_sq).sqrt());
    let alpha = indicatrix.axes.x_axis.as_dvec3();
    let gamma = indicatrix.axes.z_axis.as_dvec3();
    Some([
        alpha.mul_add(DVec3::splat(a), gamma * g).as_vec3(),
        alpha.mul_add(DVec3::splat(a), gamma * -g).as_vec3(),
    ])
}

/// `true` when the wave normal `k` lies within [`OPTIC_AXIS_GUARD_RAD`] of either optic axis of
/// `indicatrix` (either sense): the conical-refraction neighbourhood of §10.3.
#[must_use]
pub fn near_biaxial_axis(k: Vec3, indicatrix: &BiaxialIndicatrix) -> bool {
    biaxial_optic_axes(indicatrix)
        .is_some_and(|axes| axes.iter().any(|&axis| near_optic_axis(k, axis)))
}

/// `true` when a biaxial `beam` has a sample point whose wave normal is inside the guard of either
/// optic axis at `lambda_nm`: it is not subdivided further and its energy goes to the residual.
#[must_use]
pub fn beam_near_biaxial_axis(beam: &Beam, material: &GemMaterial, lambda_nm: f32) -> bool {
    material
        .biaxial_indicatrix(lambda_nm)
        .is_some_and(|indicatrix| {
            beam.state
                .points
                .iter()
                .any(|state| near_biaxial_axis(state.k, &indicatrix))
        })
}

/// The optic-axis guard of any birefringent stone for `beam` (§10.2, §10.3).
///
/// A uniaxial stone uses [`beam_near_optic_axis`]; a biaxial one tests both optic axes at the
/// wavelength of the beam's centre node of the `n_lambda` nodes. An isotropic stone is never near.
#[must_use]
pub fn beam_near_axis(beam: &Beam, material: &GemMaterial, n_lambda: u32) -> bool {
    if is_biaxial_crystal(material) {
        let lambda_nm = SpectralNode(beam.spectrum.centre()).centre_nm(n_lambda);
        beam_near_biaxial_axis(beam, material, lambda_nm)
    } else {
        beam_near_optic_axis(beam, material)
    }
}
