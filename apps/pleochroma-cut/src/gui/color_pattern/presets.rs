//! The Presets menu of the Color Pattern window: the built-in presets (the templates, "Ametrine" and
//! "Watermelon Tourmaline"), written in code, and the ones the cutter saved (the vault's
//! `color_pattern_presets`, see `store`).
//!
//! A preset carries the band rows solved for the reference path of the material it was made on.
//! Applied to another material it keeps those rows and does not solve again.

use pleochroma_cut_core::material::pattern::{
    ColorPattern, PatternAxis, PatternColor, TemplateId, ZoneColor, template,
};

/// The menu's own first entry: it prompts and picks nothing.
pub(super) const MENU_PROMPT: &str = "My Presets";

/// Amethyst purple, `[L*, C*, h]`; the typical patterns' Split and Sectors take the same seed.
/// Deeper since 2026-11-03 (it was L* 48 C* 42), after dealer photos of ametrine.
const AMETHYST: [f64; 3] = [46.0, 45.0, 305.0];
/// Citrine yellow, `[L*, C*, h]`; the typical patterns' Split and Sectors take the same seed.
/// Softer and more golden since 2026-11-03 (it was L* 82 C* 72 h 88), after the same photos.
const CITRINE: [f64; 3] = [84.0, 58.0, 85.0];

/// One entry of the menu.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Preset {
    /// The name shown in the menu.
    pub name: String,
    /// The pattern it applies.
    pub pattern: ColorPattern,
}

/// A one-color zone with `lch` and no band rows yet.
const fn unsolved(lch: [f64; 3]) -> PatternColor {
    PatternColor::Single(ZoneColor::unsolved(lch))
}

/// The presets that ship with the program: every template, then "Ametrine" (two halves, amethyst
/// and citrine) and "Watermelon Tourmaline" (a pink core in a green rim).
pub(super) fn built_in_presets() -> Vec<Preset> {
    let mut presets: Vec<Preset> = TemplateId::ALL
        .into_iter()
        .map(|id| Preset {
            name: id.label().to_owned(),
            pattern: template(id, PatternAxis::TABLE_NORMAL),
        })
        .collect();
    let mut ametrine = template(TemplateId::Split, PatternAxis::TABLE_NORMAL);
    "Amethyst".clone_into(&mut ametrine.base.name);
    ametrine.base.color = unsolved(AMETHYST);
    if let Some(zone) = ametrine.zones.first_mut() {
        "Citrine".clone_into(&mut zone.name);
        zone.color = unsolved(CITRINE);
    }
    presets.push(Preset {
        name: "Ametrine".to_owned(),
        pattern: ametrine,
    });
    presets.push(Preset {
        name: "Watermelon Tourmaline".to_owned(),
        pattern: template(TemplateId::Watermelon, PatternAxis::TABLE_NORMAL),
    });
    presets
}

/// Every entry of the menu below its prompt: the built-in presets, then the saved ones.
pub(super) fn all_presets(saved: &[(String, ColorPattern)]) -> Vec<Preset> {
    let mut all = built_in_presets();
    all.extend(saved.iter().map(|(name, pattern)| Preset {
        name: name.clone(),
        pattern: pattern.clone(),
    }));
    all
}

/// The labels of the menu: the prompt, then every preset.
pub(super) fn menu_labels(saved: &[(String, ColorPattern)]) -> Vec<String> {
    std::iter::once(MENU_PROMPT.to_owned())
        .chain(all_presets(saved).into_iter().map(|preset| preset.name))
        .collect()
}

/// The preset at menu index `index` (0 is the prompt and picks nothing).
pub(super) fn preset_at(saved: &[(String, ColorPattern)], index: i32) -> Option<Preset> {
    let at = usize::try_from(index).ok()?.checked_sub(1)?;
    all_presets(saved).into_iter().nth(at)
}

/// Whether `name` is the name of a built-in preset (ASCII case ignored): a saved preset may not
/// take it.
pub(super) fn is_built_in_name(name: &str) -> bool {
    built_in_presets()
        .iter()
        .any(|preset| preset.name.eq_ignore_ascii_case(name.trim()))
}

/// A name for a new preset of `pattern` that no preset has yet: "Watermelon 1", "Watermelon 2".
pub(super) fn suggested_name(pattern: &ColorPattern, saved: &[(String, ColorPattern)]) -> String {
    let stem = pattern
        .template
        .map_or("My Pattern", |template| template.label());
    let taken = |name: &str| {
        is_built_in_name(name)
            || saved
                .iter()
                .any(|(existing, _)| existing.eq_ignore_ascii_case(name))
    };
    // Fewer than `taken_count + 1` candidates cannot all be taken (pigeonhole).
    let taken_count = built_in_presets().len() + saved.len();
    (1..=taken_count + 1)
        .map(|n| format!("{stem} {n}"))
        .find(|name| !taken(name))
        .unwrap_or_else(|| format!("{stem} {}", taken_count + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_menu_lists_the_prompt_the_built_ins_and_the_saved_presets() {
        let saved = vec![(
            "Mine".to_owned(),
            template(TemplateId::Banded, PatternAxis::TABLE_NORMAL),
        )];
        let labels = menu_labels(&saved);
        assert_eq!(labels[0], MENU_PROMPT);
        assert_eq!(labels.len(), 1 + TemplateId::ALL.len() + 2 + 1);
        assert_eq!(labels.last().map(String::as_str), Some("Mine"));
        assert!(labels.contains(&"Ametrine".to_owned()));
        assert!(labels.contains(&"Watermelon Tourmaline".to_owned()));
    }

    #[test]
    fn the_prompt_picks_nothing_and_a_label_picks_its_preset() {
        let saved = Vec::new();
        assert!(preset_at(&saved, 0).is_none());
        assert!(preset_at(&saved, -1).is_none());
        let first = preset_at(&saved, 1).unwrap();
        assert_eq!(first.name, TemplateId::ALL[0].label());
        let last = preset_at(
            &saved,
            i32::try_from(menu_labels(&saved).len() - 1).unwrap(),
        )
        .unwrap();
        assert_eq!(last.name, "Watermelon Tourmaline");
        assert!(preset_at(&saved, 99).is_none());
    }

    #[test]
    fn ametrine_is_two_halves_in_amethyst_and_citrine() {
        let presets = built_in_presets();
        let ametrine = presets.iter().find(|p| p.name == "Ametrine").unwrap();
        assert_eq!(ametrine.pattern.zones.len(), 1);
        assert_eq!(ametrine.pattern.base.name, "Amethyst");
        assert_eq!(ametrine.pattern.zones[0].name, "Citrine");
    }

    #[test]
    fn a_suggested_name_avoids_every_taken_name() {
        let pattern = template(TemplateId::Watermelon, PatternAxis::TABLE_NORMAL);
        let saved = vec![("watermelon 1".to_owned(), pattern.clone())];
        assert_eq!(suggested_name(&pattern, &saved), "Watermelon 2");
        assert!(is_built_in_name(" ametrine "));
        assert!(!is_built_in_name("Mine"));
    }
}
