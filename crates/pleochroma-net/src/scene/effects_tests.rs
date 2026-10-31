use super::*;
use pleochroma::optics::{
    inclusions::{
        CrystalDir, CrystalFrame, Family, FamilyKind, InclusionSet,
        bragg::{DEFAULT_SEED, StructureKind, StructurePattern},
    },
    raytracer::surface::PolishPreset,
};

fn silk() -> InclusionSpec {
    InclusionSpec {
        preset: "Fine Silk".to_owned(),
        density: 0.35,
        turn_deg: None,
        sheen: None,
        color: None,
    }
}

fn opal_structure() -> StructureSpec {
    StructureSpec {
        pattern: StructurePattern::Harlequin,
        seed: DEFAULT_SEED,
        size: None,
        color_low_nm: 440.0,
        color_high_nm: 680.0,
        purity: 0.05,
        coverage: 0.8,
    }
}

/// A custom material carrying silk along its c axis and a labradorite-like structure.
fn silky_custom() -> GemMaterial {
    let families = [Family {
        kind: FamilyKind::Needle,
        dir: CrystalDir::AlongC,
        sigma: 0.2,
        p: 2,
        lobe: 0.05,
        rho: 0.0,
    }];
    GemMaterial::new_custom("My Schiller", 1.56, 0.012, 0.008, [0.0; 3])
        .with_inclusions(CrystalFrame::new(Vec3::Y, Some(Vec3::X)).resolve(&families))
        .with_structure_spec_as(
            StructureKind::Labradorite,
            &StructureSpec::default_for("Labradorite").expect("labradorite has a default"),
        )
}

fn carried() -> MaterialInclusions {
    MaterialInclusions::of_material(&silky_custom()).expect("a valid active set")
}

fn busy() -> SceneEffects {
    SceneEffects {
        inclusions: Some(silk()),
        inclusion_second_axis: Some([1.0, 0.0, 0.0]),
        structure: Some(opal_structure()),
        facet_surfaces: vec![
            FacetSurface::from_preset(PolishPreset::Commercial, 0.3).packed(),
            FacetSurface::ground(0.45, 1.0)
                .with_custom_angle(33.0)
                .packed(),
            0,
        ],
        machine: MachineSpec {
            arm_offset_deg: 12.0,
            lap_radius_mm: 125.0,
            mast_distance_mm: 40.0,
            sweep_inner_mm: 50.0,
            sweep_outer_mm: 100.0,
        },
        edge: EdgeSpec {
            width: 0.02,
            alpha: 0.3,
            halo_rq_nm: 8.0,
        },
        material_inclusions: Some(carried()),
    }
}

#[test]
fn the_default_record_is_no_effects_and_the_ideal_scene() {
    let none = SceneEffects::default();
    assert!(none.is_none());
    assert!(!none.has_surface());
    let surfaces = none.surfaces();
    assert!(none.surface_scene(&surfaces).is_ideal());
    let sapphire = GemMaterial::sapphire();
    assert!(matches!(
        none.apply_to_material(&sapphire),
        Cow::Borrowed(_)
    ));
    assert!(none.validate(0).is_ok());
}

#[test]
fn a_busy_record_round_trips_through_postcard_and_json() {
    let effects = busy();
    let bytes = postcard::to_allocvec(&effects).unwrap();
    assert_eq!(
        postcard::from_bytes::<SceneEffects>(&bytes).unwrap(),
        effects
    );
    let json = serde_json::to_string(&effects).unwrap();
    assert_eq!(
        serde_json::from_str::<SceneEffects>(&json).unwrap(),
        effects
    );
}

#[test]
fn the_machine_and_edge_records_mirror_their_engine_types() {
    let machine = busy().machine.to_machine();
    assert_eq!(MachineSpec::from_machine(&machine), busy().machine);
    assert_eq!(
        MachineSpec::default().to_machine(),
        FacetingMachine::DEFAULT
    );
    let edge = busy().edge.to_abrasion();
    assert_eq!(EdgeSpec::from_abrasion(&edge), busy().edge);
    assert!(edge.ground_active() && edge.boost_active());
}

#[test]
fn inclusions_are_applied_in_the_desktops_order() {
    let sapphire = GemMaterial::sapphire();
    let effects = SceneEffects {
        inclusions: Some(silk()),
        ..SceneEffects::default()
    };
    let traced = effects.apply_to_material(&sapphire);
    let expected = sapphire.clone().with_inclusion_spec(&silk(), None);
    assert!(matches!(traced, Cow::Owned(_)));
    assert!(traced.inclusions.is_some());
    assert_eq!(traced.inclusions, expected.inclusions);
    assert!(sapphire.inclusions.is_none(), "the input is untouched");
}

#[test]
fn a_structure_on_a_material_without_one_leaves_it_alone() {
    let sapphire = GemMaterial::sapphire();
    let effects = SceneEffects {
        structure: Some(opal_structure()),
        ..SceneEffects::default()
    };
    assert_eq!(effects.apply_to_material(&sapphire).inclusions, None);
}

#[test]
fn the_surface_scene_carries_the_words_machine_and_edge() {
    let effects = busy();
    assert!(effects.has_surface());
    let surfaces = effects.surfaces();
    let scene = effects.surface_scene(&surfaces);
    assert_eq!(scene.facet_surfaces.len(), 3);
    assert_eq!(scene.facet_surfaces[0].packed(), effects.facet_surfaces[0]);
    assert_eq!(scene.machine, effects.machine.to_machine());
    assert_eq!(scene.edge, effects.edge.to_abrasion());
    assert!(!scene.is_ideal());
}

#[test]
fn a_busy_record_validates_and_malformed_ones_are_refused() {
    assert!(busy().validate(3).is_ok());
    assert!(busy().validate(2).is_err(), "more facet words than planes");
    let bad = |edit: &dyn Fn(&mut SceneEffects)| {
        let mut effects = busy();
        edit(&mut effects);
        effects.validate(3).is_err()
    };
    assert!(bad(&|e| e.inclusions.as_mut().unwrap().density = f32::NAN));
    assert!(bad(&|e| e.inclusions.as_mut().unwrap().density = 1.5));
    assert!(bad(
        &|e| e.inclusions.as_mut().unwrap().turn_deg = Some(f32::INFINITY)
    ));
    assert!(bad(
        &|e| e.inclusions.as_mut().unwrap().preset = "x".repeat(65)
    ));
    assert!(bad(
        &|e| e.inclusion_second_axis = Some([f32::NAN, 0.0, 0.0])
    ));
    assert!(bad(&|e| e.structure.as_mut().unwrap().coverage = -0.1));
    assert!(bad(&|e| e.structure.as_mut().unwrap().color_low_nm = 900.0));
    assert!(bad(&|e| e.structure.as_mut().unwrap().size = Some(0.0)));
    assert!(bad(&|e| e.machine.lap_radius_mm = 0.0));
    assert!(bad(&|e| e.machine.sweep_inner_mm = 200.0));
    assert!(bad(&|e| e.machine.arm_offset_deg = f32::NAN));
    assert!(bad(&|e| e.edge.alpha = 2.0));
    assert!(bad(&|e| e.edge.width = -1.0));
    assert!(bad(&|e| e.material_inclusions.as_mut().unwrap().families
        [0]
    .sigma = f32::NAN));
    assert!(bad(&|e| e.material_inclusions.as_mut().unwrap().families
        [0]
    .sigma = -1.0));
    assert!(bad(&|e| e.material_inclusions.as_mut().unwrap().families
        [0]
    .dir
    .x = f32::INFINITY));
    assert!(bad(&|e| {
        let family = e.material_inclusions.as_ref().unwrap().families[0];
        e.material_inclusions.as_mut().unwrap().families = vec![family; 5];
    }));
    assert!(bad(&|e| e
        .material_inclusions
        .as_mut()
        .unwrap()
        .structure
        .as_mut()
        .unwrap()
        .peak_reflectance = 1.0));
    assert!(bad(&|e| e
        .material_inclusions
        .as_mut()
        .unwrap()
        .structure
        .as_mut()
        .unwrap()
        .cell_size = 0.0));
}

#[test]
fn the_materials_own_inclusions_are_applied_before_the_designs_choices() {
    let custom = silky_custom();
    let mut stored = custom.clone();
    stored.inclusions = None;
    let effects = SceneEffects {
        material_inclusions: MaterialInclusions::of_material(&custom),
        ..SceneEffects::default()
    };
    assert!(!effects.is_none());

    // A material that lost its set on the wire gets it back.
    let traced = effects.apply_to_material(&stored);
    assert!(matches!(traced, Cow::Owned(_)));
    let (rebuilt, local) = (traced.inclusions.unwrap(), custom.inclusions.unwrap());
    assert_eq!(rebuilt.families().len(), local.families().len());
    for (a, b) in rebuilt.families().iter().zip(local.families()) {
        assert_eq!(
            (a.kind, a.sigma, a.p, a.lobe),
            (b.kind, b.sigma, b.p, b.lobe)
        );
        assert!((a.dir - b.dir).length() < 1e-6);
    }
    assert_eq!(rebuilt.structure(), local.structure());
    assert!(traced.scatters());

    // A material that still carries its set (the desktop's) is left alone.
    assert!(matches!(
        effects.apply_to_material(&custom),
        Cow::Borrowed(_)
    ));

    // The design's preset still replaces the carried set, as `with_inclusion_spec` does locally
    // (a custom name resolves no preset, so a built-in is needed to see the replacement).
    let sapphire = GemMaterial::sapphire();
    let both = SceneEffects {
        inclusions: Some(silk()),
        ..effects.clone()
    };
    let expected = sapphire.clone().with_inclusion_spec(&silk(), None);
    assert_eq!(
        both.apply_to_material(&sapphire).inclusions,
        expected.inclusions
    );
    // A preset that resolves nothing (unknown for the name) leaves the carried set in place.
    let unresolved = both.apply_to_material(&stored);
    assert_eq!(unresolved.inclusions.unwrap().families().len(), 1);
}

#[test]
fn a_set_that_scatters_nothing_is_not_carried() {
    assert_eq!(
        MaterialInclusions::of_material(&GemMaterial::sapphire()),
        None
    );
    assert_eq!(MaterialInclusions::of_set(&InclusionSet::EMPTY), None);
    // An unrepresentable set (a non-finite strength) is not carried either: the sender keeps
    // such a render local.
    let mut broken = carried();
    broken.families[0].sigma = f32::NAN;
    assert!(broken.validate().is_err());
    // Rebuilding never panics and never lets an unusable family reach the tracer.
    assert!(broken.to_set().families().is_empty());
}
