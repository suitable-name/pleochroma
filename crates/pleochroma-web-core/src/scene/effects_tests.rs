//! The browser's effects record: round trips, validation, and that it changes the traced scene
//! only when set.

use super::*;
use crate::{
    render::handle_trace_chunk,
    scene::{CameraSpec, FinishSpec, LightingSpec, MaterialSpec, SceneSpec, planes_to_data},
};
use pleochroma::{
    geometry::cuts::StandardGemCuts,
    optics::{
        inclusions::bragg::{DEFAULT_SEED, StructurePattern},
        raytracer::{LightingPreset, surface::PolishPreset},
    },
    render_setup::Backdrop,
};

fn scene(material: &str) -> OwnedScene {
    OwnedScene::build(
        &SceneSpec {
            planes: planes_to_data(&StandardGemCuts::standard_round_brilliant()),
            finishes: FinishSpec::AllPolished,
            material: MaterialSpec::catalogue(material),
            camera: CameraSpec {
                yaw: 0.35,
                pitch: 0.28,
                distance: 5.0,
            },
            lighting: LightingSpec::new(
                LightingPreset::LightTent,
                1.0,
                0.4,
                0.35,
                Backdrop::Grey,
                16.0,
            ),
            max_bounces: 6,
            width: 16,
            height: 16,
            hdr_id: None,
        },
        None,
    )
    .expect("fixture scene builds")
}

fn silk() -> InclusionSpec {
    InclusionSpec {
        preset: "Fine Silk".to_owned(),
        density: 1.0,
        turn_deg: None,
        sheen: None,
        color: None,
    }
}

fn busy() -> EffectsSpec {
    EffectsSpec {
        inclusions: Some(silk()),
        inclusion_second_axis: Some([1.0, 0.0, 0.0]),
        structure: Some(StructureSpec {
            pattern: StructurePattern::Harlequin,
            seed: DEFAULT_SEED,
            size: None,
            color_low_nm: 440.0,
            color_high_nm: 680.0,
            purity: 0.05,
            coverage: 0.8,
        }),
        facet_surfaces: vec![
            FacetSurface::from_preset(PolishPreset::Commercial, 0.3).packed(),
            FacetSurface::ground(0.45, 1.0).packed(),
        ],
        machine: MachineSpec {
            arm_offset_deg: 10.0,
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
    }
}

fn trace(scene: &OwnedScene) -> Vec<Vec3> {
    handle_trace_chunk(scene, scene.plane_soa(), 0, 1, 0, 4)
}

#[test]
fn the_default_record_is_no_effects() {
    let none = EffectsSpec::default();
    assert!(none.is_none());
    assert!(!none.has_surface());
    assert!(none.validate(0).is_ok());
    assert_eq!(
        MachineSpec::default().to_machine(),
        FacetingMachine::DEFAULT
    );
}

#[test]
fn a_busy_record_round_trips_through_postcard() {
    let bytes = postcard::to_allocvec(&busy()).expect("encodes");
    assert_eq!(
        postcard::from_bytes::<EffectsSpec>(&bytes).expect("decodes"),
        busy()
    );
}

#[test]
fn malformed_records_are_refused() {
    assert!(busy().validate(2).is_ok());
    assert!(busy().validate(1).is_err(), "more words than planes");
    let bad = |edit: &dyn Fn(&mut EffectsSpec)| {
        let mut effects = busy();
        edit(&mut effects);
        effects.validate(2).is_err()
    };
    assert!(bad(&|e| e.inclusions.as_mut().unwrap().density = f32::NAN));
    assert!(bad(
        &|e| e.inclusions.as_mut().unwrap().preset = "x".repeat(65)
    ));
    assert!(bad(&|e| e.structure.as_mut().unwrap().coverage = 2.0));
    assert!(bad(&|e| e.machine.lap_radius_mm = 0.0));
    assert!(bad(&|e| e.edge.alpha = -1.0));
}

#[test]
fn no_effects_trace_bit_identically_to_a_scene_that_never_had_a_record() {
    let plain = scene("Sapphire");
    let mut with_none = scene("Sapphire");
    with_none
        .set_effects(EffectsSpec::default())
        .expect("applies");
    assert_eq!(trace(&plain), trace(&with_none));
    assert!(with_none.surface_scene().is_ideal());
}

#[test]
fn ground_facets_and_silk_change_the_trace() {
    let plain = scene("Sapphire");
    let mut ground = scene("Sapphire");
    ground
        .set_effects(EffectsSpec {
            facet_surfaces: vec![FacetSurface::ground(0.45, 1.0).packed(); 20],
            ..EffectsSpec::default()
        })
        .expect("applies");
    assert!(!ground.surface_scene().is_ideal());
    assert_ne!(trace(&plain), trace(&ground));

    let mut silky = scene("Sapphire");
    silky
        .set_effects(EffectsSpec {
            inclusions: Some(silk()),
            ..EffectsSpec::default()
        })
        .expect("applies");
    assert!(silky.material().inclusions.is_some());
    assert_ne!(trace(&plain), trace(&silky));
}

#[test]
fn a_second_record_replaces_the_first_and_none_restores_the_scene() {
    let plain = scene("Sapphire");
    let mut changed = scene("Sapphire");
    changed
        .set_effects(EffectsSpec {
            inclusions: Some(silk()),
            ..EffectsSpec::default()
        })
        .expect("applies");
    changed
        .set_effects(EffectsSpec::default())
        .expect("applies");
    assert!(changed.material().inclusions.is_none());
    assert_eq!(trace(&plain), trace(&changed));
}

#[test]
fn a_refused_record_leaves_the_scene_unchanged() {
    let mut scene = scene("Sapphire");
    let before = trace(&scene);
    let mut bad = busy();
    bad.edge.alpha = 5.0;
    assert!(scene.set_effects(bad).is_err());
    assert!(scene.effects().is_none());
    assert_eq!(trace(&scene), before);
}
