//! The Crystal Axis in a plan run: the rule each design is cut under, the locked stones, the
//! alternative layouts of a Preferred rule, and the single-stone re-fit and turn the stone menu
//! asks for.
//!
//! A plan whose rough has no orientation, or whose rules are all the default, goes through none of
//! this beyond the checks that find there is nothing to do: its layouts are exactly what they were
//! before the axis existed.

use super::{
    PlanJob, candidates_from,
    hulls::{design_placements, prepare_design_hulls},
    scan,
    tracker::Progress,
};
use crate::gui::rough_plan::axis::{layouts::append_locked, material, state::FreeKit};
use pleochroma_cut_core::{
    crystal_axis::CrystalOrientation,
    rough_plan::{
        CandidateDesign, DesignHull, PlacedStone, PlanInput, PlanPath, PlanSettings, RoughLayout,
        RoughModel, SingleFit,
        orientation::{
            OrientationAlternative, PlanSettingsOverrides, ResolveContext, ResolvedRule,
            plan_orientation_alternatives, refit_block_piece, refit_shaped_piece, restore_frame,
            rough_with_locked_pieces,
        },
        shaped::ShapedCtx,
    },
};
use pleochroma_vault::{
    db::sqlite::Database,
    model::{solid_extents::StoredSolidExtents, solid_hull::SolidHull},
};
use std::{collections::BTreeMap, sync::Mutex};

/// The rough a run plans and where it sits: with locked stones the rough has their pieces left
/// out of it (and may have moved to a frame of its own), without it is the job's rough as it is.
pub(super) struct PlanBase {
    /// The rough to plan.
    pub(super) model: RoughModel,
    /// The original frame is this far from the model's: `original = model + offset_mm`.
    pub(super) offset_mm: [f64; 3],
}

/// The rough `job` plans.
///
/// # Errors
///
/// The message for `error_text` when a locked stone's piece cannot be left out of the rough.
pub(super) fn plan_base(job: &PlanJob) -> Result<PlanBase, String> {
    rough_with_locked_pieces(&job.model, &job.locked)
        .map(|locked| PlanBase {
            model: locked.model,
            offset_mm: locked.offset_mm,
        })
        .map_err(|error| error.to_string())
}

/// Gives every design the rule it is cut under (the plan's rule, or its own), so the search sees
/// it. Nothing changes while the rough's orientation is Unknown, the axis is ignored, or the
/// material has no rule. The material is the built-in one of the job's name or the library's (a
/// custom material has a rule of its own), and a design is placed by its own caliper turn and
/// centre (`design_placements`), so a rule with an azimuth target is read in the design's own
/// frame, on top of the four box poses of the stone.
pub(super) fn resolve_designs(
    db: &Mutex<Database>,
    job: &PlanJob,
    designs: &mut [CandidateDesign],
    (stored, hulls): (
        &BTreeMap<i64, StoredSolidExtents>,
        &BTreeMap<i64, SolidHull>,
    ),
) {
    if job.model.orientation.is_none() {
        return;
    }
    let Some(material) = material::resolve(db, &job.material_name) else {
        return;
    };
    let placements = design_placements(stored, hulls);
    job.overrides.resolve_designs(
        designs,
        &ResolveContext {
            rough: job.model.orientation,
            material: &material,
            placements: &placements,
        },
    );
}

/// The rule of every design that has one, by entry id.
#[must_use]
pub(super) fn rules_by_design(designs: &[CandidateDesign]) -> BTreeMap<i64, ResolvedRule> {
    designs
        .iter()
        .filter_map(|design| design.orientation.map(|rule| (design.entry_id, rule)))
        .collect()
}

/// The alternatives of a Preferred rule (Best Yield, Best Orientation, Balanced), for the cut
/// order of the plan's best layout. None for a plan without a Preferred rule. `None` when the
/// plan was stopped.
pub(super) fn alternatives(
    input: &PlanInput<'_>,
    layouts: &[RoughLayout],
    lanes: usize,
    progress: &dyn Progress,
) -> Option<Vec<OrientationAlternative>> {
    let Some(best) = layouts.first() else {
        return Some(Vec::new());
    };
    plan_orientation_alternatives(input, best.cut_order, lanes, &mut |event| {
        progress.event(event)
    })
}

/// The layouts of a finished plan as the window shows them: the alternatives after the plan's own
/// (each with its label; one that repeats a layout is left out), everything back in the rough's
/// own frame, and the locked stones listed in every layout with the totals counting them.
/// Returns the layouts and the label of each ("" for the plan's own).
#[must_use]
pub(super) fn assemble(
    layouts: Vec<RoughLayout>,
    alternatives: Vec<OrientationAlternative>,
    base: &PlanBase,
    locked: &[PlacedStone],
    rough_volume_mm3: Option<f64>,
) -> (Vec<RoughLayout>, Vec<String>) {
    let mut labels = vec![String::new(); layouts.len()];
    let mut all = layouts;
    for alternative in alternatives {
        if all.contains(&alternative.layout) {
            continue;
        }
        // The alternatives are the best layout of one cut order and are not refined as the plan's
        // own layouts are, so the label says so.
        labels.push(format!("{} (Unrefined)", alternative.kind.label()));
        all.push(alternative.layout);
    }
    if !locked.is_empty() {
        for layout in &mut all {
            restore_frame(layout, base.offset_mm);
            append_locked(layout, locked, rough_volume_mm3);
        }
    }
    (all, labels)
}

/// A design the stone menu fits again: the candidate (carrying the rule it was planned under) and
/// the outline the exact fit needs.
pub(in crate::gui::rough_plan) struct DesignKit {
    /// The design as the planner sees it.
    pub(in crate::gui::rough_plan) design: CandidateDesign,
    /// Its convex outline in its caliper frame.
    pub(in crate::gui::rough_plan) hull: DesignHull,
}

/// Reads design `entry_id` and its outline from the library cache and gives it the rule it was
/// planned under (`overrides` with the planned rough `rough`; the material `material_name` is
/// built in or the library's). Run it off the UI thread: it waits for the database.
///
/// # Errors
///
/// The message for the window when the design has no usable cached measurement or outline (a
/// plan run measures them; a design that changed since may have none).
pub(in crate::gui::rough_plan) fn load_design_kit(
    db: &Mutex<Database>,
    entry_id: i64,
    rough: Option<CrystalOrientation>,
    overrides: &PlanSettingsOverrides,
    material_name: &str,
) -> Result<DesignKit, String> {
    let (stored, hulls) = scan::load_extents_and_hulls(db, &[entry_id])?;
    let mut design = candidates_from(&stored).into_iter().next().ok_or_else(|| {
        "This design has no usable measurement; plan again to measure it.".to_string()
    })?;
    let hull = prepare_design_hulls(&stored, &hulls)
        .hulls
        .into_iter()
        .next()
        .ok_or_else(|| "This design has no usable outline to fit again.".to_string())?;
    if let Some(material) = material::resolve(db, material_name) {
        overrides.resolve_designs(
            std::slice::from_mut(&mut design),
            &ResolveContext {
                rough,
                material: &material,
                placements: &design_placements(&stored, &hulls),
            },
        );
    }
    Ok(DesignKit { design, hull })
}

/// Whether `model` is planned by the plain block planner (otherwise by the shaped one).
fn is_plain_block(model: &RoughModel, settings: &PlanSettings) -> bool {
    PlanInput {
        model,
        settings,
        designs: &[],
        hulls: &[],
    }
    .path()
        == PlanPath::PlainBlock
}

/// The best stone of the design in the piece of `stone` under assignment `orient` (one of the six
/// table assignments), or under the best one the design's rule allows when `orient` is `None`:
/// the piece and its saw cuts stay, the stone is new. `None` when no stone of at least the
/// minimum width fits.
#[must_use]
pub(in crate::gui::rough_plan) fn refit(
    kit: &DesignKit,
    model: &RoughModel,
    settings: &PlanSettings,
    stone: &PlacedStone,
    orient: Option<usize>,
) -> Option<PlacedStone> {
    if is_plain_block(model, settings) {
        refit_block_piece(stone, &kit.design, orient, settings)
    } else {
        let ctx = ShapedCtx::new(model, settings).ok()?;
        refit_shaped_piece(&ctx, settings, &kit.design, stone, orient)
    }
}

/// What turning the stone of an exact fit by hand needs: the design's outline, the rough's usable
/// region and mesh, the settings, the rough's volume, and the stones of the layout after the first
/// (the locked ones), which the layout keeps.
///
/// With locked stones the region and the mesh are those of the rough with the locked pieces left
/// out (their shells are inclusions), so a turned stone keeps out of them; the kit then also holds
/// the offset between that rough's frame and the original one.
///
/// # Errors
///
/// The message for the window when the rough cannot be measured or a locked piece cannot be left
/// out of it.
pub(in crate::gui::rough_plan) fn free_kit(
    kit: &DesignKit,
    model: &RoughModel,
    settings: &PlanSettings,
    tail: Vec<PlacedStone>,
) -> Result<FreeKit, String> {
    let whole = ShapedCtx::new(model, settings).map_err(|error| error.to_string())?;
    if tail.is_empty() {
        return Ok(FreeKit {
            hull: kit.hull.clone(),
            region: whole.usable.clone(),
            settings: *settings,
            model_volume_mm3: whole.model_volume,
            ctx: Some(whole),
            tail,
            offset_mm: [0.0; 3],
        });
    }
    let locked = rough_with_locked_pieces(model, &tail).map_err(|error| error.to_string())?;
    let ctx = ShapedCtx::new(&locked.model, settings).map_err(|error| error.to_string())?;
    Ok(FreeKit {
        hull: kit.hull.clone(),
        region: ctx.usable.clone(),
        settings: *settings,
        // The yield counts the whole rough, as the plan's own layouts do.
        model_volume_mm3: whole.model_volume,
        ctx: Some(ctx),
        tail,
        offset_mm: locked.offset_mm,
    })
}

/// `fit` moved from the frame of a [`FreeKit`]'s rough back to the original frame (`offset_mm` is
/// the kit's: `original = kit frame + offset_mm`).
pub(in crate::gui::rough_plan) fn fit_in_original_frame(
    mut fit: SingleFit,
    offset_mm: [f64; 3],
) -> SingleFit {
    for (centre, shift) in fit.pose.center_mm.iter_mut().zip(offset_mm) {
        *centre += shift;
    }
    fit
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma::optics::materials::GemMaterial;
    use pleochroma_cut_core::{
        crystal_axis::{OrientationRule, OrientationRuleKind, RuleStrength},
        rough_plan::{
            Axis, CutOrder, CutPlan, RoughBase,
            fit::StonePose,
            orientation::{AlternativeKind, RuleChoice, RuleSetting},
        },
    };

    fn design(entry_id: i64) -> CandidateDesign {
        CandidateDesign {
            entry_id,
            width: 5.0,
            length: 6.0,
            height: 3.0,
            volume: 60.0,
            orientation: None,
        }
    }

    fn stone(carat: f64) -> PlacedStone {
        PlacedStone {
            entry_id: 1,
            piece_origin_mm: [0.0; 3],
            piece_size_mm: [2.0; 3],
            stone_size_mm: [2.0; 3],
            table_axis: Axis::Y,
            carat,
            volume_mm3: carat * 10.0,
            pose: StonePose {
                center_mm: [1.0; 3],
                axes: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
                mm_per_unit: 1.0,
            },
        }
    }

    fn layout(carat: f64) -> RoughLayout {
        RoughLayout {
            cut_order: CutOrder::Xyz,
            stones: vec![stone(carat)],
            cut_plan: CutPlan { slabs: Vec::new() },
            total_carat: carat,
            total_volume_mm3: carat * 10.0,
            yield_fraction: 0.1,
            exact_fit: true,
        }
    }

    #[test]
    fn only_designs_with_a_rule_are_in_the_rule_map() {
        let material = GemMaterial::by_name("Sapphire").expect("a built-in material");
        let mut designs = vec![design(1), design(2)];
        let mut overrides = PlanSettingsOverrides::default();
        overrides.set_design(
            2,
            RuleSetting {
                choice: RuleChoice::Ignore,
                reason: String::new(),
            },
        );
        overrides.resolve_designs(
            &mut designs,
            &ResolveContext {
                rough: Some(CrystalOrientation::from_angles(10.0, 0.0, None)),
                material: &material,
                placements: &BTreeMap::new(),
            },
        );
        let rules = rules_by_design(&designs);
        assert_eq!(rules.keys().copied().collect::<Vec<_>>(), [1]);
    }

    #[test]
    fn a_plan_without_an_axis_has_no_rules_at_all() {
        let designs = vec![design(1), design(2)];
        assert!(rules_by_design(&designs).is_empty());
    }

    #[test]
    fn the_alternatives_follow_the_plans_own_layouts_with_their_labels() {
        let base = PlanBase {
            model: RoughModel::new(
                RoughBase::Block {
                    x_mm: 10.0,
                    y_mm: 10.0,
                    z_mm: 10.0,
                },
                Vec::new(),
            ),
            offset_mm: [0.0; 3],
        };
        let alternatives = vec![
            OrientationAlternative {
                kind: AlternativeKind::BestYield,
                layout: layout(1.0),
            },
            OrientationAlternative {
                kind: AlternativeKind::BestOrientation,
                layout: layout(2.0),
            },
            OrientationAlternative {
                kind: AlternativeKind::Knee,
                layout: layout(3.0),
            },
        ];
        let (all, labels) = assemble(vec![layout(1.0)], alternatives, &base, &[], Some(100.0));
        // Best Yield repeats the plan's own layout and is left out.
        assert_eq!(all.len(), 3);
        assert_eq!(
            labels,
            ["", "Best Orientation (Unrefined)", "Balanced (Unrefined)"]
        );
    }

    #[test]
    fn locked_stones_are_listed_in_every_layout_and_counted() {
        let base = PlanBase {
            model: RoughModel::new(
                RoughBase::Block {
                    x_mm: 10.0,
                    y_mm: 10.0,
                    z_mm: 10.0,
                },
                Vec::new(),
            ),
            offset_mm: [1.0, 0.0, 0.0],
        };
        let (all, labels) = assemble(
            vec![layout(1.0)],
            Vec::new(),
            &base,
            &[stone(0.5)],
            Some(100.0),
        );
        assert_eq!(labels, [""]);
        assert_eq!(all[0].stones.len(), 2);
        assert!((all[0].total_carat - 1.5).abs() < 1e-12);
        // The planned stone moved back by the offset; the locked one is in the original frame.
        assert!((all[0].stones[0].pose.center_mm[0] - 2.0).abs() < 1e-12);
        assert!((all[0].stones[1].pose.center_mm[0] - 1.0).abs() < 1e-12);
    }

    #[test]
    fn a_block_is_planned_by_the_plain_planner_and_a_cut_block_by_the_shaped_one() {
        let settings = PlanSettings::default();
        let block = RoughModel::new(
            RoughBase::Block {
                x_mm: 10.0,
                y_mm: 8.0,
                z_mm: 6.0,
            },
            Vec::new(),
        );
        assert!(is_plain_block(&block, &settings));
        let cut = RoughModel::new(
            RoughBase::Block {
                x_mm: 10.0,
                y_mm: 8.0,
                z_mm: 6.0,
            },
            vec![pleochroma_cut_core::rough_plan::RoughCut::Face {
                normal: [0.0, 1.0, 0.0],
                depth_mm: 1.0,
            }],
        );
        assert!(!is_plain_block(&cut, &settings));
    }

    #[test]
    fn a_rule_of_the_designs_own_is_resolved_for_the_kit() {
        let rule = OrientationRule {
            kind: OrientationRuleKind::TableAcross,
            tolerance_deg: 10.0,
            strength: RuleStrength::Required,
        };
        let mut overrides = PlanSettingsOverrides::default();
        overrides.set_plan(RuleSetting {
            choice: RuleChoice::Set(rule),
            reason: String::new(),
        });
        let material = GemMaterial::by_name("Sapphire").expect("a built-in material");
        let mut designs = vec![design(7)];
        overrides.resolve_designs(
            &mut designs,
            &ResolveContext {
                rough: Some(CrystalOrientation::from_angles(0.0, 0.0, None)),
                material: &material,
                placements: &BTreeMap::new(),
            },
        );
        assert!(designs[0].orientation.is_some());
    }
}
