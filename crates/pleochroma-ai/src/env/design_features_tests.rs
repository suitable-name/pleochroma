use super::*;
use pleochroma_cut_core::design::{Expr, TierRelation};

fn context(design: &Design, index: usize) -> TierDesignFeatures {
    let inputs = design.meet_tier_inputs();
    tier_context(
        design,
        index,
        0.5,
        None,
        2.0,
        &MeetNameResolver::new(&inputs),
    )
    .expect("complete supported context")
}

#[test]
fn every_orbit_position_survives_and_an_overfull_orbit_is_rejected() {
    let mut design = Design::concave_fixture();
    design.meta.gear_teeth = 720;
    design.tiers[0].indices = (0..MAX_ORBIT_POSITIONS).map(|i| i as f64).collect();
    let before = context(&design, 0).to_feature_vector();
    design.tiers[0].indices[MAX_ORBIT_POSITIONS - 1] += 0.125;
    let after = context(&design, 0).to_feature_vector();
    assert_ne!(
        before[TIER_HEADER + MAX_ORBIT_POSITIONS - 1],
        after[TIER_HEADER + MAX_ORBIT_POSITIONS - 1]
    );
    design.tiers[0].indices.push(200.0);
    assert!(
        validate_design_representation(&design)
            .unwrap_err()
            .contains("128")
    );
}

#[test]
fn implicit_index_zero_has_the_same_physical_orbit_as_explicit_zero() {
    let mut design = Design::concave_fixture();
    design.meta.gear_reference_angle = 2.0;
    let explicit = program::orbit(&design, &[0.0], 3.0).unwrap();
    assert_eq!(program::orbit(&design, &[], 3.0).unwrap(), explicit);
}

#[test]
fn orbit_detachment_does_not_remove_a_facet_from_the_geometry_context() {
    let mut design = Design::concave_fixture();
    let attached = context(&design, 1).to_feature_vector();
    // Detach excludes a position from orbit-wide membership edits. It does not
    // remove its plane or give it a separate angle, and this policy changes
    // only angles/masts. Export and the scorer still use every index.
    design.tiers[1].detached = vec![design.tiers[1].indices[0]];
    assert_eq!(context(&design, 1).to_feature_vector(), attached);
    design.tiers[1].indices.clear();
    design.tiers[1].detached = vec![0.0];
    let implicit = context(&design, 1).to_feature_vector();
    design.tiers[1].detached.clear();
    assert_eq!(context(&design, 1).to_feature_vector(), implicit);
}

#[test]
fn relation_driver_operator_and_coefficient_are_observed() {
    let mut design = Design::concave_fixture();
    let driven = design.tier_id_at(3).unwrap();
    let first = design.tier_id_at(1).unwrap();
    let second = design.tier_id_at(2).unwrap();
    let relation = |driver, coefficient| {
        TierRelation::new(Expr::Sub(
            Box::new(Expr::Ref(driver)),
            Box::new(Expr::Number(coefficient)),
        ))
    };
    design.tier_relations.insert(driven, relation(first, 2.0));
    let first_program = program::relation(&design, 3).unwrap();
    design.tier_relations.insert(driven, relation(second, 2.0));
    assert_ne!(program::relation(&design, 3).unwrap(), first_program);
    design.tier_relations.insert(driven, relation(first, 3.0));
    assert_ne!(program::relation(&design, 3).unwrap(), first_program);
    design.tier_relations.insert(
        driven,
        TierRelation::new(Expr::Add(
            Box::new(Expr::Ref(first)),
            Box::new(Expr::Number(2.0)),
        )),
    );
    assert_ne!(program::relation(&design, 3).unwrap(), first_program);
}

#[test]
fn relation_capacity_is_checked_before_truncating_a_program() {
    let mut design = Design::concave_fixture();
    let mut expression = Expr::Number(40.0);
    for _ in 0..MAX_RELATION_NODES {
        expression = Expr::Neg(Box::new(expression));
    }
    design
        .tier_relations
        .insert(design.tier_id_at(3).unwrap(), TierRelation::new(expression));
    assert!(
        validate_design_representation(&design)
            .unwrap_err()
            .contains("32 observation nodes")
    );
}

#[test]
fn named_meets_and_dimensioned_target_values_are_distinct() {
    let mut design = Design::concave_fixture();
    design.tiers[1].name = "Pavilion Main".into();
    design.tiers[2].name = "Pavilion Break".into();
    design.tiers[3].constraint = MeetConstraint::MeetNamed(vec![design.tiers[1].name.clone()]);
    let first = context(&design, 3).to_feature_vector();
    design.tiers[3].constraint = MeetConstraint::MeetNamed(vec![design.tiers[2].name.clone()]);
    assert_ne!(context(&design, 3).to_feature_vector(), first);
    let id = design.tier_id_at(3).unwrap();
    design.tier_targets.insert(id, TierTarget::DepthMm(2.0));
    let depth = context(&design, 3).to_feature_vector();
    design
        .tier_targets
        .insert(id, TierTarget::TableWidthMm(2.0));
    assert_ne!(context(&design, 3).to_feature_vector(), depth);
    design.tier_targets.insert(id, TierTarget::DepthMm(3.0));
    assert_ne!(context(&design, 3).to_feature_vector(), depth);
}

#[test]
fn original_plane_context_is_shared_without_following_live_masts() {
    let design = Design::concave_fixture();
    let original = context(&design, 1);
    let mut live = original.clone();
    live.current_mast += 0.1;
    assert!(Arc::ptr_eq(
        &original.static_features,
        &live.static_features
    ));
    assert_eq!(live.static_features, original.static_features);
    assert_ne!(live.to_feature_vector(), original.to_feature_vector());
}

#[test]
fn immutable_hinge_position_and_side_are_observed() {
    let design = Design::concave_fixture();
    let inputs = design.meet_tier_inputs();
    let resolver = MeetNameResolver::new(&inputs);
    let mut hinge = TierHinge {
        point: glam::DVec3::new(1.0, 0.1, 0.2),
        azimuth_rad: 0.25,
        side: FacetSide::Pavilion,
        wall_edge: true,
    };
    let first = tier_context(&design, 1, 0.5, Some(&hinge), 2.0, &resolver).unwrap();
    hinge.point.x += 0.1;
    let displaced = tier_context(&design, 1, 0.5, Some(&hinge), 2.0, &resolver).unwrap();
    assert_ne!(first.to_feature_vector(), displaced.to_feature_vector());
    hinge.side = FacetSide::Crown;
    assert_ne!(
        displaced.to_feature_vector(),
        tier_context(&design, 1, 0.5, Some(&hinge), 2.0, &resolver)
            .unwrap()
            .to_feature_vector()
    );
}

#[test]
fn oversized_custom_rough_is_refused_without_dropping_planes() {
    let mut design = Design::concave_fixture();
    design.preform.shape = pleochroma_cut_core::preform::PreformShape::Planes(Arc::new(
        pleochroma_cut_core::preform::PlaneSet {
            planes: vec![(glam::DVec3::X, 1.0); MAX_ROUGH_PLANES + 1],
            source: String::new(),
        },
    ));
    assert!(
        validate_design_representation(&design)
            .unwrap_err()
            .contains("32")
    );
}

#[test]
fn malformed_serialized_suffixes_fail_before_packing() {
    let short_tier = r#"{"static_features":[0.0],"current_mast":0.0,"current_anchor":0.0}"#;
    let short_global = r#"{"features":[0.0],"current_width_ratio":1.0}"#;
    assert!(serde_json::from_str::<TierDesignFeatures>(short_tier).is_err());
    assert!(serde_json::from_str::<GlobalDesignFeatures>(short_global).is_err());
    let default = TierDesignFeatures::default();
    let roundtrip: TierDesignFeatures =
        serde_json::from_str(&serde_json::to_string(&default).unwrap()).unwrap();
    assert_eq!(
        roundtrip.to_feature_vector().len(),
        TierDesignFeatures::FEATURE_DIM
    );
}

#[test]
fn too_many_concave_tools_is_a_visible_refusal() {
    let mut design = Design::concave_fixture();
    design.concave_tiers = vec![design.concave_tiers[0].clone(); MAX_CONCAVE_TIERS + 1];
    assert!(
        validate_design_representation(&design)
            .unwrap_err()
            .contains("8")
    );
}

#[test]
fn authored_frosting_is_aligned_with_every_orbit_position() {
    use pleochroma_cut_core::design::TierFrosting;
    let mut design = Design::concave_fixture();
    design.tiers[1].indices = vec![0.0, 12.0, 24.0];
    let id = design.tier_id_at(1).unwrap();
    let polished = context(&design, 1).to_feature_vector();
    design
        .tier_frosting
        .insert(id, TierFrosting::Facets(vec![12.0]));
    assert_eq!(program::frosting(&design, 1), [0.0, 1.0, 0.0]);
    assert_ne!(polished, context(&design, 1).to_feature_vector());
    design.tiers[1].indices.clear();
    design
        .tier_frosting
        .insert(id, TierFrosting::Facets(vec![0.0]));
    assert_eq!(program::frosting(&design, 1), [0.0]);
    design.tier_frosting.insert(id, TierFrosting::Tier);
    assert_eq!(program::frosting(&design, 1), [1.0]);
}
