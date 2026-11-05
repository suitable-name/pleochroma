//! Built-in material data: Rutile.

use super::{CrystalSystem, GemMaterial, OpticalCharacter};
use crate::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor},
    dispersion::DispersionModel,
};
use glam::Vec3;

impl GemMaterial {
    /// Rutile (`TiO2`): a strongly uniaxial species, at a birefringence magnitude
    /// (`+0.2957`) higher than any other built-in material.
    ///
    /// Previously deliberately excluded (see
    /// `built_in_materials_aquamarine_through_citrine`'s own "Deliberately NOT added:
    /// Sphene, Rutile" doc comment) because it needed the full anisotropic Fresnel
    /// treatment `optics::raytracer::uniaxial_fresnel` provides; added once that
    /// existed.
    ///
    /// Runs the EXACT closed-form uniaxial Fresnel solve on both CPU
    /// (`optics::raytracer::uniaxial_fresnel`) and GPU (`shaders/transport_physics.wgsl`'s
    /// own port of it, wired into `shaders/spectral_transport.wgsl`'s megakernel
    /// dispatch). Used as the Tier 3 image-comparison material for GPU-parity
    /// verification (`renderer::gpu::estimator_check::rutile_material`): its extreme
    /// birefringence makes it the built-in most likely to expose CPU/GPU divergence in
    /// the closed-form solve -- it did, catching the cross-platform branch-cut bug
    /// documented in `uniaxial_fresnel::Cplx::sqrt_forward_branch`'s own doc comment.
    ///
    /// Indices and dispersion: `DeVore`, J. Opt. Soc. Am. 41, 416 (1951), as tabulated
    /// on refractiveindex.info ("`TiO2` (Titanium dioxide): Rutile phase"), with
    /// wavelength `l` in micrometres:
    /// `n_o^2 = 5.913 + 0.2441/(l^2 - 0.0803)` and
    /// `n_e^2 = 7.197 + 0.3322/(l^2 - 0.0843)`. That is one resonance plus a constant
    /// per ray, which [`DispersionModel::Sellmeier3`] represents exactly: with
    /// `n^2 = 1 + B1*l^2/(l^2 - C1) + B2*l^2/(l^2 - C2)` and a constant second term
    /// (`C2 = 0`), `K + A/(l^2 - c)` is reproduced by `B1 = A/c`, `C1 = c` and
    /// `B2 = K - 1 - A/c`; the third term is zeroed. o-ray: `B = [3.039851, 1.873149,
    /// 0]`, `C = [0.0803, 0, 0]`. e-ray: `B = [3.940688, 2.256312, 0]`, `C = [0.0843, 0,
    /// 0]`. Resulting indices: `n_o(589.3nm) = 2.6129`, `n_e(589.3nm) = 2.9086`, so
    /// `birefringence_delta = n_e - n_o = 0.2957`; Fraunhofer `Delta n(F-C)` is 0.1636
    /// (o) and 0.2072 (e). The resonances sit in the ultraviolet (283nm, 290nm), well
    /// clear of the 380-780nm sampled band.
    ///
    /// Near-UV absorption edge (~430nm): rutile's real absorption edge is a steep
    /// semiconductor band edge, not a
    /// molecular-transition Gaussian -- modelled here (this file's existing convention
    /// for every non-measured band, see e.g. Amethyst's color-centre band above) as a
    /// single strong band centred at 350nm wide enough to tail audibly into the violet
    /// by ~430nm, giving rutile's characteristic yellow-to-brown body color. TUNED
    /// (aesthetic, not a cited absorption coefficient), like every other inclusion/
    /// pleochroism band in this file that isn't a directly measured spectrum.
    ///
    /// 2026-11-04 (owner: "The rutile looks way less yellow in the photos than the
    /// rendering"): peak 0.64, 0.08 x the former 8.0. Cut rutile is lab-grown titania,
    /// near-colorless to a pale straw yellow: five photos (four of one lab-rutile dealer,
    /// one Commons synthetic round) have a whole-stone mean chroma C* of 3-10 and a
    /// saturation `s_uv` of 0.08-0.21 (median 0.14); the 8.0 band rendered C* 33, `s_uv` 0.53,
    /// a lemon yellow (Light Tent, the deck's library cushion). Strength sweep on that job:
    /// x 0.1 gives C* 10 / `s_uv` 0.17, x 0.05 C* 6 / 0.10, the lightness unchanged (L* 86).
    /// Calibration record: docs/chromophores/calibration-records.md#saturation-round-2026-11-04
    pub(super) fn built_in_material_rutile() -> Self {
        Self {
            name: "Rutile".to_string(),
            crystal_system: CrystalSystem::Tetragonal,
            optical_character: OpticalCharacter::UniaxialPositive,
            dispersion: DispersionModel::Sellmeier3 {
                b: [3.039_851, 1.873_149, 0.0],
                c: [0.0803, 0.0, 0.0],
            },
            birefringence_delta: 0.2957,
            absorption: AbsorptionTensor::uniaxial(
                vec![AbsorptionBand::new(350.0, 40.0, 0.64)],
                vec![AbsorptionBand::new(350.0, 40.0, 0.64)],
            ),
            c_axis: Vec3::Y,
            biaxial_delta_beta_alpha: None,
            alpha_axis: None,
            scattering_sigma_s: 0.0,
            scattering_g: 0.0,
            edge_rounding_radius: 0.0,
            absorption_path_scale: 1.0,
            absorption_unit: super::AbsorptionUnit::ModelUnit,
            zoning: None,
            inclusions: None,
            uniaxial_extraordinary_dispersion: Some(DispersionModel::Sellmeier3 {
                b: [3.940_688, 2.256_312, 0.0],
                c: [0.0843, 0.0, 0.0],
            }),
            biaxial_principal_dispersion: None,
        }
    }
}
