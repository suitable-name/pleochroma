use super::*;

/// This item's own named target #2: [`Edit::RetargetAngles`], single- and
/// multi-tier.
#[test]
fn retarget_angles_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        let index = rng.index(design.tiers.len());
        let old_deg = design.tiers[index].angle_deg;
        let new_deg = old_deg + rng.range_f64(1.0, 10.0);
        assert_round_trips(
            design,
            &Edit::RetargetAngles {
                changes: vec![(index, old_deg, new_deg)],
            },
            "RetargetAngles (single)",
        );
    }
}

#[test]
fn retarget_angles_multi_tier_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        // Two DISTINCT indices -- the repeated-index case is covered separately
        // below; this is the ordinary multi-tier case.
        let first = rng.index(design.tiers.len());
        let second = (first + 1) % design.tiers.len();
        if first == second {
            continue; // a one-tier fixture: nothing else to retarget alongside it.
        }
        let changes = vec![
            (
                first,
                design.tiers[first].angle_deg,
                design.tiers[first].angle_deg + rng.range_f64(1.0, 5.0),
            ),
            (
                second,
                design.tiers[second].angle_deg,
                design.tiers[second].angle_deg - rng.range_f64(1.0, 5.0),
            ),
        ];
        assert_round_trips(
            design,
            &Edit::RetargetAngles { changes },
            "RetargetAngles (multi)",
        );
    }
}

/// The SAME tier index named twice in one `RetargetAngles` must still
/// round-trip -- the inverse has to unwind in the exact reverse order it was
/// applied in, not the order the caller wrote it in.
#[test]
fn retarget_angles_same_tier_twice_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        let index = rng.index(design.tiers.len());
        let start = design.tiers[index].angle_deg;
        let mid = start + rng.range_f64(1.0, 5.0);
        let end = mid + rng.range_f64(1.0, 5.0);
        let changes = vec![(index, start, mid), (index, mid, end)];
        assert_round_trips(
            design,
            &Edit::RetargetAngles { changes },
            "RetargetAngles (repeated index)",
        );
    }
}

/// [`Edit::Batch`]: this crate's own only real-world composite (a retarget's
/// angle changes plus the material change it implies -- see
/// `Edit::describe`'s own `describe_batch` special case) exercised directly,
/// on top of `edit/tests.rs`'s own existing `Batch` coverage for the
/// error/rollback half.
#[test]
fn batch_retarget_and_material_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = round_brilliant_fixture();
        let index = rng.index(design.tiers.len());
        let old_deg = design.tiers[index].angle_deg;
        let new_deg = old_deg + rng.range_f64(1.0, 10.0);
        let edit = Edit::Batch(vec![
            Edit::RetargetAngles {
                changes: vec![(index, old_deg, new_deg)],
            },
            Edit::SetMaterial {
                material: Box::new(MaterialSelection {
                    name: Some("Sapphire".to_string()),
                    specific_gravity_override: None,
                    refractive_index_override: None,
                    body_color_override: None,
                    body_color_bands_override: None,
                    absorption_path_scale_override: None,
                    color_pattern_override: None,
                    color_variant_override: None,
                }),
            },
        ]);
        assert_round_trips(design, &edit, "Batch(RetargetAngles, SetMaterial)");
    }
}

/// A concave tier for the concave edits -- varied per seed. Indices stay inside
/// the fixture's 16-tooth gear and the name never clashes with a flat tier
/// (`generated_tier` names those `G<n>`).
fn generated_concave_tier(rng: &mut Lcg) -> ConcaveTier {
    let count = 1 + rng.index(4);
    ConcaveTier {
        name: format!("Groove {}", rng.index(99)),
        angle_deg: rng.range_f64(-60.0, -10.0),
        indices: (0..count).map(|_| f64::from(rng.index_u32(16))).collect(),
        instructions: String::new(),
        tool: ConcaveTool::Cylinder,
        tool_azimuth_deg: rng.range_f64(0.0, 90.0),
        displacement: [0.0, 0.0, rng.range_f64(0.0, 0.2)],
        diameter_ratio: rng.range_f64(0.2, 0.8),
        tool_angle_deg: None,
        motion: ToolMotion::Reciprocating,
    }
}

/// The round-brilliant fixture plus three concave tiers, added through the edit
/// vocabulary so they carry real ids.
fn concave_fixture(rng: &mut Lcg) -> Design {
    let mut design = round_brilliant_fixture();
    for position in 0..3 {
        let mut tier = generated_concave_tier(rng);
        tier.name = format!("Groove {position}");
        design
            .apply_edit(Edit::AddConcaveTier {
                index: position,
                tier,
            })
            .expect("a valid concave tier appends");
    }
    design
}

#[test]
fn add_concave_tier_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = concave_fixture(&mut rng);
        let index = rng.index(design.concave_tiers.len() + 1);
        let tier = generated_concave_tier(&mut rng);
        assert_round_trips(
            design,
            &Edit::AddConcaveTier { index, tier },
            "AddConcaveTier",
        );
    }
}

#[test]
fn remove_concave_tier_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = concave_fixture(&mut rng);
        let index = rng.index(design.concave_tiers.len());
        assert_round_trips(
            design,
            &Edit::RemoveConcaveTier { index },
            "RemoveConcaveTier",
        );
    }
}

#[test]
fn modify_concave_tier_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = concave_fixture(&mut rng);
        let index = rng.index(design.concave_tiers.len());
        let mut tier = generated_concave_tier(&mut rng);
        tier.name = "Renamed".to_owned();
        assert_round_trips(
            design,
            &Edit::ModifyConcaveTier { index, tier },
            "ModifyConcaveTier",
        );
    }
}

#[test]
fn move_concave_tier_round_trips() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let design = concave_fixture(&mut rng);
        let from = rng.index(design.concave_tiers.len());
        // Always a different slot, so the move is never a no-op.
        let to =
            (from + 1 + rng.index(design.concave_tiers.len() - 1)) % design.concave_tiers.len();
        assert_round_trips(
            design,
            &Edit::MoveConcaveTier { from, to },
            "MoveConcaveTier",
        );
    }
}

#[test]
fn undoing_a_concave_tier_removal_restores_the_exact_tier_id() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let mut design = concave_fixture(&mut rng);
        let index = rng.index(design.concave_tiers.len());
        let ids_before = design.concave_tier_ids.clone();
        assert_eq!(ids_before.len(), design.concave_tiers.len());

        let inverse = design
            .apply_edit(Edit::RemoveConcaveTier { index })
            .expect("the index is in range");
        assert!(
            !design.concave_tier_ids.contains(&ids_before[index]),
            "seed {seed}: the removed id must leave the design"
        );
        design.apply_edit(inverse).expect("undo of a removal");

        assert_eq!(
            design.concave_tier_ids, ids_before,
            "seed {seed}: undo must put every concave TierId back in its slot"
        );
    }
}

#[test]
fn remap_indices_remaps_concave_indices_and_its_undo_restores_them_verbatim() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let mut design = concave_fixture(&mut rng);
        let before = design.clone();
        let from_gear = design.meta.gear_teeth;
        let to_gear = from_gear * 2;
        let inverse = design
            .apply_edit(Edit::RemapIndices {
                from_gear,
                to_gear,
                rounding: RemapRounding::Nearest,
            })
            .expect("a remap never fails");
        for (tier, original) in design.concave_tiers.iter().zip(&before.concave_tiers) {
            let doubled: Vec<f64> = original.indices.iter().map(|i| i * 2.0).collect();
            assert_eq!(
                tier.indices, doubled,
                "seed {seed}: concave indices follow the gear"
            );
        }
        assert!(
            matches!(&inverse, Edit::Batch(parts) if matches!(
                parts.as_slice(),
                [Edit::RestoreIndices { .. }, Edit::RestoreConcaveIndices { .. }]
            )),
            "seed {seed}: the inverse is Batch[RestoreIndices, RestoreConcaveIndices], got {inverse:?}"
        );
        design.apply_edit(inverse).expect("undo of a remap");
        assert_eq!(
            design, before,
            "seed {seed}: undo restores every index verbatim"
        );
    }
}

#[test]
fn remap_indices_inverse_is_unchanged_for_a_design_without_concave_tiers() {
    let mut design = round_brilliant_fixture();
    let inverse = design
        .apply_edit(Edit::RemapIndices {
            from_gear: design.meta.gear_teeth,
            to_gear: 32,
            rounding: RemapRounding::Nearest,
        })
        .expect("a remap never fails");
    assert!(
        matches!(inverse, Edit::RestoreIndices { .. }),
        "no concave tiers: the inverse stays the plain RestoreIndices, got {inverse:?}"
    );
}

#[test]
fn modify_concave_tier_rejects_an_invalid_tier_without_mutating() {
    for &seed in &SEEDS {
        let mut rng = Lcg::new(seed);
        let mut design = concave_fixture(&mut rng);
        // The fixture's flat tiers are unnamed, and empty names are exempt from
        // uniqueness, so give one a real name for the clash case to collide with.
        design.tiers[0].name = "Pavilion main".to_string();
        let before = design.clone();
        let index = rng.index(design.concave_tiers.len());

        let mut nan = generated_concave_tier(&mut rng);
        nan.diameter_ratio = f64::NAN;
        let mut out_of_gear = generated_concave_tier(&mut rng);
        out_of_gear.indices = vec![99.0];
        let mut clash = generated_concave_tier(&mut rng);
        clash.name = "Pavilion main".to_string();
        for bad in [nan, out_of_gear, clash] {
            assert!(
                design
                    .apply_edit(Edit::ModifyConcaveTier {
                        index,
                        tier: bad.clone()
                    })
                    .is_err(),
                "seed {seed}: {bad:?} must be rejected"
            );
            assert!(
                design
                    .apply_edit(Edit::AddConcaveTier { index, tier: bad })
                    .is_err()
            );
            assert_eq!(
                design, before,
                "seed {seed}: a rejected edit must not mutate"
            );
            assert_eq!(design.concave_tier_ids, before.concave_tier_ids);
        }
        let out_of_range = design.concave_tiers.len();
        assert!(
            design
                .apply_edit(Edit::RemoveConcaveTier {
                    index: out_of_range
                })
                .is_err()
        );
        assert_eq!(design, before);
    }
}
