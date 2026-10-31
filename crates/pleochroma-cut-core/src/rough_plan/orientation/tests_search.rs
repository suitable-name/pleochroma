//! Tests of the search with orientation rules: the block planner, the alternatives, the fit, the
//! single-piece re-fit and Lock.

use glam::DVec3;
use pleochroma::optics::materials::GemMaterial;

use super::{
    AlternativeKind, LOCKED_FIRST_NOTE, ResolvedRule, StoneChoice, locked_shell,
    plan_orientation_alternatives, refit_block_piece, rough_with_locked_pieces,
};
use crate::{
    crystal_axis::{CrystalOrientation, OrientationRule, OrientationRuleKind, RuleStrength},
    rough_plan::{
        Axis, CandidateDesign, CutOrder, DesignPlacement, PlacedStone, PlanProgress, RoughBase,
        RoughBlock, RoughLayout, RoughModel,
        fit::{
            DesignHull, SingleFit, StonePose, fit_shortlisted_in_cone, fit_single_stones_ruled,
            merge_fits, merge_fits_scored, merge_rule_passes, rescale_pose, rule_passes,
            screen_designs_in_cone,
        },
        piece::{ASSIGNMENTS, Norm, stone_value},
        plan::{PlanInput, PlanPath, plan},
        plan_rough,
        tests::{box_design, settings_with},
        tree::assignment_axes,
    },
};

fn sapphire() -> GemMaterial {
    GemMaterial::by_name("Sapphire").expect("sapphire is a built-in material")
}

fn rule_of(
    kind: OrientationRuleKind,
    tolerance_deg: f64,
    strength: RuleStrength,
    orientation: CrystalOrientation,
) -> ResolvedRule {
    ResolvedRule::from_rule(
        &OrientationRule {
            kind,
            tolerance_deg,
            strength,
        },
        orientation,
        &sapphire(),
    )
    .expect("a rule that applies")
}

fn with_rule(designs: &[CandidateDesign], rule: Option<ResolvedRule>) -> Vec<CandidateDesign> {
    designs
        .iter()
        .map(|design| CandidateDesign {
            orientation: rule,
            ..*design
        })
        .collect()
}

/// A tall design: its table normal is the long box axis, so a stone prefers to lie with the
/// table across the thin direction of a slab only when the rule says so.
fn tall_design(entry_id: i64) -> CandidateDesign {
    CandidateDesign {
        entry_id,
        width: 1.0,
        length: 1.0,
        height: 2.0,
        volume: 1.6,
        orientation: None,
    }
}

fn volume(layouts: &[RoughLayout]) -> f64 {
    layouts
        .first()
        .map_or(0.0, |layout| layout.total_volume_mm3)
}

const HALF_PERCENT: RuleStrength = RuleStrength::Preferred {
    weight_per_deg: 0.5,
};

#[test]
fn a_required_table_across_rule_puts_every_table_on_the_axis() {
    let rough = RoughBlock {
        x_mm: 3.0,
        y_mm: 12.0,
        z_mm: 12.0,
    };
    let settings = settings_with(1, 0.0, 0.0, 0.5);
    let up = CrystalOrientation::from_angles(0.0, 0.0, None);
    let rule = rule_of(
        OrientationRuleKind::TableAcross,
        15.0,
        RuleStrength::Required,
        up,
    );
    let plain = [tall_design(1)];
    let ruled = with_rule(&plain, Some(rule));

    let free = plan_rough(&rough, &settings, &plain, &mut |_| true).expect("not cancelled");
    let held = plan_rough(&rough, &settings, &ruled, &mut |_| true).expect("not cancelled");
    assert!(!free.is_empty() && !held.is_empty());
    for layout in &held {
        for stone in &layout.stones {
            assert_eq!(stone.table_axis, Axis::Y, "{stone:?}");
            let deviation = rule
                .deviation_for_axes(&stone.pose.axes)
                .expect("a proper pose");
            assert!(deviation <= 15.0 + 1e-6, "{deviation}");
        }
    }
    // A rule can only cost volume.
    assert!(volume(&held) <= volume(&free) * (1.0 + 1e-12));
}

#[test]
fn a_rule_that_allows_everything_changes_nothing_and_the_plan_is_deterministic() {
    let rough = RoughBlock {
        x_mm: 7.0,
        y_mm: 9.0,
        z_mm: 5.0,
    };
    let settings = settings_with(2, 0.2, 0.1, 0.5);
    let up = CrystalOrientation::from_angles(0.0, 0.0, None);
    let rule = rule_of(
        OrientationRuleKind::TableAcross,
        90.0,
        RuleStrength::Required,
        up,
    );
    let plain = [tall_design(1), box_design(2)];
    let ruled = with_rule(&plain, Some(rule));
    let free = plan_rough(&rough, &settings, &plain, &mut |_| true).expect("not cancelled");
    let held = plan_rough(&rough, &settings, &ruled, &mut |_| true).expect("not cancelled");
    assert_eq!(free, held);
    let again = plan_rough(&rough, &settings, &ruled, &mut |_| true).expect("not cancelled");
    assert_eq!(held, again);
}

#[test]
fn the_factor_is_absent_without_a_rule() {
    let design = tall_design(1);
    let plain = Norm::plain(&design);
    let of = Norm::of(&design);
    let p = [3.0, 5.0, 7.0];
    for orient in 0..6 {
        let a = stone_value(&plain, orient, p, 0.1);
        let b = stone_value(&of, orient, p, 0.1);
        assert_eq!(a.to_bits(), b.to_bits(), "orient {orient}");
    }
}

#[test]
fn a_preferred_rule_has_alternatives_between_best_yield_and_best_orientation() {
    let rough = RoughBlock {
        x_mm: 2.5,
        y_mm: 14.0,
        z_mm: 14.0,
    };
    let settings = settings_with(1, 0.2, 0.1, 0.5);
    let along_x = CrystalOrientation::from_angles(90.0, 0.0, None);
    let rule = rule_of(
        OrientationRuleKind::InTablePlane,
        10.0,
        HALF_PERCENT,
        along_x,
    );
    let designs = with_rule(&[tall_design(1), box_design(2)], Some(rule));
    let model = RoughModel::new(
        RoughBase::Block {
            x_mm: rough.x_mm,
            y_mm: rough.y_mm,
            z_mm: rough.z_mm,
        },
        Vec::new(),
    );
    let input = PlanInput {
        model: &model,
        settings: &settings,
        designs: &designs,
        hulls: &[],
    };
    assert_eq!(input.path(), PlanPath::PlainBlock);
    let alternatives =
        plan_orientation_alternatives(&input, CutOrder::Xyz, 1, &mut |_: PlanProgress| true)
            .expect("not cancelled");
    let find = |kind| {
        alternatives
            .iter()
            .find(|alternative| alternative.kind == kind)
            .map(|alternative| &alternative.layout)
    };
    let yield_best = find(AlternativeKind::BestYield).expect("best yield");
    let knee = find(AlternativeKind::Knee).expect("balanced");
    let oriented = find(AlternativeKind::BestOrientation).expect("best orientation");
    // Each end optimises its own objective, so the volumes are ordered.
    assert!(yield_best.total_volume_mm3 >= knee.total_volume_mm3 * (1.0 - 1e-9));
    assert!(knee.total_volume_mm3 >= oriented.total_volume_mm3 * (1.0 - 1e-9));
    // Best orientation keeps every stone inside the tolerance.
    for stone in &oriented.stones {
        let deviation = rule
            .deviation_for_axes(&stone.pose.axes)
            .expect("a proper pose");
        assert!(deviation <= 10.0 + 1e-6, "{deviation}");
    }
    // The same inputs give the same layouts.
    let again =
        plan_orientation_alternatives(&input, CutOrder::Xyz, 1, &mut |_: PlanProgress| true)
            .expect("not cancelled");
    assert_eq!(alternatives.len(), again.len());
    for (a, b) in alternatives.iter().zip(&again) {
        assert_eq!(a.kind, b.kind);
        assert_eq!(a.layout, b.layout);
    }

    // A Required rule alone has no alternatives: the plan is the only layout of its kind.
    let required = with_rule(
        &[tall_design(1)],
        Some(rule_of(
            OrientationRuleKind::InTablePlane,
            10.0,
            RuleStrength::Required,
            along_x,
        )),
    );
    let input = PlanInput {
        designs: &required,
        ..input
    };
    let none = plan_orientation_alternatives(&input, CutOrder::Xyz, 1, &mut |_: PlanProgress| true)
        .expect("not cancelled");
    assert_eq!(
        none,
        [] as [crate::rough_plan::orientation::alternatives::OrientationAlternative; 0]
    );
}

#[test]
fn the_ends_of_a_preferred_rule_differ_strictly_on_a_rough_that_forces_the_trade() {
    // One tall stone (1 x 1 x 2, the 2 mm along the table normal) in a 2.05 x 1 x 1 block. The
    // only full-size stone (volume 1.6, one piece, no saw cut) has its table facing x. The
    // crystal axis (tilt 0) runs along the rough's y and the rule wants the table across it: the
    // 2 mm must lie along y, where the block is 1 mm, so the stones shrink, and the kerf makes
    // the many small pieces lose volume. (2026-10-22: the axis was along x, where the full stone
    // already obeys the rule; and without a kerf eight half-size stones tile the same 1.6.)
    let model = block_model(2.05, 1.0, 1.0);
    let settings = settings_with(1, 0.05, 0.0, 0.4);
    let along_y = CrystalOrientation::from_angles(0.0, 0.0, None);
    let rule = rule_of(
        OrientationRuleKind::TableAcross,
        10.0,
        HALF_PERCENT,
        along_y,
    );
    let designs = with_rule(&[tall_design(1)], Some(rule));
    let input = PlanInput {
        model: &model,
        settings: &settings,
        designs: &designs,
        hulls: &[],
    };
    let alternatives =
        plan_orientation_alternatives(&input, CutOrder::Xyz, 1, &mut |_: PlanProgress| true)
            .expect("not cancelled");
    let find = |kind| {
        alternatives
            .iter()
            .find(|alternative| alternative.kind == kind)
            .map(|alternative| &alternative.layout)
    };
    let yield_best = find(AlternativeKind::BestYield).expect("best yield");
    let oriented = find(AlternativeKind::BestOrientation).expect("best orientation");
    assert!(
        yield_best.total_volume_mm3 > oriented.total_volume_mm3 * (1.0 + 1e-6),
        "{} vs {}",
        yield_best.total_volume_mm3,
        oriented.total_volume_mm3
    );
    assert_ne!(yield_best, oriented);
    // The ends are different stones: the best orientation is inside the tolerance, the best yield
    // is not.
    let off = |layout: &RoughLayout| {
        layout
            .stones
            .iter()
            .filter_map(|stone| rule.deviation_for_axes(&stone.pose.axes))
            .fold(0.0_f64, f64::max)
    };
    assert!(off(oriented) <= 10.0 + 1e-6, "{}", off(oriented));
    assert!(off(yield_best) > 10.0, "{}", off(yield_best));
}

#[test]
fn a_custom_angle_azimuth_is_read_in_the_designs_own_frame() {
    // The same rule and the same stone pose: a design turned a quarter about the vertical by its
    // caliper (width_dir = z) sees the crystal axis along its own z, not x.
    let axis_along_x = CrystalOrientation::from_angles(90.0, 0.0, None);
    let rule = rule_of(
        OrientationRuleKind::CustomAngle {
            tilt_deg: 90.0,
            azimuth_deg: Some(0.0),
        },
        5.0,
        RuleStrength::Required,
        axis_along_x,
    );
    let identity = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let (plain, _) = rule.deviation_with_pose(&identity);
    let turned = rule.with_placement(DesignPlacement {
        width_dir: [0.0, 1.0],
        centre_units: [0.0; 3],
    });
    // A box pose is a half turn: it cannot bring the axis back into the design's x.
    let (placed, _) = turned.deviation_with_pose(&identity);
    assert!(plain.expect("measured") < 1e-6);
    assert!(
        (placed.expect("measured") - 90.0).abs() < 1e-6,
        "{placed:?}"
    );
    // The design's own turn changes which assignments meet the target.
    assert_ne!(rule.assignment_deviations(), turned.assignment_deviations());
}

#[test]
fn the_plan_takes_the_pose_its_azimuth_rule_credited() {
    // A Custom Angle rule with an azimuth is scored at the best of the four box poses of each
    // stone; the returned layouts must carry that pose, so nothing is left to be recorded as a
    // cutter's choice.
    let model = block_model(14.0, 9.0, 11.0);
    let settings = settings_with(2, 0.2, 0.1, 0.5);
    // A slanted axis: a half turn leaves a line along a coordinate axis where it is, so only a
    // slanted one makes the four poses differ.
    let rough = CrystalOrientation::from_angles(37.0, 123.0, None);
    let rule = rule_of(
        OrientationRuleKind::CustomAngle {
            tilt_deg: 40.0,
            azimuth_deg: Some(200.0),
        },
        5.0,
        HALF_PERCENT,
        rough,
    );
    assert!(
        rule.assignment_poses().iter().any(|&index| index != 0),
        "the rule must make some assignment prefer a turned pose"
    );
    let designs = with_rule(&[tall_design(1), box_design(2)], Some(rule));
    let input = PlanInput {
        model: &model,
        settings: &settings,
        designs: &designs,
        hulls: &[],
    };
    let layouts = plan(&input, &mut |_| true).expect("not cancelled");
    assert_ne!(layouts, [] as [crate::rough_plan::types::RoughLayout; 0]);
    for layout in layouts.iter().filter(|layout| !layout.exact_fit) {
        for stone in &layout.stones {
            let (best, index) = rule.deviation_with_pose(&stone.pose.axes);
            // The stone already stands in its best pose: no other pose does strictly better.
            assert_eq!(index, 0, "{stone:?}");
            assert_eq!(rule.deviation_for_axes(&stone.pose.axes), best);
        }
    }
}

fn box_hull(entry_id: i64, width: f64, height: f64, length: f64) -> DesignHull {
    let mut vertices = Vec::new();
    for x in [-0.5, 0.5] {
        for y in [-0.5, 0.5] {
            for z in [-0.5, 0.5] {
                vertices.push([x * width, y * height, z * length]);
            }
        }
    }
    DesignHull {
        entry_id,
        vertices,
        volume: width * height * length,
        width,
    }
}

fn block_model(x: f64, y: f64, z: f64) -> RoughModel {
    RoughModel::new(
        RoughBase::Block {
            x_mm: x,
            y_mm: y,
            z_mm: z,
        },
        Vec::new(),
    )
}

#[test]
fn the_exact_fit_stays_in_the_cone_of_a_required_rule() {
    let model = block_model(12.0, 12.0, 12.0);
    let region = model.usable_halfspaces(0.0).expect("region");
    let coarse = model.coarse_usable_halfspaces(0.0).expect("coarse region");
    let settings = settings_with(1, 0.0, 0.0, 0.5);
    let up = CrystalOrientation::from_angles(0.0, 0.0, None);
    let rule = rule_of(
        OrientationRuleKind::TableAcross,
        15.0,
        RuleStrength::Required,
        up,
    );
    let hulls = [box_hull(1, 1.0, 1.0, 3.0)];
    let fits = fit_single_stones_ruled(
        &region,
        &coarse,
        &hulls,
        &settings,
        3,
        None,
        Some(&rule),
        &mut |_| true,
    )
    .expect("not cancelled");
    assert_ne!(fits, [] as [crate::rough_plan::fit::SingleFit; 0]);
    for fit in &fits {
        let normal = DVec3::from_array(fit.pose.axes[1]);
        let tilt = normal.dot(DVec3::Y).abs().min(1.0).acos().to_degrees();
        assert!(
            tilt <= 15.0 + 1e-6,
            "table normal {tilt} degrees off the axis"
        );
    }
}

#[test]
fn a_tight_required_cone_still_finds_its_fit_off_the_grid() {
    // The axis is slanted and the cone 2 degrees wide: no direction of the coarse grids lies in it,
    // so the fit relies on the rule's own table normal being added to the tables.
    let model = block_model(12.0, 12.0, 12.0);
    let region = model.usable_halfspaces(0.0).expect("region");
    let coarse = model.coarse_usable_halfspaces(0.0).expect("coarse region");
    let settings = settings_with(1, 0.0, 0.0, 0.5);
    let slanted = CrystalOrientation::from_angles(37.0, 123.0, None);
    let rule = rule_of(
        OrientationRuleKind::TableAcross,
        2.0,
        RuleStrength::Required,
        slanted,
    );
    let hulls = [box_hull(1, 1.0, 1.0, 3.0), box_hull(2, 2.0, 1.0, 2.0)];
    let fits = fit_single_stones_ruled(
        &region,
        &coarse,
        &hulls,
        &settings,
        3,
        None,
        Some(&rule),
        &mut |_| true,
    )
    .expect("not cancelled");
    assert_eq!(fits.len(), hulls.len(), "every design fits inside the cone");
    for fit in &fits {
        let deviation = rule
            .deviation_for_axes(&fit.pose.axes)
            .expect("a proper pose");
        assert!(deviation <= 2.0 + 1e-6, "{deviation} degrees off the axis");
        assert!(fit.volume_mm3 > 0.0);
    }
}

#[test]
fn a_preferred_rule_keeps_the_in_cone_fit_over_a_bigger_off_axis_one() {
    let up = CrystalOrientation::from_angles(0.0, 0.0, None);
    let rule = rule_of(OrientationRuleKind::TableAcross, 15.0, HALF_PERCENT, up);
    let fit = |axes: [[f64; 3]; 3], volume_mm3: f64| SingleFit {
        entry_id: 1,
        pose: StonePose {
            center_mm: [0.0; 3],
            axes,
            mm_per_unit: 1.0,
        },
        volume_mm3,
        carat: volume_mm3,
    };
    // Orient 2 has its table on y (in the cone); orient 0 has it on z (90 degrees off, factor
    // 0.625).
    let inside = fit(assignment_axes(&ASSIGNMENTS[2]), 8.0);
    let outside = fit(assignment_axes(&ASSIGNMENTS[0]), 10.0);
    let kept = merge_fits_scored(vec![outside.clone(), inside.clone()], 1, &|_| Some(rule));
    assert_eq!(kept.len(), 1);
    assert!((kept[0].volume_mm3 - 8.0).abs() < 1e-12, "{kept:?}");
    // Without a rule the bigger one wins.
    let kept = merge_fits_scored(vec![outside, inside], 1, &|_| None);
    assert!((kept[0].volume_mm3 - 10.0).abs() < 1e-12);
}

#[test]
fn rescaling_a_pose_keeps_the_axes_and_finds_the_largest_scale() {
    let model = block_model(3.0, 3.0, 3.0);
    let region = model.usable_halfspaces(0.0).expect("region");
    let settings = settings_with(1, 0.0, 0.0, 0.01);
    let hull = box_hull(1, 1.0, 1.0, 1.0);
    let identity = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let fit = rescale_pose(&region, &hull, identity, &settings, None).expect("feasible");
    assert!(
        (fit.pose.mm_per_unit - 3.0).abs() < 1e-6,
        "{}",
        fit.pose.mm_per_unit
    );
    assert_eq!(fit.pose.axes, identity);

    // Turned 45 degrees about y the cube's footprint is sqrt(2) wide.
    let (s, c) = 45f64.to_radians().sin_cos();
    let turned = [[c, 0.0, -s], [0.0, 1.0, 0.0], [s, 0.0, c]];
    let fit = rescale_pose(&region, &hull, turned, &settings, None).expect("feasible");
    assert!(
        (fit.pose.mm_per_unit - 3.0 / 2f64.sqrt()).abs() < 1e-6,
        "{}",
        fit.pose.mm_per_unit
    );
}

#[test]
fn rescaling_with_a_locked_piece_keeps_the_stone_out_of_its_shell() {
    let settings = settings_with(1, 0.0, 0.0, 0.01);
    let model = block_model(6.0, 6.0, 6.0);
    let hull = box_hull(1, 1.0, 1.0, 1.0);
    let identity = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

    let whole = model.usable_halfspaces(0.0).expect("region");
    let open = rescale_pose(&whole, &hull, identity, &settings, None).expect("feasible");
    assert!((open.pose.mm_per_unit - 6.0).abs() < 1e-6);

    // A 2 mm piece locked in the corner at the origin: the rough with the piece left out is a mesh
    // rough, and the largest cube that avoids the piece is 4 mm.
    let locked_stone = stone_at_origin(2.0);
    let locked = rough_with_locked_pieces(&model, &[locked_stone]).expect("the piece locks");
    let ctx = crate::rough_plan::shaped::ShapedCtx::new(&locked.model, &settings).expect("a mesh");
    let kept = rescale_pose(&ctx.usable, &hull, identity, &settings, ctx.fit_mesh())
        .expect("a smaller stone still fits");
    assert!(
        kept.pose.mm_per_unit < open.pose.mm_per_unit - 1.0,
        "{} vs {}",
        kept.pose.mm_per_unit,
        open.pose.mm_per_unit
    );
    // The stone (a cube of its scale, its axes the identity) does not meet the locked piece. The
    // model frame of the locked rough is the original one here (offset zero).
    let half = kept.pose.mm_per_unit * 0.5;
    let stone = PlacedStone {
        stone_size_mm: [kept.pose.mm_per_unit; 3],
        pose: kept.pose,
        ..stone_at_origin(2.0)
    };
    assert!(
        half > 0.0 && apart(&stone, &locked_stone, 1e-3),
        "{stone:?}"
    );
}

#[test]
fn a_rules_passes_are_its_cone_and_for_a_preferred_rule_also_the_open_fit() {
    let up = CrystalOrientation::from_angles(0.0, 0.0, None);
    let required = rule_of(
        OrientationRuleKind::TableAcross,
        15.0,
        RuleStrength::Required,
        up,
    );
    let preferred = rule_of(OrientationRuleKind::TableAcross, 15.0, HALF_PERCENT, up);
    assert_eq!(rule_passes(None), vec![None]);
    assert_eq!(rule_passes(Some(&required)), vec![Some(required)]);
    assert_eq!(
        rule_passes(Some(&preferred)),
        vec![Some(preferred.as_required()), None]
    );

    // The chunk-level pieces the window runs equal the sequential fit.
    let model = block_model(12.0, 12.0, 12.0);
    let region = model.usable_halfspaces(0.0).expect("region");
    let coarse = model.coarse_usable_halfspaces(0.0).expect("coarse region");
    let settings = settings_with(1, 0.0, 0.0, 0.5);
    let hulls = [box_hull(1, 1.0, 1.0, 3.0), box_hull(2, 2.0, 1.0, 2.0)];
    for rule in [&required, &preferred] {
        let expected = fit_single_stones_ruled(
            &region,
            &coarse,
            &hulls,
            &settings,
            3,
            None,
            Some(rule),
            &mut |_| true,
        )
        .expect("not cancelled");
        let mut passes = Vec::new();
        for cone in rule_passes(Some(rule)) {
            let scores =
                screen_designs_in_cone(&coarse, &hulls, None, cone.as_ref(), &mut |_| true)
                    .expect("not cancelled");
            assert_eq!(scores.len(), hulls.len());
            let fits = fit_shortlisted_in_cone(
                &region,
                &hulls,
                &settings,
                None,
                cone.as_ref(),
                &mut |_| true,
            )
            .expect("not cancelled");
            passes.push(merge_fits(fits, 3));
        }
        assert_eq!(merge_rule_passes(Some(rule), passes, 3), expected);
    }
}

/// A stone with a sawn piece of `size` mm at the origin of the rough, as the planner places one.
fn stone_at_origin(size: f64) -> PlacedStone {
    PlacedStone {
        entry_id: 1,
        piece_origin_mm: [0.0; 3],
        piece_size_mm: [size; 3],
        stone_size_mm: [size; 3],
        table_axis: Axis::Y,
        carat: 1.0,
        volume_mm3: size * size * size,
        pose: StonePose {
            center_mm: [size * 0.5; 3],
            axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            mm_per_unit: size,
        },
    }
}

#[test]
fn re_fitting_a_block_piece_keeps_the_piece_and_honours_the_assignment() {
    let settings = settings_with(1, 0.0, 0.1, 0.5);
    let stone = stone_at_origin(3.0);
    let design = tall_design(1);
    for orient in [2, 3] {
        let Some(refit) = refit_block_piece(&stone, &design, Some(orient), &settings) else {
            continue;
        };
        assert_eq!(refit.piece_origin_mm, stone.piece_origin_mm);
        assert_eq!(refit.piece_size_mm, stone.piece_size_mm);
        assert_eq!(refit.table_axis, Axis::from_index(ASSIGNMENTS[orient][2]));
    }
    // Something that is not an assignment is refused.
    assert!(refit_block_piece(&stone, &design, Some(9), &settings).is_none());
    // The pose of a choice is the planner's own unless the cutter picked another.
    assert_eq!(StoneChoice::default().pose_for(&stone.pose), stone.pose);
}

/// Under an azimuth rule a re-fit takes the pose the rule credited, as a planned layout does; an
/// explicit assignment is the cutter's own and stays as placed.
#[test]
fn re_fitting_under_an_azimuth_rule_takes_the_suggested_pose() {
    let settings = settings_with(1, 0.0, 0.1, 0.5);
    let stone = stone_at_origin(3.0);
    // The axis lies in the x/z plane at azimuth 30; the rule wants the line at azimuth 150, which
    // the planner's own pose of assignment 2 is 60 degrees off and its half turn reaches.
    let rough = CrystalOrientation::from_angles(90.0, 30.0, None);
    let rule = rule_of(
        OrientationRuleKind::CustomAngle {
            tilt_deg: 90.0,
            azimuth_deg: Some(150.0),
        },
        1.0,
        RuleStrength::Required,
        rough,
    );
    assert!(rule.needs_azimuth());
    let design = CandidateDesign {
        orientation: Some(rule),
        ..tall_design(1)
    };

    let automatic = refit_block_piece(&stone, &design, None, &settings).expect("a stone fits");
    let deviation = rule
        .deviation_for_axes(&automatic.pose.axes)
        .expect("a proper pose");
    assert!(deviation < 1e-6, "turned to the target, got {deviation}");

    if let Some(own) = refit_block_piece(&stone, &design, Some(2), &settings) {
        let deviation = rule
            .deviation_for_axes(&own.pose.axes)
            .expect("a proper pose");
        assert!(
            (deviation - 60.0).abs() < 1e-6,
            "an explicit assignment keeps the planner's pose, got {deviation}"
        );
    }
}

/// Whether the finished stones' bounding boxes are apart along some axis (up to `slack` mm).
fn apart(a: &PlacedStone, b: &PlacedStone, slack: f64) -> bool {
    let span = |stone: &PlacedStone, axis: usize| {
        let half = stone.stone_size_mm[axis] * 0.5;
        (
            stone.pose.center_mm[axis] - half,
            stone.pose.center_mm[axis] + half,
        )
    };
    (0..3).any(|axis| {
        let ((a_lo, a_hi), (b_lo, b_hi)) = (span(a, axis), span(b, axis));
        a_lo >= b_hi - slack || b_lo >= a_hi - slack
    })
}

#[test]
fn locking_a_stone_on_a_block_replans_on_the_shaped_path_and_keeps_its_piece_out() {
    let settings = settings_with(1, 0.0, 0.0, 1.0);
    let model = block_model(6.0, 6.0, 6.0);
    let locked_stone = stone_at_origin(2.0);

    let locked = rough_with_locked_pieces(&model, &[locked_stone]).expect("the piece locks");
    assert!(
        locked.offset_mm.iter().all(|v| v.abs() < 1e-9),
        "{:?}",
        locked.offset_mm
    );
    assert!(matches!(locked.model.base, RoughBase::Hull { .. }));
    let designs = [box_design(1)];
    let input = PlanInput {
        model: &locked.model,
        settings: &settings,
        designs: &designs,
        hulls: &[],
    };
    assert_eq!(input.path(), PlanPath::Shaped);
    let replanned = plan(&input, &mut |_| true).expect("not cancelled");
    assert_ne!(replanned, [] as [crate::rough_plan::types::RoughLayout; 0]);
    for stone in &replanned[0].stones {
        assert!(
            apart(stone, &locked_stone, 1e-3),
            "{stone:?} meets {locked_stone:?}"
        );
    }
    assert!(LOCKED_FIRST_NOTE.contains("cut first"));

    // The shell is the piece: a box (eight corners) of the piece's size, less the inset.
    let halfspaces = model.halfspaces().expect("halfspaces");
    let shell = locked_shell(&locked_stone, &halfspaces, [0.0; 3]).expect("a solid");
    assert_eq!(shell.vertices().len(), 8);

    // Nothing locked: the model is returned as it is.
    let unchanged = rough_with_locked_pieces(&model, &[]).expect("nothing to lock");
    assert_eq!(unchanged.model, model);
}
