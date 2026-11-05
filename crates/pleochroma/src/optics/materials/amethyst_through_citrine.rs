//! Built-in material data: Amethyst, Citrine.

use super::{CrystalSystem, GemMaterial, OpticalCharacter};
use crate::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor},
    dispersion::DispersionModel,
};
use glam::Vec3;

impl GemMaterial {
    /// Continuation of [`Self::built_in_materials_aquamarine_through_citrine`] (split
    /// purely to keep each part under clippy's function-length lint): Amethyst and
    /// Citrine, both quartz.
    pub(super) fn built_in_materials_amethyst_through_citrine() -> Vec<Self> {
        vec![
            // Amethyst (alpha-Quartz, SiO2, color centre) -- physically the same
            // SiO2 crystal as the colorless "Quartz" entry above: reuses that
            // entry's exact Ghosh 1999 o-ray Sellmeier3 fit and its e-ray
            // `uniaxial_extraordinary_dispersion` and `birefringence_delta`
            // bit-for-bit, differing only in absorption. n_d = 1.54421, Delta n(F-C)
            // = 0.00781 (quartz's well-known "0.013" figure is the B-G interval, not
            // F-C; x0.579 gives 0.00753 -- see the Quartz entry above. The stored
            // convention is F-C, 486.1-656.3 nm).
            Self {
                name: "Amethyst".to_string(),
                crystal_system: CrystalSystem::Trigonal,
                optical_character: OpticalCharacter::UniaxialPositive,
                dispersion: DispersionModel::Sellmeier3 {
                    b: [0.286_041_4, 1.070_440_8, 1.102_022_4],
                    c: [0.0, 0.010_058_6, 100.0],
                },
                birefringence_delta: 0.0091,
                // Chromophore: the irradiation-induced Fe4+ ([FeO4]0 hole) centre of
                // Fe3+-bearing quartz, a broad band centred ~545nm (green-yellow) plus
                // a 357nm and a weak ~950nm band (Cox, J. Phys. C 10, 4631 (1977); the
                // Caltech Ametrine page: amethyst and citrine both hold ~40 ppm iron and
                // radiation oxidises Fe3+ to Fe4+), leaving violet/purple transmitted.
                //
                // PER-AXIS since 2026-10-27 (owner decision: a built-in on an anisotropic
                // host never carries an isotropic tensor; the old one-band 545nm
                // wavelength Gaussian, sigma 55nm, peak 1.8, was a tuned stand-in).
                // o-ray (E perp c): the free four-band energy-Gaussian fit of the Caltech
                // Anahi (Bolivia) amethyst `ameth.bo` (6.17 mm slab cut (0001), constant
                // baseline A 0.217 removed; the same fit as the `amethyst.deep_siberian`
                // variant): 364.3 / 416.1 / 537.1 / 953.8nm, FWHM 4509 / 2911 / 5175 /
                // 3073 cm^-1, rms 0.0041 A, model / data 0.98-1.02 from 400 to 650nm.
                // (The first per-axis version fixed the Report's centres and widths and
                // fitted only the amplitudes; its 545nm width (FWHM 3480 cm^-1) is too
                // narrow for this slab, it put the amplitude in the wrong place and the
                // o-ray came out at C* 66 against the measured 42 at the same strength.)
                // e-ray (E par c): ESTIMATED, the o-ray bands at the Report's e/o weights
                // 1.0 / 1.0 / 0.7 / 0.5 (a secondary table; no primary polarized amethyst
                // spectrum exists, which is why the e-ray color is the least certain part
                // and is invisible while the stone is viewed along c). Strength: ONE
                // factor 2.47 x the Anahi crystal's per-mm absorption over every band (the
                // low end of R4's "Deep Siberian" range 2.5-3.5 x); stored as per mm x
                // 2.47 x 9.905 mm, the face-up path of a 7 mm stone (3.5 mm per model unit
                // x `MODEL_UNIT_FACE_UP_PATH` 2.83), because the unit stays `ModelUnit`.
                // 2026-11-04 (owner: "Fix the remaining gaps"): the main band's centre moved from
                // the Anahi slab's 537.1 to 549 nm, inside the 540-550 nm the literature gives for
                // the amethyst centre, widths and strengths unchanged. Dealer photos of amethyst
                // have a median colored hue of 316; the Anahi centre rendered h 333 at every
                // strength (Light Tent), 545 nm h 322, 555 nm h 309. The Anahi type stays in the
                // Ametrine (it comes from that mine; its photos' purple reads h 337) and in the
                // Deep Siberian variant (red flashes).
                // Swatch (7 mm, D65, the Python replica of the app's colorimetry, not a run):
                // with c along the view the stone shows the o-ray, L*49 C*43 h 314; the
                // unpolarised mean is L*53 C*36 h 315 and the e-ray L*60 C*27 h 317 (with the
                // 537.1 nm centre 50 / 41 / 329, 54 / 35 / 331 and 61 / 27 / 336).
                // Calibration record: docs/chromophores/calibration-records.md#amethyst
                absorption: AbsorptionTensor::uniaxial(
                    vec![
                        AbsorptionBand::energy(364.3, 4_508.9, 3.65),
                        AbsorptionBand::energy(416.1, 2_911.3, 0.971),
                        AbsorptionBand::energy(549.0, 5_174.7, 2.14),
                        AbsorptionBand::energy(953.8, 3_072.8, 0.474),
                    ],
                    vec![
                        AbsorptionBand::energy(364.3, 4_508.9, 3.65),
                        AbsorptionBand::energy(416.1, 2_911.3, 0.971),
                        AbsorptionBand::energy(549.0, 5_174.7, 1.5),
                        AbsorptionBand::energy(953.8, 3_072.8, 0.237),
                    ],
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
                    b: [0.288_518_04, 1.095_099_2, 1.156_624_8],
                    c: [0.0, 0.010_210_186, 100.0],
                }),
                biaxial_principal_dispersion: None,
            },
            // Citrine (alpha-Quartz, SiO2, Fe3+) -- same host crystal/dispersion
            // reuse as Amethyst immediately above; see that entry's comment.
            Self {
                name: "Citrine".to_string(),
                crystal_system: CrystalSystem::Trigonal,
                optical_character: OpticalCharacter::UniaxialPositive,
                dispersion: DispersionModel::Sellmeier3 {
                    b: [0.286_041_4, 1.070_440_8, 1.102_022_4],
                    c: [0.0, 0.010_058_6, 100.0],
                },
                birefringence_delta: 0.0091,
                // Chromophore: Fe3+ in small hydrated oxide-hydroxide aggregates (the
                // Caltech quartz page: citrine is heated amethyst, "small aggregates of
                // iron oxide"), a charge-transfer absorption edge rising from the
                // near-UV into blue (a tail rather than a discrete band), leaving
                // yellow-orange transmitted, citrine's characteristic color.
                //
                // PER-AXIS since 2026-10-27 (owner decision: a built-in on an anisotropic
                // host never carries an isotropic tensor; the old one-band 400nm
                // wavelength Gaussian, sigma 70nm, peak 1.6, was a tuned stand-in).
                // Shape: three energy Gaussians (centres 330/410/500nm, FWHM
                // 8477/7064/5181 cm^-1, per-mm peaks 1.656 / 0.136 / 0.023) fitted by
                // relative-error weighted non-negative least squares to the 16-point
                // citrine alpha(lambda) table of `data/reference_spectra/
                // citrine_annealed_amethyst.csv` (350nm 14.5 cm^-1, 400nm 7.4, 450nm 2.8,
                // 500nm 0.95, 545nm 0.35, 605nm 0.12); that table is SECONDARY
                // (user-delivered, unverified, not an instrument spectrum: the shape is
                // the only claim). No polarized citrine spectrum was found and the
                // pleochroism of citrine is gemologically very weak, so the o- and
                // e-ray sets are EQUAL (stored pleochroic, `AbsorptionTensor::uniaxial`,
                // never `isotropic`, on this uniaxial host). Strength 0.53 x the table
                // (per mm x 0.53 x 9.905 mm, the face-up path of a 7 mm stone), chosen
                // so the swatch stays the old one: (7 mm, D65) L*92 C*51 h 94 against
                // the old L*93 C*49 h 91 (dE2000 2.0).
                // Calibration record: docs/chromophores/calibration-records.md#citrine
                // 2026-11-04 (owner: "The render seems to err toward saturated"; citrine was the
                // other way): x 2 (1.06 x the table). Colored-pixel saturation s_uv: the faceted
                // dealer photos 0.79, the Commons photos 0.95-1.13, the x 1 render 0.57 (a pale
                // lemon against golden stones), x 2 renders 0.80 at h 83. Swatch L*85 C*77 h 87.
                // The Ametrine's citrine half (11 ppma iron) stays: L* 91 h 94, within the 7 L* and
                // 10 deg its test allows.
                // Calibration record: docs/chromophores/calibration-records.md#saturation-round-2026-11-04
                absorption: AbsorptionTensor::uniaxial(
                    vec![
                        AbsorptionBand::energy(330.0, 8_477.0, 8.69),
                        AbsorptionBand::energy(410.0, 7_064.0, 0.714),
                        AbsorptionBand::energy(500.0, 5_181.0, 0.121),
                    ],
                    vec![
                        AbsorptionBand::energy(330.0, 8_477.0, 8.69),
                        AbsorptionBand::energy(410.0, 7_064.0, 0.714),
                        AbsorptionBand::energy(500.0, 5_181.0, 0.121),
                    ],
                )
                .scaled(2.0),
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
                    b: [0.288_518_04, 1.095_099_2, 1.156_624_8],
                    c: [0.0, 0.010_210_186, 100.0],
                }),
                biaxial_principal_dispersion: None,
            },
        ]
    }
}
