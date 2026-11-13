//! Shared low-level vector helpers for the uniaxial and biaxial birefringence code.
//!
//! They are the stable orthonormal-basis completion, the eigenvector sign convention, and the
//! fma-based cross and dot products that keep bit-parity with the WGSL port.

use glam::Vec3;

/// A stable (branch-minimal) orthonormal basis `(t, b)` perpendicular to unit vector
/// `n`. Used both to fill in a uniaxial [`AbsorptionTensor3`]'s two degenerate
/// principal axes (their specific directions don't matter there -- only that they're
/// orthonormal to each other and to `n`) and as a last-resort fallback when a
/// direction-dependent construction elsewhere degenerates.
pub(super) fn stable_orthonormal_basis(n: Vec3) -> (Vec3, Vec3) {
    let a = if n.x.abs() > 0.9 { Vec3::Y } else { Vec3::X };
    let t = (a - n * n.dot(a)).normalize_or_zero();
    let b = n.cross(t);
    (t, b)
}

/// Deterministic sign convention for an eigenvector (an eigenvector is only defined up
/// to an overall sign): flips `v` so that its largest-magnitude component is positive.
/// Ties (component magnitudes exactly equal) resolve in x, then y, then z priority --
/// the same `>=`/`>=` comparison structure on both the CPU (`f32`) and GPU (WGSL
/// `f32`) sides, so both agree bit-for-bit on which component decides the sign. Used by
/// `BiaxialIndicatrix::eigenvector_world`.
pub(super) fn canonicalize_eigenvector_sign(v: Vec3) -> Vec3 {
    let ax = v.x.abs();
    let ay = v.y.abs();
    let az = v.z.abs();
    let largest = if ax >= ay && ax >= az {
        v.x
    } else if ay >= az {
        v.y
    } else {
        v.z
    };
    if largest < 0.0 { -v } else { v }
}

/// `a.cross(b)` with explicit `mul_add`, so it rounds identically to the WGSL `cross_fma`.
/// Used by `BiaxialIndicatrix::eigenvector_world`, where CPU/GPU rounding must match.
pub(super) fn cross_fma(a: Vec3, b: Vec3) -> Vec3 {
    Vec3::new(
        a.y.mul_add(b.z, -(a.z * b.y)),
        a.z.mul_add(b.x, -(a.x * b.z)),
        a.x.mul_add(b.y, -(a.y * b.x)),
    )
}

/// `a.dot(b)` with explicit `mul_add`, for the same CPU/GPU rounding match as `cross_fma`.
/// Gives the sign relating two (anti)parallel cross products in `eigenvector_world`.
pub(super) fn dot_fma(a: Vec3, b: Vec3) -> f32 {
    a.x.mul_add(b.x, a.y.mul_add(b.y, a.z * b.z))
}
