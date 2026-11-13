//! The presets of the Color Pattern window: the built-in presets (the templates, "Ametrine" and
//! "Watermelon Tourmaline"), written in code, and the ones the cutter saved (the vault's
//! `color_pattern_presets`, see `store`). The gallery's My Presets section lists the built-in
//! ones that are not templates, then the saved ones, and only a saved one can be deleted.
//!
//! A preset carries the band rows solved for the reference path of the material it was made on.
//! Applied to another material it keeps those rows and does not solve again.

use pleochroma_cut_core::material::pattern::{
    ColorPattern, PatternAxis, PatternColor, TemplateId, ZoneColor, template,
};

/// Amethyst purple, `[L*, C*, h]`; the typical patterns' Split and Sectors take the same seed.
const AMETHYST: [f64; 3] = [46.0, 45.0, 305.0];
/// Citrine yellow, `[L*, C*, h]`; the typical patterns' Split and Sectors take the same seed.
const CITRINE: [f64; 3] = [84.0, 58.0, 85.0];

/// One preset.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Preset {
    /// The name shown in the gallery.
    pub name: String,
    /// The pattern it applies.
    pub pattern: ColorPattern,
    /// It ships with the program, so it cannot be deleted.
    pub built_in: bool,
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
            built_in: true,
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
        built_in: true,
    });
    presets.push(Preset {
        name: "Watermelon Tourmaline".to_owned(),
        pattern: template(TemplateId::Watermelon, PatternAxis::TABLE_NORMAL),
        built_in: true,
    });
    presets
}

/// The presets of the gallery's My Presets section: the built-in ones that are no template (the
/// templates have their own section, so a preset that is one would show twice), then the saved
/// ones. A tile index of the gallery is an index into this list.
pub(super) fn gallery_presets(saved: &[(String, ColorPattern)]) -> Vec<Preset> {
    let templates: Vec<ColorPattern> = TemplateId::ALL
        .into_iter()
        .map(|id| template(id, PatternAxis::TABLE_NORMAL))
        .collect();
    let mut all: Vec<Preset> = built_in_presets()
        .into_iter()
        .filter(|preset| !templates.contains(&preset.pattern))
        .collect();
    all.extend(saved.iter().map(|(name, pattern)| Preset {
        name: name.clone(),
        pattern: pattern.clone(),
        built_in: false,
    }));
    all
}

/// The preset of gallery tile `index`.
pub(super) fn gallery_preset_at(saved: &[(String, ColorPattern)], index: i32) -> Option<Preset> {
    let at = usize::try_from(index).ok()?;
    gallery_presets(saved).into_iter().nth(at)
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
    fn the_gallery_lists_the_built_ins_that_are_no_template_then_the_saved_presets() {
        let saved = vec![(
            "Mine".to_owned(),
            template(TemplateId::Banded, PatternAxis::TABLE_NORMAL),
        )];
        let presets = gallery_presets(&saved);
        let names: Vec<&str> = presets.iter().map(|preset| preset.name.as_str()).collect();
        // Watermelon Tourmaline is the Watermelon template, which has its own section.
        assert_eq!(names, ["Ametrine", "Mine"]);
        assert!(presets[0].built_in);
        assert!(!presets[1].built_in);
    }

    #[test]
    fn a_tile_index_picks_its_preset() {
        let saved = vec![(
            "Mine".to_owned(),
            template(TemplateId::Banded, PatternAxis::TABLE_NORMAL),
        )];
        assert_eq!(gallery_preset_at(&saved, 0).unwrap().name, "Ametrine");
        assert_eq!(gallery_preset_at(&saved, 1).unwrap().name, "Mine");
        assert!(gallery_preset_at(&saved, 2).is_none());
        assert!(gallery_preset_at(&saved, -1).is_none());
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
