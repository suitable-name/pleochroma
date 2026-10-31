//! Built-in material data: Andalusite, Opal, and the two Schott optical glasses.

use super::{CrystalSystem, GemMaterial, OpticalCharacter};
use crate::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor},
    dispersion::DispersionModel,
};
use glam::Vec3;

impl GemMaterial {
    /// Fourth and final quarter: Andalusite (the third biaxial
    /// addition), Opal, and the two Schott optical glasses.
    #[expect(clippy::too_many_lines, reason = "a data table of built-in materials")]
    pub(super) fn built_in_materials_andalusite_through_glass() -> Vec<Self> {
        vec![
            // Andalusite (Al2SiO5): n_d=1.634, B-G 0.016 -> Delta n(F-C) = 0.00926,
            // A=1.634-0.004849/0.5893^2=1.620041, B=0.004849. Abbe V_d~68.45. No
            // primary Sellmeier fit is available; LOWER CONFIDENCE
            // 2-parameter Cauchy fallback. `birefringence_delta` stored as the
            // POSITIVE n_gamma-n_alpha magnitude (this field's own doc comment: "or
            // max-min") -- `BiaxialPositive`/`BiaxialNegative` is a genuinely separate
            // property (which principal axis the acute bisectrix sits closer to, NOT
            // the sign of the total birefringence, which is never negative by the
            // alpha<=beta<=gamma sort `biaxial_indicatrix` assumes).
            Self {
                name: "Andalusite".to_string(),
                crystal_system: CrystalSystem::Orthorhombic,
                optical_character: OpticalCharacter::BiaxialNegative,
                dispersion: DispersionModel::Cauchy {
                    a: 1.620_041,
                    b: 0.004_849,
                    c: 0.0,
                },
                birefringence_delta: 0.010,
                // PLEOCHROISM: andalusite is a classic TRICHROIC teaching example,
                // commonly described as red/reddish-brown along one axis, yellow-green
                // along a second and olive along the third. REFITTED 2026-10-27 to a
                // measured crystal: the earlier entry was a QUALITATIVE, tuned pattern
                // (alpha 500nm 1.0, beta 480nm 1.8, gamma 580nm 1.4, wavelength
                // Gaussians) that put the strongest absorption on BETA. The Caltech
                // spectra (M. N. Taran, colour-zoned Fe-Ti andalusite, Brazil, FeO 0.35
                // wt% and a trace of Ti, light zone, normalised to 1 cm from 0.79 mm;
                // per-ray baselines A 0.19 / 0.08 / 0.13 per cm removed) show that
                // ALPHA (E parallel to c) carries essentially all of the Fe2+-Ti4+
                // charge-transfer colour (a band at 486nm, 2.3 per mm above the
                // baseline, the "intense brownish-red colour for light polarised in c",
                // Caltech page) while beta (E par b) and gamma (E par a) are nearly
                // colorless (0.08 and 0.04 per mm at 486nm, a UV edge only).
                //
                // AXIS MAPPING (fix the data, not the convention): `biaxial(alpha, beta,
                // gamma)` slots are the n_alpha / n_beta / n_gamma principal directions
                // (`o_ray` / `beta_ray` / `e_ray`, gamma along `c_axis`), as for
                // Alexandrite and Tanzanite. Andalusite's optical orientation is X (n_alpha)
                // = crystallographic c, Y (n_beta) = b, Z (n_gamma) = a, the optic plane
                // (010), which is exactly Caltech's labelling (alpha = E par c, beta =
                // E par b, gamma = E par a); the repo's earlier note (beta par a, gamma
                // par b) is not supported by it. So the Fe-Ti colour goes in the
                // `o_ray` (alpha) slot.
                //
                // Bands (energy Gaussians, non-linear least squares over 380-900nm,
                // rms 0.017 / 0.011 / 0.006 per mm): alpha a UV edge (308nm), a 430nm
                // shoulder, the 497nm Fe2+-Ti4+ band (FWHM 4718 cm^-1) and two weak red
                // ones (620, 700nm); beta its UV edge (on the 300nm bound), a 380nm band
                // and a faint broad one; gamma the same two UV-side bands. Strength
                // 0.10 x the light zone's per-mm absorption (the alpha band is
                // 2.3 per mm there, opaque over a 9.9 mm stone; R2's "0.2-0.3 of this"
                // is too dark for a swatch; stored as per mm x 0.10 x 9.905 mm, the
                // face-up path of a 7 mm stone; the dark zone is ~4 x the light one and
                // saturated). The measured colours DISAGREE with the old swatch, and
                // the old swatch is the wrong one: (7 mm, D65) alpha L*63 C*39 h 41
                // (brownish red-orange), beta L*98 C*7 h 108 and gamma L*99 C*3 h 111
                // (pale yellow), unpolarised (mean of three) L*89 C*9 h 60 against the
                // old L*82 C*22 h 356 (a pink: the old blend of red-brown, yellow-green
                // and violet rays; dE2000 17.3). The three-ray mean has no green: the
                // greenish, olive and brown stones of the trade owe their colour to
                // Fe3+, Mn3+ (viridine) and V3+ / Cr3+, which are colour variants, not
                // this Fe-Ti default.
                // Calibration record: docs/chromophores/calibration-records.md#andalusite
                absorption: AbsorptionTensor::biaxial(
                    vec![
                        // alpha (E par c): the Fe2+-Ti4+ ray, brownish red
                        AbsorptionBand::energy(308.1, 7_382.0, 4.87),
                        AbsorptionBand::energy(430.0, 4_867.0, 0.745),
                        AbsorptionBand::energy(496.8, 4_718.0, 2.02),
                        AbsorptionBand::energy(620.0, 2_151.0, 0.197),
                        AbsorptionBand::energy(700.0, 3_631.0, 0.272),
                    ],
                    vec![
                        // beta (E par b): pale yellow, UV edge
                        AbsorptionBand::energy(300.0, 7_180.0, 6.42),
                        AbsorptionBand::energy(380.0, 6_522.0, 0.306),
                        AbsorptionBand::energy(520.0, 12_000.0, 0.0422),
                    ],
                    vec![
                        // gamma (E par a): nearly colorless
                        AbsorptionBand::energy(370.0, 5_253.0, 0.221),
                        AbsorptionBand::energy(423.6, 4_330.0, 0.0417),
                        AbsorptionBand::energy(520.0, 12_000.0, 0.0226),
                    ],
                ),
                c_axis: Vec3::Y,
                // No primary 3-index andalusite measurement was available in this
                // pass. Commonly tabulated ranges (e.g. Mindat/Gemology Project)
                // place n_alpha~1.629-1.640, n_beta~1.633-1.644, n_gamma~1.638-1.650;
                // midpoints (1.6345, 1.6385, 1.644) give fraction
                // (1.6385-1.6345)/(1.644-1.6345) = 0.421 -- BUT andalusite's negative
                // optic sign means the acute bisectrix sits toward alpha (beta closer
                // to gamma), the opposite lean from the three existing (all positive-
                // sign) biaxial built-ins above, so this uses 1-0.421 = 0.579 (fraction
                // of birefringence_delta beta sits ABOVE alpha) applied here as
                // delta_beta_alpha = 0.579 * 0.010 = 0.00579, rounded to 0.0058.
                biaxial_delta_beta_alpha: Some(0.0058),
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
            // Opal (amorphous hydrated SiO2, common/body-color opal -- NOT precious
            // opal's structural play-of-color, which this Gaussian-band absorption
            // model has no mechanism to represent at all: that effect is diffraction
            // off an ordered silica-sphere lattice, not a wavelength-selective
            // absorption coefficient, and is explicitly OUT OF SCOPE for this entry.
            // `crystal_system: Cubic` is a placeholder here,
            // not a mineralogical claim -- opal is a mineraloid with no true crystal
            // structure at all, and `CrystalSystem` has no "amorphous" variant; Cubic
            // is chosen only because it is this enum's existing isotropic-only
            // variant, matching this entry's `Isotropic`/`birefringence_delta: 0.0`
            // (a real, correct claim: amorphous silica has no birefringence).
            //
            // Dispersion: `n_d=1.45` is the standard reference value for common
            // opal's essentially glass-like silica; "Disp" chosen as a small but
            // non-zero Delta n(F-C) = 0.001 (not a cited figure -- deliberately a
            // small hand-picked value, "~0.0, use a tiny
            // positive value," since opal's classical dispersion is minor and
            // visually dominated by the unmodelled structural play-of-color anyway).
            // A=1.45-0.000523/0.5893^2=1.448493, B=0.000523.
            Self {
                name: "Opal".to_string(),
                crystal_system: CrystalSystem::Cubic,
                optical_character: OpticalCharacter::Isotropic,
                dispersion: DispersionModel::Cauchy {
                    a: 1.448_493,
                    b: 0.000_523,
                    c: 0.0,
                },
                birefringence_delta: 0.0,
                // Body color only, and even that varies far too widely (white,
                // black, fire/orange body opal) for one representative band set --
                // left colorless (empty band set) as the neutral reference; a
                // specific body-color variant (e.g. fire opal's Fe3+ tint) is a
                // candidate follow-up built-in, same convention as un-added Topaz/
                // Tanzanite variants above. Play-of-color is NOT modelled -- see this
                // entry's own comment above.
                absorption: AbsorptionTensor::isotropic(vec![]),
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
            // Glass (Schott N-BK7) -- the most common optical crown glass, included
            // as a colorless isotropic reference/calibration material (e.g. for
            // comparing a gemstone's fire against ordinary glass).
            //
            // Source: SCHOTT optical glass data sheet (N-BK7), Sellmeier coefficients
            // as tabulated by refractiveindex.info ("SCHOTT-optical: N-BK7"):
            //   n^2-1 = 1.03961212*l^2/(l^2-0.00600069867) + 0.231792344*l^2/(l^2-0.0200179144)
            //           + 1.01046945*l^2/(l^2-103.560653)
            // Independently verified by hand computation: n_d = 1.51672
            // (catalogue nd = 1.5168, matching to <0.0001 -- the small residual is the
            // catalogue's helium d-line 587.56nm vs this crate's sodium-D convention
            // 589.3nm), Delta n(F-C) = 0.00808 (catalogue-derived 0.0081), Abbe
            // V_d = 63.98 (catalogue Vd = 64.17) -- all within the 0.002
            // tight tolerance.
            Self {
                name: "Glass (N-BK7)".to_string(),
                crystal_system: CrystalSystem::Cubic,
                optical_character: OpticalCharacter::Isotropic,
                dispersion: DispersionModel::Sellmeier3 {
                    b: [1.039_612_2, 0.231_792_35, 1.010_469_4],
                    c: [0.006_000_699, 0.020_017_914, 103.560_65],
                },
                birefringence_delta: 0.0,
                // colorless glass: empty band set, zero absorption at every
                // wavelength.
                absorption: AbsorptionTensor::isotropic(vec![]),
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
            // Glass (Schott F2) -- a common dense flint glass, the classic
            // high-dispersion counterpart to N-BK7's crown glass above (an
            // achromatic-doublet pairing in real optics), included for the same
            // colorless-reference reason.
            //
            // Source: SCHOTT optical glass data sheet (F2), Sellmeier coefficients as
            // tabulated by refractiveindex.info ("SCHOTT-optical: F2"):
            //   n^2-1 = 1.34533359*l^2/(l^2-0.00997743871) + 0.209073176*l^2/(l^2-0.0470450767)
            //           + 0.937357162*l^2/(l^2-111.886764)
            // Independently verified by hand computation: n_d = 1.61980
            // (catalogue nd = 1.62004, matching to <0.0003 -- same helium-d/sodium-D
            // line offset as N-BK7 above), Delta n(F-C) = 0.01705 (catalogue-derived
            // 0.0170), matching the catalogue Vd = 36.37 directly.
            Self {
                name: "Glass (F2)".to_string(),
                crystal_system: CrystalSystem::Cubic,
                optical_character: OpticalCharacter::Isotropic,
                dispersion: DispersionModel::Sellmeier3 {
                    b: [1.345_333_6, 0.209_073_17, 0.937_357_2],
                    c: [0.009_977_439, 0.047_045_077, 111.886_76],
                },
                birefringence_delta: 0.0,
                // colorless glass: empty band set, zero absorption at every
                // wavelength.
                absorption: AbsorptionTensor::isotropic(vec![]),
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
