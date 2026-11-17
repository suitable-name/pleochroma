//! Tests of the Stone Pose: the angles, the rotation, and the posed stone.

use super::*;
use crate::{
    geometry::{brep::GemPolyhedron, cuts::StandardGemCuts, tool::StoneGeometry},
    optics::{
        absorption::AbsorptionTensor,
        inclusions::{CrystalFrame, Family, FamilyKind},
        raytracer::environment::tray_plane_y,
        zoning::{ZoneAbsorption, ZonedAbsorption},
    },
};
use glam::DVec3;

const EPS: f32 = 1e-5;

fn near(a: Vec3, b: Vec3) -> bool {
    (a - b).length() < EPS
}

fn pose() -> StonePose {
    StonePose {
        tilt_deg: 63.0,
        turn_deg: 140.0,
        spin_deg: 25.0,
    }
}

fn ball() -> ToolPrimitive {
    ToolPrimitive {
        kind: 0,
        sweep_kind: 2,
        _pad: [0; 2],
        origin: [0.3, -0.2, 0.5, 0.1],
        axis: [0.0, 1.0, 0.0, 0.0],
        profile: [0.1, 0.1, 0.2, 0.0],
        sweep_dir: [1.0, 0.0, 0.0, 0.0],
    }
}

fn silk() -> Family {
    Family {
        kind: FamilyKind::Needle,
        dir: crate::optics::inclusions::CrystalDir::AlongC,
        sigma: 0.2,
        p: 2,
        lobe: 0.05,
        rho: 0.0,
    }
}

fn zoned_material() -> GemMaterial {
    let tensor = AbsorptionTensor::isotropic(Vec::new());
    let mut zoning = ZonedAbsorption::new(ZoneAbsorption::per_mm(tensor));
    zoning.frame = ZoneFrame::from_rotation_vector([0.0, 0.4, 0.0], [0.0; 3]);
    GemMaterial::sapphire().with_zoning(zoning)
}

/// The identity pose hands back the planes, tools and material bit for bit.
#[test]
fn identity_returns_the_inputs_unchanged() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let tools = [ball()];
    let material = GemMaterial::sapphire();
    let posed = pose_stone(StonePose::IDENTITY, &planes, &tools, &material);
    assert_eq!(posed.planes.len(), planes.len());
    for (a, b) in posed.planes.iter().zip(&planes) {
        assert_eq!(a.d.to_bits(), b.d.to_bits());
        for k in 0..3 {
            assert_eq!(a.normal[k].to_bits(), b.normal[k].to_bits());
        }
    }
    assert_eq!(posed.tools, tools);
    assert_eq!(posed.material, material);
    assert_eq!(posed.up, Vec3::Y);
    assert_eq!(posed.side, Vec3::X);
    assert_eq!(StonePose::IDENTITY.rotation(), Mat3::IDENTITY);
    assert!(StonePose::default().is_identity());
}

/// Tilt 90, turn 0 maps the table normal +Y to +Z, the side of a camera at yaw 0.
#[test]
fn tilt_ninety_turn_zero_maps_up_to_plus_z() {
    let r = StonePose::from_array([90.0, 0.0, 0.0]).rotation();
    assert!(near(r * Vec3::Y, Vec3::Z));
    // Turn 90 about +Y takes +Z to +X, the eye of a camera at yaw 90.
    let r = StonePose::from_array([90.0, 90.0, 0.0]).rotation();
    assert!(near(r * Vec3::Y, Vec3::X));
    // The spin turns about the stone's own vertical and leaves the table normal alone.
    let r = StonePose::from_array([0.0, 0.0, 70.0]).rotation();
    assert!(near(r * Vec3::Y, Vec3::Y));
    assert!(near(r * Vec3::X, Vec3::new(0.34202, 0.0, -0.93969)));
}

/// Turn and spin wrap into [0, 360); the tilt clamps to 0..=180.
#[test]
fn turn_wraps_and_tilt_clamps() {
    let p = StonePose::from_array([200.0, 370.0, -10.0]);
    assert_eq!(p.tilt_deg, 180.0);
    assert!((p.turn_deg - 10.0).abs() < 1e-4);
    assert!((p.spin_deg - 350.0).abs() < 1e-4);
    assert_eq!(
        StonePose::from_array([-5.0, 360.0, 720.0]).to_array(),
        [0.0; 3]
    );
    assert!(StonePose::from_array([0.0, -1e-9, 0.0]).is_identity());
    assert!(StonePose::from_array([0.0, 360.0, 0.0]).is_identity());
    assert_eq!(
        StonePose::from_array([30.0, 45.0, 60.0]).to_array(),
        [30.0, 45.0, 60.0]
    );
}

/// A non-finite component reads as 0.
#[test]
fn non_finite_reads_as_zero() {
    let p = StonePose::from_array([f32::NAN, f32::INFINITY, f32::NEG_INFINITY]);
    assert_eq!(p.to_array(), [0.0; 3]);
    assert!(p.is_identity());
    let p = StonePose::from_array([45.0, f32::NAN, 10.0]);
    assert_eq!(p.to_array(), [45.0, 0.0, 10.0]);
}

/// Planes keep their offset and turn their normals with the rotation.
#[test]
fn planes_keep_d_and_rotate_normals() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let r = pose().rotation();
    let posed = pose_stone(pose(), &planes, &[], &GemMaterial::sapphire());
    for (after, before) in posed.planes.iter().zip(&planes) {
        assert_eq!(after.d.to_bits(), before.d.to_bits());
        assert!(near(
            Vec3::from(after.normal),
            r * Vec3::from(before.normal)
        ));
        assert!((Vec3::from(after.normal).length() - 1.0).abs() < 1e-5);
    }
    assert!(near(posed.up, r * Vec3::Y));
    assert!(near(posed.side, r * Vec3::X));
}

/// The tray plane of the posed stone sits at the lowest vertex of the rotated stone.
#[test]
fn the_posed_stone_stands_on_its_lowest_vertex() {
    let planes = StandardGemCuts::standard_round_brilliant();
    let r = pose().rotation();
    let hull = GemPolyhedron::from_planes(planes.clone()).expect("a closed stone");
    let expected_low = hull
        .vertices
        .iter()
        .map(|v| (r * *v).y)
        .fold(f32::INFINITY, f32::min);
    let posed = pose_stone(pose(), &planes, &[], &GemMaterial::sapphire());
    let plane_y = tray_plane_y(StoneGeometry::planes_only(&posed.planes)).expect("a closed stone");
    // The tray is lowered by 1e-5 of the bounding radius below the lowest vertex.
    assert!(plane_y <= expected_low);
    assert!(
        (plane_y - expected_low).abs() < 1e-3,
        "{plane_y} vs {expected_low}"
    );
    let unposed = tray_plane_y(StoneGeometry::planes_only(&planes)).expect("a closed stone");
    assert!(
        (plane_y - unposed).abs() > 1e-3,
        "the tray moves with the pose"
    );
}

/// Tools turn their origin, axis and sweep direction; the profile and the `w` components stay.
#[test]
fn tools_rotate_with_the_stone() {
    let r = pose().rotation();
    let planes = StandardGemCuts::standard_round_brilliant();
    let posed = pose_stone(pose(), &planes, &[ball()], &GemMaterial::sapphire());
    let (a, b) = (posed.tools[0], ball());
    let v = |row: [f32; 4]| Vec3::new(row[0], row[1], row[2]);
    assert!(near(v(a.origin), r * v(b.origin)));
    assert!(near(v(a.axis), r * v(b.axis)));
    assert!(near(v(a.sweep_dir), r * v(b.sweep_dir)));
    assert_eq!(a.origin[3], b.origin[3]);
    assert_eq!(a.axis[3], b.axis[3]);
    assert_eq!(a.profile, b.profile);
    assert_eq!((a.kind, a.sweep_kind), (b.kind, b.sweep_kind));
}

/// The crystal axis, an explicit second axis, the zone frame and the inclusion directions turn.
#[test]
fn material_quantities_rotate_with_the_stone() {
    let r = pose().rotation();
    let planes = StandardGemCuts::standard_round_brilliant();

    let mut material = zoned_material();
    material.c_axis = Vec3::new(1.0, 1.0, 0.0).normalize();
    material.alpha_axis = Some(Vec3::Z);
    let set = CrystalFrame::new(material.c_axis, None).resolve(&[silk()]);
    material = material.with_inclusions(set);
    let posed = pose_stone(pose(), &planes, &[], &material).material;

    assert!(near(posed.c_axis, r * material.c_axis));
    assert!(near(posed.alpha_axis.expect("kept explicit"), r * Vec3::Z));

    let before = material.zoning.as_ref().expect("zoning").frame;
    let after = posed.zoning.as_ref().expect("zoning").frame;
    let probe = DVec3::new(0.3, 0.8, -0.5);
    let want = (r * before.direction(probe).as_vec3()).as_dvec3();
    assert!((after.direction(probe) - want).length() < 1e-5);
    assert_eq!(after.translation, DVec3::ZERO);

    let before = material.inclusions.as_ref().expect("set").families()[0].dir;
    let after = posed.inclusions.as_ref().expect("set").families()[0].dir;
    assert!(near(after, r * before));
}

/// A biaxial material without a second axis gets the tracer's completion made explicit, then turned.
#[test]
fn a_missing_second_axis_is_made_explicit_before_it_turns() {
    let r = pose().rotation();
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::all_materials()
        .into_iter()
        .find(|m| m.biaxial_delta_beta_alpha.is_some() && m.alpha_axis.is_none())
        .expect("a biaxial built-in");
    let posed = pose_stone(pose(), &planes, &[], &material).material;
    let completion = alpha_axis_from_turn(material.c_axis, 0.0);
    assert!(near(posed.alpha_axis.expect("explicit"), r * completion));
    // A material that never reads the second axis keeps `None`.
    let uniaxial = GemMaterial::sapphire();
    assert!(uniaxial.alpha_axis.is_none());
    assert!(
        pose_stone(pose(), &planes, &[], &uniaxial)
            .material
            .alpha_axis
            .is_none()
    );
}

/// Face the Camera: turn is the eye's azimuth, tilt is 90 degrees minus its elevation, clamped.
#[test]
fn facing_takes_turn_from_yaw_and_tilt_from_elevation() {
    let (yaw, pitch) = (0.8_f32, 0.45_f32);
    let eye = Vec3::new(
        pitch.cos() * yaw.sin(),
        pitch.sin(),
        pitch.cos() * yaw.cos(),
    ) * 7.0;
    let p = StonePose::facing(eye, 33.0);
    assert!((p.turn_deg - yaw.to_degrees()).abs() < 1e-3);
    assert!((p.tilt_deg - (90.0 - pitch.to_degrees())).abs() < 1e-3);
    assert_eq!(p.spin_deg, 33.0);
    // The table normal of the facing pose points at the eye.
    assert!((p.rotation() * Vec3::Y - eye.normalize()).length() < 1e-4);

    // Yaw 0 at the horizon stands the table upright toward +Z.
    let p = StonePose::facing(Vec3::Z, 0.0);
    assert!((p.tilt_deg - 90.0).abs() < 1e-4);
    assert!(p.turn_deg.abs() < 1e-4);

    // Below the horizon the tilt clamps to 90, straight above it is 0.
    assert_eq!(
        StonePose::facing(Vec3::new(0.0, -0.5, 1.0), 0.0).tilt_deg,
        90.0
    );
    assert_eq!(StonePose::facing(Vec3::Y, 0.0).tilt_deg, 0.0);
    // The turn wraps into [0, 360): an eye at -X is 270 degrees.
    assert!((StonePose::facing(-Vec3::X, 0.0).turn_deg - 270.0).abs() < 1e-3);
    // A zero direction gives the identity turn and tilt.
    assert_eq!(
        StonePose::facing(Vec3::ZERO, 12.0).to_array(),
        [0.0, 0.0, 12.0]
    );
}

/// Whether two rotations agree element by element.
fn same_rotation(a: Mat3, b: Mat3) -> bool {
    (a - b).to_cols_array().iter().all(|v| v.abs() < 1e-4)
}

#[test]
fn from_rotation_gives_back_the_pose_and_keeps_the_turn_at_the_poles() {
    for tilt in [5.0_f32, 45.0, 90.0, 135.0, 175.0] {
        for turn in [0.0_f32, 30.0, 120.0, 250.0, 359.0] {
            for spin in [0.0_f32, 15.0, 200.0] {
                let pose = StonePose {
                    tilt_deg: tilt,
                    turn_deg: turn,
                    spin_deg: spin,
                };
                let back = StonePose::from_rotation(pose.rotation(), StonePose::IDENTITY);
                assert!(
                    same_rotation(back.rotation(), pose.rotation()),
                    "{pose:?} came back as {back:?}"
                );
                assert!(
                    (back.tilt_deg - tilt).abs() < 1e-2,
                    "{pose:?} came back as {back:?}"
                );
            }
        }
    }
    // Table straight up or down: the turn is undefined, so the previous one is kept and the
    // spin takes the rest.
    for tilt in [0.0_f32, 180.0] {
        let pose = StonePose {
            tilt_deg: tilt,
            turn_deg: 70.0,
            spin_deg: 20.0,
        };
        let previous = StonePose {
            tilt_deg: 3.0,
            turn_deg: 70.0,
            spin_deg: 0.0,
        };
        let back = StonePose::from_rotation(pose.rotation(), previous);
        assert!((back.turn_deg - 70.0).abs() < 1e-3, "{back:?}");
        assert!(same_rotation(back.rotation(), pose.rotation()), "{back:?}");
    }
}
