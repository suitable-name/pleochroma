//! The Stone Pose: the stone stood on the tray at an angle other than the one it was designed at.
//!
//! A pose is a rotation about the model origin (the camera's orbit centre) applied to the stone
//! only: the tray, the lights, the environment map and the camera stay in world space. The tray
//! plane follows the posed stone's lowest vertex (`tray_plane_y`), so it drops as the stone
//! tilts. Everything that reads the stone as designed (material overrides, the stone width, the
//! girdle finish, the metrics) runs on the unrotated planes first; [`pose_stone`] is the last
//! step before tracing and turns every stone-frame quantity with the same rotation.
//!
//! The identity pose returns the inputs unchanged, without float work, so an unposed render keeps
//! every picture, hash and golden bit for bit.

use crate::{
    geometry::{plane::GpuFacetPlane, tool::ToolPrimitive},
    optics::{birefringence::alpha_axis_from_turn, materials::GemMaterial, zoning::ZoneFrame},
};
use glam::{DMat3, DQuat, Mat3, Vec3};

/// Largest tilt, degrees: 180 lays the stone on its table with the pavilion up.
const MAX_TILT_DEG: f32 = 180.0;
/// One full turn, degrees; turn and spin wrap into `[0, 360)`.
const FULL_TURN_DEG: f32 = 360.0;

/// How the stone stands on the tray: tilt, turn and spin, in degrees.
///
/// The rotation is `Ry(turn) * Rx(tilt) * Ry(spin)` (right-handed, about the model origin): the
/// stone is spun about its own table normal, tilted forward about the world X axis, then turned
/// about the world vertical. Tilt 90 with turn 0 stands the table upright facing +Z, the side a
/// camera at yaw 0 looks from. The default is the identity (the stone as designed).
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct StonePose {
    /// Tilt, degrees, `0..=180`: 0 is table up, 90 stands the table upright, 180 is table down.
    pub tilt_deg: f32,
    /// Turn about the world vertical, degrees, `[0, 360)`: the direction the table faces when the
    /// stone is tilted.
    pub turn_deg: f32,
    /// Spin about the stone's own vertical axis before the tilt, degrees, `[0, 360)`: it decides
    /// which side of an oval, pear or emerald cut ends up on the tray.
    pub spin_deg: f32,
}

/// A value as the pose stores it: a non-finite one reads as 0, then it wraps into `[0, 360)`.
fn wrapped(deg: f32) -> f32 {
    if !deg.is_finite() {
        return 0.0;
    }
    let w = deg.rem_euclid(FULL_TURN_DEG);
    // `rem_euclid` of a tiny negative value rounds up to exactly 360.
    if w >= FULL_TURN_DEG { 0.0 } else { w }
}

impl StonePose {
    /// The stone as designed.
    pub const IDENTITY: Self = Self {
        tilt_deg: 0.0,
        turn_deg: 0.0,
        spin_deg: 0.0,
    };

    /// The pose of `[tilt, turn, spin]` degrees, clamped (see [`Self::clamped`]).
    #[must_use]
    pub fn from_array(a: [f32; 3]) -> Self {
        Self {
            tilt_deg: a[0],
            turn_deg: a[1],
            spin_deg: a[2],
        }
        .clamped()
    }

    /// The pose as `[tilt, turn, spin]` degrees.
    #[must_use]
    pub const fn to_array(self) -> [f32; 3] {
        [self.tilt_deg, self.turn_deg, self.spin_deg]
    }

    /// The pose within its ranges: tilt clamped to `0..=180`, turn and spin wrapped into
    /// `[0, 360)`, a non-finite component read as 0.
    #[must_use]
    pub fn clamped(self) -> Self {
        let tilt = if self.tilt_deg.is_finite() {
            self.tilt_deg.clamp(0.0, MAX_TILT_DEG)
        } else {
            0.0
        };
        Self {
            tilt_deg: tilt,
            turn_deg: wrapped(self.turn_deg),
            spin_deg: wrapped(self.spin_deg),
        }
    }

    /// Whether the clamped pose is exactly `0, 0, 0`.
    #[must_use]
    pub fn is_identity(self) -> bool {
        let c = self.clamped();
        c.tilt_deg == 0.0 && c.turn_deg == 0.0 && c.spin_deg == 0.0
    }

    /// The rotation of the pose: `Ry(turn) * Rx(tilt) * Ry(spin)` of the clamped angles. The
    /// identity pose gives exactly [`Mat3::IDENTITY`].
    #[must_use]
    pub fn rotation(self) -> Mat3 {
        if self.is_identity() {
            return Mat3::IDENTITY;
        }
        let c = self.clamped();
        Mat3::from_rotation_y(c.turn_deg.to_radians())
            * Mat3::from_rotation_x(c.tilt_deg.to_radians())
            * Mat3::from_rotation_y(c.spin_deg.to_radians())
    }

    /// The pose that turns the table to a camera: `eye_dir` is the direction from the model
    /// origin to the actual eye (after roll and pan), `spin_deg` is kept. The turn is the eye's
    /// azimuth, `atan2(x, z)`; the tilt is 90 degrees minus the eye's elevation, clamped to
    /// `0..=90` so a camera below the horizon never lays the stone on its table. A zero
    /// direction gives turn and tilt 0.
    #[must_use]
    pub fn facing(eye_dir: Vec3, spin_deg: f32) -> Self {
        let e = eye_dir.normalize_or_zero();
        let (turn, tilt) = if e == Vec3::ZERO {
            (0.0, 0.0)
        } else {
            let elevation = e.y.clamp(-1.0, 1.0).asin().to_degrees();
            (
                e.x.atan2(e.z).to_degrees(),
                (90.0 - elevation).clamp(0.0, 90.0),
            )
        };
        Self {
            tilt_deg: tilt,
            turn_deg: turn,
            spin_deg,
        }
        .clamped()
    }

    /// The pose whose [`Self::rotation`] is `r`, for a gesture that turns the posed stone in the
    /// world (about the camera's axes) rather than moving one slider.
    ///
    /// The tilt is the angle between the stone's table normal (`r * +Y`) and the world vertical,
    /// the turn the azimuth that normal faces, and the spin what is left about the stone's own
    /// axis. With the table straight up or down the turn is undefined: `previous`'s turn is kept
    /// and the spin takes the rest, so a gesture never jumps there. `r` must be a rotation.
    #[must_use]
    pub fn from_rotation(r: Mat3, previous: Self) -> Self {
        /// Below this horizontal length of the table normal the turn is read as undefined.
        const POLE: f32 = 1e-5;
        let normal = r * Vec3::Y;
        let tilt = normal.y.clamp(-1.0, 1.0).acos();
        let horizontal = normal.x.hypot(normal.z);
        let turn = if horizontal > POLE {
            normal.x.atan2(normal.z)
        } else {
            previous.clamped().turn_deg.to_radians()
        };
        // `r = Ry(turn) * Rx(tilt) * Ry(spin)`, so what is left after the first two is
        // `Ry(spin)`, whose z column is `(sin spin, 0, cos spin)`.
        let rest = (Mat3::from_rotation_y(turn) * Mat3::from_rotation_x(tilt)).transpose() * r;
        let spin = rest.z_axis.x.atan2(rest.z_axis.z);
        Self {
            tilt_deg: tilt.to_degrees(),
            turn_deg: turn.to_degrees(),
            spin_deg: spin.to_degrees(),
        }
        .clamped()
    }
}

/// A stone with a pose applied: the planes, tools and stone-frame material quantities rotated.
#[derive(Debug, Clone, PartialEq)]
pub struct PosedStone {
    /// The facet planes: normals rotated, `d` unchanged (the rotation is about the origin).
    pub planes: Vec<GpuFacetPlane>,
    /// The concave tools: origin, axis and sweep direction rotated, profile unchanged.
    pub tools: Vec<ToolPrimitive>,
    /// The material with its crystal axis, explicit second axis, zone frame and inclusions
    /// rotated.
    pub material: GemMaterial,
    /// The stone's own up (`R * +Y`), the scratch direction along the girdle.
    pub up: Vec3,
    /// The stone's own `R * +X`, the scratch direction on a facet parallel to the table.
    pub side: Vec3,
}

/// `r * v` for the xyz of a four-component row, the `w` kept.
fn rotate_row(r: Mat3, row: [f32; 4]) -> [f32; 4] {
    let v = r * Vec3::new(row[0], row[1], row[2]);
    [v.x, v.y, v.z, row[3]]
}

/// The pose as a zone frame: `R` as a unit quaternion, no translation.
fn zone_frame(r: Mat3) -> ZoneFrame {
    let m = DMat3::from_cols(
        r.x_axis.as_dvec3(),
        r.y_axis.as_dvec3(),
        r.z_axis.as_dvec3(),
    );
    ZoneFrame {
        rotation: DQuat::from_mat3(&m).normalize(),
        translation: glam::DVec3::ZERO,
    }
}

/// The material with every stone-frame quantity turned by `r`.
///
/// A biaxial material without a stated second axis first gets the tracer's own completion made
/// explicit (`alpha_axis_from_turn(c, 0)`), because the tracer's fallback is computed from the
/// axis in the frame it is read in and would not turn with it.
fn rotate_material(r: Mat3, traced: &GemMaterial) -> GemMaterial {
    let mut m = traced.clone();
    let explicit_alpha = match m.alpha_axis {
        Some(a) => Some(a),
        None if m.biaxial_delta_beta_alpha.is_some() => Some(alpha_axis_from_turn(m.c_axis, 0.0)),
        None => None,
    };
    m.alpha_axis = explicit_alpha.map(|a| r * a);
    m.c_axis = r * m.c_axis;
    m.zoning = m.zoning.as_ref().map(|z| z.transformed(&zone_frame(r)));
    m.inclusions = m.inclusions.as_ref().map(|set| set.rotated(r));
    m
}

/// The stone in `pose`: the last step before tracing.
///
/// `planes`, `tools` and `traced` are the stone as designed (with every material override
/// already applied). With [`StonePose::is_identity`] they come back unchanged.
#[must_use]
pub fn pose_stone(
    pose: StonePose,
    planes: &[GpuFacetPlane],
    tools: &[ToolPrimitive],
    traced: &GemMaterial,
) -> PosedStone {
    if pose.is_identity() {
        return PosedStone {
            planes: planes.to_vec(),
            tools: tools.to_vec(),
            material: traced.clone(),
            up: Vec3::Y,
            side: Vec3::X,
        };
    }
    let r = pose.rotation();
    let planes = planes
        .iter()
        .map(|p| {
            let n = r * Vec3::from(p.normal);
            GpuFacetPlane {
                normal: n.to_array(),
                d: p.d,
            }
        })
        .collect();
    let tools = tools
        .iter()
        .map(|t| ToolPrimitive {
            origin: rotate_row(r, t.origin),
            axis: rotate_row(r, t.axis),
            sweep_dir: rotate_row(r, t.sweep_dir),
            ..*t
        })
        .collect();
    PosedStone {
        planes,
        tools,
        material: rotate_material(r, traced),
        up: r * Vec3::Y,
        side: r * Vec3::X,
    }
}

#[cfg(test)]
mod tests;
