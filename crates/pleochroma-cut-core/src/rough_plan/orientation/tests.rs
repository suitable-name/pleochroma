//! Tests of the resolved rule, the rough's orientation functions and the rows.

use glam::DVec3;
use pleochroma::optics::materials::GemMaterial;

use super::{
    ResolvedRule, RuleHold, RuleStatus, RuleTarget, StoneChoice, align_to_edge, align_to_face,
    fit_to_crystal_length, stone_orientation_in_stone_frame, stone_orientation_row,
};
use crate::{
    crystal_axis::{
        CrystalOrientation, OrientationRule, OrientationRuleKind, RuleStrength, default_rule,
    },
    rough_plan::{
        fit::StonePose, piece::ASSIGNMENTS, placement::DesignPlacement, tree::assignment_axes,
    },
};

fn sapphire() -> GemMaterial {
    GemMaterial::by_name("Sapphire").expect("sapphire is a built-in material")
}

fn rule(kind: OrientationRuleKind, tolerance_deg: f64, strength: RuleStrength) -> OrientationRule {
    OrientationRule {
        kind,
        tolerance_deg,
        strength,
    }
}

/// Half a percent per degree, as the rule type spells it.
const HALF_PERCENT: RuleStrength = RuleStrength::Preferred {
    weight_per_deg: 0.5,
};

fn resolved(
    kind: OrientationRuleKind,
    tolerance_deg: f64,
    strength: RuleStrength,
    orientation: CrystalOrientation,
) -> ResolvedRule {
    ResolvedRule::from_rule(
        &rule(kind, tolerance_deg, strength),
        orientation,
        &sapphire(),
    )
    .expect("a rule that applies")
}

/// The pose of a stone placed in assignment `orient` of the planner, at the origin, scale 1.
fn pose_of(orient: usize) -> StonePose {
    StonePose {
        center_mm: [0.0; 3],
        axes: assignment_axes(&ASSIGNMENTS[orient]),
        mm_per_unit: 1.0,
    }
}

#[test]
fn a_preferred_table_across_rule_charges_half_a_percent_per_degree_beyond_the_tolerance() {
    let up = CrystalOrientation::from_angles(0.0, 0.0, None);
    let rule = resolved(OrientationRuleKind::TableAcross, 15.0, HALF_PERCENT, up);
    // The rule's weight is a percent; the resolved one is the fraction.
    assert!(matches!(
        rule.hold,
        RuleHold::Preferred { weight_per_deg } if (weight_per_deg - 0.005).abs() < 1e-15
    ));
    let factors = rule.assignment_factors();
    for (orient, assignment) in ASSIGNMENTS.iter().enumerate() {
        if assignment[2] == 1 {
            // The table faces the axis: no penalty.
            assert!((factors[orient] - 1.0).abs() < 1e-15, "orient {orient}");
        } else {
            // 90 degrees off, 75 beyond the tolerance: 1 - 0.005 * 75.
            assert!(
                (factors[orient] - 0.625).abs() < 1e-9,
                "orient {orient}: {}",
                factors[orient]
            );
        }
    }
}

#[test]
fn a_required_rule_forbids_what_is_beyond_the_tolerance() {
    let up = CrystalOrientation::from_angles(0.0, 0.0, None);
    let rule = resolved(
        OrientationRuleKind::TableAcross,
        15.0,
        RuleStrength::Required,
        up,
    );
    for (orient, factor) in rule.assignment_factors().into_iter().enumerate() {
        let wanted = if ASSIGNMENTS[orient][2] == 1 {
            1.0
        } else {
            0.0
        };
        assert!((factor - wanted).abs() < 1e-15, "orient {orient}");
    }
    // 10 degrees off is inside 15; 20 is not.
    let near = CrystalOrientation::from_angles(10.0, 0.0, None);
    let far = CrystalOrientation::from_angles(20.0, 0.0, None);
    let table_up = assignment_axes(&ASSIGNMENTS[2]);
    let with = |orientation| ResolvedRule {
        orientation,
        ..rule
    };
    assert!((with(near).factor_for_axes(&table_up) - 1.0).abs() < 1e-15);
    assert!(with(far).factor_for_axes(&table_up).abs() < 1e-15);
}

#[test]
fn an_in_plane_rule_is_the_mirror_image() {
    let up = CrystalOrientation::from_angles(0.0, 0.0, None);
    let rule = resolved(
        OrientationRuleKind::InTablePlane,
        10.0,
        RuleStrength::Required,
        up,
    );
    for (orient, factor) in rule.assignment_factors().into_iter().enumerate() {
        // The axis is along y: a table facing y has the axis across it (90 degrees off the
        // plane); a table facing x or z has the axis in its plane.
        let wanted = if ASSIGNMENTS[orient][2] == 1 {
            0.0
        } else {
            1.0
        };
        assert!((factor - wanted).abs() < 1e-15, "orient {orient}");
    }
}

#[test]
fn an_azimuth_target_picks_the_pose_that_reaches_it() {
    // The axis lies in the x/z plane at azimuth 30. In the planner's orient-2 stone (x = rough x,
    // y = rough y, z = rough z) it has azimuth 30; the half turn about the stone's x axis
    // mirrors it to azimuth -30, which is the line at azimuth 150.
    let rough = CrystalOrientation::from_angles(90.0, 30.0, None);
    let rule = resolved(
        OrientationRuleKind::CustomAngle {
            tilt_deg: 90.0,
            azimuth_deg: Some(150.0),
        },
        1.0,
        RuleStrength::Required,
        rough,
    );
    assert!(rule.needs_azimuth());
    assert!((rule.assignment_factors()[2] - 1.0).abs() < 1e-15);
    assert_eq!(rule.assignment_poses()[2], 1);
    // Pose 0 alone would be 60 degrees off.
    let (deviation, pose) = rule.deviation_with_pose(&assignment_axes(&ASSIGNMENTS[2]));
    assert_eq!(pose, 1);
    assert!(deviation.expect("measurable") < 1e-6);
    let plain = rule.deviation_for_axes(&assignment_axes(&ASSIGNMENTS[2]));
    assert!(
        (plain.expect("measurable") - 60.0).abs() < 1e-6,
        "{plain:?}"
    );
}

#[test]
fn a_rule_without_an_azimuth_ignores_the_placement_of_the_design() {
    let rough = CrystalOrientation::from_angles(40.0, 10.0, None);
    let across = resolved(OrientationRuleKind::TableAcross, 15.0, HALF_PERCENT, rough);
    let placed = DesignPlacement {
        width_dir: [0.0, 1.0],
        centre_units: [0.1, 0.2, 0.3],
    };
    // The class of the rule does not depend on where the design sits.
    assert_eq!(across.with_placement(placed), across);
    let custom = resolved(
        OrientationRuleKind::CustomAngle {
            tilt_deg: 90.0,
            azimuth_deg: Some(20.0),
        },
        5.0,
        HALF_PERCENT,
        rough,
    );
    assert_ne!(custom.with_placement(placed), custom);
}

#[test]
fn the_materials_default_rule_resolves() {
    let material = sapphire();
    let default = default_rule(&material).expect("sapphire has a rule");
    let up = CrystalOrientation::from_angles(0.0, 0.0, None);
    let resolved = ResolvedRule::from_rule(&default, up, &material).expect("it applies");
    assert_eq!(resolved.target, RuleTarget::TableAcross);
    assert!((resolved.tolerance_deg - 15.0).abs() < 1e-12);
    // A rule of weight zero has no effect, so it does not resolve.
    let weightless = OrientationRule {
        strength: RuleStrength::Preferred {
            weight_per_deg: 0.0,
        },
        ..default
    };
    assert!(ResolvedRule::from_rule(&weightless, up, &material).is_none());
    // And no rule at all does not either.
    let none = rule(OrientationRuleKind::None, 15.0, RuleStrength::Required);
    assert!(ResolvedRule::from_rule(&none, up, &material).is_none());
}

#[test]
fn a_color_named_rule_resolves_to_a_principal_direction() {
    let tanzanite = GemMaterial::by_name("Tanzanite").expect("a built-in material");
    let default = default_rule(&tanzanite).expect("tanzanite has a rule");
    let rough = CrystalOrientation::from_angles(0.0, 0.0, Some(20.0));
    let resolved = ResolvedRule::from_rule(&default, rough, &tanzanite).expect("it applies");
    assert!(matches!(resolved.target, RuleTarget::Through(_)));
}

#[test]
fn aligning_to_a_face_or_an_edge_sets_the_axis() {
    let up = align_to_face([0.0, 1.0, 0.0], None).expect("a normal");
    assert!(up.axis.tilt_deg().abs() < 1e-9);
    // The opposite normal is the same line.
    let down = align_to_face([0.0, -3.0, 0.0], None).expect("a normal");
    assert_eq!(up, down);
    let along_x = align_to_face([2.0, 0.0, 0.0], Some(40.0)).expect("a normal");
    assert!((along_x.axis.tilt_deg() - 90.0).abs() < 1e-9);
    assert!(along_x.axis.azimuth_deg().abs() < 1e-9);
    assert_eq!(along_x.second_turn_deg, Some(40.0));
    let edge = align_to_edge([1.0, 1.0, 1.0], [1.0, 1.0, 6.0], None).expect("two points");
    assert!((edge.axis.tilt_deg() - 90.0).abs() < 1e-9);
    assert!((edge.axis.azimuth_deg() - 90.0).abs() < 1e-9);
    assert!(align_to_edge([1.0; 3], [1.0; 3], None).is_none());
    assert!(align_to_face([0.0; 3], None).is_none());
    assert!(align_to_face([f64::NAN, 0.0, 1.0], None).is_none());
}

/// The eight corners of a box `sx` by `sy` by `sz` centred on the origin, turned about `y` by
/// `turn_deg`.
fn box_corners(sx: f64, sy: f64, sz: f64, turn_deg: f64) -> Vec<DVec3> {
    let turn = glam::DMat3::from_rotation_y(turn_deg.to_radians());
    let mut out = Vec::new();
    for x in [-0.5, 0.5] {
        for y in [-0.5, 0.5] {
            for z in [-0.5, 0.5] {
                out.push(turn * DVec3::new(x * sx, y * sy, z * sz));
            }
        }
    }
    out
}

#[test]
fn fit_to_crystal_length_finds_the_long_direction() {
    let fit = fit_to_crystal_length(&box_corners(2.0, 2.0, 10.0, 0.0), None).expect("a box");
    // Along z: tilt 90, azimuth 90.
    assert!((fit.orientation.axis.tilt_deg() - 90.0).abs() < 1e-6);
    assert!((fit.orientation.axis.azimuth_deg() - 90.0).abs() < 1e-6);
    // Variances 25 (along z) and 1: confidence 1 - 1/25.
    assert!((fit.confidence - 0.96).abs() < 1e-9, "{}", fit.confidence);
    assert!((fit.elongation - 5.0).abs() < 1e-9, "{}", fit.elongation);

    // The same box turned 30 degrees about y: the axis turns with it.
    let turned = fit_to_crystal_length(&box_corners(2.0, 2.0, 10.0, 30.0), None).expect("a box");
    let direction = turned.orientation.axis.direction();
    let wanted = glam::DMat3::from_rotation_y(30f64.to_radians()) * DVec3::Z;
    assert!(direction.dot(wanted).abs() > 1.0 - 1e-9, "{direction:?}");
}

#[test]
fn a_cube_has_no_crystal_length() {
    let fit = fit_to_crystal_length(&box_corners(3.0, 3.0, 3.0, 0.0), None).expect("a cube");
    assert!(fit.confidence < 1e-9, "{}", fit.confidence);
    assert!(fit_to_crystal_length(&[DVec3::ZERO], None).is_none());
    assert!(fit_to_crystal_length(&[DVec3::ONE, DVec3::ONE], None).is_none());
}

#[test]
fn the_true_orientation_comes_from_the_actual_pose() {
    // The axis runs along the rough's z. The planner's orient-3 stone has its width along rough z,
    // so the axis lies along the stone's x axis: tilt 90, azimuth 0.
    let rough = CrystalOrientation::from_angles(90.0, 90.0, None);
    let base = pose_of(3);
    let stone = stone_orientation_in_stone_frame(&rough, &base, &DesignPlacement::IDENTITY)
        .expect("a proper pose");
    assert!((stone.axis.tilt_deg() - 90.0).abs() < 1e-6);
    assert!(
        stone.axis.azimuth_deg().abs() < 1e-6 || (stone.axis.azimuth_deg() - 180.0).abs() < 1e-6
    );

    // Orient 2 (width along rough x) with the axis at azimuth 30: the pose the cutter chose
    // decides the azimuth the design gets, not the planner's own pose.
    let rough = CrystalOrientation::from_angles(90.0, 30.0, None);
    let planner = pose_of(2);
    let chosen = StoneChoice {
        pose: 1,
        ..StoneChoice::default()
    }
    .pose_for(&planner);
    let azimuth = |pose: &StonePose| {
        stone_orientation_in_stone_frame(&rough, pose, &DesignPlacement::IDENTITY)
            .expect("a proper pose")
            .axis
            .azimuth_deg()
    };
    assert!((azimuth(&planner) - 30.0).abs() < 1e-6);
    assert!((azimuth(&chosen) - 150.0).abs() < 1e-6);
}

#[test]
fn a_free_pose_replaces_the_box_pose() {
    let base = pose_of(2);
    let free = StonePose {
        center_mm: [1.0, 2.0, 3.0],
        axes: assignment_axes(&ASSIGNMENTS[3]),
        mm_per_unit: 2.0,
    };
    let choice = StoneChoice {
        pose: 2,
        free_pose: Some(free),
        ..StoneChoice::default()
    };
    assert_eq!(choice.pose_for(&base), free);
    assert!(choice.is_overridden());
    // A lock or a reason alone is no override; the default changes nothing.
    assert!(
        !StoneChoice {
            locked: true,
            ..StoneChoice::default()
        }
        .is_overridden()
    );
    assert!(StoneChoice::default().is_default());
    // A pose that does not exist falls back to the planner's own.
    let missing = StoneChoice {
        pose: 9,
        ..StoneChoice::default()
    };
    assert_eq!(missing.pose_for(&base), base);
}

#[test]
fn the_row_says_followed_off_or_overridden() {
    let material = sapphire();
    let across = default_rule(&material).expect("sapphire has a rule");
    let up = CrystalOrientation::from_angles(0.0, 0.0, None);
    let row = |pose: &StonePose, choice: Option<&StoneChoice>| {
        stone_orientation_row(
            &up,
            pose,
            &DesignPlacement::IDENTITY,
            &material,
            Some(&across),
            choice,
        )
        .expect("a proper pose")
    };

    // Table facing y: the axis runs through it.
    let followed = row(&pose_of(2), None);
    assert_eq!(followed.status, RuleStatus::Followed);
    assert!(followed.axis_to_table_deg.abs() < 1e-6);
    assert_eq!(followed.status.text(), "\u{2713}");
    assert!(
        followed.color_direction_deg.is_none(),
        "sapphire is uniaxial"
    );

    // Table facing z: the axis lies across it.
    let off = row(&pose_of(0), None);
    match off.status {
        // The row says how far the axis is from the rule's target (90), not the excess.
        RuleStatus::Off { degrees } => assert!((degrees - 90.0).abs() < 1e-6, "{degrees}"),
        other => panic!("expected Off, got {other:?}"),
    }
    assert!((off.axis_to_table_deg - 90.0).abs() < 1e-6);

    // A pose the cutter chose is an override, with its Reason.
    let choice = StoneChoice {
        pose: 1,
        reason: "cut for color".to_string(),
        ..StoneChoice::default()
    };
    let overridden = row(&pose_of(0), Some(&choice));
    assert_eq!(
        overridden.status.text(),
        "overridden (cut for color)".to_string()
    );

    // No rule: nothing to say.
    let plain = stone_orientation_row(
        &up,
        &pose_of(0),
        &DesignPlacement::IDENTITY,
        &material,
        None,
        None,
    )
    .expect("a proper pose");
    assert_eq!(plain.status, RuleStatus::NoRule);
    assert_eq!(plain.status.text(), "");
}

#[test]
fn a_biaxial_material_reports_the_color_direction() {
    let tanzanite = GemMaterial::by_name("Tanzanite").expect("a built-in material");
    let rough = CrystalOrientation::from_angles(0.0, 0.0, Some(0.0));
    let row = stone_orientation_row(
        &rough,
        &pose_of(2),
        &DesignPlacement::IDENTITY,
        &tanzanite,
        None,
        None,
    )
    .expect("a proper pose");
    let angle = row.color_direction_deg.expect("tanzanite is biaxial");
    assert!((0.0..=90.0).contains(&angle), "{angle}");
}
