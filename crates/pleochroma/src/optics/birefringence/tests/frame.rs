//! `alpha_axis_from_turn` and the framed constructors: turn zero is the gamma-only
//! completion bit for bit, a turn of 90 degrees is perpendicular to both the completion
//! and gamma, a turn of 180 degrees is the same line.

use crate::optics::birefringence::{AbsorptionTensor3, BiaxialIndicatrix, alpha_axis_from_turn};
use glam::Vec3;

fn gamma_axes() -> [Vec3; 5] {
    [
        Vec3::Y,
        Vec3::X,
        Vec3::Z,
        Vec3::new(0.35, 0.82, -0.45).normalize(),
        Vec3::new(-0.6, 0.1, 0.79).normalize(),
    ]
}

fn bits(v: Vec3) -> [u32; 3] {
    v.to_array().map(f32::to_bits)
}

/// Turn zero is exactly the alpha column the gamma-only completion produces, in both the
/// index frame and the tensor frame, and the framed constructors with that alpha rebuild
/// the unframed frames bit for bit.
#[test]
fn turn_zero_equals_the_completion_bit_for_bit() {
    for gamma in gamma_axes() {
        let alpha = alpha_axis_from_turn(gamma, 0.0);
        let indicatrix = BiaxialIndicatrix::from_gamma_axis(1.6, 1.65, 1.75, gamma);
        let tensor = AbsorptionTensor3::biaxial(1.0, 2.0, 3.0, gamma);
        assert_eq!(bits(alpha), bits(indicatrix.axes.x_axis), "gamma {gamma:?}");
        assert_eq!(bits(alpha), bits(tensor.axes.x_axis), "gamma {gamma:?}");

        let framed_index = BiaxialIndicatrix::from_frame(1.6, 1.65, 1.75, gamma, alpha);
        let framed_tensor = AbsorptionTensor3::biaxial_framed(1.0, 2.0, 3.0, gamma, alpha);
        for (framed, plain) in [
            (framed_index.axes.x_axis, indicatrix.axes.x_axis),
            (framed_index.axes.y_axis, indicatrix.axes.y_axis),
            (framed_index.axes.z_axis, indicatrix.axes.z_axis),
            (framed_tensor.axes.x_axis, tensor.axes.x_axis),
            (framed_tensor.axes.y_axis, tensor.axes.y_axis),
            (framed_tensor.axes.z_axis, tensor.axes.z_axis),
        ] {
            assert_eq!(bits(framed), bits(plain), "gamma {gamma:?}");
        }
    }
}

/// A quarter turn is a unit vector perpendicular to gamma and to the zero-turn alpha (so
/// it is the beta direction of the completion, right-handed about gamma).
#[test]
fn turn_ninety_is_perpendicular_to_both() {
    for gamma in gamma_axes() {
        let zero = alpha_axis_from_turn(gamma, 0.0);
        let quarter = alpha_axis_from_turn(gamma, 90.0);
        assert!(
            (quarter.length() - 1.0).abs() < 1e-5,
            "unit, gamma {gamma:?}"
        );
        assert!(
            quarter.dot(gamma).abs() < 1e-5,
            "perpendicular to gamma {gamma:?}"
        );
        assert!(
            quarter.dot(zero).abs() < 1e-5,
            "perpendicular to turn 0 {gamma:?}"
        );
        // Right-handed about gamma: gamma x alpha(0) is the +90 degree direction.
        assert!(
            (quarter - gamma.cross(zero)).length() < 1e-5,
            "right-handed turn, gamma {gamma:?}"
        );
    }
}

/// Half a turn is the negated vector: the same line.
#[test]
fn turn_one_eighty_is_the_same_line() {
    for gamma in gamma_axes() {
        let zero = alpha_axis_from_turn(gamma, 0.0);
        let half = alpha_axis_from_turn(gamma, 180.0);
        assert!((half + zero).length() < 1e-5, "negated, gamma {gamma:?}");
        assert!(half.dot(zero) < -0.999, "antiparallel, gamma {gamma:?}");
    }
}

/// A stale second direction (not perpendicular to gamma) is projected onto the plane
/// perpendicular to gamma instead of producing a skewed frame, and a request parallel to
/// gamma falls back to the completion.
#[test]
fn a_skewed_or_parallel_alpha_still_gives_an_orthonormal_frame() {
    let gamma = Vec3::new(0.35, 0.82, -0.45).normalize();
    let skewed = (alpha_axis_from_turn(gamma, 20.0) + gamma * 0.5).normalize();
    for alpha in [skewed, gamma, Vec3::ZERO] {
        let axes = BiaxialIndicatrix::from_frame(1.6, 1.65, 1.75, gamma, alpha).axes;
        assert!((axes.x_axis.length() - 1.0).abs() < 1e-5);
        assert!((axes.y_axis.length() - 1.0).abs() < 1e-5);
        assert!(axes.x_axis.dot(axes.y_axis).abs() < 1e-5);
        assert!(axes.x_axis.dot(axes.z_axis).abs() < 1e-5);
        assert!((axes.x_axis.cross(axes.y_axis) - axes.z_axis).length() < 1e-5);
    }
    let fallback = BiaxialIndicatrix::from_frame(1.6, 1.65, 1.75, gamma, gamma).axes;
    let completion = BiaxialIndicatrix::from_gamma_axis(1.6, 1.65, 1.75, gamma).axes;
    assert_eq!(bits(fallback.x_axis), bits(completion.x_axis));
}

/// The `Option` dispatchers are exactly the unframed constructors for `None`.
#[test]
fn the_oriented_constructors_with_none_are_the_unframed_ones() {
    for gamma in gamma_axes() {
        let a = BiaxialIndicatrix::from_oriented_frame(1.6, 1.65, 1.75, gamma, None);
        let b = BiaxialIndicatrix::from_gamma_axis(1.6, 1.65, 1.75, gamma);
        assert_eq!(bits(a.axes.x_axis), bits(b.axes.x_axis));
        assert_eq!(bits(a.axes.y_axis), bits(b.axes.y_axis));
        let t = AbsorptionTensor3::biaxial_oriented(1.0, 2.0, 3.0, gamma, None);
        let u = AbsorptionTensor3::biaxial(1.0, 2.0, 3.0, gamma);
        assert_eq!(bits(t.axes.x_axis), bits(u.axes.x_axis));
        assert_eq!(bits(t.axes.y_axis), bits(u.axes.y_axis));
    }
}
