//! Parses the preform-edit form and the New Design dialog's own fields into
//! `pleochroma-cut-core` spec types, and words the note the Preform tab shows for a
//! planned piece of rough.

use super::number_expr::eval_number;
use crate::material::material_name_from_index;
use pleochroma_cut_core::{FreshDesignSpec, MaterialSelection, PreformShape, PreformSpec};

/// The note the Preform tab shows under the shape choice when the design's rough is a
/// planned piece from the Rough Planner (a plane set), and `""` for a block or cylinder.
///
/// It names where the piece came from (the set's source, when it has one) and how many
/// planes it has, and says that the three size fields show its bounding box and that
/// Apply Preform replaces it with the shape chosen above.
#[must_use]
pub fn preform_source_note(preform: &PreformSpec) -> String {
    let PreformShape::Planes(set) = &preform.shape else {
        return String::new();
    };
    let count = set.planes.len();
    let origin = if set.source.is_empty() {
        format!("Planned piece ({count} planes)")
    } else {
        format!(
            "Planned piece from the Rough Planner: {} ({count} planes)",
            set.source
        )
    };
    format!(
        "{origin}. The fields show its bounding box; Apply Preform replaces it with the shape above."
    )
}

/// Parses the preform form's shape choice and three dimension fields into a
/// [`PreformSpec`].
///
/// `cylinder_sides` is the caller's fixed choice, not read from the design, so a reapply
/// after a gear change keeps it. This differs from
/// [`pleochroma_cut_core::PreformSpec::cylinder_for_schedule`], which sets the side count to
/// the index gear for a preform rebuilt from an authored schedule.
///
/// # Errors
///
/// A message naming the field that is not a number, or saying the dimensions must
/// be positive and finite.
pub fn parse_preform_form(
    shape_index: i32,
    half_width: &str,
    length_over_width: &str,
    depth: &str,
    cylinder_sides: usize,
) -> Result<PreformSpec, String> {
    let half_width =
        eval_number(half_width, None).map_err(|error| error.message("Half-width", half_width))?;
    let length_over_width = eval_number(length_over_width, None)
        .map_err(|error| error.message("Length/width", length_over_width))?;
    let depth = eval_number(depth, None).map_err(|error| error.message("Depth", depth))?;
    if !(half_width.is_finite() && length_over_width.is_finite() && depth.is_finite())
        || half_width <= 0.0
        || length_over_width <= 0.0
        || depth <= 0.0
    {
        return Err("Preform dimensions must be positive, finite numbers.".to_string());
    }

    Ok(if shape_index == 0 {
        PreformSpec::block(half_width, length_over_width, depth)
    } else {
        PreformSpec::cylinder(cylinder_sides, half_width, length_over_width, depth)
    })
}

/// Builds a [`FreshDesignSpec`] from the New Design dialog's fields.
///
/// The spec is for `pleochroma_cut_core::Design::fresh_from_spec`. It takes an already-resolved
/// gear tooth count and [`PreformSpec`], with the symmetry-order, mirror and material fields.
///
/// The caller resolves the gear and the preform first (`gear_choice_to_teeth`, then
/// [`parse_preform_form`]), since a cylinder preform's fold count needs the gear. They stay
/// separate calls to keep this function under clippy's `too_many_arguments`.
///
/// `symmetry_order_text` must be a positive whole number. `material_index` reuses
/// [`material_name_from_index`] (the fixed built-in list); the material is only a starting
/// point that the design settings can change afterwards.
///
/// # Errors
///
/// A message when `symmetry_order_text` is not a positive whole number.
pub fn parse_new_design_form(
    gear_teeth: i32,
    preform: PreformSpec,
    symmetry_order_text: &str,
    mirror: bool,
    material_index: i32,
) -> Result<FreshDesignSpec, String> {
    let symmetry_order: u32 = symmetry_order_text.trim().parse().map_err(|_| {
        format!(
            "Symmetry order '{}' is not a whole number.",
            symmetry_order_text.trim()
        )
    })?;
    if symmetry_order < 1 {
        return Err("Symmetry order must be a positive whole number.".to_string());
    }
    Ok(FreshDesignSpec {
        gear_teeth,
        symmetry_order,
        mirror,
        material: MaterialSelection {
            name: material_name_from_index(material_index),
            specific_gravity_override: None,
            refractive_index_override: None,
            body_color_override: None,
            body_color_bands_override: None,
            absorption_path_scale_override: None,
            color_pattern_override: None,
            color_variant_override: None,
        },
        preform,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_block_or_cylinder_has_no_source_note() {
        assert_eq!(preform_source_note(&PreformSpec::block(1.0, 1.0, 1.0)), "");
        assert_eq!(
            preform_source_note(&PreformSpec::cylinder(96, 1.0, 1.0, 1.0)),
            ""
        );
    }

    #[test]
    fn a_planned_piece_names_its_source_and_plane_count() {
        let planes = PreformSpec::block(1.0, 1.0, 1.0).planes();
        let named = PreformSpec::from_planes(planes.clone(), "plan 'Aqua', stone 1".to_string())
            .expect("a block's planes close");
        assert_eq!(
            preform_source_note(&named),
            "Planned piece from the Rough Planner: plan 'Aqua', stone 1 (6 planes). The fields \
             show its bounding box; Apply Preform replaces it with the shape above."
        );
        let unnamed = PreformSpec::from_planes(planes, String::new()).expect("closes");
        assert_eq!(
            preform_source_note(&unnamed),
            "Planned piece (6 planes). The fields show its bounding box; Apply Preform replaces \
             it with the shape above."
        );
    }

    #[test]
    fn parse_preform_form_builds_a_block_at_index_zero() {
        let preform = parse_preform_form(0, "1.2", "1.5", "0.8", 96).unwrap();
        assert_eq!(preform.shape, PreformShape::Block);
        assert_eq!(preform.half_width, 1.2);
        assert_eq!(preform.length_over_width, 1.5);
        assert_eq!(preform.depth, 0.8);
    }

    #[test]
    fn parse_preform_form_takes_arithmetic_in_every_dimension() {
        let preform = parse_preform_form(0, "2.4 / 2", "1 + 0.5", "(0.4 + 0.4)", 96).unwrap();
        assert_eq!(preform.half_width, 1.2);
        assert_eq!(preform.length_over_width, 1.5);
        assert_eq!(preform.depth, 0.8);
        let message = parse_preform_form(0, "1", "1 / 0", "1", 96).unwrap_err();
        assert_eq!(
            message,
            "Length/width '1 / 0' cannot be calculated: it divides by zero."
        );
        let message = parse_preform_form(0, "wide", "1", "1", 96).unwrap_err();
        assert_eq!(message, "Half-width 'wide' is not a number.");
    }

    #[test]
    fn parse_preform_form_builds_a_cylinder_with_the_given_side_count() {
        let preform = parse_preform_form(1, "1.0", "1.0", "0.8", 64).unwrap();
        assert_eq!(preform.shape, PreformShape::Cylinder { sides: 64 });
    }

    #[test]
    fn parse_preform_form_rejects_zero_or_negative_dimensions() {
        assert!(parse_preform_form(0, "0.0", "1.0", "0.8", 96).is_err());
        assert!(parse_preform_form(0, "1.0", "-1.0", "0.8", 96).is_err());
        assert!(parse_preform_form(0, "1.0", "1.0", "0.0", 96).is_err());
    }

    #[test]
    fn parse_preform_form_rejects_unparseable_text() {
        assert!(parse_preform_form(0, "wide", "1.0", "0.8", 96).is_err());
    }

    // --- parse_new_design_form ---

    #[test]
    fn parse_new_design_form_round_trips_gear_symmetry_mirror_and_material() {
        let preform = PreformSpec::cylinder(80, 1.5, 1.0, 1.5);
        let quartz = crate::material::builtin_preset_names()
            .iter()
            .position(|name| name == "Quartz")
            .and_then(|index| i32::try_from(index).ok())
            .expect("Quartz is a built-in preset");
        let spec = parse_new_design_form(80, preform, "6", false, quartz).unwrap();
        assert_eq!(spec.gear_teeth, 80);
        assert_eq!(spec.symmetry_order, 6);
        assert!(!spec.mirror);
        assert_eq!(spec.material.name.as_deref(), Some("Quartz"));
        assert_eq!(spec.preform.half_width, 1.5);
    }

    #[test]
    fn parse_new_design_form_none_material_index_means_no_material_selected() {
        let preform = PreformSpec::cylinder(50, 1.0, 1.0, 1.0);
        let spec = parse_new_design_form(50, preform, "8", true, 0).unwrap();
        assert_eq!(spec.gear_teeth, 50);
        assert_eq!(spec.material.name, None);
    }

    #[test]
    fn parse_new_design_form_rejects_a_non_positive_symmetry_order() {
        let preform = PreformSpec::cylinder(96, 1.0, 1.0, 1.0);
        assert!(parse_new_design_form(96, preform.clone(), "0", true, 0).is_err());
        assert!(parse_new_design_form(96, preform, "wide", true, 0).is_err());
    }
}
