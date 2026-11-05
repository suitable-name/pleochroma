//! Built-in material data: Titanite (Sphene).
//!
//! Titanite (`CaTiSiO5`) is monoclinic and biaxial positive, with a fire above diamond's and a
//! birefringence of about 0.12 that doubles every facet edge seen through the stone. It was left
//! out until 2026-11-03 because that birefringence is far beyond anything the biaxial code had
//! been checked at (Zircon's 0.059 was the largest); `tests/titanite.rs` now checks the
//! refraction into it at this magnitude.

use super::{AbsorptionUnit, CrystalSystem, GemMaterial, OpticalCharacter};
use crate::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor},
    dispersion::DispersionModel,
};
use glam::Vec3;

/// The shared ultraviolet pole of the three index curves, `lambda_0^2` in square micrometres
/// (`lambda_0` = 0.1524 um).
pub(super) const TITANITE_POLE_UM2: f32 = 0.023_238;

/// The one-pole Sellmeier curve `n^2 = 1 + b l^2 / (l^2 - TITANITE_POLE_UM2)`, `l` in um.
const fn titanite_curve(b: f32) -> DispersionModel {
    DispersionModel::Sellmeier3 {
        b: [b, 0.0, 0.0],
        c: [TITANITE_POLE_UM2, 0.0, 0.0],
    }
}

impl GemMaterial {
    /// Titanite (Sphene).
    ///
    /// INDICES (measured): Busz, Neues Jahrbuch fur Mineralogie, Beilage-Band 5, 330 (1887), as
    /// tabulated in Dana, System of Mineralogy, 6th ed. (1892), p. 714 (scan:
    /// archive.org/details/mineralogyjames00danarich, page 714): St. Gotthard titanite, whose
    /// analysis (`SiO2` 29.12, `TiO2` 42.09, `CaO` 27.90 %) holds no iron, like gem material.
    ///
    /// | line | nm | `n_alpha` | `n_beta` | `n_gamma` |
    /// |---|---|---|---|---|
    /// | Li | 670.8 | 1.8766 | 1.8839 | 1.9987 |
    /// | Na | 589.3 | 1.8879 | 1.8940 | 2.0093 |
    /// | Tl | 535.0 | 1.8989 | 1.9041 | 2.0232 |
    ///
    /// Fitted (least squares, all nine values) with one-pole Sellmeier curves that share ONE
    /// ultraviolet pole, 0.1524 um; separate poles cross near 400 nm, outside the measured range.
    /// Largest residual 0.0013 (on `n_gamma`, whose three values do not lie on any one-pole
    /// curve; `n_alpha` and `n_beta` within 0.0008). At the D line the curves give `n_alpha`
    /// 1.88795, `n_beta` 1.89415, `n_gamma` 2.01057: birefringence 0.1226 (gem tables:
    /// 0.100-0.135, GIA Gem Reference Guide via `LibreTexts` Gemology 16.66) and `2V_z` 27 deg
    /// (Busz: 27.0 deg; Handbook of Mineralogy: 17-40 deg, biaxial positive, dispersion r > v
    /// strong). The B-G dispersion (430.8-686.7 nm) is 0.057 for `n_alpha` and `n_beta` and 0.064
    /// for `n_gamma`, extrapolated below 535 nm; the gem tables' 0.051 (IGS, Gemdat after
    /// Schumann 2001) names no ray or sample. Diamond's is 0.044.
    ///
    /// COLOR (yellow-green, the commonest gem sphene): Rossman, Caltech Mineral Spectroscopy
    /// Server, Titanite1478 (yellow-green, Madagascar, 3.91 mm, E||b and E||c), a figure only,
    /// digitised 2026-11-03 (`data/reference_spectra/caltech_titanite_1478_digitized_3p91mm.csv`).
    /// Napierian alpha per mm = ln 10 (A - 0.125) / 3.91, 0.125 being the plot's flat level near
    /// 900 nm (reflection loss and scatter); three energy Gaussians fitted per ray, 430/460 to
    /// 780 nm (relative rms 3.4 % / 4.1 %): an ultraviolet edge, the 609 nm band that makes E||b
    /// yellow-green, and weak bands near 485-570 nm. The widths below are the fitted sigma x
    /// 2.35482: `AbsorptionBand::energy` takes the FWHM.
    /// - beta (Y) is E||b as measured.
    /// - gamma (Z) = 2 x E||c and alpha (X) = 0.3 x E||c: ESTIMATED. No pure X or Z spectrum is
    ///   published; on Rossman's plate E||c mixes Z (cos^2 51 deg = 0.4, Z ^ c = 51 deg in Dana's
    ///   setting) with X (0.6), which Dana (after Rosenbusch) and the Handbook give as nearly
    ///   colorless, while Z is red-yellow to orange. 0.4 x 2 + 0.6 x 0.3 = 0.98 rebuilds E||c.
    ///
    /// STRENGTH: 0.7 times the sample's absorption per mm, over the 9.905 mm face-up path of a
    /// 7 mm stone (model units = per mm x 9.905). Chosen from renders of the Pentalume Sapphire
    /// design: 0.5 times is a light lemon-chartreuse (Light Tent L* 65 C* 37 h 96-99), 1.0 times a
    /// deep golden yellow (L* 52 C* 33-36 h 91-95); gem sphene is a medium yellow-green. The same
    /// day 0.4 times that, 0.28 times the sample, after the dealer photos (see the note at the
    /// bands).
    ///
    /// ORIENTATION: the `n_gamma` direction (Z, `c_axis`) lies in the table, so face-up light
    /// travels across the optic plane's acute bisectrix and shows the full birefringence: the
    /// doubled pavilion edges and the Z ray's golden tone that make a sphene look like one.
    /// Along the table normal the stone is viewed nearly down its optic axes (2V 27 deg) and looks
    /// tamer. A design's Crystal Axis turns it anywhere.
    pub(super) fn built_in_material_titanite() -> Self {
        Self {
            name: "Titanite (Sphene)".to_string(),
            crystal_system: CrystalSystem::Monoclinic,
            optical_character: OpticalCharacter::BiaxialPositive,
            // n_beta; n_alpha and n_gamma are `biaxial_principal_dispersion` below.
            dispersion: titanite_curve(2.414_63),
            // n_gamma - n_alpha at the D line of the two principal curves.
            birefringence_delta: 0.122_618,
            // 2026-11-03 (owner: "match the pictures of actual gemstones"): every peak x 0.4, i.e.
            // 0.28 x the sample. Twenty-four dealer photos (GemSelect) have a median hue of 108
            // (colored pixels); the 0.7 x sample rendered h 94, this h 101 (Light Tent; 0.3 x gave
            // h 102 and paler, a turned crystal h 97 with no doubling face-up).
            // 2026-11-04 (owner: "Fix the remaining gaps"): beta's 609 nm band, the yellow-green of
            // E||b, x 3 (an estimate: gem sphene is greener than the one measured crystal), the
            // rest as before. Light Tent h 108 (the photos' median 108; x 2 gave 104), chroma as
            // before. Swatch (the Python replica, not a run): three-ray mean L*88 C*35 h 111,
            // beta h 136 (with the band x 1 L*89 C*37 h 104, beta h 113).
            absorption: AbsorptionTensor::biaxial(
                vec![
                    // alpha
                    AbsorptionBand::energy(400.0, 4_294.0, 1.496),
                    AbsorptionBand::energy(567.2, 5_451.4, 0.081_4),
                    AbsorptionBand::energy(484.4, 2_292.9, 0.117_4),
                ],
                vec![
                    // beta
                    AbsorptionBand::energy(323.56, 8_950.2, 10.22),
                    AbsorptionBand::energy(609.22, 2_182.9, 0.530_7),
                    AbsorptionBand::energy(520.0, 5_445.5, 0.093_68),
                ],
                vec![
                    // gamma
                    AbsorptionBand::energy(400.0, 4_294.0, 9.972),
                    AbsorptionBand::energy(567.2, 5_451.4, 0.542_8),
                    AbsorptionBand::energy(484.4, 2_292.9, 0.782_4),
                ],
            ),
            c_axis: Vec3::X,
            // n_beta - n_alpha at the D line of the curves.
            biaxial_delta_beta_alpha: Some(0.006_194),
            alpha_axis: None,
            scattering_sigma_s: 0.0,
            scattering_g: 0.0,
            edge_rounding_radius: 0.0,
            absorption_path_scale: 1.0,
            absorption_unit: AbsorptionUnit::ModelUnit,
            zoning: None,
            inclusions: None,
            uniaxial_extraordinary_dispersion: None,
            // n_alpha and n_gamma, the same shared pole.
            biaxial_principal_dispersion: Some([
                titanite_curve(2.392_77),
                titanite_curve(2.838_81),
            ]),
        }
    }
}
