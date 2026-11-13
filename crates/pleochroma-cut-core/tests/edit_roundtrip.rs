//! Property tests for the edit vocabulary.
//!
//! Every [`Edit`] variant, applied through
//! [`Design::apply_edit`] and then undone by applying the [`Edit`] it returns, must reproduce
//! the exact original [`Design`]. `#[derive(PartialEq)]` on [`Design`] is the oracle, so no
//! field is compared by hand.
//!
//! The cases come from a small seeded generator over a fixed multi-tier fixture, at several
//! seeds each, so a combination of indices and values that breaks an undo has more chances to
//! show up than one hand-picked example gives it. The hand-picked cases are in `src/edit/tests/`.
//! This is an integration test, so it uses only the crate's public API.
//!
//! [`Lcg`] is that generator. It is deterministic (a fixed seed gives the same sequence on every
//! run, so a failure reproduces without a printed seed) and has no dependencies.

use pleochroma::geometry::meet_solver::MeetConstraint;
use pleochroma_cut_core::{
    ConstraintTier, Design, Edit, History, MaterialSelection, PreformSpec, RemapRounding,
    TierTarget,
    design::{ConcaveTier, ConcaveTool, RelationExpr, TierRelation, ToolMotion},
};

/// A dependency-free, seeded xorshift64* generator, local to this file. It is not
/// cryptographic.
struct Lcg(u64);

impl Lcg {
    const fn new(seed: u64) -> Self {
        // `0` would stay `0` forever under xorshift -- never a seed this file
        // actually passes, but folded in defensively so a future caller can't
        // silently get a degenerate all-zero stream.
        Self(seed | 1)
    }

    const fn next_u64(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }

    /// A `usize` in `0..bound`. `bound == 0` always returns `0` (the caller's
    /// own responsibility to skip an empty range beforehand).
    const fn index(&mut self, bound: usize) -> usize {
        if bound == 0 {
            0
        } else {
            #[expect(
                clippy::cast_possible_truncation,
                reason = "the remainder is below `bound`, which is a usize"
            )]
            let picked = (self.next_u64() % bound as u64) as usize;
            picked
        }
    }

    /// [`Self::index`] as a `u32` (the bound is small in every caller).
    fn index_u32(&mut self, bound: usize) -> u32 {
        u32::try_from(self.index(bound)).expect("a test index below a small bound fits u32")
    }

    /// A finite `f64` in `[lo, hi)`.
    fn range_f64(&mut self, lo: f64, hi: f64) -> f64 {
        let unit = (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
        unit.mul_add(hi - lo, lo)
    }

    const fn bool(&mut self) -> bool {
        self.next_u64() & 1 == 0
    }
}

/// A small round-brilliant-shaped fixture: one girdle tier plus two pavilion and two crown
/// tiers. `Design::from_asc_schedule` pins every imported tier to
/// `MeetConstraint::ScaleReference`, so the design always solves and no generated edit needs
/// to handle a missing anchor. Five tiers give `MoveTier`, `RemapIndices` and
/// `RetargetAngles` interior positions to work on.
fn round_brilliant_fixture() -> Design {
    const ASC: &str = "GemCad 5.0\n\
         g 16 0.0\n\
         y 4 n\n\
         I 1.54\n\
         a 90.000000 1.00000000 0 4 8 12 G Set girdle thickness\n\
         a -42.000000 0.60000000 0 4 8 12 G Set stone size\n\
         a -38.000000 0.55000000 2 6 10 14 G Set stone size\n\
         a 32.000000 0.45000000 0 4 8 12 G Set stone size\n\
         a 40.000000 0.40000000 2 6 10 14 G Set stone size\n";
    let schedule = pleochroma_formats::asc::parse_asc(ASC).expect("fixture .asc must parse");
    let mut design = Design::from_asc_schedule(PreformSpec::block(2.0, 1.0, 2.0), &schedule);
    design.girdle_diameter_mm = Some(6.5);
    design.material = MaterialSelection {
        name: Some("Diamond".to_string()),
        specific_gravity_override: None,
        refractive_index_override: Some(1.54),
        body_color_override: None,
        body_color_bands_override: None,
        absorption_path_scale_override: None,
        color_pattern_override: None,
        color_variant_override: None,
    };
    design
}

/// Applies `edit`, then applies the [`Edit`] it returns (its own computed
/// inverse) -- asserting `design` ends up exactly equal (`PartialEq`) to how
/// it started, tier identities included ([`Design::tier_ids_eq`]). `label` is folded
/// into every panic message so a failing seed names which variant/seed
/// combination broke, not just "assertion failed."
fn assert_round_trips(mut design: Design, edit: &Edit, label: &str) {
    let before = design.clone();
    let inverse = design
        .apply_edit(edit.clone())
        .unwrap_or_else(|e| panic!("{label}: apply failed for {edit:?}: {e}"));
    // Every case is generated to change something: a round trip of a no-op edit
    // would not catch a broken apply, such as an inverse built from the wrong field.
    assert_ne!(design, before, "{label}: edit was a no-op");
    design
        .apply_edit(inverse.clone())
        .unwrap_or_else(|e| panic!("{label}: undo failed for {inverse:?} (from {edit:?}): {e}"));
    assert_eq!(
        design, before,
        "{label}: {edit:?} then its own inverse {inverse:?} did not reproduce the original design"
    );
    assert!(
        design.tier_ids_eq(&before),
        "{label}: {edit:?} then its own inverse {inverse:?} changed which TierId sits in a slot"
    );
}

/// A fresh [`ConstraintTier`] for [`Edit::AddTier`]/[`Edit::ModifyTier`] --
/// varied per seed so a run touches different angles/indices, not the exact
/// same literal tier every time.
fn generated_tier(rng: &mut Lcg) -> ConstraintTier {
    let mast = rng.range_f64(0.2, 0.9);
    ConstraintTier {
        angle_deg: rng.range_f64(-60.0, 60.0),
        name: format!("G{}", rng.index(99)),
        indices: vec![f64::from(rng.index_u32(16))],
        constraint: MeetConstraint::ScaleReference(mast),
        imported_meet: None,
        original_notes: None,
        detached: Vec::new(),
    }
}

const SEEDS: [u64; 5] = [1, 7, 42, 1_000_003, 0xDEAD_BEEF];

#[test]
fn add_tier_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        let index = rng.index(design.tiers.len() + 1); // insertion point, inclusive of the end
        let tier = generated_tier(&mut rng);
        assert_round_trips(design, &Edit::AddTier { index, tier }, "AddTier");
    }
}

#[test]
fn remove_tier_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        let index = rng.index(design.tiers.len());
        assert_round_trips(design, &Edit::RemoveTier { index }, "RemoveTier");
    }
}

/// Also covers a removed tier that carries a cheater offset and a note, whose inverse is a
/// multi-step `Batch` (see `apply_remove_tier`).
#[test]
fn remove_tier_with_cheater_offset_and_note_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let mut design = round_brilliant_fixture();
        let index = rng.index(design.tiers.len());
        design
            .cheater_offsets_deg
            .insert(index, rng.range_f64(-5.0, 5.0));
        design
            .tier_notes
            .insert(index, "polish carefully".to_string());
        assert_round_trips(
            design,
            &Edit::RemoveTier { index },
            "RemoveTier (with offset+note)",
        );
    }
}

/// Undoing a removal brings back the removed tier's EXACT `TierId` (a plain
/// `AddTier` inverse would allocate a fresh one), with the target keyed by that
/// id attached to it again and every other tier's id untouched.
#[test]
fn undoing_a_tier_removal_restores_the_exact_tier_id() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let mut design = round_brilliant_fixture();
        let index = rng.index(design.tiers.len());
        let removed_id = design
            .tier_id_at(index)
            .expect("a design imported from .asc gives every tier an id");
        let target = TierTarget::DepthMm(1.25);
        design
            .apply_edit(Edit::SetTierTarget {
                index,
                target: Some(target),
            })
            .expect("the fixture has a tier at this index");
        let before = design.clone();

        let inverse = design
            .apply_edit(Edit::RemoveTier { index })
            .expect("the index is in range");
        assert_eq!(
            design.index_of_tier_id(removed_id),
            None,
            "seed {seed}: the removed id must leave the design"
        );
        design.apply_edit(inverse).expect("undo of a removal");

        assert_eq!(
            design.tier_id_at(index),
            Some(removed_id),
            "seed {seed}: undo must put the same TierId back in the same slot"
        );
        assert!(
            design.tier_ids_eq(&before),
            "seed {seed}: every other tier keeps its id"
        );
        assert_eq!(
            design.tier_target(index),
            Some(target),
            "seed {seed}: the target keyed by the id is attached again"
        );
    }
}

/// A tier pushed straight onto `tiers` has no `TierId` (so a target, which is keyed by
/// id, cannot be set on it); `ensure_tier_ids` gives it one without disturbing the
/// ids already there, and a second call changes nothing.
#[test]
fn ensure_tier_ids_seeds_missing_ids_and_keeps_existing_ones() {
    let mut design = round_brilliant_fixture();
    let existing: Vec<_> = (0..design.tiers.len())
        .map(|index| design.tier_id_at(index))
        .collect();
    let seeded = generated_tier(&mut Lcg::new(3));
    design.tiers.push(seeded);
    let new_index = design.tiers.len() - 1;
    assert_eq!(design.tier_id_at(new_index), None, "pushed without an id");
    assert!(
        design
            .apply_edit(Edit::SetTierTarget {
                index: new_index,
                target: Some(TierTarget::DepthMm(1.0)),
            })
            .is_err(),
        "a target needs the tier's id"
    );

    design.ensure_tier_ids();
    for (index, id) in existing.iter().enumerate() {
        assert_eq!(design.tier_id_at(index), *id, "tier {index} keeps its id");
    }
    let new_id = design
        .tier_id_at(new_index)
        .expect("the new tier has an id");
    assert!(
        existing.iter().all(|id| *id != Some(new_id)),
        "and it is not one already in use"
    );
    design
        .apply_edit(Edit::SetTierTarget {
            index: new_index,
            target: Some(TierTarget::DepthMm(1.0)),
        })
        .expect("the seeded tier can now carry a target");

    let ids_after: Vec<_> = (0..design.tiers.len())
        .map(|index| design.tier_id_at(index))
        .collect();
    design.ensure_tier_ids();
    let ids_again: Vec<_> = (0..design.tiers.len())
        .map(|index| design.tier_id_at(index))
        .collect();
    assert_eq!(ids_after, ids_again, "a second call is a no-op");
}

/// The same identity guarantee through `History`: undo, redo and a second undo of
/// a removal each leave the exact ids the matching point in time had.
#[test]
fn history_undo_and_redo_of_a_removal_keep_the_tier_ids() {
    let mut design = round_brilliant_fixture();
    let mut history = History::new();
    let before = design.clone();

    history
        .apply(&mut design, Edit::RemoveTier { index: 1 })
        .expect("the index is in range");
    let after_removal = design.clone();

    assert!(history.undo(&mut design).expect("undo replays"));
    assert!(design.tier_ids_eq(&before), "undo restores the ids");
    assert!(history.redo(&mut design).expect("redo replays"));
    assert!(
        design.tier_ids_eq(&after_removal),
        "redo removes the same tier again"
    );
    assert!(history.undo(&mut design).expect("second undo replays"));
    assert!(
        design.tier_ids_eq(&before),
        "a second undo still restores the original ids"
    );
}

#[test]
fn move_tier_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        let tier_count = design.tiers.len();
        let from = rng.index(tier_count);
        // A move to itself is a no-op (`assert_round_trips` requires a real
        // change) -- resample until `to != from`, bounded by `tier_count`
        // itself so this can never loop forever on a design with only one tier.
        let mut to = rng.index(tier_count);
        for _ in 0..tier_count {
            if to != from {
                break;
            }
            to = rng.index(tier_count);
        }
        if to == from {
            continue; // exhausted the resample budget above; try the next seed.
        }
        assert_round_trips(design, &Edit::MoveTier { from, to }, "MoveTier");
    }
}

#[test]
fn modify_tier_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        let index = rng.index(design.tiers.len());
        let tier = generated_tier(&mut rng);
        assert_round_trips(design, &Edit::ModifyTier { index, tier }, "ModifyTier");
    }
}

#[test]
fn set_constraint_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        let index = rng.index(design.tiers.len());
        let constraint = if rng.bool() {
            MeetConstraint::MeetExisting
        } else {
            MeetConstraint::ScaleReference(rng.range_f64(0.1, 1.0))
        };
        assert_round_trips(
            design,
            &Edit::SetConstraint { index, constraint },
            "SetConstraint",
        );
    }
}

/// [`Edit::SetIndices`] round trip.
#[test]
fn set_indices_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        let index = rng.index(design.tiers.len());
        let indices = vec![f64::from(rng.index_u32(16)), f64::from(rng.index_u32(16))];
        let detached = vec![f64::from(rng.index_u32(16))];
        assert_round_trips(
            design,
            &Edit::SetIndices {
                index,
                indices,
                detached,
            },
            "SetIndices",
        );
    }
}

#[test]
fn set_preform_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        let preform = PreformSpec::block(
            rng.range_f64(1.0, 3.0),
            rng.range_f64(0.5, 2.0),
            rng.range_f64(1.0, 3.0),
        );
        assert_round_trips(design, &Edit::SetPreform { preform }, "SetPreform");
    }
}

/// [`Edit::SetPreformYOffset`] round trip. The native file round trip is a separate test,
/// `preform_y_offset_round_trips_through_save_and_load` in `native/tests/`.
#[test]
fn set_preform_y_offset_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        let y_offset = rng.range_f64(-1.0, 1.0);
        assert_round_trips(
            design,
            &Edit::SetPreformYOffset { y_offset },
            "SetPreformYOffset",
        );
    }
}

#[test]
fn set_girdle_diameter_mm_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        let girdle_diameter_mm = Some(rng.range_f64(3.0, 12.0));
        assert_round_trips(
            design,
            &Edit::SetGirdleDiameterMm { girdle_diameter_mm },
            "SetGirdleDiameterMm",
        );
    }
}

#[test]
fn set_material_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        let material = MaterialSelection {
            name: Some(if rng.bool() { "Sapphire" } else { "Ruby" }.to_string()),
            specific_gravity_override: None,
            refractive_index_override: Some(rng.range_f64(1.5, 1.9)),
            body_color_override: None,
            body_color_bands_override: None,
            absorption_path_scale_override: None,
            color_pattern_override: None,
            color_variant_override: None,
        };
        assert_round_trips(
            design,
            &Edit::SetMaterial {
                material: Box::new(material),
            },
            "SetMaterial",
        );
    }
}

#[test]
fn set_meta_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        let edit = Edit::SetMeta {
            headers: vec![format!("Design #{}", rng.index(9999))],
            footnotes: vec!["cut by a property test".to_string()],
            gear_reference_angle: rng.range_f64(0.0, 22.5),
        };
        assert_round_trips(design, &edit, "SetMeta");
    }
}

#[test]
fn set_cheater_offset_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        let index = rng.index(design.tiers.len());
        let offset_deg = Some(rng.range_f64(-5.0, 5.0));
        assert_round_trips(
            design,
            &Edit::SetCheaterOffset { index, offset_deg },
            "SetCheaterOffset",
        );
    }
}

#[test]
fn set_tier_note_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        let index = rng.index(design.tiers.len());
        let note = Some(format!("note #{}", rng.index(9999)));
        assert_round_trips(design, &Edit::SetTierNote { index, note }, "SetTierNote");
    }
}

/// [`Edit::SetTierTarget`] round trip.
#[test]
fn set_tier_target_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        let index = rng.index(design.tiers.len());
        let target = Some(match rng.index(3) {
            0 => TierTarget::DepthMm(rng.range_f64(0.1, 2.0)),
            1 => TierTarget::GirdleThicknessMm(rng.range_f64(0.05, 0.5)),
            _ => TierTarget::TableWidthMm(rng.range_f64(1.0, 5.0)),
        });
        assert_round_trips(
            design,
            &Edit::SetTierTarget { index, target },
            "SetTierTarget",
        );
    }
}

/// A relation that makes the tier at `index` follow another tier of `design` by a
/// generated offset, for the [`Edit::SetTierRelation`] tests.
fn generated_relation(design: &Design, index: usize, rng: &mut Lcg) -> TierRelation {
    let count = design.tiers.len();
    // Always a DIFFERENT tier, so the relation never reads its own tier.
    let reader = (index + 1 + rng.index(count - 1)) % count;
    let reference = design
        .tier_id_at(reader)
        .expect("a design imported from .asc gives every tier an id");
    TierRelation::new(RelationExpr::offset_from(
        reference,
        rng.range_f64(-5.0, 5.0),
    ))
}

/// [`Edit::SetTierRelation`] -- setting a relation and undoing it.
#[test]
fn set_tier_relation_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        let index = rng.index(design.tiers.len());
        let relation = Some(generated_relation(&design, index, &mut rng));
        assert_round_trips(
            design,
            &Edit::SetTierRelation { index, relation },
            "SetTierRelation",
        );
    }
}

/// Replacing a relation, and clearing one, each undo back to the previous relation.
#[test]
fn replacing_and_clearing_a_tier_relation_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let mut design = round_brilliant_fixture();
        let index = rng.index(design.tiers.len());
        let first = generated_relation(&design, index, &mut rng);
        design
            .apply_edit(Edit::SetTierRelation {
                index,
                relation: Some(first),
            })
            .expect("the index is in range");
        let second = generated_relation(&design, index, &mut rng);
        assert_round_trips(
            design.clone(),
            &Edit::SetTierRelation {
                index,
                relation: Some(second),
            },
            "SetTierRelation (replace)",
        );
        assert_round_trips(
            design,
            &Edit::SetTierRelation {
                index,
                relation: None,
            },
            "SetTierRelation (clear)",
        );
    }
}

/// Removing a tier that follows a relation takes the relation with it, and undoing
/// the removal puts both back -- the relation stays attached to the same `TierId`.
#[test]
fn removing_a_driven_tier_round_trips_with_its_relation() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let mut design = round_brilliant_fixture();
        let index = rng.index(design.tiers.len());
        let relation = generated_relation(&design, index, &mut rng);
        design
            .apply_edit(Edit::SetTierRelation {
                index,
                relation: Some(relation.clone()),
            })
            .expect("the index is in range");
        let driven_id = design.tier_id_at(index).expect("the tier has an id");

        let mut removed = design.clone();
        let inverse = removed
            .apply_edit(Edit::RemoveTier { index })
            .expect("the index is in range");
        assert!(
            removed.tier_relation_for_id(driven_id).is_none(),
            "seed {seed}: the removed tier's relation leaves with it"
        );
        removed.apply_edit(inverse).expect("undo of a removal");
        assert_eq!(removed, design, "seed {seed}: undo restores the design");
        assert_eq!(
            removed.tier_relation_for_id(driven_id),
            Some(&relation),
            "seed {seed}: the relation is attached to the same tier again"
        );
        assert_round_trips(design, &Edit::RemoveTier { index }, "RemoveTier (driven)");
    }
}

#[test]
fn set_schedule_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        let edit = Edit::SetSchedule {
            gear_teeth: 8 + i32::try_from(rng.index(56)).unwrap_or(8),
            symmetry_order: 2 + u32::try_from(rng.index(6)).unwrap_or(2),
            mirror: rng.bool(),
        };
        assert_round_trips(design, &edit, "SetSchedule");
    }
}

/// [`Edit::RemapIndices`]. Its inverse is [`Edit::RestoreIndices`], a verbatim snapshot
/// rather than a reverse remap (a lossy ratio cannot be reversed), so here the edit and its
/// inverse are different variants.
#[test]
fn remap_indices_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        let from_gear = design.meta.gear_teeth;
        let to_gear = [8, 12, 20, 24, 32][rng.index(5)];
        let rounding = [
            RemapRounding::Nearest,
            RemapRounding::Floor,
            RemapRounding::Ceil,
        ][rng.index(3)];
        assert_round_trips(
            design,
            &Edit::RemapIndices {
                from_gear,
                to_gear,
                rounding,
            },
            "RemapIndices",
        );
    }
}

/// [`Edit::RestoreIndices`] applied directly. Its inverse is again a `RestoreIndices`, with
/// the two snapshots swapped.
#[test]
fn restore_indices_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        let index = rng.index(design.tiers.len());
        let tiers = vec![(index, vec![f64::from(rng.index_u32(16))], Vec::new())];
        assert_round_trips(design, &Edit::RestoreIndices { tiers }, "RestoreIndices");
    }
}

#[path = "edit_roundtrip/retarget_and_concave.rs"]
mod retarget_and_concave;
