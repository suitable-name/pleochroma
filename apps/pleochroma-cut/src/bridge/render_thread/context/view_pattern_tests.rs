//! Scope, resolution and invalidation of preview-only stone patterns.

use super::*;
use pleochroma_cut_core::material::pattern::{
    PatternAxis, PatternColor, TemplateId, ZoneColor, template,
};
use std::sync::{Arc, Mutex};

fn context() -> RenderContext {
    RenderContext {
        material_name: "Ametrine".into(),
        planes_owner: PlanesOwner::Catalogue { entry_id: 17 },
        ..RenderContext::default()
    }
}

fn pattern(id: TemplateId) -> ColorPattern {
    let mut pattern = template(id, PatternAxis::TABLE_NORMAL);
    for zone in std::iter::once(&mut pattern.base).chain(&mut pattern.zones) {
        zone.color = PatternColor::Single(ZoneColor::solved(
            [50.0, 20.0, 90.0],
            vec![[550.0, 50.0, 0.2]],
        ));
    }
    pattern
}

#[test]
fn a_preview_pattern_never_changes_the_material_preset() {
    let mut ctx = context();
    let original = ctx.base_pattern_material().expect("material");
    let pattern = pattern(TemplateId::Banded);
    ctx.set_view_color_pattern(Some(pattern.clone()))
        .expect("valid");
    assert_eq!(ctx.current_view_color_pattern(), Some(&pattern));
    assert_eq!(ctx.base_pattern_material(), Some(original.clone()));
    assert_eq!(ctx.material_override, None);
    assert_ne!(ctx.tinted_material_override(), Some(original));
    assert!(ctx.zoned_material());
    assert!(!ctx.zone_swatch_colors().is_empty());
    ctx.set_view_color_pattern(None).expect("reset");
    assert_eq!(ctx.tinted_material_override(), None);
    assert!(
        ctx.zoned_material(),
        "the built-in Ametrine pattern is back"
    );
}

#[test]
fn switching_library_designs_drops_the_pattern_permanently() {
    let mut ctx = context();
    ctx.set_view_color_pattern(Some(pattern(TemplateId::Banded)))
        .expect("valid");
    for entry_id in [18, 17] {
        assert!(ctx.claim_active_planes(
            Arc::clone(&ctx.active_planes),
            ctx.design_gear,
            PlanesOwner::Catalogue { entry_id },
        ));
        assert!(ctx.current_view_color_pattern().is_none());
        assert!(ctx.view_color_pattern.is_none());
    }
}

#[test]
fn switching_materials_drops_the_pattern_before_returning_to_the_old_one() {
    let ctx = Mutex::new(context());
    RenderContext::lock(&ctx)
        .set_view_color_pattern(Some(pattern(TemplateId::Banded)))
        .expect("valid");
    RenderContext::lock(&ctx).material_name = "Sapphire".into();
    RenderContext::lock(&ctx).material_name = "Ametrine".into();
    let guard = RenderContext::lock(&ctx);
    assert!(guard.current_view_color_pattern().is_none());
    assert!(guard.view_color_pattern.is_none());
    drop(guard);
}

#[test]
fn uniform_pattern_wins_over_the_preview_body_color_and_replaces_builtin_zones() {
    let mut ctx = context();
    ctx.view_body_color = Some(crate::bridge::render_thread::ViewColor::triple_only([
        0.5, 0.0, 1.0,
    ]));
    ctx.set_view_color_pattern(Some(pattern(TemplateId::Uniform)))
        .expect("valid");
    assert_eq!(ctx.effective_view_body_color(), None);
    assert!(!ctx.zoned_material());
    let material = ctx.tinted_material_override().expect("patterned material");
    assert!(material.zoning.is_none());
    assert!(!material.absorption.o_ray.is_empty());
}

#[test]
fn pattern_changes_invalidate_the_scene_but_reapplying_it_does_not() {
    let mut ctx = context();
    let original = ctx.scene_generation();
    let pattern = pattern(TemplateId::Banded);
    ctx.set_view_color_pattern(Some(pattern.clone()))
        .expect("valid");
    let changed = ctx.scene_generation();
    assert!(changed > original);
    ctx.set_view_color_pattern(Some(pattern)).expect("valid");
    assert_eq!(ctx.scene_generation(), changed);
    ctx.set_view_color_pattern(None).expect("reset");
    assert!(ctx.scene_generation() > changed);
}

#[test]
fn size_changes_keep_the_pattern_and_resize_its_relative_geometry() {
    let mut ctx = context();
    ctx.stone_width_mm = 7.0;
    ctx.set_view_color_pattern(Some(pattern(TemplateId::Banded)))
        .expect("valid");
    let small = ctx.tinted_material_override().expect("material");
    ctx.stone_width_mm = 14.0;
    let large = ctx.tinted_material_override().expect("material");
    assert_ne!(small.zoning, large.zoning);
    assert!(ctx.current_view_color_pattern().is_some());
}

#[test]
fn invalid_patterns_and_linked_design_targets_are_refused() {
    let mut ctx = context();
    assert!(
        ctx.set_view_color_pattern(Some(template(
            TemplateId::Banded,
            PatternAxis::TABLE_NORMAL,
        )))
        .is_err()
    );
    assert!(ctx.view_color_pattern.is_none());
    ctx.planes_owner = PlanesOwner::Editor { generation: 3 };
    assert!(
        ctx.set_view_color_pattern(Some(pattern(TemplateId::Banded)))
            .is_err()
    );
    assert!(ctx.view_color_pattern.is_none());
}
