//! Per-material optics queries: the biaxial indicatrix, the extraordinary-ray
//! index lookup, and the GPU-routing predicate.

use super::GemMaterial;
use crate::optics::birefringence::BiaxialIndicatrix;

impl GemMaterial {
    /// Builds this material's full [`BiaxialIndicatrix`] (three principal indices
    /// `n_alpha` <= `n_beta` <= `n_gamma` plus their orthonormal axis frame) at
    /// `lambda_nm`. `None` unless the material is biaxial (`biaxial_delta_beta_alpha` is
    /// set); that field's doc comment gives the convention: base `dispersion` curve ==
    /// `n_beta`, `birefringence_delta` == `n_gamma` - `n_alpha`, `c_axis` is the `n_gamma`
    /// axis, and `alpha_axis`, when set, is the `n_alpha` axis.
    ///
    /// With [`GemMaterial::biaxial_principal_dispersion`] set, `n_alpha` and `n_gamma`
    /// come from their own curves and `n_beta` stays the base curve, so the spreads and
    /// the optic-axis angle vary with wavelength. The sort below is a last-resort guard
    /// that keeps `n_alpha <= n_beta <= n_gamma`; valid materials never reach it
    /// (custom curves that cross are refused earlier, and the built-ins never cross
    /// between 380 and 780 nm, which a test pins). Without the field, the achromatic
    /// offsets apply.
    #[must_use]
    pub fn biaxial_indicatrix(&self, lambda_nm: f32) -> Option<BiaxialIndicatrix> {
        let delta_beta_alpha = self.biaxial_delta_beta_alpha?;
        let n_beta = self.dispersion.evaluate(lambda_nm);
        let (n_alpha, n_beta, n_gamma) = match self.biaxial_principal_dispersion {
            None => {
                let n_alpha = n_beta - delta_beta_alpha;
                (n_alpha, n_beta, n_alpha + self.birefringence_delta)
            }
            Some([alpha_curve, gamma_curve]) => {
                let mut sorted = [
                    alpha_curve.evaluate(lambda_nm),
                    n_beta,
                    gamma_curve.evaluate(lambda_nm),
                ];
                sorted.sort_by(f32::total_cmp);
                sorted.into()
            }
        };
        // `alpha_axis == None` is exactly `from_gamma_axis` (the arbitrary completion).
        Some(BiaxialIndicatrix::from_oriented_frame(
            n_alpha,
            n_beta,
            n_gamma,
            self.c_axis,
            self.alpha_axis,
        ))
    }

    /// This uniaxial material's extraordinary-ray index at `lambda_nm`, given the
    /// already-evaluated ordinary index `n_o` at that SAME wavelength (every caller
    /// already has `n_o` in hand from `self.dispersion.evaluate(lambda_nm)`, so this
    /// takes it rather than recomputing it).
    ///
    /// Uses [`Self::uniaxial_extraordinary_dispersion`] (a genuine independent e-ray
    /// curve) when present, else the constant-offset `n_o + birefringence_delta`. See that
    /// field's doc comment for which built-ins carry a curve, and for the GPU port
    /// (`extraordinary_dispersion_evaluate`, called from
    /// `shaders/transport_bounce/08a_index_setup.wgsl`), which mirrors this exactly.
    #[must_use]
    pub fn extraordinary_index_at(&self, lambda_nm: f32, n_o: f32) -> f32 {
        self.uniaxial_extraordinary_dispersion
            .map_or(n_o + self.birefringence_delta, |e_dispersion| {
                e_dispersion.evaluate(lambda_nm)
            })
    }

    /// GPU routing predicate: whether this material's optics are supported by the GPU
    /// backend (`renderer::shaders::spectral_transport.wgsl`'s `transport_main`).
    ///
    /// A pure function of the material alone. Isotropic and uniaxial materials (the
    /// `theta_c` fixed-point iteration, the 50/50 ordinary/extraordinary eigenmode split,
    /// `extraordinary_poynting_dir` walk-off) are GPU-capable, and so are biaxial ones
    /// (`biaxial_delta_beta_alpha.is_some()`). That relies on two well-conditioned helpers
    /// near mode degeneracy: `BiaxialIndicatrix::eigenvector_world` (the null vector of
    /// `Gamma = P.B.P`, a sign-aligned sum of row-pair cross products) and the discriminant
    /// in `wave_indices` (`BiaxialIndicatrix::precise_root_near`, which avoids the `B^2-4C`
    /// cancellation). See both doc comments in `optics::birefringence`.
    ///
    /// Checks only material data, never the machine (GPU availability, VRAM, load). The
    /// viewer and a remote render worker each decide independently whether to send a scene
    /// to the GPU, so they must reach the same answer from the scene alone. `lib::BUILD_ID`
    /// (a hash of the crate version) pairs only builds of one release, so the rule is
    /// identical on both. A caller assembling a scene
    /// (material, geometry, environment) calls this for each distinct material and routes
    /// the render to the GPU only if all return `true`.
    ///
    /// A ZONED material is supported only when the GPU zone kernels can render it. A
    /// mesh-shell zone, a scattering zoned stone, invalid zoning, or a zone with more than
    /// `MAX_ABSORPTION_BANDS` bands is CPU only (`renderer::buffers::gpu_zoning_decline`
    /// names the reason). Without this check a zoned material would render only its base
    /// zone on the GPU. A material whose own band sets exceed `MAX_ABSORPTION_BANDS` is CPU
    /// only too (`renderer::buffers::gpu_material_decline` names that reason).
    ///
    /// Not `const`, because it inspects the zone list. `scene_routes_to_gpu` is the
    /// scene-level check built on this.
    #[must_use]
    pub fn gpu_supported(&self) -> bool {
        crate::renderer::buffers::gpu_material_decline(self).is_none()
    }
}
