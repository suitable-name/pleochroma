//! Opens the existing pattern editor on one stone without creating a material.

use super::{
    colors::lch_of_rows,
    model::f32_of,
    session::deps,
    wiring::{Opening, open_session},
};
use crate::{MainWindow, gui::show_toast};
use pleochroma::{
    optics::{
        absorption::AbsorptionTensor,
        materials::{AbsorptionUnit, GemMaterial},
        zoning::{ZoneAbsorption, ZonedAbsorption},
    },
    render_setup::absorption_path_scale_for,
};
use pleochroma_cut_core::material::pattern::{ColorPattern, OwnColor, pattern_holds_tensor};
use pleochroma_editor::lch_color::reference_path_mm;
use std::rc::Rc;

/// A guarded, undoable stone edit supplied by the editor or library-preview controller.
pub type StonePatternApply = dyn Fn(&ColorPattern) -> Result<(), String>;

/// The stone and pattern captured when its Color Pattern button was pressed.
pub struct StonePatternOpening {
    /// Human-readable target name for the window title.
    pub label: String,
    /// The stored override, or a lossless seed made with [`seed_stone_pattern`].
    pub pattern: ColorPattern,
    /// The stone's effective width, millimetres.
    pub stone_width_mm: f64,
    /// Optics of the currently displayed stone, before the new pattern is applied.
    pub base_material: GemMaterial,
    /// Whether the stone's material supports two colors per zone.
    pub two_colors_allowed: bool,
    /// The color the stone has without a pattern, per direction, which a typical pattern is built
    /// on (`None`: worked out from the base material and the pattern's own base zone).
    pub own: Option<OwnColor>,
    /// True for a saved/undoable design edit, false for a view-only preview.
    pub targets_design: bool,
    /// Must refuse an application when the captured stone or material is no longer current.
    pub apply: Rc<StonePatternApply>,
}

/// Opens the pattern editor on a captured stone target.
pub fn open_stone_pattern(ui: &MainWindow, opening: StonePatternOpening) {
    let Some(deps) = deps() else {
        show_toast(ui, "The color pattern editor is not ready yet.", "warning");
        return;
    };
    let note = if opening.targets_design {
        "Changes apply only to this stone and can be undone. Save the design to keep them."
    } else {
        "Preview only: changes apply to this library or material preview, not to a saved design."
    };
    open_session(
        ui,
        &deps,
        Opening {
            material_name: opening.label,
            stone_apply: Some(opening.apply),
            targets_design: opening.targets_design,
            pattern: opening.pattern,
            import_note: Some(note.to_owned()),
            stone_width_mm: opening.stone_width_mm,
            base_material: opening.base_material,
            two_colors_allowed: opening.two_colors_allowed,
            own: opening.own,
        },
    );
}

/// Checks that a pattern can hold the tensor without losing color: as a picked color when the
/// band-row format carries every band, else as a color variant of exact rows (`pattern_color`).
/// Only a tensor with an unusable number (a negative peak, a zero centre) is refused.
fn representable(tensor: &AbsorptionTensor) -> Result<(), String> {
    if pattern_holds_tensor(tensor) {
        Ok(())
    } else {
        Err("This material's absorption bands cannot be edited as a color pattern without changing their shape.".to_owned())
    }
}

/// Builds a pattern from the displayed material, preserving its optical density.
///
/// `model_width` is needed only to convert a model-unit body color to per-millimetre bands.
/// Existing zones already use millimetres and keep their original frame and geometry.
///
/// # Errors
/// Returns a reason when the color cannot be represented without loss or its scale is unknown.
pub fn seed_stone_pattern(
    material: &GemMaterial,
    stone_width_mm: f64,
    model_width: Option<f64>,
) -> Result<ColorPattern, String> {
    let zoning = if let Some(zoning) = &material.zoning {
        zoning.clone()
    } else {
        let mut tensor = material.absorption.clone();
        if material.absorption_unit == AbsorptionUnit::ModelUnit {
            let width = f32_of(stone_width_mm);
            let scale = absorption_path_scale_for(AbsorptionUnit::ModelUnit, width, model_width)
                .zip(absorption_path_scale_for(
                    AbsorptionUnit::PerMm,
                    width,
                    model_width,
                ))
                .map(|(old, new)| old / new)
                .filter(|scale| scale.is_finite() && *scale > 0.0)
                .ok_or_else(|| {
                    "Wait for the stone geometry before editing its color pattern.".to_owned()
                })?;
            for band in tensor
                .o_ray
                .iter_mut()
                .chain(&mut tensor.e_ray)
                .chain(tensor.beta_ray.iter_mut().flatten())
            {
                band.peak *= scale;
            }
        }
        ZonedAbsorption::new(ZoneAbsorption::per_mm(tensor))
    };
    // A third (beta) color axis becomes a three-color zone; every band set must survive the row
    // encoding without changing its shape.
    for absorption in
        std::iter::once(&zoning.base).chain(zoning.zones.iter().map(|zone| &zone.absorption))
    {
        representable(&absorption.tensor)?;
    }
    zoning.validate().map_err(|error| error.to_string())?;
    let path = reference_path_mm(Some(stone_width_mm));
    let pattern = ColorPattern::from_zoned(&zoning, false, |rows| lch_of_rows(rows, path));
    pattern.to_zoned().map_err(|error| error.to_string())?;
    Ok(pattern)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_cut_core::material::pattern::{PatternAxis, apply_color_pattern};

    #[test]
    fn an_ametrine_seed_keeps_its_original_colors_and_boundaries() {
        let original = GemMaterial::ametrine();
        let pattern = seed_stone_pattern(&original, 7.0, None).expect("per-mm zones");
        assert!(!pattern.relative_to_stone);
        assert_eq!(pattern.axis, PatternAxis::TABLE_NORMAL);
        assert_eq!(
            pattern.axis.frame(),
            original.zoning.as_ref().unwrap().frame
        );
        assert_eq!(pattern.to_zoned().expect("zones"), original.zoning);
        assert_eq!(
            GemMaterial::ametrine(),
            original,
            "the preset remains untouched"
        );
    }

    #[test]
    fn a_model_unit_seed_preserves_density_at_the_same_stone_size() {
        let original = GemMaterial::sapphire();
        let model_width = Some(2.4);
        let width = 12.0;
        let pattern = seed_stone_pattern(&original, width, model_width).expect("known scale");
        let changed = apply_color_pattern(original.clone(), &pattern, width).expect("applies");
        let before_scale =
            absorption_path_scale_for(original.absorption_unit, 12.0, model_width).unwrap();
        let after_scale =
            absorption_path_scale_for(changed.absorption_unit, 12.0, model_width).unwrap();
        for (before, after) in original
            .absorption
            .o_ray
            .iter()
            .zip(&changed.absorption.o_ray)
        {
            let before = before.peak * before_scale;
            let after = after.peak * after_scale;
            assert!((before - after).abs() <= 1e-5 * before.abs().max(1.0));
        }
        assert_eq!(
            original.absorption.o_ray.len(),
            changed.absorption.o_ray.len()
        );
        assert!(seed_stone_pattern(&original, width, None).is_err());
    }

    #[test]
    fn every_built_in_material_opens_as_a_pattern_without_losing_a_band() {
        use pleochroma_cut_core::material::pattern::{PatternColor, rows_carry_tensor};
        let mut exact = 0;
        for material in GemMaterial::built_in() {
            let pattern = seed_stone_pattern(material, 7.0, Some(2.0))
                .unwrap_or_else(|error| panic!("{}: {error}", material.name));
            apply_color_pattern(material.clone(), &pattern, 7.0)
                .unwrap_or_else(|error| panic!("{}: {error}", material.name));
            if material.zoning.is_some() {
                continue;
            }
            // The pattern is the stone's own color: every band is in it, picked or exact.
            if rows_carry_tensor(&material.absorption) {
                assert!(pattern.base.color.is_custom(), "{}", material.name);
            } else {
                exact += 1;
                assert!(
                    matches!(pattern.base.color, PatternColor::Variant { .. }),
                    "{}",
                    material.name
                );
            }
        }
        assert!(exact > 0, "some built-in material has a narrow energy band");
    }

    #[test]
    fn a_colorless_stone_is_a_solved_uniform_pattern() {
        let pattern = seed_stone_pattern(&GemMaterial::diamond(), 7.0, Some(2.0)).unwrap();
        assert!(pattern.unsolved_colors().is_empty());
        assert!(pattern.to_zoned().unwrap().is_none());
    }

    #[test]
    fn a_third_color_axis_becomes_a_three_color_zone_instead_of_being_refused() {
        use pleochroma_cut_core::material::pattern::PatternColor;
        let mut material = GemMaterial::sapphire();
        material.absorption.beta_ray = Some(material.absorption.o_ray.clone());
        material.absorption.is_pleochroic = true;
        let pattern =
            seed_stone_pattern(&material, 7.0, Some(2.0)).expect("three colors are representable");
        let PatternColor::Trichroic { alpha, beta, gamma } = &pattern.base.color else {
            panic!("a trichroic tensor seeds three colors");
        };
        // The model-unit conversion scales the third set like the other two.
        assert_eq!(alpha.bands, beta.bands);
        assert!(gamma.is_solved());
        let changed = apply_color_pattern(material.clone(), &pattern, 7.0).expect("applies");
        assert!(changed.absorption.beta_ray.is_some());
        assert_eq!(
            changed.absorption.is_pleochroic,
            material.absorption.is_pleochroic
        );
    }

    #[test]
    fn an_imported_offset_frame_is_preserved_instead_of_reinterpreted_as_a_preset() {
        let mut material = GemMaterial::ametrine();
        material.zoning.as_mut().unwrap().frame.translation.x = 1.25;
        let pattern = seed_stone_pattern(&material, 7.0, None).expect("imported zones");
        assert!(matches!(pattern.axis, PatternAxis::Frame { .. }));
        assert_eq!(pattern.to_zoned().unwrap(), material.zoning);
    }
}
