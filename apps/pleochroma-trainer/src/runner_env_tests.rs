//! Source augmentation applies only to missing values and is shared by retarget passes.

use super::*;
use crate::dataset::{DatasetSplit, DbDesignRecord};
use pleochroma_ai::model::ModelTier;
use pleochroma_cut_core::{PreformSpec, native};

fn native_entry(authored: bool) -> crate::dataset::DesignEntry {
    let schedule = pleochroma_formats::asc::parse_asc(
        "GemCad 5.0\ng 4 0.0\ny 1 n\nI 1.62\n\
a 90.000000 1.00000000 0 1 2 3 G Set girdle thickness\n\
a 0.000000 0.60000000 G Set stone size\n\
a -0.000000 0.55000000 G Set stone size\n",
    )
    .unwrap();
    let mut design = Design::from_asc_schedule(PreformSpec::block(2.0, 1.4, 3.0), &schedule);
    if authored {
        design.material.name = Some("Tanzanite".into());
        design.material.refractive_index_override = Some(1.702);
        design.material.absorption_path_scale_override = Some(1.7);
        design.render_effects.c_axis_override_enabled = true;
        design.render_effects.c_axis_tilt_deg = 24.0;
        design.render_effects.c_axis_azimuth_deg = 113.0;
    }
    let text = native::design_to_string(&design, None, &native::DesignExtras::default()).unwrap();
    let record = DbDesignRecord {
        id: 1,
        title: "native source".into(),
        filename: "native.psdd".into(),
        asc_bytes: text.into_bytes(),
    };
    DatasetSplit::from_records(&[record], 0, 128)
        .unwrap()
        .entries
        .remove(0)
}

#[test]
fn an_unmaterialled_native_varies_sources_but_keeps_a_recorded_index_subset() {
    let entry = native_entry(false);
    assert!(entry.is_native());
    assert!(entry.authored_material.is_none());
    let original = entry.design.clone();
    let mut catalogue_names = std::collections::BTreeSet::new();
    let mut recorded_index_episodes = 0;
    for seed in 0..128 {
        let sample = sampled_source_material(seed, &entry.design, None).unwrap();
        assert_eq!(
            sample,
            sampled_source_material(seed, &entry.design, None).unwrap()
        );
        if sample.2 == "[authored imported index]" {
            recorded_index_episodes += 1;
            assert!((sample.1.n_d - 1.62).abs() < 1e-12);
        } else {
            assert!(
                MINERAL_SPECTRUM
                    .iter()
                    .any(|material| material.name == sample.0)
            );
            catalogue_names.insert(sample.0);
        }
    }
    assert!(recorded_index_episodes > 0 && recorded_index_episodes < 128);
    assert!(catalogue_names.len() > 1);
    assert_eq!(
        entry.design, original,
        "episode sampling cannot author the source design"
    );
}

#[test]
fn authored_native_source_and_overrides_are_retained_for_every_seed() {
    let entry = native_entry(true);
    let material = entry.authored_material.as_ref().unwrap();
    let original = entry.design.clone();
    for seed in 0..32 {
        let (name, actual, provenance) =
            sampled_source_material(seed, &entry.design, Some(material)).unwrap();
        assert_eq!(name, "Tanzanite");
        assert_eq!(actual, *material);
        assert_eq!(provenance, "[authored native material]");
    }
    assert_eq!(entry.design, original);
}

#[test]
fn native_same_material_targets_use_the_identical_source_context() {
    let mut cfg = RunConfig::new(TrainingMode::Retarget, ModelTier::Small);
    cfg.same_material_frac = 1.0;
    for authored in [false, true] {
        let entry = native_entry(authored);
        for seed in 0..32 {
            let source =
                sampled_source_material(seed, &entry.design, entry.authored_material.as_ref())
                    .unwrap();
            let target = pick_target(&cfg, seed, &entry.design, entry.authored_material.as_ref())
                .unwrap()
                .unwrap();
            assert_eq!(target.name, source.0);
            assert_eq!(target.material, source.1);
            assert_eq!(target.augmentation, source.2);
        }
    }
}
