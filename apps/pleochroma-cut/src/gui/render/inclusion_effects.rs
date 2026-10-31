//! The Stone tab's Inclusions group: the conversion between its controls (`RenderEffectsModel`)
//! and the design's [`InclusionEffects`], and the facts the group needs about the material (its
//! presets, and which of them wait for a Second Axis).
//!
//! The controls are the authority while the tab shows: a write-back builds the design's choice
//! from them, keeping the design's own `f64` value wherever a control holds exactly its `f32`
//! (so an unchanged control writes no history entry). The pure conversions here carry the tests;
//! the Slint glue is in [`super::physics_effects_ui`].

use crate::{MainWindow, RenderEffectsModel};
use pleochroma::optics::{
    inclusions::{
        CrystalDir,
        catalogue::{self, PresetDef},
        spec::{DEFAULT_DENSITY, Sheen, SilkColor},
    },
    materials::GemMaterial,
};
use pleochroma_cut_core::{InclusionEffects, design::effects_spec::canonical_preset};
use slint::{ComponentHandle, Model, ModelRc, SharedString, VecModel};

/// The note under the group while a preset waits for a Second Axis.
pub(in crate::gui) const SECOND_AXIS_NOTE: &str = "Cat's-Eye Silk needs a Second Axis. Set the Crystal Axis for this stone and set its Second Axis first.";

/// The note under the group for a material the catalogue has no preset for.
pub(in crate::gui) const NO_PRESET_NOTE: &str = "This material has no inclusion presets.";

/// Whether `name` is a custom material's: not exactly a built-in's (`by_name` also matches a
/// built-in name inside a longer one, which would hide a custom material that starts with one).
#[must_use]
pub(in crate::gui) fn is_custom_name(name: &str) -> bool {
    !GemMaterial::all_materials()
        .iter()
        .any(|built_in| built_in.name.eq_ignore_ascii_case(name))
}

/// How many inclusion families a custom material carries of its own (authored in the Material
/// Editor and stored on its row); 0 for a built-in, which gets its inclusions from the catalogue
/// presets only.
#[must_use]
pub(in crate::gui) fn own_family_count(material: &GemMaterial) -> usize {
    if !is_custom_name(&material.name) {
        return 0;
    }
    material
        .inclusions
        .as_ref()
        .map_or(0, |set| set.families().len())
}

/// The read-only line the Inclusions group shows for a material's own families, "" for none.
#[must_use]
pub(in crate::gui) fn own_families_text(count: usize) -> String {
    match count {
        0 => String::new(),
        1 => "Material's Own Inclusions (1 Family)".to_string(),
        n => format!("Material's Own Inclusions ({n} Families)"),
    }
}

/// A value read off an `f32` control, rounded to six decimals like the design's stored values.
fn round6(value: f32) -> f64 {
    (f64::from(value) * 1e6).round() / 1e6
}

/// The `f32` a design value shows as on a control.
#[expect(
    clippy::cast_possible_truncation,
    reason = "the Slint controls are f32; the design keeps f64"
)]
pub(in crate::gui) const fn narrow(value: f64) -> f32 {
    value as f32
}

/// The design value for a control reading `control`: `base` when the control holds exactly
/// `base` at `f32` precision, else the control's value rounded to six decimals.
#[must_use]
pub(in crate::gui) fn keep_or(base: Option<f64>, control: f32) -> f64 {
    match base {
        Some(base) if narrow(base).to_bits() == control.to_bits() => base,
        _ => round6(control),
    }
}

/// What the Inclusions controls hold.
#[derive(Debug, Clone, PartialEq)]
pub(in crate::gui) struct InclusionControls {
    /// The preset, "" for none.
    pub(in crate::gui) preset: String,
    pub(in crate::gui) density: f32,
    pub(in crate::gui) turn_set: bool,
    pub(in crate::gui) turn_deg: f32,
    /// 0 the preset's own, 1 Soft, 2 Sharp.
    pub(in crate::gui) sheen: i32,
    /// 0 the preset's own, 1 Grey, 2 Slightly Blue, 3 Blue.
    pub(in crate::gui) color: i32,
}

impl Default for InclusionControls {
    fn default() -> Self {
        Self {
            preset: String::new(),
            density: DEFAULT_DENSITY,
            turn_set: false,
            turn_deg: 0.0,
            sheen: 0,
            color: 0,
        }
    }
}

/// The control index of a sheen choice.
#[must_use]
pub(in crate::gui) const fn sheen_index(sheen: Option<Sheen>) -> i32 {
    match sheen {
        None => 0,
        Some(Sheen::Soft) => 1,
        Some(Sheen::Sharp) => 2,
    }
}

/// The sheen choice of a control index (anything unknown is the preset's own).
#[must_use]
pub(in crate::gui) const fn sheen_from_index(index: i32) -> Option<Sheen> {
    match index {
        1 => Some(Sheen::Soft),
        2 => Some(Sheen::Sharp),
        _ => None,
    }
}

/// The control index of a silk color choice.
#[must_use]
pub(in crate::gui) const fn color_index(color: Option<SilkColor>) -> i32 {
    match color {
        None => 0,
        Some(SilkColor::Grey) => 1,
        Some(SilkColor::SlightlyBlue) => 2,
        Some(SilkColor::Blue) => 3,
    }
}

/// The silk color choice of a control index (anything unknown is the preset's own).
#[must_use]
pub(in crate::gui) const fn color_from_index(index: i32) -> Option<SilkColor> {
    match index {
        1 => Some(SilkColor::Grey),
        2 => Some(SilkColor::SlightlyBlue),
        3 => Some(SilkColor::Blue),
        _ => None,
    }
}

/// The controls that show `effects`.
#[must_use]
pub(in crate::gui) fn controls_for(effects: Option<&InclusionEffects>) -> InclusionControls {
    let Some(effects) = effects else {
        return InclusionControls::default();
    };
    InclusionControls {
        preset: effects.preset.to_string(),
        density: narrow(effects.density),
        turn_set: effects.turn_deg.is_some(),
        turn_deg: effects.turn_deg.map_or(0.0, narrow),
        sheen: sheen_index(effects.sheen),
        color: color_index(effects.color),
    }
}

/// The design's choice the controls stand for: `None` for no preset (or one the catalogue does
/// not know), else the choice with the design's own values kept where the controls still hold
/// them.
#[must_use]
pub(in crate::gui) fn effects_from_controls(
    controls: &InclusionControls,
    base: Option<&InclusionEffects>,
) -> Option<InclusionEffects> {
    let preset = canonical_preset(&controls.preset)?;
    Some(
        InclusionEffects {
            preset,
            density: keep_or(base.map(|b| b.density), controls.density),
            turn_deg: controls
                .turn_set
                .then(|| keep_or(base.and_then(|b| b.turn_deg), controls.turn_deg)),
            sheen: sheen_from_index(controls.sheen),
            color: color_from_index(controls.color),
        }
        .sanitised(),
    )
}

/// Whether a preset's families run along the `n_alpha` direction, so they mean something only
/// once the stone's Second Axis is set.
#[must_use]
pub(in crate::gui) fn needs_second_axis(preset: &PresetDef) -> bool {
    preset
        .families
        .iter()
        .any(|family| family.dir == CrystalDir::AlphaAxis)
}

/// The presets offered for `material_name` with whether each one waits for a Second Axis.
#[must_use]
pub(in crate::gui) fn preset_rows(
    material_name: &str,
    second_axis_set: bool,
) -> Vec<(&'static str, bool)> {
    catalogue::presets_for(material_name)
        .iter()
        .map(|preset| (preset.name, needs_second_axis(preset) && !second_axis_set))
        .collect()
}

/// The one plain sentence under the group: why a preset waits, that the material has none, or
/// that the saved preset is not one of this material's.
#[must_use]
pub(in crate::gui) fn note_for(rows: &[(&str, bool)], chosen: &str) -> &'static str {
    let chosen_row = rows
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(chosen));
    if rows.is_empty() {
        return if chosen.is_empty() {
            NO_PRESET_NOTE
        } else {
            "The saved inclusion preset is not one of this material's, so it is not drawn."
        };
    }
    if !chosen.is_empty() && chosen_row.is_none() {
        return "The saved inclusion preset is not one of this material's, so it is not drawn.";
    }
    if rows.iter().any(|(_, blocked)| *blocked) {
        return SECOND_AXIS_NOTE;
    }
    ""
}

/// Reads the Inclusions controls.
#[must_use]
pub(in crate::gui) fn read_controls(ui: &MainWindow) -> InclusionControls {
    let model = ui.global::<RenderEffectsModel>();
    InclusionControls {
        preset: model.get_inclusion_preset().to_string(),
        density: model.get_inclusion_density(),
        turn_set: model.get_inclusion_turn_set(),
        turn_deg: model.get_inclusion_turn_deg(),
        sheen: model.get_inclusion_sheen(),
        color: model.get_inclusion_color(),
    }
}

/// Shows `controls` in the Inclusions controls. Setting a property from Rust fires no callback.
pub(in crate::gui) fn write_controls(ui: &MainWindow, controls: &InclusionControls) {
    let model = ui.global::<RenderEffectsModel>();
    model.set_inclusion_preset(SharedString::from(controls.preset.as_str()));
    model.set_inclusion_density(controls.density);
    model.set_inclusion_turn_set(controls.turn_set);
    model.set_inclusion_turn_deg(controls.turn_deg);
    model.set_inclusion_sheen(controls.sheen);
    model.set_inclusion_color(controls.color);
}

/// Shows the preset rows, leaving the models alone when they already show them (a refresh per
/// slider tick must not rebuild the buttons).
pub(in crate::gui) fn show_preset_rows(ui: &MainWindow, rows: &[(&str, bool)]) {
    let model = ui.global::<RenderEffectsModel>();
    let shown: Vec<(String, bool)> = model
        .get_inclusion_preset_names()
        .iter()
        .zip(model.get_inclusion_preset_blocked().iter())
        .map(|(name, blocked)| (name.to_string(), blocked))
        .collect();
    let same = shown.len() == rows.len()
        && shown
            .iter()
            .zip(rows)
            .all(|((a, ab), (b, bb))| a.as_str() == *b && ab == bb);
    if same {
        return;
    }
    model.set_inclusion_preset_names(ModelRc::new(VecModel::from(
        rows.iter()
            .map(|(name, _)| SharedString::from(*name))
            .collect::<Vec<_>>(),
    )));
    model.set_inclusion_preset_blocked(ModelRc::new(VecModel::from(
        rows.iter().map(|(_, blocked)| *blocked).collect::<Vec<_>>(),
    )));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fine_silk() -> InclusionEffects {
        InclusionEffects::of_preset("Fine Silk").expect("preset")
    }

    #[test]
    fn controls_round_trip_to_the_same_choice() {
        let effects = InclusionEffects {
            density: 0.123_456_789,
            turn_deg: Some(33.333_333_3),
            sheen: Some(Sheen::Sharp),
            color: Some(SilkColor::Blue),
            ..fine_silk()
        };
        let controls = controls_for(Some(&effects));
        assert_eq!(controls.preset, "Fine Silk");
        assert!(controls.turn_set);
        // The design's own f64 values survive an f32 control: no spurious history entry.
        assert_eq!(
            effects_from_controls(&controls, Some(&effects)),
            Some(effects)
        );
    }

    #[test]
    fn no_preset_is_no_inclusions_and_a_default_choice_is_the_catalogue_strength() {
        assert_eq!(controls_for(None), InclusionControls::default());
        assert_eq!(
            effects_from_controls(&InclusionControls::default(), None),
            None
        );
        let controls = InclusionControls {
            preset: "rutile silk".to_string(),
            ..InclusionControls::default()
        };
        let effects = effects_from_controls(&controls, None).expect("a known preset");
        assert_eq!(effects.preset, "Rutile Silk");
        assert_eq!(narrow(effects.density).to_bits(), DEFAULT_DENSITY.to_bits());
        assert_eq!(effects.turn_deg, None);
    }

    #[test]
    fn a_moved_control_replaces_the_design_value() {
        let base = fine_silk();
        let mut controls = controls_for(Some(&base));
        controls.density = 0.6;
        controls.turn_set = true;
        controls.turn_deg = 45.0;
        let effects = effects_from_controls(&controls, Some(&base)).expect("preset");
        assert_eq!(narrow(effects.density).to_bits(), 0.6f32.to_bits());
        assert_eq!(effects.turn_deg, Some(45.0));
    }

    #[test]
    fn the_index_mappings_round_trip() {
        for sheen in [None, Some(Sheen::Soft), Some(Sheen::Sharp)] {
            assert_eq!(sheen_from_index(sheen_index(sheen)), sheen);
        }
        for color in [
            None,
            Some(SilkColor::Grey),
            Some(SilkColor::SlightlyBlue),
            Some(SilkColor::Blue),
        ] {
            assert_eq!(color_from_index(color_index(color)), color);
        }
    }

    #[test]
    fn cats_eye_silk_is_disabled_without_a_second_axis() {
        let blocked = preset_rows("Chrysoberyl (Yellow)", false);
        assert_eq!(blocked, vec![("Cat's-Eye Silk", true)]);
        assert_eq!(note_for(&blocked, ""), SECOND_AXIS_NOTE);
        let open = preset_rows("Chrysoberyl (Yellow)", true);
        assert_eq!(open, vec![("Cat's-Eye Silk", false)]);
        assert_eq!(note_for(&open, "Cat's-Eye Silk"), "");
        // The corundum silk runs in the basal plane and never waits.
        let sapphire = preset_rows("Sapphire", false);
        assert_eq!(sapphire.len(), 3);
        assert!(sapphire.iter().all(|(_, blocked)| !blocked));
    }

    /// A custom material carrying the first Sapphire preset's families as its own.
    fn custom_with_silk(name: &str) -> GemMaterial {
        let families = catalogue::presets_for("Sapphire")[0].families;
        let set = pleochroma::optics::inclusions::CrystalFrame::new(glam::Vec3::Y, None)
            .resolve(families);
        GemMaterial::new_custom(name, 1.77, 0.018, -0.008, [0.0; 3]).with_inclusions(set)
    }

    #[test]
    fn a_custom_materials_own_families_are_counted_and_a_built_ins_are_not() {
        let silky = custom_with_silk("My Silky Stone");
        let count = own_family_count(&silky);
        assert!(count > 0, "the preset's families reach the material");
        assert_eq!(own_family_count(&custom_with_silk("Sapphire")), 0);
        let plain = GemMaterial::new_custom("My Plain Stone", 1.77, 0.018, -0.008, [0.0; 3]);
        assert_eq!(own_family_count(&plain), 0);
        assert!(is_custom_name("My Silky Stone") && !is_custom_name("sapphire"));
    }

    #[test]
    fn the_own_families_line_counts_them() {
        assert_eq!(own_families_text(0), "");
        assert_eq!(own_families_text(1), "Material's Own Inclusions (1 Family)");
        assert_eq!(
            own_families_text(3),
            "Material's Own Inclusions (3 Families)"
        );
    }

    #[test]
    fn the_note_names_a_material_without_presets_and_a_stranded_preset() {
        assert_eq!(note_for(&preset_rows("Diamond", false), ""), NO_PRESET_NOTE);
        assert!(note_for(&preset_rows("Diamond", false), "Fine Silk").contains("not one of"));
        assert!(note_for(&preset_rows("Sapphire", false), "Rain").contains("not one of"));
    }
}
