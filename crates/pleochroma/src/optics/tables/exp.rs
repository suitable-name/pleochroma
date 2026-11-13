//! The absorption exponential `exp(-x)` as a table of `2^-f`.
//!
//! `exp(a)` for `a <= 0` is `2^-n * 2^-f` with `x = -a * log2(e)`, `n = floor(x)` and `f = x - n`
//! in `[0, 1)`. The factor `2^-n` is built from the exponent bits; `2^-f` is read from a table of
//! 129 values by linear interpolation. The relative error of the interpolation is at most
//! `ln(2)^2 / (8 * 128^2)`, which is 3.7e-6, well inside the 1e-4 bound the tests enforce.

/// Number of interpolation cells of the `2^-f` table.
pub const EXP_CELLS: usize = 128;

/// Number of values of the table: one more than the cells.
pub const EXP_VALUES: usize = EXP_CELLS + 1;

/// `log2(e)`, the same constant the WGSL twin uses.
const LOG2_E: f32 = std::f32::consts::LOG2_E;

/// The most negative argument [`ExpTable::exp`] serves from the table.
///
/// `-87` keeps `x = -a * log2(e)` below 126, where `2^-n` is still a normal `f32`. Below it the
/// exponential is under the smallest normal number and the call falls back to `f32::exp`.
pub const EXP_TABLE_MIN_ARG: f32 = -87.0;

/// `2^-f` sampled at `f = i / 128`, with the lookup of `exp(a)` for `a <= 0`.
#[derive(Clone, Debug, PartialEq)]
pub struct ExpTable {
    values: [f32; EXP_VALUES],
}

impl Default for ExpTable {
    fn default() -> Self {
        Self::new()
    }
}

/// `2^-n` for the integral `n` in `[0, 126]`, built from the exponent bits.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "n is an integral float in [0, 126], so it converts exactly"
)]
const fn pow2_neg(n: f32) -> f32 {
    f32::from_bits((127 - n as u32) << 23)
}

impl ExpTable {
    /// Builds the table; the values are computed in `f64` and rounded once to `f32`.
    #[must_use]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the f64 table values are rounded to f32 on purpose"
    )]
    pub fn new() -> Self {
        let mut values = [0.0f32; EXP_VALUES];
        for (i, slot) in values.iter_mut().enumerate() {
            *slot = (-(i as f64) / EXP_CELLS as f64).exp2() as f32;
        }
        Self { values }
    }

    /// The table values, in the order the GPU buffer stores them.
    #[must_use]
    pub const fn values(&self) -> &[f32; EXP_VALUES] {
        &self.values
    }

    /// `exp(a)` for `a` in `[EXP_TABLE_MIN_ARG, 0]`; any other argument (positive, NaN, or
    /// below the range) returns `a.exp()`.
    ///
    /// The relative error against the exact exponential is below 4e-6 from the interpolation,
    /// plus the `f32` rounding of the argument reduction.
    #[must_use]
    pub fn exp(&self, a: f32) -> f32 {
        if !(EXP_TABLE_MIN_ARG..=0.0).contains(&a) {
            return a.exp();
        }
        let x = (-a) * LOG2_E;
        let n = x.floor();
        let (cell, t) = super::split_cell((x - n) * EXP_CELLS as f32, EXP_CELLS);
        let lo = self.values[cell];
        let hi = self.values[cell + 1];
        t.mul_add(hi - lo, lo) * pow2_neg(n)
    }

    /// [`Self::exp`] on eight lanes, the shape of the tracer's Beer-Lambert call.
    ///
    /// When every lane is inside the table range the eight lookups are used; otherwise the whole
    /// array goes to the polynomial [`crate::simd::exp_f32x8`], so an out-of-range lane (or a
    /// NaN) behaves exactly as it does with the tables off.
    #[must_use]
    pub fn exp_array(&self, args: [f32; 8]) -> [f32; 8] {
        if !args.iter().all(|a| (EXP_TABLE_MIN_ARG..=0.0).contains(a)) {
            return crate::simd::exp_f32x8(args);
        }
        args.map(|a| self.exp(a))
    }
}
