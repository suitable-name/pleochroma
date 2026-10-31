//! How a scene's crystal orientation meets the material it is applied to.
//!
//! A custom material can carry a default orientation baked into its principal frame (a Crystal
//! Axis and a Second Axis, `pleochroma_cut_core::material::with_default_orientation`). When a
//! scene then sets a Crystal Axis of its own but no Second Axis, the material's baked Second
//! Axis belongs to the OLD Crystal Axis and must not be kept: the stone gets the material's own
//! completion for the new axis (turn zero), the rule `pleochroma_editor::stone_orientation`
//! applies to the editor's scoring scenes.

use pleochroma::{
    optics::{birefringence::alpha_axis_from_turn, materials::GemMaterial},
    render_setup::MaterialOverrides,
};

/// `overrides` with the Second Axis of turn zero added when the scene sets a Crystal Axis but
/// no Second Axis, and `base` holds a baked Second Axis. Every other case is returned unchanged,
/// so a scene without a baked default renders exactly as before.
#[must_use]
pub(super) fn with_baked_second_axis(
    mut overrides: MaterialOverrides,
    base: &GemMaterial,
) -> MaterialOverrides {
    if let Some(gamma) = overrides.c_axis_override
        && overrides.alpha_axis_override.is_none()
        && base.alpha_axis.is_some()
    {
        overrides.alpha_axis_override = Some(alpha_axis_from_turn(gamma, 0.0));
    }
    overrides
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::Vec3;

    fn tanzanite_with_baked_alpha() -> GemMaterial {
        let mut material = GemMaterial::by_name("Tanzanite").expect("a built-in material");
        material.alpha_axis = Some(alpha_axis_from_turn(Vec3::Y, 60.0));
        material
    }

    fn with_axis(axis: Option<Vec3>, alpha: Option<Vec3>) -> MaterialOverrides {
        MaterialOverrides {
            inclusion_sigma_s: 0.0,
            c_axis_override: axis,
            alpha_axis_override: alpha,
            edge_rounding_radius: 0.0,
            stone_width_mm: 0.0,
        }
    }

    #[test]
    fn a_new_axis_without_a_second_axis_gets_the_completion_of_that_axis() {
        let gamma = Vec3::X;
        let resolved =
            with_baked_second_axis(with_axis(Some(gamma), None), &tanzanite_with_baked_alpha());
        assert_eq!(
            resolved.alpha_axis_override,
            Some(alpha_axis_from_turn(gamma, 0.0))
        );
    }

    #[test]
    fn every_other_case_is_untouched() {
        let baked = tanzanite_with_baked_alpha();
        let plain = GemMaterial::by_name("Tanzanite").expect("a built-in material");
        // No axis set: the material's own frame stays.
        let none = with_baked_second_axis(with_axis(None, None), &baked);
        assert_eq!(
            (none.c_axis_override, none.alpha_axis_override),
            (None, None)
        );
        // A Second Axis given: it wins.
        let given = with_baked_second_axis(with_axis(Some(Vec3::X), Some(Vec3::Z)), &baked);
        assert_eq!(given.alpha_axis_override, Some(Vec3::Z));
        // No baked Second Axis: nothing to replace, today's behaviour.
        let axis_only = with_baked_second_axis(with_axis(Some(Vec3::X), None), &plain);
        assert_eq!(axis_only.alpha_axis_override, None);
    }
}
