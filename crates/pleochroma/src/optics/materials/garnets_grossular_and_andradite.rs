//! Built-in material data: Grossular (Tsavorite) and Andradite (Demantoid) garnets.

use super::{CrystalSystem, GemMaterial, OpticalCharacter};
use crate::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor},
    dispersion::DispersionModel,
};
use glam::Vec3;

impl GemMaterial {
    /// Continuation of [`Self::built_in_materials_garnets_pyrope_through_spessartine`]
    /// (split purely to keep each part under clippy's function-length lint):
    /// Grossular (Tsavorite) and Andradite (Demantoid). See that function's doc
    /// comment for the shared garnet-group dispersion-fit convention (2-parameter
    /// Cauchy, B-G->F-C converted).
    pub(super) fn built_in_materials_garnets_grossular_and_andradite() -> Vec<Self> {
        vec![
            // Grossular (Tsavorite variety, Ca3Al2(SiO4)3:V/Cr): n_d=1.734, B-G 0.028
            // -> Delta n(F-C) = 0.01621, A=1.734-0.008486/0.5893^2=1.709572,
            // B=0.008486. Abbe V_d~45.71.
            Self {
                name: "Grossular Garnet (Tsavorite)".to_string(),
                crystal_system: CrystalSystem::Cubic,
                optical_character: OpticalCharacter::Isotropic,
                dispersion: DispersionModel::Cauchy {
                    a: 1.709_572,
                    b: 0.008_486,
                    c: 0.0,
                },
                birefringence_delta: 0.0,
                // Chromophore: V3+ in the octahedral site, bands at 436 and 606 nm, with the UV
                // edge of the high-vanadium type; windows at 520 nm (green) and beyond 700 nm
                // (deep red). Energy-Gaussian fit (log residual, 380-780 nm) to the Caltech
                // spectrum of tsavorite GRR 229 (Tanzania, 0.758 mm),
                // `data/reference_spectra/caltech_tsavorite_grr229_0p758mm.csv`, over the 8.8 mm
                // face-up path of a 7 mm stone. Peaks are halved from the sample's vanadium
                // (0.25 x it), and `.scaled(0.7)` applies on top: 0.175 x the sample's vanadium.
                // Swatch (7 mm, D65): L*80 C*62 h 137.
                // The strength is set by the median hue of dealer photos (139). Saturation is
                // a little below the x 1 render (s_uv 0.88 against 1.11) because tsavorite is
                // sold small, and the shorter paths of those stones are not the 7 mm reference.
                // Calibration record: docs/chromophores/calibration-records.md#tsavorite
                // Calibration record: docs/chromophores/calibration-records.md#saturation-round-2026-11-04
                absorption: AbsorptionTensor::isotropic(vec![
                    // measured spectra, see the note above
                    AbsorptionBand::energy(393.0, 3_729.0, 3.575),
                    AbsorptionBand::energy(436.0, 2_341.0, 3.445),
                    AbsorptionBand::energy(490.0, 2_500.0, 0.555),
                    AbsorptionBand::energy(606.0, 2_727.0, 1.495),
                    AbsorptionBand::energy(666.0, 1_166.0, 0.331_5),
                ])
                .scaled(0.7),
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
                uniaxial_extraordinary_dispersion: None,
                biaxial_principal_dispersion: None,
            },
            // Andradite (Demantoid variety, Ca3Fe2(SiO4)3:Cr): n_d=1.887, B-G 0.057
            // (demantoid's famously-cited dispersion, HIGHER than diamond's 0.044 B-G-
            // equivalent figure) -> Delta n(F-C) = 0.03300,
            // A=1.887-0.017276/0.5893^2=1.837264, B=0.017276. Abbe V_d~26.88.
            Self {
                name: "Andradite Garnet (Demantoid)".to_string(),
                crystal_system: CrystalSystem::Cubic,
                optical_character: OpticalCharacter::Isotropic,
                dispersion: DispersionModel::Cauchy {
                    a: 1.837_264,
                    b: 0.017_276,
                    c: 0.0,
                },
                birefringence_delta: 0.0,
                // Chromophore: Fe3+ (the 440 nm band and the O to Fe3+ charge-transfer edge,
                // absorbing below 450 nm) plus Cr3+ near 615-650 nm, leaving green and deep-red
                // windows. Eight energy-Gaussian bands fitted to the sum of THREE components:
                // two measured and one tuned. Measured: the Russian demantoid slab (Caltech
                // GIA 2952, `caltech_demantoid_gia2952_2p5mm.csv`) at 0.19 x and the Topazolite
                // variant (Fe3+ andradite GRR 48, Fe3+ only) at 0.15 x, in the units of the
                // dealer-photo mix of 2026-11-03 (0.15 x and 0.35 x, then x 0.35, which is gone).
                // TUNED, NOT MEASURED: one extra energy Gaussian at 613 nm, FWHM 4880 cm^-1, peak
                // 0.473 per model unit, a Cr3+-region stand-in with no spectrum behind it. It
                // takes yellow-orange light out of the stone to lower the chroma at constant hue:
                // the two measured spectra alone cannot reach the target, which is the CIELAB
                // midpoint of the slab alone (L*41 C*92 h 140, 2026-10-19) and the dealer-photo
                // mix (L*85 C*59 h 115). Target L*63 a*-47.7 b*56.3 (C*74 h 130); the three
                // strengths were solved together for L*, a*, b*. Swatch L*63.0 C*73.8 h 130.2
                // (dE2000 0.03 from the target; the eight-band fit is 0.005 from the sum).
                // Calibration record: docs/chromophores/calibration-records.md#demantoid
                // Calibration record: docs/chromophores/calibration-records.md#loyd-v0111-round-2026-11-16
                absorption: AbsorptionTensor::isotropic(vec![
                    // fitted to the three components, see the note above
                    AbsorptionBand::energy(380.6, 1_518.7, 7.703),
                    AbsorptionBand::energy(421.5, 4_835.5, 5.17),
                    AbsorptionBand::energy(437.9, 669.1, 2.904),
                    AbsorptionBand::energy(604.5, 3_536.2, 1.459),
                    AbsorptionBand::energy(1_000.0, 3_401.3, 1.196),
                    AbsorptionBand::energy(625.2, 1_277.4, 0.565_4),
                    AbsorptionBand::energy(648.5, 492.4, 0.204),
                    AbsorptionBand::energy(686.1, 828.7, 0.187_9),
                ]),
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
                uniaxial_extraordinary_dispersion: None,
                biaxial_principal_dispersion: None,
            },
        ]
    }
}
