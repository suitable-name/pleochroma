//! A custom material's color pattern surviving a design-file round trip inside the
//! `[material.custom]` snapshot (`CustomMaterialSnapshot::pattern`, the `encode_pattern` text).

use super::fixtures::simple_design;
use crate::{
    material::{
        decode_pattern, encode_pattern,
        pattern::{ColorPattern, PatternAxis, TemplateId, template},
    },
    native::{
        CustomMaterialSnapshot, DesignExtras, design_from_str, design_to_string,
        gem_material_from_custom_snapshot,
    },
};

const NAME: &str = "Patterned Gem";

/// A two-zone pattern with every color solved by a distinct band row.
fn solved_bicolor() -> ColorPattern {
    let mut pattern = template(TemplateId::Bicolor, PatternAxis::TABLE_NORMAL);
    for (n, at) in pattern.unsolved_colors().into_iter().enumerate() {
        #[expect(
            clippy::cast_precision_loss,
            reason = "a handful of zone indices, exact in f32"
        )]
        let centre = 40.0f32.mul_add(n as f32, 450.0);
        assert!(pattern.set_color_bands(at, vec![[centre, 30.0, 0.5]]));
    }
    pattern
}

/// `snapshot` attached to a design named [`NAME`], written and opened again.
fn round_trip(snapshot: &CustomMaterialSnapshot) -> CustomMaterialSnapshot {
    let mut design = simple_design();
    design.material.name = Some(NAME.to_string());
    let extras = DesignExtras {
        custom_material: Some(snapshot),
        ..DesignExtras::default()
    };
    let text = design_to_string(&design, None, &extras).expect("must save");
    design_from_str(&text)
        .expect("must load back")
        .restorable_custom_material
        .expect("the snapshot is restorable")
}

/// The pattern text reaches the file and comes back as the identical pattern.
#[test]
fn a_color_pattern_survives_a_save_and_open() {
    let pattern = solved_bicolor();
    let text = encode_pattern(&pattern).expect("the pattern encodes");
    let snapshot = CustomMaterialSnapshot::new(1.62, 0.017, 0.0, None, "Cubic", "Isotropic")
        .with_pattern(Some(text.clone()));
    let restored = round_trip(&snapshot);
    assert_eq!(restored, snapshot);
    let decoded = decode_pattern(restored.pattern.as_deref().expect("the pattern is kept"))
        .expect("the stored text decodes");
    assert_eq!(encode_pattern(&decoded).expect("re-encodes"), text);
}

/// A snapshot without a pattern writes no key, restores with none, and builds the same plain
/// material as before.
#[test]
fn a_snapshot_without_a_pattern_restores_without_one() {
    let snapshot = CustomMaterialSnapshot::new(1.62, 0.017, 0.0, None, "Cubic", "Isotropic");
    let restored = round_trip(&snapshot);
    assert_eq!(restored.pattern, None);
    let rebuilt = gem_material_from_custom_snapshot(NAME, &restored);
    assert!(rebuilt.zoning.is_none());
}

#[test]
fn a_stones_pattern_round_trips_without_creating_a_custom_material() {
    let mut design = simple_design();
    design.material.name = Some("Ametrine".to_owned());
    let original = design.material.clone();
    let pattern = solved_bicolor();
    let material = original.clone().with_color_pattern(Some(pattern.clone()));
    let inverse = design
        .apply_edit(crate::Edit::SetMaterial {
            material: Box::new(material),
        })
        .expect("edit");
    let text = design_to_string(&design, None, &DesignExtras::default()).expect("save");
    assert!(text.contains("color_pattern_override"));
    assert!(!text.contains("[material.custom]"));
    let loaded = design_from_str(&text).expect("load");
    assert_eq!(loaded.design.material, design.material);
    assert_eq!(loaded.design.material.color_pattern_override, Some(pattern));
    design.apply_edit(inverse).expect("undo");
    assert_eq!(design.material, original);
    assert!(
        !design_to_string(&design, None, &DesignExtras::default())
            .expect("save original")
            .contains("color_pattern_override")
    );
}

#[test]
fn a_stones_pattern_replaces_the_zones_but_keeps_the_preset_intact() {
    use crate::material::{MaterialSelection, pattern::apply_color_pattern};
    use pleochroma::optics::materials::GemMaterial;
    let preset = GemMaterial::ametrine();
    let original = preset.clone();
    let pattern = solved_bicolor();
    let edited = apply_color_pattern(preset, &pattern, 12.0).expect("resolved");
    assert_ne!(edited.zoning, original.zoning);
    assert_eq!(edited.name, original.name);
    assert_eq!(edited.dispersion, original.dispersion);
    assert_eq!(GemMaterial::ametrine(), original);
    let selection = MaterialSelection::none().with_color_pattern(Some(pattern));
    assert_eq!(
        selection.apply_overrides_at_width(original.clone(), 12.0),
        edited
    );
    assert_eq!(
        selection
            .with_color_pattern(None)
            .apply_overrides(original.clone()),
        original
    );
}

#[test]
fn relative_patterns_resolve_from_authored_geometry_at_each_stone_size() {
    use crate::{material::pattern::apply_color_pattern, rough_plan::zoned_plan::resolve_relative};
    use pleochroma::optics::materials::GemMaterial;
    let mut pattern = solved_bicolor();
    pattern.relative_to_stone = true;
    let authored = pattern.to_zoned().expect("zones").expect("nonuniform");
    let small = apply_color_pattern(GemMaterial::ametrine(), &pattern, 7.0).expect("small");
    let large = apply_color_pattern(small, &pattern, 14.0).expect("large");
    assert_eq!(large.zoning, resolve_relative(&authored, 14.0));
    assert_eq!(pattern.to_zoned().expect("unchanged"), Some(authored));
}

#[test]
fn uniform_patterns_preserve_their_base_color_and_remove_inherited_zones() {
    use crate::material::pattern::apply_color_pattern;
    use pleochroma::optics::materials::{AbsorptionUnit, GemMaterial};
    let mut pattern = solved_bicolor();
    let base = pattern
        .to_zoned()
        .expect("zones")
        .expect("nonuniform")
        .base
        .tensor;
    pattern.zones.clear();
    let material = apply_color_pattern(GemMaterial::ametrine(), &pattern, 7.0).expect("uniform");
    assert!(material.zoning.is_none());
    assert_eq!(material.absorption, base);
    assert_eq!(material.absorption_unit, AbsorptionUnit::PerMm);
}

#[test]
fn an_invalid_or_future_stone_pattern_is_refused_on_load() {
    use crate::native::{design_from_file, design_to_file};
    let mut file = design_to_file(&simple_design(), None, &DesignExtras::default());
    let mut pattern = solved_bicolor();
    // One past the newest version this build reads (version 2 is readable since the color
    // variants campaign: a pattern with a variant or a three-color zone).
    pattern.version = crate::material::pattern::PATTERN_FORMAT_VERSION + 1;
    file.material.color_pattern_override = Some(encode_pattern(&pattern).expect("JSON"));
    assert!(design_from_file(file.clone()).is_err());
    pattern.version = crate::material::pattern::PATTERN_BASE_FORMAT_VERSION;
    pattern.base.color = crate::material::pattern::PatternColor::Single(
        crate::material::pattern::ZoneColor::unsolved([50.0, 30.0, 20.0]),
    );
    file.material.color_pattern_override = Some(encode_pattern(&pattern).expect("JSON"));
    assert!(design_from_file(file).is_err());
}
