//! Built-in material data: Nanosital.
//!
//! Nanosital is a manufactured glass-ceramic, not a mineral and not a laboratory-grown copy of
//! one: nanometre-sized spinel-group crystals (spinel `MgAl2O4` to gahnite `ZnAl2O4`) grown by a
//! controlled anneal inside an aluminosilicate glass of the Si-Al-Mg-Zn system, with Ti and Zr
//! oxides as nucleating agents. It was developed in Russia in 2013 ("nanositall", glass-ceramic)
//! and is mass-produced in many colors as an imitation of sapphire, ruby, emerald, tsavorite,
//! peridot, topaz, tourmaline, spinel, garnet, kunzite and citrine. Its color variants are in
//! `data/color_variants/nanosital.toml`, its chromophores in the `nanosital` host of
//! `data/chromophores.toml`.
//!
//! Primary source: W. Soonthorntantikul, W. Vertriest, K. Avakyan, "Glass-ceramics as imitation
//! gemstones: a study on a silicon-aluminum-magnesium-zinc system", Gems & Gemology 62(2),
//! 164-176 (Summer 2026), doi 10.5741/GEMS.62.2.164 (60 samples, GIA laboratory, Bangkok).

use super::{AbsorptionUnit, CrystalSystem, GemMaterial, OpticalCharacter};
use crate::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor},
    dispersion::DispersionModel,
};
use glam::Vec3;

impl GemMaterial {
    /// Nanosital (Si-Al-Mg-Zn glass-ceramic).
    ///
    /// INDEX (measured): Gems & Gemology 62(2), 2026: every sample singly refractive, RI 1.60-1.63
    /// for every color (Table 1); the blue glass-ceramic GC-02 measured 1.622 (Table 2). The entry
    /// takes `n_d` = 1.620, the centre of the measured range. The trade figures (1.65-1.70, the
    /// manufacturer's text in Russian Jeweller, 13 July 2016, russianjeweller.ru/2/8/8822, and the
    /// Australian Facetors' Guild page facetorsguild.com.au/Nanosital-Properties) are well above
    /// every GIA measurement and are not used.
    ///
    /// ISOTROPY: the glass matrix and the cubic spinel crystals are both isotropic; GIA saw single
    /// refraction with occasional anomalous double refraction, which is not modelled. `Cubic` is
    /// this enum's isotropic variant (as for the glasses and Opal): the crystal phase is cubic,
    /// the glass has no lattice.
    ///
    /// DISPERSION, CHOICE: the only published figure is the manufacturer's B-G 0.015 (the two
    /// trade sources above; no independent measurement found). It is converted to F-C with the
    /// house ratio 0.579 (see the Emerald entry): Delta n(F-C) = 0.008685, a two-term Cauchy curve
    /// b = 0.008685 / (1/0.4861^2 - 1/0.6563^2) = 0.004546 um^2, a = 1.620 - b / 0.5893^2 =
    /// 1.606909. The curve gives B-G 0.0149 and Abbe `V_d` = 71.4. That `V_d` is high for an
    /// `n_d` 1.62 silicate, where 55-60 is usual (spinel itself has `V_d` 60.6, and the `TiO2`
    /// nucleating agent lowers `V_d` further), so the figure may well understate the fire. A
    /// measured dispersion replaces it.
    ///
    /// COLOR, CHOICE: the default is the cobalt blue of a sapphire imitation, the best-documented
    /// spectrum (GIA GC-02 and figure 8A). The three Co2+ bands sit at the GIA positions 545, 585
    /// and 625 nm. Their FWHM (1100 cm^-1) and strengths are the ESTIMATED Co Blue of the silicate
    /// glass host (0.06 wt% `CoO`: 0.48 / 0.69 / 0.60 per mm, see
    /// `docs/chromophores/quartz_diamond_zoisite_synthetics_glass.md`), in model units of a 7 mm stone (per mm x
    /// 9.905). Face-up 7 mm swatch about L* 50, C* 86, h 279: a royal blue near the Sapphire
    /// default. GC-02 also holds erbium, whose narrow lines are left out. Strengths are estimates
    /// until calibrated against a measured spectrum
    /// (`docs/chromophores/calibration-records.md#nanosital`).
    pub(super) fn built_in_material_nanosital() -> Self {
        Self {
            name: "Nanosital".to_string(),
            crystal_system: CrystalSystem::Cubic,
            optical_character: OpticalCharacter::Isotropic,
            dispersion: DispersionModel::Cauchy {
                a: 1.606_909,
                b: 0.004_546,
                c: 0.0,
            },
            birefringence_delta: 0.0,
            absorption: AbsorptionTensor::isotropic(vec![
                AbsorptionBand::energy(545.0, 1_100.0, 4.754),
                AbsorptionBand::energy(585.0, 1_100.0, 6.834),
                AbsorptionBand::energy(625.0, 1_100.0, 5.943),
            ]),
            c_axis: Vec3::Y,
            biaxial_delta_beta_alpha: None,
            alpha_axis: None,
            scattering_sigma_s: 0.0,
            scattering_g: 0.0,
            edge_rounding_radius: 0.0,
            absorption_path_scale: 1.0,
            absorption_unit: AbsorptionUnit::ModelUnit,
            zoning: None,
            inclusions: None,
            uniaxial_extraordinary_dispersion: None,
            biaxial_principal_dispersion: None,
        }
    }
}
