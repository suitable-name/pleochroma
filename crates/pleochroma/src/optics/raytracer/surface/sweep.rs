//! The faceting machine and the sweep of the stone across the lap.
//!
//! Scratches on a polished facet are the grooves of the lap's last strokes. At every contact
//! point the lap moves along the tangent of the circle through that point, so the scratch on
//! the facet runs along that tangent. While polishing, the cutter moves the stone between an
//! inner and an outer radius of the lap; the tangent turns with the radius, so over the sweep the
//! scratches fan out over a range of angles. The renderer draws one angle per sample from that
//! range (uniform in contact radius), so the image shows the true mix of directions.
//!
//! Geometry. The lap centre is the origin; the dop arm lies along a line at perpendicular distance
//! `d` (the `mast_distance_mm`, signed: which side of the centre the mast stands on) from the
//! centre, and the contact point sits on that line at radius `r >= |d|`. The angle between the
//! radial line through the contact point and the arm is `φ(r) = asin(d / r)`; the scratch is
//! perpendicular to the radial line, so relative to the girdle-parallel direction of the facet
//! it is rotated by `arm_offset + φ(r)`. An arm that points at the lap centre has `d = 0`:
//! `φ = 0` at every radius, the scratches run parallel to the girdle and the fan has no width.
//!
//! The setting is a per-user preference (cutters' machines differ), never part of a saved
//! design. The last polishing pass decides the scratches, so the sweep range is the range of
//! that pass.

use std::f32::consts::PI;

use glam::Vec3;

use super::facet_surface::FacetSurface;

/// Where on the lap the stone travels: the inner and outer contact radius in millimetres.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SweepRange {
    /// The smaller contact radius.
    pub inner_mm: f32,
    /// The larger contact radius.
    pub outer_mm: f32,
}

/// A named sweep range, as a fraction of the lap radius.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SweepPreset {
    /// A short stroke around the middle of the lap (55 % to 65 % of the radius).
    Narrow,
    /// The usual stroke (40 % to 80 % of the radius).
    Typical,
    /// Nearly the whole lap (20 % to 95 % of the radius).
    Wide,
}

impl SweepPreset {
    /// The inner and outer contact radius as fractions of the lap radius.
    #[must_use]
    pub const fn fractions(self) -> (f32, f32) {
        match self {
            Self::Narrow => (0.55, 0.65),
            Self::Typical => (0.40, 0.80),
            Self::Wide => (0.20, 0.95),
        }
    }

    /// The range for a lap of radius `lap_radius_mm`.
    #[must_use]
    pub fn range(self, lap_radius_mm: f32) -> SweepRange {
        let (inner, outer) = self.fractions();
        SweepRange {
            inner_mm: inner * lap_radius_mm,
            outer_mm: outer * lap_radius_mm,
        }
    }
}

/// The faceting machine the scratch directions are derived from.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FacetingMachine {
    /// The angle in degrees between the dop arm and the radial line through the contact point,
    /// added to the geometric angle of the mast; `0` when the arm points at the lap centre.
    pub arm_offset_deg: f32,
    /// The radius of the lap in millimetres.
    pub lap_radius_mm: f32,
    /// The distance in millimetres of the line of the dop arm from the lap centre; `0` when the
    /// arm points at the centre. The sign names the side of the centre the mast stands on.
    pub mast_distance_mm: f32,
    /// The contact radii of the last polishing pass.
    pub sweep: SweepRange,
}

impl FacetingMachine {
    /// The default machine: a lap of 100 mm radius, the arm pointing at its centre, a typical
    /// sweep. The scratches run parallel to the girdle.
    pub const DEFAULT: Self = Self {
        arm_offset_deg: 0.0,
        lap_radius_mm: 100.0,
        mast_distance_mm: 0.0,
        sweep: SweepRange {
            inner_mm: 40.0,
            outer_mm: 80.0,
        },
    };

    /// The scratch angle in degrees, relative to the girdle-parallel direction of the facet, at
    /// contact radius `radius_mm`.
    #[must_use]
    pub fn angle_at_radius_deg(&self, radius_mm: f32) -> f32 {
        let d = self.mast_distance_mm;
        if d == 0.0 {
            return self.arm_offset_deg;
        }
        // The contact point cannot be closer to the centre than the arm's line.
        let r = radius_mm.max(d.abs());
        self.arm_offset_deg + (d / r).clamp(-1.0, 1.0).asin().to_degrees()
    }

    /// The scratch angle at the middle of the sweep: what a cutter who holds the stone still
    /// ("No Sweep") polishes with.
    #[must_use]
    #[expect(
        clippy::manual_midpoint,
        reason = "the WGSL twin and the pinned goldens use this exact arithmetic"
    )]
    pub fn nominal_angle_deg(&self) -> f32 {
        self.angle_at_radius_deg(0.5 * (self.sweep.inner_mm + self.sweep.outer_mm))
    }

    /// The scratch angle of a sample: `u` in `[0, 1)` picks the contact radius uniformly over the
    /// sweep. The WGSL twin draws `u` from the same stream.
    #[must_use]
    pub fn sample_angle_deg(&self, u: f32) -> f32 {
        let radius = u.mul_add(
            self.sweep.outer_mm - self.sweep.inner_mm,
            self.sweep.inner_mm,
        );
        self.angle_at_radius_deg(radius)
    }

    /// The smallest and largest scratch angle over the sweep, in degrees.
    #[must_use]
    pub fn angle_range_deg(&self) -> (f32, f32) {
        let a = self.angle_at_radius_deg(self.sweep.inner_mm);
        let b = self.angle_at_radius_deg(self.sweep.outer_mm);
        (a.min(b), a.max(b))
    }

    /// The density, per degree, of the angle drawn by [`Self::sample_angle_deg`] at
    /// `angle_deg`; `0` outside the swept range. The sweep is uniform in contact radius `r`, and
    /// `sin(φ) = d / r`, so `p(φ) = |d| cos φ / (sin² φ (r_out − r_in))` per radian.
    ///
    /// Valid for a mast off the centre (`d != 0`) and a sweep that stays on the far side of the
    /// arm line (`inner >= |d|`); an arm through the centre has a single angle and no density.
    #[must_use]
    pub fn angle_density_per_deg(&self, angle_deg: f32) -> f32 {
        let d = self.mast_distance_mm;
        let (lo, hi) = self.angle_range_deg();
        let width = self.sweep.outer_mm - self.sweep.inner_mm;
        if d == 0.0 || width <= 0.0 || angle_deg < lo || angle_deg > hi {
            return 0.0;
        }
        let phi = (angle_deg - self.arm_offset_deg).to_radians();
        let sin_phi = phi.sin();
        if sin_phi == 0.0 {
            return 0.0;
        }
        d.abs() * phi.cos().abs() / (sin_phi * sin_phi * width) * (PI / 180.0)
    }
}

impl Default for FacetingMachine {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// The scratch angle in degrees for one sample on a facet with surface `surface`.
///
/// A Custom Angle fixes it, "No Sweep" collapses the fan to the nominal angle, otherwise the sweep
/// draws it from `u`.
#[must_use]
pub fn scratch_angle_deg(surface: &FacetSurface, machine: &FacetingMachine, u: f32) -> f32 {
    surface.custom_angle_deg().unwrap_or_else(|| {
        if surface.no_sweep() {
            machine.nominal_angle_deg()
        } else {
            machine.sample_angle_deg(u)
        }
    })
}

/// The direction of the scratches in the plane of a facet with outward normal `normal`, for a
/// scratch angle `angle_deg` measured from the girdle-parallel direction about the normal.
///
/// The girdle-parallel direction is `normalize(n × Y)` (model `+Y` is up); on a facet that faces
/// straight up or down, where it vanishes, it is model `+X`, the index-0 convention of the
/// crystal axis.
#[must_use]
pub fn scratch_direction(normal: Vec3, angle_deg: f32) -> Vec3 {
    let girdle = normal.cross(Vec3::Y);
    let girdle = if girdle.length_squared() < 1e-8 {
        // The table: take +X, made perpendicular to the normal.
        (Vec3::X - normal * normal.dot(Vec3::X)).normalize_or_zero()
    } else {
        girdle.normalize()
    };
    let (sin_a, cos_a) = angle_deg.to_radians().sin_cos();
    (girdle * cos_a + normal.cross(girdle) * sin_a).normalize_or_zero()
}
