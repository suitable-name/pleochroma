//! The model <-> spec conversions of the render-physics sections and the notes under the family
//! list.

use super::*;

fn row(kind: i32, dir: i32) -> FamilyRow {
    FamilyRow {
        kind,
        dir,
        turn: 60.0,
        tilt: 30.0,
        strength: 0.2,
        color: 1,
        cone: 0.1,
        forward: 0.3,
    }
}

#[test]
fn every_direction_and_kind_survives_the_trip_through_a_row() {
    let dirs = [
        CrystalDir::AlongC,
        CrystalDir::Basal { turn_deg: 60.0 },
        CrystalDir::AlphaAxis,
        CrystalDir::Tilted {
            tilt_deg: 30.0,
            turn_deg: 45.0,
        },
    ];
    for kind in [FamilyKind::Needle, FamilyKind::Platelet] {
        for dir in dirs {
            for p in [0, 1, 2, 4] {
                let family = Family {
                    kind,
                    dir,
                    sigma: 0.3,
                    p,
                    lobe: 0.2,
                    rho: 0.4,
                };
                assert_eq!(row_to_family(&family_to_row(&family)), family);
            }
        }
    }
    // Random needles have no direction: whatever the row held, the family has none.
    let random = row_to_family(&row(2, 3));
    assert_eq!(random.kind, FamilyKind::RandomNeedle);
    assert_eq!(random.dir, CrystalDir::AlongC);
}

#[test]
fn an_edit_is_limited_to_its_range_and_unknown_input_is_refused() {
    let mut r = row(0, 0);
    assert!(apply_edit(&mut r, FIELD_KIND, 1.0));
    assert_eq!(r.kind, 1);
    assert!(apply_edit(&mut r, FIELD_DIR, 9.0));
    assert_eq!(r.dir, 3, "a choice stays inside its combo");
    assert!(apply_edit(&mut r, FIELD_TURN, 400.0));
    assert!((r.turn - 360.0).abs() < f32::EPSILON);
    assert!(apply_edit(&mut r, FIELD_TILT, -5.0));
    assert!(r.tilt.abs() < f32::EPSILON);
    assert!(apply_edit(&mut r, FIELD_FORWARD, 2.0));
    assert!((r.forward - 0.95).abs() < f32::EPSILON);
    let before = r.clone();
    assert!(!apply_edit(&mut r, FIELD_STRENGTH, f32::NAN));
    assert!(!apply_edit(&mut r, 99, 1.0));
    assert_eq!(r, before, "a refused edit leaves the row alone");
}

#[test]
fn rows_become_families_within_the_limit_and_need_a_density() {
    assert_eq!(families_of(&[]), Ok(None));
    let one = families_of(&[row(0, 0)]).expect("usable").expect("one");
    assert_eq!(one.len(), 1);
    let five = vec![row(0, 0); MAX_FAMILIES + 1];
    assert!(families_of(&five).is_err());
    let mut silent = row(0, 0);
    silent.strength = 0.0;
    assert!(families_of(&[row(0, 0), silent]).is_err());
}

#[test]
fn the_notes_name_only_what_is_wrong() {
    assert_eq!(family_notes(&[row(0, 0)], 1), "");
    let alpha = family_notes(&[row(0, 2)], 1);
    assert!(alpha.contains("Alpha Axis"));
    assert_eq!(
        family_notes(&[row(0, 2)], 3),
        "",
        "a biaxial stone fixes it"
    );
    assert_eq!(
        family_notes(&[row(2, 2)], 1),
        "",
        "random needles have no direction to note"
    );
    let isotropic = family_notes(&[row(0, 1)], 0);
    assert!(isotropic.contains("isotropic"));
    let full = family_notes(&vec![row(0, 0); MAX_FAMILIES], 1);
    assert!(full.contains("at most four"));
    let both = family_notes(&[row(0, 2), row(0, 2), row(0, 2), row(0, 2)], 1);
    assert_eq!(both.lines().count(), 2);
}

#[test]
fn the_signature_changes_with_any_row() {
    let a = rows_signature(&[row(0, 0)]);
    assert_ne!(a, rows_signature(&[row(1, 0)]));
    assert_ne!(a, rows_signature(&[row(0, 0), row(0, 0)]));
    assert_eq!(a, rows_signature(&[row(0, 0)]));
}

#[test]
fn a_structure_names_its_kind_when_it_is_stored_and_without_one_it_is_kept_unapplied() {
    let mut form = StructureForm::with_defaults(1, true);
    assert_eq!(form.kind, 1);
    let stored = form.resolve().expect("usable").expect("on");
    assert_eq!(stored.kind, Some(StructureKind::Labradorite));
    let text = custom_structure_to_json(&stored).expect("serialises");
    assert!(text.contains("labradorite"));
    assert_eq!(StructureForm::from_stored(Some(&text)), form);

    // A stored choice with no kind opens as "Stored, Not Rendered" and is saved without one.
    form.kind = KIND_UNAPPLIED;
    let kindless = form.resolve().expect("usable").expect("on");
    assert_eq!(kindless.kind, None);
    let text = custom_structure_to_json(&kindless).expect("serialises");
    assert!(!text.contains("kind"));
    assert_eq!(StructureForm::from_stored(Some(&text)).kind, KIND_UNAPPLIED);
}

#[test]
fn a_structure_that_is_off_or_unreadable_stores_nothing() {
    assert_eq!(StructureForm::with_defaults(0, false).resolve(), Ok(None));
    assert!(!StructureForm::from_stored(None).enabled);
    assert!(!StructureForm::from_stored(Some("not json")).enabled);
    let mut backwards = StructureForm::with_defaults(0, true);
    backwards.color_low = 700.0;
    backwards.color_high = 500.0;
    assert!(backwards.resolve().is_err());
}

#[test]
fn defaults_are_the_kinds_own_and_a_custom_size_is_kept() {
    let opal = StructureForm::with_defaults(0, true);
    let labradorite = StructureForm::with_defaults(1, true);
    assert_eq!(opal.pattern, 1, "opal defaults to Harlequin");
    assert_eq!(
        labradorite.pattern, 2,
        "labradorite defaults to Broad Flash"
    );
    assert!(!opal.use_size);
    let mut sized = opal;
    sized.use_size = true;
    sized.size = 0.35;
    let spec = sized.resolve().expect("usable").expect("on").spec;
    assert_eq!(spec.size, Some(0.35));
}

#[test]
fn reseeding_moves_on_deterministically_and_keeps_the_seed_bits() {
    let seed = seed_to_i32(0xDEAD_BEEF);
    assert_eq!(seed_from_i32(seed), 0xDEAD_BEEF);
    assert_ne!(next_seed(seed), seed);
    assert_eq!(next_seed(seed), next_seed(seed));
}

#[test]
fn a_resolved_form_builds_the_material_it_describes() {
    let form = PhysicsForm {
        curves: CurvesForm::default(),
        families: vec![row(0, 0)],
        structure: StructureForm::with_defaults(0, true),
    };
    let resolved = form
        .resolve(OpticalCharacter::UniaxialNegative)
        .expect("usable");
    assert!(resolved.per_axis.is_none());
    assert!(resolved.families_text().contains("Needle"));
    assert!(resolved.structure_text().contains("opal"));
    let base = GemMaterial::new_custom("Probe", 1.77, 0.02, -0.008, [0.0; 3]);
    let material = resolved.apply(base, None);
    let set = material.oriented_inclusions().expect("silk is on");
    assert_eq!(set.families().len(), 1);
    assert!(set.structure().is_some(), "a named kind is applied");

    let mut kindless = form;
    kindless.structure.kind = KIND_UNAPPLIED;
    let resolved = kindless
        .resolve(OpticalCharacter::UniaxialNegative)
        .expect("usable");
    let material = resolved.apply(
        GemMaterial::new_custom("Probe", 1.77, 0.02, -0.008, [0.0; 3]),
        None,
    );
    assert!(
        material
            .oriented_inclusions()
            .is_some_and(|s| s.structure().is_none()),
        "a structure without a kind is stored but never applied"
    );
}

#[test]
fn an_empty_form_clears_every_column() {
    let form = PhysicsForm::from_row(None);
    let resolved = form.resolve(OpticalCharacter::Isotropic).expect("usable");
    assert_eq!(resolved.per_axis_text(), "");
    assert_eq!(resolved.families_text(), "");
    assert_eq!(resolved.structure_text(), "");
}
