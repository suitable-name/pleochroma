//! Chart paths of the Material Details page: index curves and absorption spectra.
//!
//! They are `M x y L x y` path data in a 0..100 square (the convention of
//! `view_model::curve_path`), so a front end only draws them.
//!
//! The index curve follows the Material Editor's sparkline: 81 samples from 380 to 780 nm in 5 nm
//! steps, a vertical range of the curve's own spread plus a 10 % margin that is never narrower
//! than [`MIN_CURVE_SPAN`].

use std::fmt::Write as _;

use pleochroma::optics::dispersion::{DispersionModel, LINE_B_NM, LINE_G_NM};

/// Number of 5 nm steps of an index curve from 380 to 780 nm.
pub const CURVE_STEPS: u16 = 80;
/// The narrowest vertical span of an index plot, so a nearly flat curve is not magnified into noise.
pub const MIN_CURVE_SPAN: f32 = 0.002;
/// The wavelength at the left edge of every chart, nanometers.
pub const PLOT_MIN_NM: f32 = DispersionModel::BAND_MIN_NM;
/// The wavelength at the right edge of every chart, nanometers.
pub const PLOT_MAX_NM: f32 = DispersionModel::BAND_MAX_NM;
/// Number of 2 nm steps of a spectrum path from 380 to 780 nm.
pub const SPECTRUM_STEPS: u16 = 200;
/// The sodium D line, nanometers (the line a gem's index is quoted at).
pub const LINE_D_NM: f32 = 589.3;
/// The Fraunhofer F line, nanometers.
pub const LINE_F_NM: f32 = 486.1;
/// The Fraunhofer C line, nanometers.
pub const LINE_C_NM: f32 = 656.3;

/// A Fraunhofer line drawn as a vertical guide.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FraunhoferGuide {
    /// The line's letter: "G", "F", "D", "C" or "B".
    pub label: &'static str,
    /// Its wavelength, nanometers.
    pub wavelength_nm: f32,
    /// Its horizontal position in the 0..100 viewbox.
    pub x: f32,
}

/// A labelled gridline of a chart.
#[derive(Debug, Clone, PartialEq)]
pub struct AxisTick {
    /// The label ("1.750", "500 nm").
    pub text: String,
    /// The position in the 0..100 viewbox: `y` for a vertical axis, `x` for a horizontal one.
    pub position: f32,
}

/// The horizontal position of `lambda_nm` in the 0..100 viewbox.
#[must_use]
pub fn x_of_wavelength(lambda_nm: f32) -> f32 {
    (lambda_nm - PLOT_MIN_NM) / (PLOT_MAX_NM - PLOT_MIN_NM) * 100.0
}

/// The five Fraunhofer guides, G, F, D, C, B (short to long wavelength).
#[must_use]
pub fn fraunhofer_guides() -> [FraunhoferGuide; 5] {
    [
        ("G", LINE_G_NM),
        ("F", LINE_F_NM),
        ("D", LINE_D_NM),
        ("C", LINE_C_NM),
        ("B", LINE_B_NM),
    ]
    .map(|(label, wavelength_nm)| FraunhoferGuide {
        label,
        wavelength_nm,
        x: x_of_wavelength(wavelength_nm),
    })
}

/// The labelled wavelengths of the horizontal axis: 400, 500, 600 and 700 nm.
#[must_use]
pub fn wavelength_ticks() -> Vec<AxisTick> {
    [400.0_f32, 500.0, 600.0, 700.0]
        .into_iter()
        .map(|nm| AxisTick {
            text: format!("{nm:.0} nm"),
            position: x_of_wavelength(nm),
        })
        .collect()
}

/// The wavelength of index-curve step `step` (0..=[`CURVE_STEPS`]).
#[must_use]
pub fn index_wavelength(step: u16) -> f32 {
    f32::from(step).mul_add(5.0, PLOT_MIN_NM)
}

/// The wavelength of spectrum step `step` (0..=[`SPECTRUM_STEPS`]).
#[must_use]
pub fn spectrum_wavelength(step: u16) -> f32 {
    f32::from(step).mul_add(2.0, PLOT_MIN_NM)
}

/// The index of `f` at every step of an index curve.
#[must_use]
pub fn sample_index(f: impl Fn(f32) -> f32) -> Vec<f32> {
    (0..=CURVE_STEPS).map(|s| f(index_wavelength(s))).collect()
}

/// The vertical range `(bottom, top)` that holds every curve of `curves` with a 10 % margin, at
/// least [`MIN_CURVE_SPAN`] tall: the Material Editor's rule applied to several curves at once.
#[must_use]
pub fn shared_index_range(curves: &[Vec<f32>]) -> (f32, f32) {
    let all = curves.iter().flatten().copied();
    let low = all.clone().fold(f32::INFINITY, f32::min);
    let high = all.fold(f32::NEG_INFINITY, f32::max);
    if !low.is_finite() || !high.is_finite() {
        return (0.0, MIN_CURVE_SPAN);
    }
    let centre = f32::midpoint(high, low);
    let half = (high - low).max(MIN_CURVE_SPAN) / 2.0 * 1.1;
    (centre - half, centre + half)
}

/// The path of index `samples` (one per 5 nm step) between the plot's `bottom` and `top` indices.
#[must_use]
pub fn index_samples_path(samples: &[f32], bottom: f32, top: f32) -> String {
    let range = (top - bottom).max(f32::EPSILON);
    let mut path = String::new();
    for (step, n) in (0..=CURVE_STEPS).zip(samples) {
        let x = f32::from(step) * (100.0 / f32::from(CURVE_STEPS));
        let fraction = ((n - bottom) / range).clamp(0.0, 1.0);
        let y = (1.0 - fraction) * 100.0;
        if step > 0 {
            path.push(' ');
        }
        let command = if step == 0 { 'M' } else { 'L' };
        let _ = write!(path, "{command} {x:.2} {y:.2}");
    }
    path
}

/// The curve `n(wavelength)` of `model` from 380 to 780 nm as path data, drawn between the indices
/// `lo` (bottom edge) and `hi` (top edge) of the plot.
#[must_use]
pub fn index_curve_path(model: &DispersionModel, lo: f32, hi: f32) -> String {
    index_samples_path(&sample_index(|l| model.evaluate(l)), lo, hi)
}

/// [`index_curve_path`] with the plot range fitted to the curve itself (the Material Editor's
/// sparkline): the path and the indices at the top and bottom edges, in that order.
#[must_use]
pub fn index_curve_path_fitted(model: &DispersionModel) -> (String, f32, f32) {
    let samples = sample_index(|l| model.evaluate(l));
    let (bottom, top) = shared_index_range(std::slice::from_ref(&samples));
    (index_samples_path(&samples, bottom, top), top, bottom)
}

/// Gridlines for the index axis: two to four multiples of a round step inside `bottom..=top`,
/// labelled with three decimals; the plot's two edges when no round step fits.
#[must_use]
pub fn index_ticks(bottom: f32, top: f32) -> Vec<AxisTick> {
    let range = f64::from(top - bottom);
    let (b, t) = (f64::from(bottom), f64::from(top));
    for step in [0.001, 0.002, 0.005, 0.01, 0.02, 0.05, 0.1, 0.2, 0.5, 1.0] {
        let first = (b / step).ceil();
        let last = (t / step).floor();
        let count = last - first + 1.0;
        if (2.0..=4.0).contains(&count) {
            let mut ticks = Vec::new();
            for n in 0_u8..4 {
                let k = first + f64::from(n);
                if k > last {
                    break;
                }
                let value = k * step;
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "plot positions and indices are small values; f32 is the chart precision"
                )]
                ticks.push(AxisTick {
                    text: format!("{value:.3}"),
                    position: ((1.0 - (value - b) / range) * 100.0) as f32,
                });
            }
            return ticks;
        }
    }
    vec![
        AxisTick {
            text: format!("{top:.3}"),
            position: 0.0,
        },
        AxisTick {
            text: format!("{bottom:.3}"),
            position: 100.0,
        },
    ]
}

/// The path of a spectrum given as `values` in `0..=1` (one per 2 nm step, 1 at the top of the
/// plot).
#[must_use]
pub fn spectrum_path(values: &[f32]) -> String {
    let mut path = String::new();
    for (step, v) in (0..=SPECTRUM_STEPS).zip(values) {
        let x = f32::from(step) * (100.0 / f32::from(SPECTRUM_STEPS));
        let y = (1.0 - v.clamp(0.0, 1.0)) * 100.0;
        if step > 0 {
            path.push(' ');
        }
        let command = if step == 0 { 'M' } else { 'L' };
        let _ = write!(path, "{command} {x:.2} {y:.2}");
    }
    path
}
