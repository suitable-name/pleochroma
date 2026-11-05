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
                // Chromophore: V3+ in the octahedral site, bands at 436 and 606 nm,
                // with the UV edge of the high-vanadium type; transmission windows at
                // 520 nm (green) and beyond 700 nm (deep red, T(740) = 0.89 at 7 mm).
                // FITTED 2026-10-19 by a bounded
                // least-squares energy-Gaussian fit (log residual, 380-780 nm) to the
                // Caltech Mineral Spectroscopy Server spectrum of tsavorite GRR 229
                // (Tanzania; Ca2.97 Al1.75 Cr0.01 V0.11 Ti0.02; 0.758 mm; baseline
                // 0.042 removed),
                // `data/reference_spectra/caltech_tsavorite_grr229_0p758mm.csv`, at
                // 0.5 x the sample's vanadium (about 0.9 wt% V2O3, a fine saturated
                // stone) over the 8.8 mm face-up path of a 7 mm stone. Swatch (7 mm,
                // D65): L*56 C*86 h=147; p = 1.5: h 150, p = 2: h 151. Multiply every
                // peak by s/0.5 for another V content (0.35 x: L*65; 0.18 x ~ 0.32
                // wt% V2O3: L*78).
                // Calibration record: docs/chromophores/calibration-records.md#tsavorite
                // 2026-11-03 (owner: "match the pictures of actual gemstones"): every peak x 0.5,
                // i.e. 0.25 x the GRR 229 sample's vanadium. Twenty-two dealer photos of tsavorite
                // (GemSelect) have a median hue of 139 (colored pixels); the 0.5 x sample default
                // rendered h 152 (Light Tent), this one h 143. Swatch L*73 C*72 h 140 (it was L*56
                // C*86 h 147). The Mint variant is now 0.72 x this.
                // 2026-11-04 (owner: "The render seems to err toward saturated"): x 0.7 of that
                // (0.175 x the sample's vanadium). Colored-pixel saturation s_uv: the faceted dealer
                // photos 0.88, the x 1 render 1.11 (x 0.5: 0.76 at h 137); x 0.63 matches the
                // photos, but tsavorite is sold small (most photos show stones under 6 mm, whose
                // paths are shorter than the 7 mm reference), so 0.7. Swatch L*80 C*62 h 137,
                // T(650) 0.44. That is where the Mint variant (0.18 x the vanadium) sat, so Mint
                // is now half this default (0.0875 x, L*89 C*41 h 133).
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
                // Chromophore: Fe3+ (440 nm band and the O->Fe3+ charge-transfer
                // edge, total absorption below 450 nm) plus Cr3+ at 578/629 nm;
                // windows at 520-545 nm (green) and beyond 720 nm (deep red,
                // T(740) = 0.75 at 7 mm), an NIR band at ~880 nm. FITTED 2026-10-19
                // by a bounded
                // least-squares energy-Gaussian fit (log residual, 380-780 nm) to the
                // Caltech Mineral Spectroscopy Server spectrum of the Russian
                // demantoid slab GIA 2952 (2.5 mm; scattering baseline 0.816
                // removed),
                // `data/reference_spectra/caltech_demantoid_gia2952_2p5mm.csv`, at
                // the slab's own composition over the 8.8 mm face-up path of a 7 mm
                // stone. Swatch (7 mm, D65): L*41 C*92 h=140; p = 1.5: h 144, p = 2:
                // h 149 (0.75 x all peaks: L*50). The heated Ural demantoid GRR 2667
                // is 2.6 x more absorbing per cm and would be near-black at 7 mm.
                // Calibration record: docs/chromophores/calibration-records.md#demantoid
                // 2026-11-03 (owner: "match the pictures of actual gemstones"): REPLACED the
                // Russian slab alone by 0.15 x that spectrum (its Cr3+) plus the measured Fe3+
                // andradite GRR 48 (the Topazolite variant, k 0.35), refitted to eight energy
                // Gaussians (dE2000 0.46 against the sum). Nineteen dealer photos (GemSelect;
                // today's demantoid is mostly the iron-rich Namibian and Madagascan kind) have a
                // median hue of 117 (colored pixels); the Cr-rich Russian slab rendered h 150, this
                // mix h 120 (Light Tent). Swatch L*62 C*72 h 115 (it was L*41 C*92 h 140).
                // 2026-11-04 (owner: "The render seems to err toward saturated"): x 0.35 of that mix.
                // Colored-pixel saturation s_uv: the faceted dealer photos 0.62 (mostly small
                // stones), the one usable Commons photo 0.96, the x 1 render 0.98 (x 0.25: 0.72 at
                // h 115, x 0.5: 0.85); the factor takes half the gap in log terms. Swatch L*85 C*59
                // h 115, T(650) 0.68.
                // Calibration record: docs/chromophores/calibration-records.md#saturation-round-2026-11-04
                absorption: AbsorptionTensor::isotropic(vec![
                    // measured spectra, see the note above
                    AbsorptionBand::energy(389.1, 2_548.1, 13.57),
                    AbsorptionBand::energy(433.3, 1_631.4, 9.215),
                    AbsorptionBand::energy(439.7, 332.3, 5.583),
                    AbsorptionBand::energy(465.0, 1_031.0, 1.577),
                    AbsorptionBand::energy(615.6, 2_317.1, 1.357),
                    AbsorptionBand::energy(489.6, 5_365.9, 1.083),
                    AbsorptionBand::energy(484.1, 1_100.1, 0.704_2),
                    AbsorptionBand::energy(466.1, 37_650.6, 0.068_85),
                ])
                .scaled(0.35),
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
