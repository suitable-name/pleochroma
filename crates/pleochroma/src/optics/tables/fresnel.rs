//! Dielectric Fresnel amplitudes through two smooth lookup tables.
//!
//! Every scalar Fresnel site of the tracer has one side of the interface in air (`n = 1`). With
//! the glass index `n` and the two cosines, all four amplitudes are products of exact factors and
//! one of two denominators:
//!
//! ```text
//! A = 1 / (cos_i + n cos_t)        B = 1 / (n cos_i + cos_t)
//! entry (air -> glass):  r_s = (cos_i - n cos_t) A   r_p = (n cos_i - cos_t) B
//!                        t_s = 2 cos_i A             t_p = 2 cos_i B
//! exit (glass -> air):   r_s = (n cos_i - cos_t) B   r_p = (cos_i - n cos_t) A
//!                        t_s = 2 n cos_i B           t_p = 2 n cos_i A
//! ```
//!
//! `A` and `B` are smooth and positive, so a bilinear table over `(n, u)` interpolates them with a
//! small relative error. The key `u` is the cosine that keeps the functions smooth: `cos_i` for an
//! entry, `cos_t` for an exit (as a function of the other cosine the exit side has a square root
//! singularity at the critical angle). Because the exact factors stay outside the table, the
//! relative error of a transmission amplitude equals the relative error of `A` or `B`, and the
//! absolute error of a reflection amplitude (whose magnitude is at most one) is no larger.
//!
//! The tables are built per render for the material's own index range. The build measures the
//! worst relative error on a grid of interior points and refines the grid until it is below
//! [`FRESNEL_REL_TOLERANCE`]; a branch that cannot reach it is left out, and every lookup that no
//! branch serves returns `None` so the caller keeps its closed form.

use crate::optics::materials::GemMaterial;

/// Relative error the build requires of the interpolated `A` and `B`.
///
/// It bounds the absolute error of `r_s` and `r_p` (each at most one in magnitude) and of the
/// unpolarised reflectance `0.5 (r_s^2 + r_p^2)` (twice this), and the relative error of `t_s`
/// and `t_p`. The tests require 1e-4; this leaves a factor of five.
pub const FRESNEL_REL_TOLERANCE: f32 = 2.0e-5;

/// Cells over the key `u` the build starts from.
const START_KEY_CELLS: usize = 128;
/// Cells over the glass index the build starts from.
const START_INDEX_CELLS: usize = 4;
/// Most cells over the key the refinement reaches.
const MAX_KEY_CELLS: usize = 1024;
/// Most cells over the glass index the refinement reaches.
const MAX_INDEX_CELLS: usize = 128;
/// Added to each end of the scanned index range, so a dispersive companion channel still hits.
const INDEX_MARGIN: f32 = 0.01;
/// The lowest glass index a branch covers; the formulas assume `n > 1`.
const INDEX_FLOOR: f32 = 1.001;
/// First wavelength (nm) of the index scan.
const SCAN_FIRST_NM: f32 = 380.0;
/// Step (nm) of the index scan.
const SCAN_STEP_NM: f32 = 10.0;
/// Number of scan wavelengths: 380 to 780 nm.
const SCAN_COUNT: usize = 41;
/// Fractions of a cell the measurement visits in each direction.
const SAMPLE_FRACTIONS: [f64; 3] = [0.25, 0.5, 0.75];

/// Which side of the interface the glass is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FresnelSide {
    /// Air to glass: `n1 = 1`, `n2 = n`; the table key is `cos_i`.
    Entry,
    /// Glass to air: `n1 = n`, `n2 = 1`; the table key is `cos_t`.
    Exit,
}

/// The four scalar Fresnel amplitudes of one interface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FresnelAmplitudes {
    /// Reflection amplitude, s polarisation.
    pub r_s: f32,
    /// Reflection amplitude, p polarisation.
    pub r_p: f32,
    /// Transmission amplitude, s polarisation.
    pub t_s: f32,
    /// Transmission amplitude, p polarisation.
    pub t_p: f32,
}

impl FresnelAmplitudes {
    /// The amplitudes from the closed forms the tracer uses, bit for bit.
    #[must_use]
    pub fn closed_form(n1: f32, n2: f32, cos_i: f32, cos_t: f32) -> Self {
        let (r_s, r_p) = closed_form_reflection(n1, n2, cos_i, cos_t);
        Self {
            r_s,
            r_p,
            t_s: (2.0 * n1 * cos_i) / f32::mul_add(n2, cos_t, n1 * cos_i),
            t_p: (2.0 * n1 * cos_i) / f32::mul_add(n1, cos_t, n2 * cos_i),
        }
    }
}

/// The reflection amplitudes `(r_s, r_p)` exactly as the tracer's inline expressions compute them.
#[must_use]
pub fn closed_form_reflection(n1: f32, n2: f32, cos_i: f32, cos_t: f32) -> (f32, f32) {
    let r_s = f32::mul_add(n2, -cos_t, n1 * cos_i) / f32::mul_add(n2, cos_t, n1 * cos_i);
    let r_p = f32::mul_add(n1, -cos_t, n2 * cos_i) / f32::mul_add(n1, cos_t, n2 * cos_i);
    (r_s, r_p)
}

/// Linear interpolation `a + t (b - a)` with one fused multiply-add, the form the WGSL twin uses.
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    t.mul_add(b - a, a)
}

/// The exact `(A, B)` pair in `f64` for glass index `n` and key `u`.
fn exact_ab(side: FresnelSide, n: f64, u: f64) -> (f64, f64) {
    let other = (1.0 - u.mul_add(-u, 1.0) / (n * n)).max(0.0).sqrt();
    let (cos_i, cos_t) = match side {
        FresnelSide::Entry => (u, other),
        FresnelSide::Exit => (other, u),
    };
    (1.0 / n.mul_add(cos_t, cos_i), 1.0 / n.mul_add(cos_i, cos_t))
}

/// The index range `[lo, hi]` a material shows between 380 and 780 nm, widened by a margin.
///
/// It covers the ordinary index, the extraordinary index and, for a biaxial material, the three
/// principal indices; every effective index of a mode lies between them. `None` if the material
/// yields a non-finite index.
#[must_use]
pub fn material_index_range(material: &GemMaterial) -> Option<(f32, f32)> {
    let mut lo = f32::INFINITY;
    let mut hi = f32::NEG_INFINITY;
    let mut take = |n: f32| {
        lo = lo.min(n);
        hi = hi.max(n);
    };
    for k in 0..SCAN_COUNT {
        let lambda = SCAN_STEP_NM.mul_add(k as f32, SCAN_FIRST_NM);
        let n_o = material.dispersion.evaluate(lambda);
        take(n_o);
        take(material.extraordinary_index_at(lambda, n_o));
        if let Some(indicatrix) = material.biaxial_indicatrix(lambda) {
            take(indicatrix.n_alpha);
            take(indicatrix.n_beta);
            take(indicatrix.n_gamma);
        }
    }
    if !(lo.is_finite() && hi.is_finite()) {
        return None;
    }
    Some((
        (lo - INDEX_MARGIN).max(INDEX_FLOOR),
        (hi + INDEX_MARGIN).max(INDEX_MARGIN.mul_add(2.0, INDEX_FLOOR)),
    ))
}

/// One bilinear table of `(A, B)` over the glass index and the key cosine.
#[derive(Clone, Debug, PartialEq)]
pub struct FresnelBranch {
    side: FresnelSide,
    n_lo: f32,
    n_hi: f32,
    inv_dn: f32,
    index_cells: usize,
    key_cells: usize,
    values: Vec<f32>,
    max_rel_error: f32,
}

impl FresnelBranch {
    /// Builds the branch for glass indices in `[n_lo, n_hi]`, refining until the measured
    /// relative error is at most [`FRESNEL_REL_TOLERANCE`]. `None` if the caps are reached first
    /// or the range is empty.
    ///
    /// Each round measures the error along each axis (on the nodes of the other one) and
    /// doubles the cells of the axis that contributes more; the full interior measurement then
    /// confirms the result.
    #[must_use]
    pub fn build(side: FresnelSide, n_lo: f32, n_hi: f32) -> Option<Self> {
        if n_lo.is_nan() || n_hi.is_nan() || n_lo <= 1.0 || n_hi <= n_lo {
            return None;
        }
        let mut index_cells = START_INDEX_CELLS;
        let mut key_cells = START_KEY_CELLS;
        loop {
            let mut branch = Self::fill(side, (n_lo, n_hi), (index_cells, key_cells));
            let (key_error, index_error) = branch.axis_errors();
            if key_error + index_error <= FRESNEL_REL_TOLERANCE {
                branch.max_rel_error = branch.measure();
                if branch.max_rel_error <= FRESNEL_REL_TOLERANCE {
                    return Some(branch);
                }
            }
            let key_can_grow = key_cells < MAX_KEY_CELLS;
            let index_can_grow = index_cells < MAX_INDEX_CELLS;
            if key_can_grow && (key_error >= index_error || !index_can_grow) {
                key_cells *= 2;
            } else if index_can_grow {
                index_cells *= 2;
            } else {
                return None;
            }
        }
    }

    /// Samples the exact `(A, B)` on the node grid.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the f64 node values are rounded to f32 on purpose"
    )]
    fn fill(side: FresnelSide, range: (f32, f32), cells: (usize, usize)) -> Self {
        let (n_lo, n_hi) = range;
        let (index_cells, key_cells) = cells;
        let span = f64::from(n_hi) - f64::from(n_lo);
        let mut values = Vec::with_capacity(2 * (index_cells + 1) * (key_cells + 1));
        for j in 0..=index_cells {
            let n = f64::from(n_lo) + span * j as f64 / index_cells as f64;
            for i in 0..=key_cells {
                let (a, b) = exact_ab(side, n, i as f64 / key_cells as f64);
                values.push(a as f32);
                values.push(b as f32);
            }
        }
        Self {
            side,
            n_lo,
            n_hi,
            inv_dn: index_cells as f32 / (n_hi - n_lo),
            index_cells,
            key_cells,
            values,
            max_rel_error: 0.0,
        }
    }

    /// The glass index at `position` cells from `n_lo`, rounded to `f32`.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the probe point is rounded to f32, the precision the lookup takes"
    )]
    fn index_at(&self, position: f64) -> f32 {
        let span = f64::from(self.n_hi) - f64::from(self.n_lo);
        (f64::from(self.n_lo) + span * position / self.index_cells as f64) as f32
    }

    /// The key at `position` cells from zero, rounded to `f32`.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the probe point is rounded to f32, the precision the lookup takes"
    )]
    fn key_at(&self, position: f64) -> f32 {
        (position / self.key_cells as f64) as f32
    }

    /// The larger relative error of the interpolated `A` and `B` against the exact values at one
    /// point, or infinity when the lookup declines the point.
    fn rel_error_at(&self, n: f32, u: f32) -> f64 {
        let Some(got) = self.lookup(n, u) else {
            return f64::INFINITY;
        };
        let want = exact_ab(self.side, f64::from(n), f64::from(u));
        ((f64::from(got.0) - want.0) / want.0)
            .abs()
            .max(((f64::from(got.1) - want.1) / want.1).abs())
    }

    /// The worst relative error along each axis, `(key, index)`: interior points of the key cells
    /// on every index node, and interior points of the index cells on every key node. On a node
    /// line the bilinear interpolation reduces to one dimension.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the error is reported as f32"
    )]
    fn axis_errors(&self) -> (f32, f32) {
        let mut worst_key = 0.0f64;
        let mut worst_index = 0.0f64;
        for j in 0..=self.index_cells {
            let n = self.index_at(j as f64);
            for i in 0..self.key_cells {
                for fi in SAMPLE_FRACTIONS {
                    worst_key = worst_key.max(self.rel_error_at(n, self.key_at(i as f64 + fi)));
                }
            }
        }
        for i in 0..=self.key_cells {
            let u = self.key_at(i as f64);
            for j in 0..self.index_cells {
                for fj in SAMPLE_FRACTIONS {
                    worst_index =
                        worst_index.max(self.rel_error_at(self.index_at(j as f64 + fj), u));
                }
            }
        }
        (worst_key as f32, worst_index as f32)
    }

    /// The worst relative error of the lookup against the exact `(A, B)` over interior points of
    /// every cell (the quarter, half and three-quarter points in each direction).
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the error is reported as f32"
    )]
    fn measure(&self) -> f32 {
        let mut worst = 0.0f64;
        for j in 0..self.index_cells {
            for i in 0..self.key_cells {
                for fj in SAMPLE_FRACTIONS {
                    for fi in SAMPLE_FRACTIONS {
                        let n = self.index_at(j as f64 + fj);
                        let u = self.key_at(i as f64 + fi);
                        worst = worst.max(self.rel_error_at(n, u));
                    }
                }
            }
        }
        worst as f32
    }

    /// The bilinear interpolation of `(A, B)` at glass index `n` and key `u`, or `None` outside
    /// the branch's index range or `u` outside `[0, 1]`.
    #[must_use]
    pub fn lookup(&self, n: f32, u: f32) -> Option<(f32, f32)> {
        if n.is_nan() || u.is_nan() || n < self.n_lo || n > self.n_hi || !(0.0..=1.0).contains(&u) {
            return None;
        }
        let (row, tn) = super::split_cell((n - self.n_lo) * self.inv_dn, self.index_cells);
        let (column, tu) = super::split_cell(u * self.key_cells as f32, self.key_cells);
        let stride = self.key_cells + 1;
        let lo = 2 * (row * stride + column);
        let hi = lo + 2 * stride;
        let values = &self.values;
        let first = lerp(
            lerp(values[lo], values[lo + 2], tu),
            lerp(values[hi], values[hi + 2], tu),
            tn,
        );
        let second = lerp(
            lerp(values[lo + 1], values[lo + 3], tu),
            lerp(values[hi + 1], values[hi + 3], tu),
            tn,
        );
        Some((first, second))
    }

    /// The side of the interface this branch serves.
    #[must_use]
    pub const fn side(&self) -> FresnelSide {
        self.side
    }

    /// Lowest glass index the branch serves.
    #[must_use]
    pub const fn n_lo(&self) -> f32 {
        self.n_lo
    }

    /// Highest glass index the branch serves.
    #[must_use]
    pub const fn n_hi(&self) -> f32 {
        self.n_hi
    }

    /// Cells over the glass index.
    #[must_use]
    pub const fn index_cells(&self) -> usize {
        self.index_cells
    }

    /// Cells over the key cosine.
    #[must_use]
    pub const fn key_cells(&self) -> usize {
        self.key_cells
    }

    /// Node values, `(A, B)` pairs ordered by index node then key node.
    #[must_use]
    pub fn values(&self) -> &[f32] {
        &self.values
    }

    /// The reciprocal cell width over the glass index, as the GPU buffer stores it.
    #[must_use]
    pub const fn inv_dn(&self) -> f32 {
        self.inv_dn
    }

    /// The worst relative error measured at build time.
    #[must_use]
    pub const fn max_rel_error(&self) -> f32 {
        self.max_rel_error
    }

    /// The amplitudes at one interface, or `None` when this branch does not serve the inputs.
    ///
    /// `n` is the glass index and the key is `cos_i` for an entry or `cos_t` for an exit.
    #[must_use]
    pub fn amplitudes(&self, n: f32, cos_i: f32, cos_t: f32) -> Option<FresnelAmplitudes> {
        match self.side {
            FresnelSide::Entry => {
                let (a, b) = self.lookup(n, cos_i)?;
                Some(FresnelAmplitudes {
                    r_s: (-n).mul_add(cos_t, cos_i) * a,
                    r_p: n.mul_add(cos_i, -cos_t) * b,
                    t_s: 2.0 * cos_i * a,
                    t_p: 2.0 * cos_i * b,
                })
            }
            FresnelSide::Exit => {
                let (a, b) = self.lookup(n, cos_t)?;
                Some(FresnelAmplitudes {
                    r_s: n.mul_add(cos_i, -cos_t) * b,
                    r_p: (-n).mul_add(cos_t, cos_i) * a,
                    t_s: 2.0 * n * cos_i * b,
                    t_p: 2.0 * n * cos_i * a,
                })
            }
        }
    }
}
