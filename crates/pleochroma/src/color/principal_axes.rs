//! Principal-axis weights of a pleochroic crystal: how much of unpolarised light travelling
//! along a wave normal `k` is absorbed by each principal axis of the absorption tensor.
//!
//! The absorption quadratic form is linear in the principal coefficients, so a segment of
//! length `L` travelling along `k` has, for each eigenmode `m`, the optical depth
//! `sum_p alpha_p(lambda) * L * w_p^m(k)`, where `w_p^m(k)` is the squared projection of the
//! mode's E-field on principal axis `p` ([`PrincipalFrame::mode_weights`]; the E-field, not the
//! D-field, is what the absorption quadratic form is written in). The weights of one mode sum
//! to 1 (a mode is a unit vector).
//!
//! The optical depth is therefore per MODE, and unpolarised light is an equal mixture of the
//! two modes: its transmittance is the mean of the two modes' transmittances,
//! `(exp(-d_a) + exp(-d_b)) / 2`, never `exp(-(d_a + d_b) / 2)`. The mean-weight shortcut
//! [`PrincipalFrame::weights`] (`w_p = (w_p^a + w_p^b) / 2`, a mean of weights, not a
//! transmittance) is exact only when both modes see the same absorption (a path along an optic
//! axis, or equal bands in every slot); for a path whose depth is additive per segment
//! (`D_p = sum_segments L * w_p(k)`, wavelength independent) it is what the doubling tracer
//! accumulates per ray, while the face-up swatch ([`face_up_body_color`]) keeps the two modes
//! separate.
//!
//! Slot order everywhere: `[0, 1, 2]` = the `o_ray` coefficient, the `beta_ray` coefficient
//! (the `o_ray` one for a uniaxial tensor), the `e_ray` coefficient; the frame's axes columns
//! are in the same order (uniaxial: two arbitrary in-plane axes, then `c`; biaxial: alpha,
//! beta, gamma of the material's indicatrix).

use glam::{Mat3, Vec3};

use super::body_color::{BodyColor, Illuminant, body_color_transmittance, body_colors};
use crate::optics::{
    absorption::{AbsorptionBand, AbsorptionTensor},
    birefringence::{AbsorptionTensor3, BiaxialIndicatrix, assigned_mode_e_field_uniaxial},
    materials::GemMaterial,
};

/// Which pair of eigenmodes the weights are taken from.
#[derive(Debug, Clone, Copy)]
enum Modes {
    /// Ordinary and extraordinary modes about `c_axis`; `delta_n` is `n_e - n_o` (it only
    /// sets the extraordinary mode's walk-off).
    Uniaxial { c_axis: Vec3, delta_n: f32 },
    /// The two eigenmodes of a genuinely biaxial indicatrix.
    Biaxial(BiaxialIndicatrix),
}

/// The principal frame of a crystal's absorption tensor plus the eigenmodes needed to weight
/// it; see the module docs. Plain `Copy` data, built once per evaluation.
#[derive(Debug, Clone, Copy)]
pub struct PrincipalFrame {
    /// Principal axes as columns, in the slot order of the module docs.
    axes: Mat3,
    modes: Modes,
    /// Ordinary (biaxial: middle) index at the d-line, used by [`Self::face_up_weights`].
    n_o: f32,
}

impl PrincipalFrame {
    /// A uniaxial frame about `c_axis` (any length; the zero vector falls back to `+Y`).
    #[must_use]
    pub fn uniaxial(c_axis: Vec3, n_o: f32, delta_n: f32) -> Self {
        let c = c_axis.try_normalize().unwrap_or(Vec3::Y);
        Self {
            axes: AbsorptionTensor3::uniaxial(0.0, 0.0, c).axes,
            modes: Modes::Uniaxial { c_axis: c, delta_n },
            n_o,
        }
    }

    /// A biaxial frame: the principal axes are the indicatrix's own.
    #[must_use]
    pub const fn biaxial(indicatrix: BiaxialIndicatrix) -> Self {
        Self {
            axes: indicatrix.axes,
            modes: Modes::Biaxial(indicatrix),
            n_o: indicatrix.n_beta,
        }
    }

    /// The frame of `material`'s own optics at the d-line: its biaxial indicatrix when it has
    /// one, else the uniaxial frame about `material.c_axis`. Built whether or not the
    /// material's absorption is pleochroic; a caller that only wants a pleochroic one uses
    /// [`Self::for_pleochroic`].
    #[must_use]
    pub fn of_material(material: &GemMaterial) -> Self {
        if let Some(indicatrix) = material.biaxial_indicatrix(589.3) {
            return Self::biaxial(indicatrix);
        }
        let n_o = material.dispersion.evaluate(589.3).max(1.1);
        let n_e = material.extraordinary_index_at(589.3, n_o);
        Self::uniaxial(material.c_axis, n_o, n_e - n_o)
    }

    /// [`Self::of_material`] when the material's absorption tensor is pleochroic, `None`
    /// otherwise (an isotropic tensor needs no axis weights: its path length is all there is).
    #[must_use]
    pub fn for_pleochroic(material: &GemMaterial) -> Option<Self> {
        material
            .absorption
            .is_pleochroic
            .then(|| Self::of_material(material))
    }

    /// The principal axes as columns, in the slot order of the module docs.
    #[must_use]
    pub const fn axes(&self) -> Mat3 {
        self.axes
    }

    /// The weight of each principal axis for EACH eigenmode of a wave travelling along the unit
    /// wave normal `k_hat` in a medium of ordinary index `index`: the squared projection of the
    /// mode's E-field on the axis, one `[f32; 3]` per mode (each sums to 1).
    ///
    /// Uniaxial: the ordinary mode has no component along `c`; the extraordinary mode's E-field
    /// is what [`assigned_mode_e_field_uniaxial`] returns. Biaxial: the eigenmodes of the
    /// indicatrix are electric-displacement (D) directions; each is converted to its E-field
    /// with `d_to_e_direction` before projecting, because the absorption weight is an E-field
    /// quantity (D and E coincide along a principal axis and differ elsewhere).
    #[must_use]
    pub fn mode_weights(&self, k_hat: Vec3, index: f32) -> ([f32; 3], [f32; 3]) {
        let (mode_a, mode_b) = match self.modes {
            Modes::Uniaxial { c_axis, delta_n } => (
                assigned_mode_e_field_uniaxial(k_hat, c_axis, false, index, index + delta_n),
                assigned_mode_e_field_uniaxial(k_hat, c_axis, true, index, index + delta_n),
            ),
            Modes::Biaxial(indicatrix) => {
                let (d_a, d_b) = indicatrix.eigen_polarizations(k_hat);
                (
                    indicatrix.d_to_e_direction(d_a),
                    indicatrix.d_to_e_direction(d_b),
                )
            }
        };
        let local_a = self.axes.transpose() * mode_a;
        let local_b = self.axes.transpose() * mode_b;
        (
            (local_a * local_a).to_array(),
            (local_b * local_b).to_array(),
        )
    }

    /// The mean over both eigenmodes of [`Self::mode_weights`]: `(w^a + w^b) / 2`, a mean of
    /// WEIGHTS, not a transmittance. Sums to 1.
    ///
    /// Using it as `sum_p alpha_p w_p` in a single `exp` gives `exp(-(d_a + d_b) / 2)`, which is
    /// not the unpolarised transmittance `(exp(-d_a) + exp(-d_b)) / 2` unless both modes see the
    /// same absorption. A caller that exponentiates (a swatch, a color) uses
    /// [`Self::mode_weights`]; this mean is for callers that accumulate a linear path length per
    /// principal axis.
    ///
    /// Uniaxial: the ordinary mode has no component along `c`, so `w_c = 0.5 (e_e . c)^2`;
    /// along the optic axis `w_c = 0`.
    #[must_use]
    #[expect(
        clippy::manual_midpoint,
        reason = "`f32::midpoint` rounds differently from `(a + b) * 0.5`; these weights feed swatches whose pinned crystal-axis hues must not move"
    )]
    pub fn weights(&self, k_hat: Vec3, index: f32) -> [f32; 3] {
        let (a, b) = self.mode_weights(k_hat, index);
        [
            (a[0] + b[0]) * 0.5,
            (a[1] + b[1]) * 0.5,
            (a[2] + b[2]) * 0.5,
        ]
    }

    /// [`Self::weights`] for the face-up wave normal `k = -Y` (the swatch's single path
    /// direction) at the frame's own d-line index.
    #[must_use]
    pub fn face_up_weights(&self) -> [f32; 3] {
        self.weights(Vec3::NEG_Y, self.n_o)
    }

    /// [`Self::mode_weights`] for the face-up wave normal `k = -Y` at the frame's own d-line
    /// index: the per-mode weights the face-up swatch exponentiates separately.
    #[must_use]
    pub fn face_up_mode_weights(&self) -> ([f32; 3], [f32; 3]) {
        self.mode_weights(Vec3::NEG_Y, self.n_o)
    }
}

/// The three principal absorption coefficients of `tensor` at `lambda_nm`, in the slot order of
/// the module docs, each clamped to be non-negative: `[o_ray, beta_ray or o_ray, e_ray]`.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    reason = "band evaluation takes an f32 wavelength; nanometres are in the normal float range"
)]
pub fn principal_alphas(tensor: &AbsorptionTensor, lambda_nm: f64) -> [f64; 3] {
    let lambda = lambda_nm as f32;
    let sum = |bands: &[AbsorptionBand]| -> f64 {
        f64::from(bands.iter().map(|band| band.evaluate(lambda)).sum::<f32>()).max(0.0)
    };
    let o = sum(&tensor.o_ray);
    let beta = tensor.beta_ray.as_deref().map_or(o, sum);
    [o, beta, sum(&tensor.e_ray)]
}

/// The face-up body color of `tensor` over `path`, oriented as `frame` says.
///
/// `path` is in the unit of the tensor (per-mm absorption over millimetres, or model units over
/// model units). The wave normal is the single `k = -Y`, unpolarised: each of the two
/// eigenmodes `m` has its own optical depth `d_m = path * sum_p alpha_p w_p^m(-Y)`, and the
/// unpolarised transmittance is the mean of the modes' transmittances,
/// `(exp(-d_a) + exp(-d_b)) / 2` (not `exp` of the mean depth).
///
/// Without a frame, or for a non-pleochroic tensor, this is exactly
/// `body_colors(tensor, path, ill).unpolarised` (bit for bit), the orientation-free swatch.
#[must_use]
pub fn face_up_body_color(
    tensor: &AbsorptionTensor,
    frame: Option<&PrincipalFrame>,
    path: f64,
    ill: Illuminant,
) -> BodyColor {
    let Some(frame) = frame.filter(|_| tensor.is_pleochroic) else {
        return body_colors(tensor, path, ill).unpolarised;
    };
    let (w_a, w_b) = frame.face_up_mode_weights();
    let (w_a, w_b) = (normalised_weights(w_a), normalised_weights(w_b));
    body_color_transmittance(
        |lambda| two_mode_transmittance(tensor, w_a, w_b, path, lambda),
        ill,
    )
}

/// One mode's weights widened to `f64` and renormalised to sum to exactly 1 (the `f32`
/// projections sum to 1 only to about `1e-7`, which would otherwise leak into the depth of a
/// tensor with equal absorption in every slot).
fn normalised_weights(w: [f32; 3]) -> [f64; 3] {
    let w = w.map(f64::from);
    let total = w[0] + w[1] + w[2];
    if total > 0.0 { w.map(|v| v / total) } else { w }
}

/// The unpolarised transmittance at `lambda_nm` of `path` of `tensor` for the two modes with
/// slot weights `w_a` and `w_b`: `0.5 (exp(-d_a) + exp(-d_b))`, each depth `path * (alpha . w)`
/// clamped at 0.
#[expect(
    clippy::manual_midpoint,
    reason = "`f64::midpoint` rounds differently from `0.5 * (a + b)`; the swatch hues pinned in the crystal-axis presets must not move"
)]
fn two_mode_transmittance(
    tensor: &AbsorptionTensor,
    w_a: [f64; 3],
    w_b: [f64; 3],
    path: f64,
    lambda_nm: f64,
) -> f64 {
    let a = principal_alphas(tensor, lambda_nm);
    let depth = |w: [f64; 3]| (path * a[2].mul_add(w[2], a[1].mul_add(w[1], a[0] * w[0]))).max(0.0);
    0.5 * ((-depth(w_a)).exp() + (-depth(w_b)).exp())
}

/// The face-up unpolarised transmittance of `tensor` over `path` at one wavelength, for the
/// two eigenmodes of `frame` at `k = -Y`: the integrand [`face_up_body_color`] colours.
#[cfg(test)]
pub(crate) fn face_up_transmittance_at(
    tensor: &AbsorptionTensor,
    frame: &PrincipalFrame,
    path: f64,
    lambda_nm: f64,
) -> f64 {
    let (w_a, w_b) = frame.face_up_mode_weights();
    two_mode_transmittance(
        tensor,
        normalised_weights(w_a),
        normalised_weights(w_b),
        path,
        lambda_nm,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sum(w: [f32; 3]) -> f32 {
        w.iter().sum()
    }

    fn close(a: [f32; 3], b: [f32; 3], tolerance: f32) -> bool {
        a.iter().zip(b).all(|(x, y)| (x - y).abs() < tolerance)
    }

    fn directions() -> Vec<Vec3> {
        let mut out = Vec::new();
        for i in 0..7 {
            for j in 0..11 {
                let polar = (i as f32 + 0.5) / 7.0 * std::f32::consts::PI;
                let azimuth = j as f32 / 11.0 * std::f32::consts::TAU;
                out.push(Vec3::new(
                    polar.sin() * azimuth.cos(),
                    polar.cos(),
                    polar.sin() * azimuth.sin(),
                ));
            }
        }
        out
    }

    #[test]
    fn uniaxial_weights_sum_to_one_and_the_ordinary_mode_has_no_c_part() {
        let frame = PrincipalFrame::uniaxial(Vec3::new(0.3, 0.8, -0.2), 1.77, -0.008);
        for k in directions() {
            let w = frame.weights(k, 1.77);
            assert!((sum(w) - 1.0).abs() < 1e-5, "{k:?}: {w:?}");
            assert!(w[2] <= 0.5 + 1e-5, "{k:?}: {w:?}");
            assert!(w.iter().all(|&v| v >= -1e-6), "{k:?}: {w:?}");
        }
    }

    #[test]
    fn along_the_optic_axis_the_c_weight_is_zero() {
        let frame = PrincipalFrame::uniaxial(Vec3::Y, 1.77, -0.008);
        let w = frame.weights(Vec3::NEG_Y, 1.77);
        assert!(w[2].abs() < 1e-6, "{w:?}");
        assert!((w[0] + w[1] - 1.0).abs() < 1e-5, "{w:?}");
        // Perpendicular to the axis the extraordinary mode lies along it: w_c is one half.
        let side = frame.weights(Vec3::X, 1.77);
        assert!((side[2] - 0.5).abs() < 1e-3, "{side:?}");
    }

    #[test]
    fn biaxial_weights_sum_to_one() {
        let indicatrix =
            BiaxialIndicatrix::from_gamma_axis(1.57, 1.575, 1.58, Vec3::new(0.1, 1.0, 0.4));
        let frame = PrincipalFrame::biaxial(indicatrix);
        for k in directions() {
            let w = frame.weights(k, 1.575);
            assert!((sum(w) - 1.0).abs() < 1e-4, "{k:?}: {w:?}");
        }
    }

    fn tourmaline_like(c_axis: Vec3) -> GemMaterial {
        let mut material = GemMaterial::sapphire();
        material.absorption = AbsorptionTensor::uniaxial(
            vec![AbsorptionBand::new(580.0, 60.0, 0.5)],
            vec![AbsorptionBand::new(450.0, 50.0, 0.5)],
        );
        material.c_axis = c_axis;
        material
    }

    #[test]
    fn the_face_up_swatch_depends_on_the_axis_direction() {
        let along = tourmaline_like(Vec3::Y);
        let across = tourmaline_like(Vec3::X);
        let swatch = |m: &GemMaterial| {
            face_up_body_color(
                &m.absorption,
                PrincipalFrame::for_pleochroic(m).as_ref(),
                2.5,
                Illuminant::D65,
            )
        };
        assert_ne!(swatch(&along).srgb, swatch(&across).srgb);
        // c along the view direction: the stone shows the ordinary color alone.
        let o_only = body_colors(&along.absorption, 2.5, Illuminant::D65).o_ray;
        for (a, b) in swatch(&along).lab.iter().zip(o_only.lab) {
            assert!(
                (a - b).abs() < 1e-3,
                "{:?} vs {:?}",
                swatch(&along).lab,
                o_only.lab
            );
        }
    }

    /// A band so wide that its value is its peak at every wavelength near 550 nm.
    fn flat_band(peak: f32) -> AbsorptionBand {
        AbsorptionBand::new(550.0, 1.0e6, peak)
    }

    #[test]
    fn two_modes_average_transmittance_not_depth() {
        // Face-up k = -Y with c along X: the ordinary mode is along Z (ordinary slots only),
        // the extraordinary mode along c (the e_ray slot only, no walk-off at 90 degrees).
        // o_ray absorbs nothing, e_ray has depth 2.0 * 2.0 = 4 over the path.
        let tensor = AbsorptionTensor::uniaxial(Vec::new(), vec![flat_band(2.0)]);
        let frame = PrincipalFrame::uniaxial(Vec3::X, 1.77, -0.008);
        let t = face_up_transmittance_at(&tensor, &frame, 2.0, 550.0);
        let mean_of_transmittances = f64::midpoint(1.0, (-4.0_f64).exp());
        assert!((mean_of_transmittances - 0.50916).abs() < 1e-4);
        assert!(
            (t - mean_of_transmittances).abs() < 1e-4,
            "{t} vs {mean_of_transmittances}"
        );
        // The old mean-weight shortcut, exp of the mean depth, is 0.1353 and is not what is
        // returned.
        assert!((t - (-2.0_f64).exp()).abs() > 0.3, "{t}");
    }

    #[test]
    fn equal_absorption_in_both_modes_matches_the_old_result() {
        let bands = vec![
            AbsorptionBand::new(580.0, 60.0, 0.5),
            AbsorptionBand::new(450.0, 40.0, 0.3),
        ];
        let tensor = AbsorptionTensor::uniaxial(bands.clone(), bands);
        assert!(tensor.is_pleochroic);
        for c_axis in [Vec3::Y, Vec3::X, Vec3::new(0.3, 0.8, -0.2)] {
            let frame = PrincipalFrame::uniaxial(c_axis, 1.77, -0.008);
            let new = face_up_body_color(&tensor, Some(&frame), 2.5, Illuminant::D65);
            let old = body_colors(&tensor, 2.5, Illuminant::D65).unpolarised;
            for (a, b) in new.lab.iter().zip(old.lab) {
                assert!(
                    (a - b).abs() < 1e-9,
                    "{c_axis:?}: {:?} vs {:?}",
                    new.lab,
                    old.lab
                );
            }
        }
    }

    #[test]
    fn zero_thickness_is_white() {
        let material = tourmaline_like(Vec3::new(0.3, 0.8, -0.2));
        let frame = PrincipalFrame::for_pleochroic(&material).expect("pleochroic");
        let swatch = face_up_body_color(&material.absorption, Some(&frame), 0.0, Illuminant::D65);
        assert!((swatch.lab[0] - 100.0).abs() < 1e-6, "{:?}", swatch.lab);
    }

    #[test]
    fn along_the_optic_axis_both_modes_see_the_ordinary_ray() {
        // c along the view direction: both modes lie in the o_ray slots, so the transmittance
        // is the ordinary ray's, whatever the e_ray bands say.
        let material = tourmaline_like(Vec3::Y);
        let frame = PrincipalFrame::for_pleochroic(&material).expect("pleochroic");
        for lambda in [450.0, 550.0, 580.0, 650.0] {
            let t = face_up_transmittance_at(&material.absorption, &frame, 2.5, lambda);
            let alpha_o = principal_alphas(&material.absorption, lambda)[0];
            assert!((t - (-alpha_o * 2.5).exp()).abs() < 1e-6, "{lambda}: {t}");
        }
    }

    #[test]
    fn biaxial_mode_weights_use_e_fields() {
        let indicatrix = BiaxialIndicatrix::from_gamma_axis(1.60, 1.63, 1.70, Vec3::Y);
        let frame = PrincipalFrame::biaxial(indicatrix);
        let project = |v: Vec3| (indicatrix.axes.transpose() * v).powf(2.0).to_array();

        let k = Vec3::new(0.4, -0.7, 0.55).normalize();
        let (d_a, d_b) = indicatrix.eigen_polarizations(k);
        let (w_a, w_b) = frame.mode_weights(k, 1.63);
        let expect_a = project(indicatrix.d_to_e_direction(d_a));
        let expect_b = project(indicatrix.d_to_e_direction(d_b));
        assert!(close(w_a, expect_a, 1e-6), "{w_a:?} vs {expect_a:?}");
        assert!(close(w_b, expect_b, 1e-6), "{w_b:?} vs {expect_b:?}");
        // Oblique to every principal axis, E is not D: the weights are not the D-based ones.
        let from_d = project(d_a);
        assert!(
            (0..3).any(|p| (w_a[p] - from_d[p]).abs() > 1e-4),
            "{w_a:?} vs D-based {from_d:?}"
        );

        // Along a principal axis D and E coincide.
        for axis in 0..3 {
            let k = indicatrix.axes.col(axis);
            let (d_a, d_b) = indicatrix.eigen_polarizations(k);
            let (w_a, w_b) = frame.mode_weights(k, 1.63);
            assert!(close(w_a, project(d_a), 1e-5), "axis {axis}: {w_a:?}");
            assert!(close(w_b, project(d_b), 1e-5), "axis {axis}: {w_b:?}");
        }
    }

    #[test]
    fn mode_weights_each_sum_to_one() {
        let uniaxial = PrincipalFrame::uniaxial(Vec3::new(0.3, 0.8, -0.2), 1.77, -0.008);
        let biaxial = PrincipalFrame::biaxial(BiaxialIndicatrix::from_gamma_axis(
            1.57,
            1.575,
            1.58,
            Vec3::new(0.1, 1.0, 0.4),
        ));
        for k in directions() {
            let (a, b) = uniaxial.mode_weights(k, 1.77);
            assert!((sum(a) - 1.0).abs() < 1e-5, "{k:?}: {a:?}");
            assert!((sum(b) - 1.0).abs() < 1e-5, "{k:?}: {b:?}");
            let (a, b) = biaxial.mode_weights(k, 1.575);
            assert!((sum(a) - 1.0).abs() < 1e-4, "{k:?}: {a:?}");
            assert!((sum(b) - 1.0).abs() < 1e-4, "{k:?}: {b:?}");
            // The mean of the two modes is `weights`.
            let mean = biaxial.weights(k, 1.575);
            let expected = [
                f32::midpoint(a[0], b[0]),
                f32::midpoint(a[1], b[1]),
                f32::midpoint(a[2], b[2]),
            ];
            assert!(close(mean, expected, 1e-6), "{mean:?} vs {expected:?}");
        }
    }

    #[test]
    fn a_non_pleochroic_swatch_is_the_old_one_bit_for_bit() {
        let material = GemMaterial::diamond().with_body_color([0.2, 0.5, 0.9]);
        assert!(PrincipalFrame::for_pleochroic(&material).is_none());
        let frame = PrincipalFrame::of_material(&material);
        let new = face_up_body_color(&material.absorption, Some(&frame), 2.5, Illuminant::D65);
        let old = body_colors(&material.absorption, 2.5, Illuminant::D65).unpolarised;
        assert_eq!(new.lab.map(f64::to_bits), old.lab.map(f64::to_bits));
        assert_eq!(new.srgb, old.srgb);
    }
}
