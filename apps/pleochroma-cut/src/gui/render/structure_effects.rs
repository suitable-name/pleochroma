//! The Stone tab's Play-of-Color group (Schiller for labradorite): the conversion between its
//! controls (`RenderEffectsModel`) and the design's [`StructureEffects`], the material's default
//! choice, and the Reseed step.
//!
//! The seed lives in a control (as decimal text) so a Reseed and the debounced commit share one
//! path: the commit reads the controls, whatever the design still holds. The Slint glue is in
//! [`super::physics_effects_ui`].

use super::inclusion_effects::{is_custom_name, keep_or, narrow};
use crate::{MainWindow, RenderEffectsModel};
use pleochroma::optics::{
    inclusions::bragg::{
        DEFAULT_SEED, StructureKind, StructurePattern, StructureSpec, structure_kind,
    },
    materials::GemMaterial,
};
use pleochroma_cut_core::StructureEffects;
use slint::{ComponentHandle, SharedString};

/// What the Play-of-Color controls hold.
#[derive(Debug, Clone, PartialEq)]
pub(in crate::gui) struct StructureControls {
    pub(in crate::gui) on: bool,
    /// 0 Pinfire, 1 Harlequin, 2 Broad Flash.
    pub(in crate::gui) pattern: i32,
    pub(in crate::gui) size_auto: bool,
    pub(in crate::gui) size: f32,
    pub(in crate::gui) low_nm: f32,
    pub(in crate::gui) high_nm: f32,
    pub(in crate::gui) purity: f32,
    pub(in crate::gui) coverage: f32,
    /// The seed as decimal digits.
    pub(in crate::gui) seed: String,
}

impl Default for StructureControls {
    fn default() -> Self {
        Self {
            on: false,
            pattern: 1,
            size_auto: true,
            size: 0.2,
            low_nm: 440.0,
            high_nm: 680.0,
            purity: 0.05,
            coverage: 0.8,
            seed: DEFAULT_SEED.to_string(),
        }
    }
}

/// The control index of a pattern.
#[must_use]
pub(in crate::gui) const fn pattern_index(pattern: StructurePattern) -> i32 {
    match pattern {
        StructurePattern::Pinfire => 0,
        StructurePattern::Harlequin => 1,
        StructurePattern::BroadFlash => 2,
    }
}

/// The pattern of a control index (anything unknown is Harlequin).
#[must_use]
pub(in crate::gui) const fn pattern_from_index(index: i32) -> StructurePattern {
    match index {
        0 => StructurePattern::Pinfire,
        2 => StructurePattern::BroadFlash,
        _ => StructurePattern::Harlequin,
    }
}

/// The group's title for `material_name`: Schiller for labradorite, Play-of-Color otherwise.
#[must_use]
pub(in crate::gui) fn title_for(material_name: &str) -> &'static str {
    match structure_kind(material_name) {
        Some(StructureKind::Labradorite) => "Schiller",
        _ => "Play-of-Color",
    }
}

/// The read-only line the Play-of-Color group shows for a custom material's own structure.
pub(in crate::gui) const OWN_STRUCTURE_TEXT: &str = "Material's Own Play-of-Color";

/// Whether `material` is a custom material that carries a play-of-color structure of its own
/// (authored in the Material Editor with a kind it imitates). A built-in gets its structure from
/// the design's choice instead.
#[must_use]
pub(in crate::gui) fn has_own_structure(material: &GemMaterial) -> bool {
    is_custom_name(&material.name)
        && material
            .inclusions
            .as_ref()
            .is_some_and(|set| set.structure().is_some())
}

/// The controls that show `effects`: switched off for `None`, else every value of the choice.
#[must_use]
pub(in crate::gui) fn controls_for(effects: Option<&StructureEffects>) -> StructureControls {
    let Some(effects) = effects else {
        return StructureControls::default();
    };
    StructureControls {
        on: true,
        pattern: pattern_index(effects.pattern),
        size_auto: effects.size.is_none(),
        size: effects
            .size
            .map_or_else(|| effects.pattern.cell_size(), narrow),
        low_nm: narrow(effects.color_low_nm),
        high_nm: narrow(effects.color_high_nm),
        purity: narrow(effects.purity),
        coverage: narrow(effects.coverage),
        seed: effects.seed.to_string(),
    }
}

/// The controls switched on with `material_name`'s own default choice, or `None` for a material
/// without a structure.
#[must_use]
pub(in crate::gui) fn default_controls_for(material_name: &str) -> Option<StructureControls> {
    let spec = StructureSpec::default_for(material_name)?;
    Some(controls_for(Some(&StructureEffects::from_spec(&spec))))
}

/// The design's choice the controls stand for: `None` while switched off, else the choice with
/// the design's own values kept where the controls still hold them.
#[must_use]
pub(in crate::gui) fn effects_from_controls(
    controls: &StructureControls,
    base: Option<&StructureEffects>,
) -> Option<StructureEffects> {
    if !controls.on {
        return None;
    }
    let seed = controls
        .seed
        .trim()
        .parse::<u32>()
        .ok()
        .or_else(|| base.map(|b| b.seed))
        .unwrap_or(DEFAULT_SEED);
    Some(
        StructureEffects {
            pattern: pattern_from_index(controls.pattern),
            seed,
            size: (!controls.size_auto).then(|| keep_or(base.and_then(|b| b.size), controls.size)),
            color_low_nm: keep_or(base.map(|b| b.color_low_nm), controls.low_nm),
            color_high_nm: keep_or(base.map(|b| b.color_high_nm), controls.high_nm),
            purity: keep_or(base.map(|b| b.purity), controls.purity),
            coverage: keep_or(base.map(|b| b.coverage), controls.coverage),
        }
        .sanitised(),
    )
}

/// `controls` with a new seed (the Reseed button).
pub(in crate::gui) fn reseed(controls: &mut StructureControls) {
    let current = controls.seed.trim().parse::<u32>().unwrap_or(DEFAULT_SEED);
    controls.seed = StructureEffects::reseeded(current).to_string();
}

/// Reads the Play-of-Color controls.
#[must_use]
pub(in crate::gui) fn read_controls(ui: &MainWindow) -> StructureControls {
    let model = ui.global::<RenderEffectsModel>();
    StructureControls {
        on: model.get_structure_on(),
        pattern: model.get_structure_pattern(),
        size_auto: model.get_structure_size_auto(),
        size: model.get_structure_size(),
        low_nm: model.get_structure_low_nm(),
        high_nm: model.get_structure_high_nm(),
        purity: model.get_structure_purity(),
        coverage: model.get_structure_coverage(),
        seed: model.get_structure_seed().to_string(),
    }
}

/// Shows `controls` in the Play-of-Color controls. Setting a property from Rust fires no
/// callback.
pub(in crate::gui) fn write_controls(ui: &MainWindow, controls: &StructureControls) {
    let model = ui.global::<RenderEffectsModel>();
    model.set_structure_on(controls.on);
    model.set_structure_pattern(controls.pattern);
    model.set_structure_size_auto(controls.size_auto);
    model.set_structure_size(controls.size);
    model.set_structure_low_nm(controls.low_nm);
    model.set_structure_high_nm(controls.high_nm);
    model.set_structure_purity(controls.purity);
    model.set_structure_coverage(controls.coverage);
    model.set_structure_seed(SharedString::from(controls.seed.as_str()));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opal() -> StructureEffects {
        StructureEffects::from_spec(
            &StructureSpec::default_for("Precious Opal (White)").expect("spec"),
        )
    }

    #[test]
    fn an_off_structure_is_none_and_the_default_controls_are_off() {
        assert!(!StructureControls::default().on);
        assert_eq!(controls_for(None), StructureControls::default());
        assert_eq!(
            effects_from_controls(&StructureControls::default(), None),
            None
        );
    }

    #[test]
    fn controls_round_trip_to_the_same_choice() {
        let effects = StructureEffects {
            size: Some(0.123_456_7),
            purity: 0.061_234_5,
            seed: 4_000_000_000,
            ..opal()
        };
        let controls = controls_for(Some(&effects));
        assert!(controls.on && !controls.size_auto);
        assert_eq!(controls.seed, "4000000000");
        assert_eq!(
            effects_from_controls(&controls, Some(&effects)),
            Some(effects)
        );
        // Without the design's value the control's rounded one is written.
        let fresh = effects_from_controls(&controls, None).expect("on");
        assert_eq!(fresh.seed, 4_000_000_000);
    }

    #[test]
    fn the_materials_default_switches_the_group_on() {
        let opal = default_controls_for("Precious Opal (Black)").expect("an opal");
        assert!(opal.on && opal.size_auto);
        assert_eq!(opal.pattern, pattern_index(StructurePattern::Harlequin));
        let labradorite = default_controls_for("Labradorite").expect("labradorite");
        assert_eq!(
            labradorite.pattern,
            pattern_index(StructurePattern::BroadFlash)
        );
        assert_eq!(default_controls_for("Sapphire"), None);
        assert_eq!(title_for("Labradorite"), "Schiller");
        assert_eq!(title_for("Precious Opal (White)"), "Play-of-Color");
    }

    #[test]
    fn a_custom_materials_own_structure_is_found_and_a_built_ins_is_not() {
        let spec = StructureSpec::default_for("Labradorite").expect("spec");
        let plain = GemMaterial::new_custom("My Schiller", 1.56, 0.012, 0.008, [0.0; 3]);
        assert!(!has_own_structure(&plain));
        let own = plain.with_structure_spec_as(StructureKind::Labradorite, &spec);
        assert!(has_own_structure(&own));
        let mut named_like_a_built_in = own;
        named_like_a_built_in.name = "Labradorite".to_string();
        assert!(!has_own_structure(&named_like_a_built_in));
    }

    #[test]
    fn a_reseed_changes_only_the_seed() {
        let mut controls = default_controls_for("Labradorite").expect("labradorite");
        let before = controls.clone();
        reseed(&mut controls);
        assert_ne!(controls.seed, before.seed);
        assert_eq!(
            StructureControls {
                seed: before.seed.clone(),
                ..controls.clone()
            },
            before
        );
        let a = effects_from_controls(&before, None).expect("on");
        let b = effects_from_controls(&controls, None).expect("on");
        assert_ne!(a.seed, b.seed);
        assert_eq!(a.pattern, b.pattern);
    }

    #[test]
    fn the_pattern_index_round_trips() {
        for pattern in [
            StructurePattern::Pinfire,
            StructurePattern::Harlequin,
            StructurePattern::BroadFlash,
        ] {
            assert_eq!(pattern_from_index(pattern_index(pattern)), pattern);
        }
    }
}
