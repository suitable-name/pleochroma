//! Exact convex geometry for beams: the side test, convex polygons, polylines, plane frames,
//! homographies and conics (beam guide §13).

pub mod circle;
pub mod frame;
pub mod homography;
pub mod polygon;
pub mod polyline;
pub mod side;

#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests_circle;
#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests_frame;
#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests_homography;
#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests_polygon;
#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests_polyline;
#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests_side;

/// The numerical tolerances of the beam geometry (beam guide §13).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tolerances {
    /// The side test's relative epsilon, scaled by the plane's magnitude.
    pub side_eps_rel: f64,
    /// The epsilon for a point on a pixel-cell edge.
    pub cell_eps: f64,
    /// Polygons with a smaller area in square pixels are dropped as slivers.
    pub sliver_area_px2: f64,
    /// The bisection stop on exact rays, in pixels.
    pub bisect_px: f64,
    /// `ε_chord`: the chord refinement stop, in pixels.
    pub chord_px: f64,
}

impl Tolerances {
    /// The default tolerances.
    pub const DEFAULT: Self = Self {
        side_eps_rel: 1e-10,
        cell_eps: 1e-9,
        sliver_area_px2: 1e-12,
        bisect_px: 1e-9,
        chord_px: 1e-3,
    };
}

impl Default for Tolerances {
    fn default() -> Self {
        Self::DEFAULT
    }
}
