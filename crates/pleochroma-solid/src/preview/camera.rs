//! The orbit camera's pose math, shared by the desktop's
//! `gui::render::camera_lighting` and the web app's Solid view.
//!
//! Pure functions over [`CameraPose`]: the distance clamp and the fit distances, the
//! drag, zoom and standard-view steps, and [`RESET_POSE`]. The fit distances and
//! [`planar_bounding_radius`] are checked against projected stone meshes in
//! `camera/tests.rs`.

use super::types::{CameraPose, StoneGeometryBuf};
use pleochroma::{
    geometry::stone_metrics::{SolidStatus, build_solid_mesh},
    optics::raytracer::{DEFAULT_FOV_DEG, DEFAULT_POSE},
};
use std::f32::consts::FRAC_PI_2;

/// How much of the image the fitted stone leaves free: its outline spans `1 / FIT_MARGIN`
/// of the half-extent of the image's shorter side.
const FIT_MARGIN: f32 = 1.15;

/// The Reset pose: the shared [`DEFAULT_POSE`].
pub const RESET_POSE: CameraPose = CameraPose {
    roll: 0.0,
    yaw: DEFAULT_POSE.yaw,
    pitch: DEFAULT_POSE.pitch,
    distance: DEFAULT_POSE.distance,
};

/// The orbit camera's zoom clamp for a solid with the given bounding radius.
///
/// A fixed range would clip a large stone's facets at minimum distance and leave a
/// small one lost at maximum, so the clamp scales with the mesh. The factors make a mesh
/// of [`super::DEFAULT_MESH_BOUNDING_RADIUS`] (`1.5`, a round brilliant's half-width)
/// give the range `[1.2, 8.0]`.
#[must_use]
pub fn orbit_distance_bounds(bounding_radius: f64) -> (f32, f32) {
    const MIN_FACTOR: f32 = 0.8;
    const MAX_FACTOR: f32 = 16.0 / 3.0;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "f32 is the camera precision; a mesh bounding radius is in the normal float range"
    )]
    let radius = (bounding_radius as f32).max(0.01);
    (radius * MIN_FACTOR, radius * MAX_FACTOR)
}

/// The orbit distance at which a sphere of `bounding_radius` fills the view's height.
///
/// The sphere touches the vertical edges of the field of view, times a small margin so the "Fit"
/// pose leaves a little room. Mirrors the `v = ... * fov_tan` term of `Camera::generate_ray`.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    reason = "f32 is the camera precision; a mesh bounding radius is in the normal float range"
)]
pub fn fit_distance_for_radius(bounding_radius: f64) -> f32 {
    let half_fov_tan = (DEFAULT_FOV_DEG.to_radians() * 0.5).tan();
    (bounding_radius as f32).max(0.01) / half_fov_tan * FIT_MARGIN
}

/// The orbit distance at which a sphere of `bounding_radius` is seen whole in an image
/// of `aspect` (width over height), whatever the orbit angle.
///
/// The sphere is centred on the orbit centre; its outline sits at `1 / FIT_MARGIN` of
/// the shorter side's half-extent. The true outline is fitted (a sphere at distance `d`
/// spans `r / sqrt(d^2 - r^2)`, not `r / d`), using the narrower field of view. The
/// vertical field is fixed (`Camera::generate_ray`), so an image taller than wide sees
/// only `aspect` times as far sideways.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    reason = "f32 is the camera precision; a mesh bounding radius is in the normal float range"
)]
pub fn fit_distance_for_view(bounding_radius: f64, aspect: f32) -> f32 {
    let narrow = if aspect.is_finite() && aspect > 0.0 {
        aspect.min(1.0)
    } else {
        1.0
    };
    let outline_tan = (DEFAULT_FOV_DEG.to_radians() * 0.5).tan() * narrow / FIT_MARGIN;
    // `r / sqrt(d^2 - r^2) = outline_tan` solved for `d`.
    (bounding_radius as f32).max(0.01) * outline_tan.recip().hypot(1.0)
}

/// The farthest corner of `stone`'s plane arrangement from the orbit centre, in the
/// model units the renderers draw in; `None` when the planes do not close a solid.
///
/// Measured from the planes, so it describes the stone the caller holds now, not the
/// last frame drawn. Concave tools are ignored: they only remove material, so the
/// planar solid encloses the carved one.
#[must_use]
pub fn planar_bounding_radius(stone: &StoneGeometryBuf) -> Option<f64> {
    let planes: Vec<_> = stone
        .planes
        .iter()
        .map(|plane| plane.to_halfspace_f64())
        .collect();
    match build_solid_mesh(&planes) {
        SolidStatus::Closed(mesh) => Some(
            mesh.positions
                .iter()
                .map(|position| position.length())
                .fold(0.0_f64, f64::max),
        ),
        _ => None,
    }
}

/// Wraps an orbit pitch into `[-pi, pi)`: the camera may orbit freely over the
/// poles (see `Camera::new`'s pole handling), the wrap only keeps the persisted
/// value bounded.
#[must_use]
pub fn wrap_pitch(pitch: f32) -> f32 {
    use std::f32::consts::PI;
    (pitch + PI).rem_euclid(2.0 * PI) - PI
}

/// One orbit drag step of `(dx, dy)` logical pixels, at 0.008 rad per pixel. The
/// desktop's `ViewportModel.camera_orbit` uses the same rule.
///
/// Horizontal drag is inverted because the hand is on the gem, not on the camera;
/// vertical keeps its sign. The pitch wraps ([`wrap_pitch`]). Deltas are first mapped
/// through the image-plane roll, so a rolled view drags along the picture. Distance and
/// roll are unchanged.
#[must_use]
pub fn orbit_step(pose: CameraPose, dx: f32, dy: f32) -> CameraPose {
    let (dx, dy) = unroll_drag_delta(pose.roll, dx, dy);
    CameraPose {
        roll: pose.roll,
        yaw: dx.mul_add(-0.008, pose.yaw),
        pitch: wrap_pitch(dy.mul_add(0.008, pose.pitch)),
        distance: pose.distance,
    }
}

/// Maps a screen-space drag into the camera's unrolled image plane.
///
/// Screen Y points down. This is the inverse of `Camera::with_roll`'s image
/// rotation; apply it to orbit/light deltas, not to picking coordinates that
/// already use a rolled camera. Zero roll preserves the original deltas exactly.
#[must_use]
pub fn unroll_drag_delta(roll: f32, dx: f32, dy: f32) -> (f32, f32) {
    if roll == 0.0 {
        return (dx, dy);
    }
    let (sin, cos) = roll.sin_cos();
    (cos.mul_add(dx, sin * dy), cos.mul_add(dy, -sin * dx))
}

/// One wheel step of `delta` logical pixels: 0.002 per pixel, scrolling up moves closer.
///
/// Clamped to [`orbit_distance_bounds`] of the shown solid's `bounding_radius`.
#[must_use]
pub fn zoom_step(distance: f32, delta: f32, bounding_radius: f64) -> f32 {
    let (min, max) = orbit_distance_bounds(bounding_radius);
    delta.mul_add(-0.002, distance).clamp(min, max)
}

/// The desktop's pose pills (`ViewportModel.set_view`) as a camera pose.
///
/// `0` Front (girdle edge-on,
/// index 0 towards the viewer), `1` Top, `2` Bottom, `3` Left and `4` Right, each a
/// canonical yaw and pitch with the distance kept. `5` Fit keeps the angle and sets the
/// distance from the shown solid's `bounding_radius`, clamped to
/// [`orbit_distance_bounds`]. `6` Turn 90 turns the stone a quarter turn about its
/// vertical axis ([`turn_quarter_yaw`]), keeping tilt and zoom. Every view but Fit also
/// clears any roll. Unknown kinds are Front.
///
/// Fit assumes a picture at least as wide as tall (aspect 1, see
/// [`fit_distance_for_view`]). The web and mobile callers draw the Solid picture at
/// least 1.6:1 wide ([`super::view::with_min_aspect`]), so the height is the shorter side
/// there. A caller with another aspect calls [`fit_distance_for_view`] itself.
#[must_use]
pub fn standard_view(kind: i32, pose: CameraPose, bounding_radius: f64) -> CameraPose {
    if kind == 6 {
        return CameraPose {
            yaw: turn_quarter_yaw(pose.yaw),
            roll: 0.0,
            ..pose
        };
    }
    if kind == 5 {
        let (min, max) = orbit_distance_bounds(bounding_radius);
        return CameraPose {
            distance: fit_distance_for_view(bounding_radius, 1.0).clamp(min, max),
            ..pose
        };
    }
    let (yaw, pitch) = match kind {
        1 => (0.0, FRAC_PI_2),
        2 => (0.0, -FRAC_PI_2),
        3 => (-FRAC_PI_2, 0.0),
        4 => (FRAC_PI_2, 0.0),
        _ => (0.0, 0.0),
    };
    CameraPose {
        yaw,
        pitch,
        roll: 0.0,
        ..pose
    }
}

/// The Turn 90 control: the stone turns a quarter turn about its vertical axis (the yaw).
///
/// An oval that runs top to bottom in the Top view then lies across the picture, while
/// the drag directions stay those of every other view. The Top and Bottom views turn too,
/// because `Camera::new` orients the picture by the yaw there as well.
#[must_use]
pub fn turn_quarter_yaw(yaw: f32) -> f32 {
    (yaw + FRAC_PI_2).rem_euclid(std::f32::consts::TAU)
}

#[cfg(test)]
#[path = "camera/roll_tests.rs"]
mod roll_tests;
#[cfg(test)]
mod tests;
