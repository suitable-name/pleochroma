//! Pleochroic Beer-Lambert absorption and Stokes/Mueller frame rotation.
//!
//! [`apply_absorption`]'s deterministic per-bounce attenuation, the shared
//! [`channel_absorption_alphas_assigned`] it and
//! [`super::scattering::maybe_scatter_or_extinguish`] both build on, and
//! [`signed_frame_rotation_psi`]'s signed rotation angle recovery.

use super::{
    NUM_CHANNELS,
    camera::Ray,
    refraction::{RayMaterialContext, RayWavelengthCache},
};
use crate::optics::{
    absorption::AbsorptionBand,
    birefringence::{BirefringenceParams, assigned_mode_alpha, assigned_mode_e_field_uniaxial},
    polarization::{MuellerMatrix, StokesVector},
    tables::beer_lambert_factors,
};
use glam::Vec3;

// Used only by the test-only `channel_absorption_alphas`.
#[cfg(test)]
use crate::optics::{
    birefringence::effective_pleochroic_alpha, polarization::electric_field_direction,
};

/// Evaluates a material's absorption coefficient at `lambda_nm` as the sum of its
/// individual chromophore [`AbsorptionBand`](crate::optics::absorption::AbsorptionBand)s.
///
/// An empty band slice sums to `0.0` (colorless material).
#[must_use]
pub fn spectral_absorption(bands: &[AbsorptionBand], lambda_nm: f32) -> f32 {
    bands.iter().map(|band| band.evaluate(lambda_nm)).sum()
}

/// The signed azimuth that rotates the Stokes reference frame from `prev_normal` to
/// `current_normal` (both plane-of-incidence normals) about propagation `axis`.
///
/// `acos` alone returns a value in [0, pi] and drops the turn direction. The sign comes from
/// the component of `prev_normal x current_normal` along `axis`, so Q and U mix with the
/// correct sign. Callers must reject near-zero normals (no defined plane of incidence)
/// first.
#[inline]
pub(crate) fn signed_frame_rotation_psi(
    prev_normal: Vec3,
    current_normal: Vec3,
    axis: Vec3,
) -> f32 {
    let cos_psi = prev_normal.dot(current_normal).clamp(-1.0, 1.0);
    let sin_psi = prev_normal
        .cross(current_normal)
        .dot(axis.normalize_or_zero());
    sin_psi.atan2(cos_psi)
}

/// Rotates `stokes` into this bounce's plane-of-incidence frame.
///
/// Uses [`signed_frame_rotation_psi`]. Leaves `stokes` unchanged when there is no previous
/// plane or a plane normal degenerates (normal incidence). Returns this bounce's plane
/// normal, to be stored as `prev_plane_normal` for the next bounce.
///
/// `k_hat` is the wave normal `k`, not the Poynting vector `S`: the plane of incidence is a
/// property of `k`.
pub(crate) fn rotate_stokes_to_plane_of_incidence(
    k_hat: Vec3,
    normal: Vec3,
    prev_plane_normal: Option<Vec3>,
    stokes: &mut [StokesVector; NUM_CHANNELS],
) -> Vec3 {
    let current_plane_normal = k_hat.cross(normal).normalize_or_zero();
    if let Some(prev_normal) = prev_plane_normal
        && current_plane_normal.length_squared() > 1e-6
        && prev_normal.length_squared() > 1e-6
    {
        let psi = signed_frame_rotation_psi(prev_normal, current_plane_normal, k_hat);
        let rot_matrix = MuellerMatrix::frame_rotation(psi);
        for s in stokes.iter_mut() {
            *s = s.apply_matrix(&rot_matrix);
        }
    }
    current_plane_normal
}

/// Applies pleochroic Beer-Lambert absorption over one interior path length.
///
/// Each channel uses `alpha = e . A . e`, where `e` is the assigned eigenmode's geometric
/// E-field direction (see [`channel_absorption_alphas_assigned`]). Called only inside the gem.
///
/// `is_extraordinary` selects the eigenmode the path was assigned at its last air-to-crystal
/// entry (uniaxial: ordinary or extraordinary; biaxial: mode A or mode B).
pub(super) fn apply_absorption(
    ctx: &RayMaterialContext,
    cache: &RayWavelengthCache,
    k_hat: Vec3,
    is_extraordinary: bool,
    path_len: f32,
    stokes: &mut [StokesVector; NUM_CHANNELS],
) {
    let alphas = channel_absorption_alphas_assigned(ctx, cache, k_hat, is_extraordinary);
    // Model units -> absorption-length units. See `GemMaterial::absorption_path_scale`'s
    // doc comment. `absorption_path_scale == 1.0` (every built-in) makes this multiply
    // an exact IEEE 754 no-op.
    let scaled_path_len = path_len * ctx.material.absorption_path_scale;
    // Vectorized Beer-Lambert: `exp_f32x8` is a few-ULP polynomial exponential, not
    // bit-identical to `f32::exp` (see the `src/simd/` docs). With the optics tables off
    // (`cache.tables == None`) `beer_lambert_factors` is exactly that call.
    let mut args = [0f32; NUM_CHANNELS];
    for (a, alpha) in args.iter_mut().zip(&alphas) {
        *a = -alpha * scaled_path_len;
    }
    let trans = beer_lambert_factors(cache.tables.as_deref(), args);
    for (k, s) in stokes.iter_mut().enumerate() {
        *s = s.scale(trans[k]);
    }
}

/// Plain (non-scattering) Beer-Lambert absorption over one interior segment.
///
/// `segment` is the segment's ray (`origin`, unit `dir`, model units) and its length
/// `path_len`. Without zones this is exactly [`apply_absorption`] (the ray is ignored).
/// A zoned material
/// takes the per-channel optical depth `sum_z alpha_z * len_z` from
/// `zoned::segment_optical_depths`, which every zoned absorption site shares, including
/// `scattering::hg::maybe_scatter_or_extinguish_zoned`.
#[inline]
pub(crate) fn apply_segment_absorption(
    ctx: &RayMaterialContext,
    cache: &RayWavelengthCache,
    k_hat: Vec3,
    is_extraordinary: bool,
    segment: (Ray, f32),
    stokes: &mut [StokesVector; NUM_CHANNELS],
) {
    let (ray, path_len) = segment;
    if let Some(depths) =
        super::zoned::segment_optical_depths(ctx, cache, k_hat, is_extraordinary, ray, path_len)
    {
        let mut args = [0f32; NUM_CHANNELS];
        for (a, depth) in args.iter_mut().zip(&depths) {
            *a = -depth;
        }
        let trans = beer_lambert_factors(cache.tables.as_deref(), args);
        for (k, s) in stokes.iter_mut().enumerate() {
            *s = s.scale(trans[k]);
        }
        return;
    }
    apply_absorption(ctx, cache, k_hat, is_extraordinary, path_len, stokes);
}

/// Per-channel absorption coefficient `alpha` of the ASSIGNED eigenmode.
///
/// Evaluated once per bounce and shared by [`apply_absorption`] and
/// [`super::scattering::maybe_scatter_or_extinguish`], which needs the same `sigma_a` to form
/// `sigma_t = sigma_a + sigma_s`.
///
/// Takes no Stokes vector: the assigned mode's polarization is exact geometry, not something
/// read off the light's Stokes state, which can drift in azimuth after internal reflections.
/// See [`crate::optics::birefringence::assigned_mode_alpha`].
///
/// # Isotropic-by-symmetry materials
///
/// For `!ctx.is_anisotropic` there is no eigenmode assignment, so `is_extraordinary` is
/// ignored and the result is `midpoint(quadratic_form(eigen_a), quadratic_form(eigen_b))`:
/// the unpolarized limit of [`crate::optics::birefringence::effective_pleochroic_alpha`].
/// Both eigen directions are geometry-only. Built-in isotropic materials have equal
/// eigen-absorptions, so the result does not depend on the directions. An isotropic but
/// dichroic material would need a full diattenuator Mueller matrix, which is not implemented.
///
/// `pub(crate)`: `renderer::gpu::transport_check` feeds this real function's output to the
/// standalone WGSL scattering kernel, so the two cannot diverge.
///
/// `k_hat` is the wave normal `k`, not the Poynting vector `S`: the eigen-polarizations are
/// transverse to `k`.
pub(crate) fn channel_absorption_alphas_assigned(
    ctx: &RayMaterialContext,
    cache: &RayWavelengthCache,
    k_hat: Vec3,
    is_extraordinary: bool,
) -> [f32; NUM_CHANNELS] {
    let c_axis = ctx.c_axis;
    let mut alphas = [0.0f32; NUM_CHANNELS];

    if !ctx.is_anisotropic {
        let (eigen_a, eigen_b) = cache.hero_indicatrix.map_or_else(
            || {
                (
                    BirefringenceParams::ordinary_eigen_polarization(k_hat, c_axis),
                    BirefringenceParams::extraordinary_eigen_polarization(k_hat, c_axis),
                )
            },
            |ind| ind.eigen_polarizations(k_hat),
        );
        for (k, alpha_slot) in alphas.iter_mut().enumerate() {
            *alpha_slot = f32::midpoint(
                cache.tensor_ch[k].quadratic_form(eigen_a),
                cache.tensor_ch[k].quadratic_form(eigen_b),
            );
        }
        return alphas;
    }

    // Bit-identical to a fresh `material.biaxial_indicatrix(ctx.lambdas[ctx.hero_idx])`
    // call -- see `RayWavelengthCache::hero_indicatrix`'s doc comment.
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
    for (k, alpha_slot) in alphas.iter_mut().enumerate() {
        // Against the cached tensor rather than rebuilding a fresh `AbsorptionTensor3`
        // every bounce -- see `RayWavelengthCache::tensor_ch`'s doc comment.
        *alpha_slot = assigned_mode_alpha(&cache.tensor_ch[k], e_mode_hat);
    }
    alphas
}

/// Test-only reference: the per-channel alpha as a degree-of-polarization blend between the
/// light's Stokes-derived electric-field direction and the eigenmodes' unpolarized average.
///
/// The transport path uses [`channel_absorption_alphas_assigned`]. This function remains as
/// the reference that the tests pin the assigned-mode path against, and as an unpolarized
/// probe for `raytracer::scattering`'s tests. The GPU harness checks
/// `birefringence::pleochroic_channel_alpha` (the same per-channel combination) instead.
#[cfg(test)]
pub(crate) fn channel_absorption_alphas(
    ctx: &RayMaterialContext,
    cache: &RayWavelengthCache,
    current_plane_normal: Vec3,
    k_hat: Vec3,
    stokes: &[StokesVector; NUM_CHANNELS],
) -> [f32; NUM_CHANNELS] {
    let c_axis = ctx.c_axis;
    // Bit-identical to a fresh `material.biaxial_indicatrix(ctx.lambdas[ctx.hero_idx])`
    // call -- see `RayWavelengthCache::hero_indicatrix`'s doc comment.
    let indicatrix = cache.hero_indicatrix;
    let (eigen_a, eigen_b) = indicatrix.map_or_else(
        || {
            (
                BirefringenceParams::ordinary_eigen_polarization(k_hat, c_axis),
                BirefringenceParams::extraordinary_eigen_polarization(k_hat, c_axis),
            )
        },
        |ind| ind.eigen_polarizations(k_hat),
    );

    let mut alphas = [0.0f32; NUM_CHANNELS];
    for (k, alpha_slot) in alphas.iter_mut().enumerate() {
        // Calls `effective_pleochroic_alpha` against the cached tensor rather than
        // rebuilding a fresh `AbsorptionTensor3` every bounce -- see
        // `RayWavelengthCache::tensor_ch`'s doc comment.
        let e_hat = electric_field_direction(&stokes[k], current_plane_normal, k_hat);
        *alpha_slot = effective_pleochroic_alpha(
            &cache.tensor_ch[k],
            e_hat,
            eigen_a,
            eigen_b,
            stokes[k].degree_of_polarization(),
        );
    }
    alphas
}

/// A slab at `absorption_path_scale = 2.0` must attenuate exactly like a slab of twice the
/// model-unit thickness at scale `1.0`. Both products are `2.0 * d`, which is exact in IEEE
/// 754, so the results must be bit-for-bit identical.
#[cfg(test)]
mod absorption_path_scale_tests {
    use super::*;
    use crate::optics::{
        materials::GemMaterial, polarization::StokesVector,
        raytracer::refraction::build_ray_wavelength_cache,
    };

    #[test]
    fn scaled_slab_attenuates_bit_identically_to_doubled_thickness_slab() {
        let ruby_like =
            GemMaterial::new_custom("ruby-like slab probe", 1.76, 0.0, 0.0, [0.5, 1.0, 1.5]);
        assert_eq!(
            ruby_like.crystal_system,
            crate::optics::materials::CrystalSystem::Cubic,
            "test premise: birefringence_delta=0.0 must yield an isotropic material \
             (no birefringence machinery to complicate the comparison)"
        );

        let d = 0.37f32;
        let material_scale1_thickness2d = ruby_like.clone(); // absorption_path_scale == 1.0
        let material_scale2_thicknessd = ruby_like.with_absorption_path_scale(2.0);

        let lambdas: [f32; NUM_CHANNELS] = [420.0, 460.0, 500.0, 540.0, 580.0, 620.0, 660.0, 700.0];
        let ray_dir = Vec3::new(0.0, -1.0, 0.0);
        let stokes_in = [StokesVector::unpolarized(1.0); NUM_CHANNELS];

        let ctx_a = RayMaterialContext {
            material: &material_scale1_thickness2d,
            lambdas,
            hero_idx: 0,
            c_axis: Vec3::Y,
            is_anisotropic: false,
            enable_internal_mode_coupling: true,
        };
        let ctx_b = RayMaterialContext {
            material: &material_scale2_thicknessd,
            lambdas,
            hero_idx: 0,
            c_axis: Vec3::Y,
            is_anisotropic: false,
            enable_internal_mode_coupling: true,
        };
        let cache_a = build_ray_wavelength_cache(&ctx_a);
        let cache_b = build_ray_wavelength_cache(&ctx_b);

        let mut stokes_a = stokes_in;
        let mut stokes_b = stokes_in;
        apply_absorption(
            &ctx_a,
            &cache_a,
            ray_dir,
            false,   // is_extraordinary: irrelevant here (is_anisotropic == false)
            2.0 * d, // scale=1.0: full doubled thickness in model units
            &mut stokes_a,
        );
        apply_absorption(
            &ctx_b,
            &cache_b,
            ray_dir,
            false, // is_extraordinary: irrelevant here (is_anisotropic == false)
            d,     // scale=2.0: half the model-unit thickness
            &mut stokes_b,
        );

        for k in 0..NUM_CHANNELS {
            let a = (stokes_a[k].i, stokes_a[k].q, stokes_a[k].u, stokes_a[k].v);
            let b = (stokes_b[k].i, stokes_b[k].q, stokes_b[k].u, stokes_b[k].v);
            assert_eq!(
                a, b,
                "channel {k}: scale=1.0/thickness=2d must attenuate bit-identically to \
                 scale=2.0/thickness=d (got {a:?} vs {b:?})"
            );
        }
    }
}

/// [`channel_absorption_alphas_assigned`] checked against the test-only DOP-blended
/// [`channel_absorption_alphas`].
#[cfg(test)]
mod assigned_mode_alpha_tests {
    use super::*;
    use crate::optics::{
        materials::{CrystalSystem, GemMaterial},
        raytracer::{refraction::build_ray_wavelength_cache, spectral_absorption},
    };

    /// (i) For an isotropic material the assigned-mode alphas equal the reference's output
    /// bit for bit when the reference gets an unpolarized Stokes vector. At `p == 0.0` the
    /// reference collapses exactly to its unpolarized term, which the isotropic branch
    /// computes directly.
    #[test]
    fn isotropic_material_assigned_alphas_match_old_unpolarized_output_bit_for_bit() {
        let material = GemMaterial::new_custom("isotropic probe", 1.62, 0.01, 0.0, [0.3, 0.6, 0.9]);
        assert_eq!(
            material.crystal_system,
            CrystalSystem::Cubic,
            "test premise: birefringence_delta=0.0 must yield an isotropic material"
        );
        let lambdas: [f32; NUM_CHANNELS] = [420.0, 460.0, 500.0, 540.0, 580.0, 620.0, 660.0, 700.0];
        let ctx = RayMaterialContext {
            material: &material,
            lambdas,
            hero_idx: 0,
            c_axis: Vec3::Y,
            is_anisotropic: false,
            enable_internal_mode_coupling: true,
        };
        let cache = build_ray_wavelength_cache(&ctx);
        let k_hat = Vec3::new(0.3, -0.9, 0.2).normalize();
        let stokes_unpolarized = [StokesVector::unpolarized(1.0); NUM_CHANNELS];

        let old_alphas =
            channel_absorption_alphas(&ctx, &cache, Vec3::ZERO, k_hat, &stokes_unpolarized);
        for is_extraordinary in [false, true] {
            let new_alphas =
                channel_absorption_alphas_assigned(&ctx, &cache, k_hat, is_extraordinary);
            for k in 0..NUM_CHANNELS {
                assert_eq!(
                    old_alphas[k].to_bits(),
                    new_alphas[k].to_bits(),
                    "channel {k} (is_extraordinary={is_extraordinary}): isotropic new alpha \
                     must equal the old unpolarized-DOP output bit-for-bit (old={}, new={})",
                    old_alphas[k],
                    new_alphas[k]
                );
            }
        }
    }

    /// (ii) With real Tourmaline data (strongly dichroic at 430 nm), a path perpendicular to
    /// `c_axis` reads the o-ray band sum in ordinary mode and the e-ray band sum in
    /// extraordinary mode.
    #[test]
    fn tourmaline_assigned_mode_alphas_read_o_and_e_ray_band_sums_perpendicular_to_c_axis() {
        const LAMBDA_NM: f32 = 430.0; // Fe2+-Ti4+ IVCT band, strongly dichroic at this wavelength
        let tourmaline =
            GemMaterial::by_name("Tourmaline").expect("Tourmaline must be a built-in material");
        let c_axis = tourmaline.c_axis; // Vec3::X, the documented cut-orientation override
        let alpha_o = spectral_absorption(&tourmaline.absorption.o_ray, LAMBDA_NM);
        let alpha_e = spectral_absorption(&tourmaline.absorption.e_ray, LAMBDA_NM);
        assert!(
            alpha_o > alpha_e * 2.0,
            "test premise: Tourmaline's o-ray must be substantially stronger than its e-ray at \
             430nm (o={alpha_o}, e={alpha_e})"
        );

        let propagation_dir = if c_axis.x.abs() > 0.9 {
            Vec3::Y
        } else {
            Vec3::X
        };
        assert!(
            propagation_dir.dot(c_axis).abs() < 1e-6,
            "test premise: propagation_dir must be exactly perpendicular to c_axis"
        );

        let lambdas = [LAMBDA_NM; NUM_CHANNELS];
        let ctx = RayMaterialContext {
            material: &tourmaline,
            lambdas,
            hero_idx: 0,
            c_axis,
            is_anisotropic: true,
            enable_internal_mode_coupling: true,
        };
        let cache = build_ray_wavelength_cache(&ctx);

        let ordinary_alphas =
            channel_absorption_alphas_assigned(&ctx, &cache, propagation_dir, false);
        let extraordinary_alphas =
            channel_absorption_alphas_assigned(&ctx, &cache, propagation_dir, true);
        for k in 0..NUM_CHANNELS {
            assert!(
                (ordinary_alphas[k] - alpha_o).abs() < 1e-4,
                "channel {k}: ordinary-mode alpha ({}) must equal the o-ray band sum ({alpha_o})",
                ordinary_alphas[k]
            );
            assert!(
                (extraordinary_alphas[k] - alpha_e).abs() < 1e-4,
                "channel {k}: extraordinary-mode alpha ({}) must equal the e-ray band sum \
                 ({alpha_e})",
                extraordinary_alphas[k]
            );
        }
    }

    /// (iii) The assigned-mode driver takes no Stokes parameter, so it cannot depend on the
    /// light's polarization. The reference does: two Stokes vectors with the same (Q, U)
    /// azimuth but different V give different alphas for a strongly dichroic
    /// extraordinary-mode path. A camera path's Stokes vector is importance, not the light's
    /// true state, and drifts in azimuth after internal reflections, so the blend is unsound.
    #[test]
    fn assigned_mode_alphas_have_no_stokes_dependence_unlike_old_dop_blend() {
        const LAMBDA_NM: f32 = 430.0;
        let tourmaline =
            GemMaterial::by_name("Tourmaline").expect("Tourmaline must be a built-in material");
        let c_axis = tourmaline.c_axis;
        let lambdas = [LAMBDA_NM; NUM_CHANNELS];
        let ctx = RayMaterialContext {
            material: &tourmaline,
            lambdas,
            hero_idx: 0,
            c_axis,
            is_anisotropic: true,
            enable_internal_mode_coupling: true,
        };
        let cache = build_ray_wavelength_cache(&ctx);
        // Oblique to c_axis, so `electric_field_direction`'s azimuth is well-defined and
        // the extraordinary mode is genuinely dichroic here.
        let k_hat = Vec3::new(0.4, 0.5, 0.767_2).normalize();
        let s_axis = k_hat.cross(c_axis).normalize();

        // Same (Q, U) -- same azimuth, same electric_field_direction -- different V.
        let stokes_low_dop = [StokesVector::new(1.0, 0.6, 0.3, 0.0); NUM_CHANNELS];
        let stokes_high_dop = [StokesVector::new(1.0, 0.6, 0.3, 0.7); NUM_CHANNELS];
        let old_low = channel_absorption_alphas(&ctx, &cache, s_axis, k_hat, &stokes_low_dop);
        let old_high = channel_absorption_alphas(&ctx, &cache, s_axis, k_hat, &stokes_high_dop);
        assert!(
            (old_low[0] - old_high[0]).abs() > 1e-4,
            "test premise: the OLD DOP-blended formula must actually be sensitive to a Stokes \
             V component that leaves the azimuth (Q, U) unchanged (old_low={}, old_high={})",
            old_low[0],
            old_high[0]
        );

        // The assigned-mode driver takes no Stokes parameter, so there is nothing to vary.
        let new_alphas = channel_absorption_alphas_assigned(&ctx, &cache, k_hat, true);
        assert!(
            new_alphas.iter().all(|a| a.is_finite()),
            "new_alphas must be finite: {new_alphas:?}"
        );
    }
}

#[cfg(test)]
mod frame_rotation_sign_tests {
    use super::*;

    /// Two bounces with the same plane of incidence (parallel normals) must give psi ~= 0:
    /// no Q/U mixing when the frame has not turned.
    #[test]
    fn same_plane_of_incidence_gives_zero_psi() {
        let prev_normal = Vec3::new(0.0, 0.0, 1.0);
        let current_normal = Vec3::new(0.0, 0.0, 1.0);
        let axis = Vec3::new(1.0, 0.0, 0.0);
        let psi = signed_frame_rotation_psi(prev_normal, current_normal, axis);
        assert!(
            psi.abs() < 1e-5,
            "psi should be ~0 for identical plane-of-incidence normals (got {psi})"
        );
    }

    /// Swapping the previous and current normals must flip the sign of psi, which an
    /// unsigned angle cannot represent.
    #[test]
    fn reversing_plane_normal_order_flips_psi_sign() {
        // Two plane-of-incidence normals at a genuine angle to each other, and a
        // propagation axis with a nonzero component along their cross product so the
        // sign is well-defined (not a degenerate coplanar case).
        let normal_a = Vec3::new(1.0, 0.0, 0.0);
        let normal_b = Vec3::new(0.0, 1.0, 0.0).normalize();
        let axis = Vec3::new(0.0, 0.0, 1.0);

        let psi_forward = signed_frame_rotation_psi(normal_a, normal_b, axis);
        let psi_reversed = signed_frame_rotation_psi(normal_b, normal_a, axis);

        assert!(
            psi_forward.abs() > 0.1,
            "psi_forward should be a genuine nonzero rotation (got {psi_forward})"
        );
        assert!(
            (psi_forward + psi_reversed).abs() < 1e-4,
            "reversing the normal order should flip the sign of psi (forward={psi_forward}, reversed={psi_reversed})"
        );
    }

    /// Sanity check that the recovered angle actually matches the geometric angle
    /// between the two normals (90 degrees here), not just that it's nonzero.
    #[test]
    fn psi_magnitude_matches_geometric_angle_between_normals() {
        let normal_a = Vec3::new(1.0, 0.0, 0.0);
        let normal_b = Vec3::new(0.0, 1.0, 0.0);
        let axis = Vec3::new(0.0, 0.0, 1.0);
        let psi = signed_frame_rotation_psi(normal_a, normal_b, axis);
        assert!(
            (psi.abs() - std::f32::consts::FRAC_PI_2).abs() < 1e-4,
            "psi magnitude should match the 90 degree angle between the normals (got {})",
            psi.to_degrees()
        );
    }
}
