//! Boundary-matching linear algebra.
//!
//! Gauss-Jordan elimination for the 4x4 complex systems every entry/internal solve
//! reduces to, and the tangential-field-continuity projection that builds each
//! system's rows.

use super::{complex::Cplx, frame::ModeFields};
use glam::Vec3;

/// Solves a 4x4 complex linear system `A x = b` by Gauss-Jordan elimination with partial
/// pivoting. Every boundary solve in this module has this shape: four tangential-continuity
/// equations for four amplitudes.
pub(super) fn solve4(mut a: [[Cplx; 4]; 4], mut b: [Cplx; 4]) -> [Cplx; 4] {
    for col in 0..4 {
        let mut piv = col;
        let mut piv_mag = a[col][col].norm_sqr();
        for (row, row_data) in a.iter().enumerate().skip(col + 1) {
            let mag = row_data[col].norm_sqr();
            if mag > piv_mag {
                piv = row;
                piv_mag = mag;
            }
        }
        a.swap(col, piv);
        b.swap(col, piv);
        let pivot = a[col][col];
        for x in &mut a[col][col..] {
            *x = x.div(pivot);
        }
        b[col] = b[col].div(pivot);
        for row in 0..4 {
            if row == col {
                continue;
            }
            let factor = a[row][col];
            if factor.norm_sqr() == 0.0 {
                continue;
            }
            #[allow(
                clippy::needless_range_loop,
                reason = "row `row` is updated from row `col` of the same matrix; an iterator would need a split borrow"
            )]
            for j in col..4 {
                a[row][j] = a[row][j].sub(factor.mul(a[col][j]));
            }
            b[row] = b[row].sub(factor.mul(b[col]));
        }
    }
    b
}

/// [`solve4`] for two right-hand sides against the same matrix `a`, with one elimination.
/// Bit-identical to two separate [`solve4`] calls: the elimination on `a` does not depend
/// on `b`, so both right-hand sides follow the same operations in the same order.
pub(super) fn solve4_two_rhs(
    mut a: [[Cplx; 4]; 4],
    mut b1: [Cplx; 4],
    mut b2: [Cplx; 4],
) -> ([Cplx; 4], [Cplx; 4]) {
    for col in 0..4 {
        let mut piv = col;
        let mut piv_mag = a[col][col].norm_sqr();
        for (row, row_data) in a.iter().enumerate().skip(col + 1) {
            let mag = row_data[col].norm_sqr();
            if mag > piv_mag {
                piv = row;
                piv_mag = mag;
            }
        }
        a.swap(col, piv);
        b1.swap(col, piv);
        b2.swap(col, piv);
        let pivot = a[col][col];
        for x in &mut a[col][col..] {
            *x = x.div(pivot);
        }
        b1[col] = b1[col].div(pivot);
        b2[col] = b2[col].div(pivot);
        for row in 0..4 {
            if row == col {
                continue;
            }
            let factor = a[row][col];
            if factor.norm_sqr() == 0.0 {
                continue;
            }
            #[allow(
                clippy::needless_range_loop,
                reason = "row `row` is updated from row `col` of the same matrix; an iterator would need a split borrow"
            )]
            for j in col..4 {
                a[row][j] = a[row][j].sub(factor.mul(a[col][j]));
            }
            b1[row] = b1[row].sub(factor.mul(b1[col]));
            b2[row] = b2[row].sub(factor.mul(b2[col]));
        }
    }
    (b1, b2)
}

#[inline]
pub(super) fn tangential_components(fields: &ModeFields, that: Vec3, s_axis: Vec3) -> [Cplx; 4] {
    [
        fields.e.dot_real(that),
        fields.e.dot_real(s_axis),
        fields.h.dot_real(that),
        fields.h.dot_real(s_axis),
    ]
}
