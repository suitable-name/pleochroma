//! Regression test for `MuellerMatrix::fresnel_transmission` at normal incidence.
//!
//! With no polarization effect there, the (1,1) and (3,3) elements must be equal.

use pleochroma::optics::polarization::MuellerMatrix;

// ---------------------------------------------------------------------------
// fresnel_transmission must not apply a sqrt() to the (3,3) element. At normal incidence
// m11 and m33 of the Mueller matrix must be equal.
// ---------------------------------------------------------------------------
#[test]
fn fresnel_transmission_normal_incidence_m11_equals_m33() {
    let n1 = 1.0f32;
    let n2 = 2.4178f32;
    let cos_i = 1.0f32;
    let cos_t = 1.0f32;

    // Standard Fresnel amplitude transmission coefficients.
    let t_s = (2.0 * n1 * cos_i) / n2.mul_add(cos_t, n1 * cos_i);
    let t_p = (2.0 * n1 * cos_i) / n1.mul_add(cos_t, n2 * cos_i);

    let m = MuellerMatrix::fresnel_transmission(n1, n2, cos_i, cos_t, t_s, t_p);

    // glam::Mat4 is column-major and MuellerMatrix::fresnel_transmission is built via
    // Mat4::from_cols_array(&[a, b, 0, 0,  b, a, 0, 0,  0, 0, c, 0,  0, 0, 0, c]).
    // Column 0 is [a, b, 0, 0] and column 2 is [0, 0, c, 0], so:
    //   m11 (row0, col0) = m.x_axis.x
    //   m33 (row2, col2) = m.z_axis.z
    let m11 = m.x_axis.x;
    let m33 = m.z_axis.z;

    // Reference value for this index pair.
    assert!(
        (m11 - 0.8279).abs() < 1e-3,
        "expected m11 ~= 0.8279, got {m11}"
    );

    assert!(
        (m11 - m33).abs() < 1e-5,
        "at normal incidence m11 and m33 must agree (no polarization effect): m11={m11}, m33={m33}"
    );

    // Guard against an extra sqrt() on m33, which would give about 0.9099.
    assert!(
        (m33 - 0.9099).abs() > 0.01,
        "m33 looks like the old, buggy sqrt() value: {m33}"
    );
}
