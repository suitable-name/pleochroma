//! Converting a startup `get_custom_materials()` row read into the render context's
//! custom-material, specific-gravity and gem-data lists.

use crate::gui::optics::crystal_optics::{
    custom_material_gem_data_from_rows, custom_material_specific_gravity_from_rows,
    gem_material_from_row,
};
use pleochroma::optics::materials::GemMaterial;
use pleochroma_editor::material_reference::CustomGemData;

/// Converts the startup `get_custom_materials()` rows into both
/// `RenderContext::custom_materials` and `RenderContext::custom_material_specific_gravity`.
///
/// Both lists come from the same rows, so they never disagree. The specific-gravity list is
/// filled at startup, not only after a save or delete, so a custom material's carat estimate
/// is available from the first browse. Kept as a pure function so it can be unit-tested
/// without a `MainWindow`.
#[must_use]
pub(super) fn initial_custom_materials_and_specific_gravity(
    rows: &[pleochroma_vault::model::material::CustomMaterialRow],
) -> (Vec<GemMaterial>, Vec<(String, f64)>) {
    let materials = rows.iter().map(gem_material_from_row).collect();
    let specific_gravity = custom_material_specific_gravity_from_rows(rows);
    (materials, specific_gravity)
}

/// The startup rows' entered gem data as `RenderContext::custom_material_gem_data`: one entry for
/// each row that holds at least one usable value. Filled at startup with the other two lists, so
/// the Stone tab card and the Material Reference show a custom material's data from the first
/// browse.
#[must_use]
pub(super) fn initial_custom_gem_data(
    rows: &[pleochroma_vault::model::material::CustomMaterialRow],
) -> Vec<(String, CustomGemData)> {
    custom_material_gem_data_from_rows(rows)
}

#[cfg(test)]
mod tests {
    use super::initial_custom_materials_and_specific_gravity;
    use pleochroma_vault::model::material::CustomMaterialRow;

    fn custom_row(name: &str, specific_gravity: Option<f32>) -> CustomMaterialRow {
        CustomMaterialRow {
            name: name.to_string(),
            refractive_index: 1.7,
            dispersion: 0.02,
            birefringence: 0.0,
            absorption_rgb: [0.0, 0.0, 0.0],
            crystal_system: None,
            optical_character: None,
            biaxial_delta_beta_alpha: None,
            per_axis_dispersion_json: None,
            specific_gravity,
            color_recipe_json: None,
            dispersion_model_json: None,
            absorption_bands_json: None,
            c_axis_tilt_deg: None,
            c_axis_azimuth_deg: None,
            c_axis_second_azimuth_deg: None,
            inclusion_families_json: None,
            structure_json: None,
            gem_data_json: None,
        }
    }

    // --- initial_custom_gem_data ---

    /// Only a row with usable gem data adds an entry; an unreadable text is skipped.
    #[test]
    fn startup_gem_data_lists_only_rows_with_usable_values() {
        let mut with_data = custom_row("My Garnet", None);
        with_data.gem_data_json =
            Some(r#"{"mohs":[7.0,7.5],"cleavage":"None","toughness":["good","excellent"]}"#.into());
        let mut broken = custom_row("Broken", None);
        broken.gem_data_json = Some("not json".into());
        let mut empty = custom_row("Empty", None);
        empty.gem_data_json = Some("{}".into());
        let rows = [with_data, broken, empty, custom_row("Plain", None)];

        let data = super::initial_custom_gem_data(&rows);

        assert_eq!(data.len(), 1);
        assert_eq!(data[0].0, "My Garnet");
        assert_eq!(data[0].1.mohs, Some((7.0, 7.5)));
        assert_eq!(data[0].1.cleavage.as_deref(), Some("None"));
        assert!(data[0].1.toughness.is_some());
    }

    // --- initial_custom_materials_and_specific_gravity ---

    /// Every row becomes a `GemMaterial`; only rows with a recorded specific gravity add an
    /// entry to the second list.
    #[test]
    fn startup_helper_populates_both_lists_from_one_row_read() {
        let rows = [
            custom_row("My Garnet", Some(3.90)),
            custom_row("Custom Diamond", None),
        ];
        let (materials, specific_gravity) = initial_custom_materials_and_specific_gravity(&rows);

        assert_eq!(materials.len(), 2, "every row becomes a GemMaterial");
        assert!(materials.iter().any(|m| m.name == "My Garnet"));
        assert!(materials.iter().any(|m| m.name == "Custom Diamond"));

        // The `f32` to `f64` widening is not exact for a literal, so compare by name and
        // within a tolerance rather than with `assert_eq!` on a `vec![...]` literal.
        assert_eq!(
            specific_gravity.len(),
            1,
            "only the row with a recorded SG contributes an entry, and a row \
             with none is skipped rather than showing up as a spurious Some(0.0)"
        );
        assert_eq!(specific_gravity[0].0, "My Garnet");
        assert!((specific_gravity[0].1 - 3.90_f64).abs() < 1e-6);
    }

    /// An empty catalogue gives two empty lists, with no placeholder entry.
    #[test]
    fn startup_helper_is_empty_for_no_custom_materials() {
        let (materials, specific_gravity) = initial_custom_materials_and_specific_gravity(&[]);
        assert_eq!(materials.len(), 0);
        assert_eq!(specific_gravity.len(), 0);
    }
}
