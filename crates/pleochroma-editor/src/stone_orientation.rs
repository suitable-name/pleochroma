//! A stone's crystal orientation for every consumer outside the live render.
//!
//! That is every consumer that traces or scores a stone: the Optimize, Sweep, Tune, Baseline, Trials, Retarget and Verdict scoring
//! scenes, the tilt profile, Compare, the swatches, the command line and the web app.
//!
//! The rule is the render's (plan section 2): the design's own Crystal Axis (its
//! `[render_effects]` override) beats the material's default, which beats the built-in's axis.
//! A custom material's default is baked into the material itself
//! (`pleochroma_cut_core::material::with_default_orientation`), so the only thing resolved here
//! is the design's own setting; with the override off every function returns the material
//! untouched, which keeps a design that never set an axis byte-identical.

use glam::Vec3;
use pleochroma::optics::{
    birefringence::alpha_axis_from_turn,
    materials::{GemMaterial, OpticalCharacter},
};
use pleochroma_cut_core::{
    CrystalAxis, StoneRenderEffects, material::pattern::follow_noted_pattern, stone_orientation,
};

/// The pair `material_for_stone_oriented` takes.
///
/// It is `pleochroma::render_setup::material_for_stone_oriented`'s orientation: the Crystal Axis
/// (gamma) and the optional Second Axis (alpha).
pub type Orientation = (Vec3, Option<Vec3>);

/// The orientation `effects` gives `material`, or `None` when the stone keeps its own axes.
///
/// That is when the stone keeps the material's
/// own axes: the design's override is off, or the material is isotropic (its axis is
/// meaningless, and every consumer hides it).
///
/// The Crystal Axis is the vector the angle sliders have always produced
/// ([`CrystalAxis::slider_direction`]), so a design that already carried an axis renders with
/// the very bits it had. A Second Axis turn rotates alpha from the material's own completion about
/// the CANONICAL axis line (`CrystalOrientation::alpha`, the one the planner rows, the hand-off and
/// the sheet use): alpha is a line, so its sign does not matter, but a turn about the slider vector
/// at tilt 90 and azimuth 180 or more would run the other way round. A design with an axis but no
/// turn, on a material that holds a Second Axis of its own (a baked default), gets the material's
/// own completion again (turn zero) rather than an alpha that belongs to a different Crystal
/// Axis.
#[must_use]
pub fn orientation_override(
    effects: &StoneRenderEffects,
    material: &GemMaterial,
) -> Option<Orientation> {
    if material.optical_character == OpticalCharacter::Isotropic {
        return None;
    }
    let orientation = stone_orientation(effects, None)?;
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the slider angles are degrees in 0..=360, far inside f32's range"
    )]
    let gamma = CrystalAxis::slider_direction(
        effects.c_axis_tilt_deg as f32,
        effects.c_axis_azimuth_deg as f32,
    );
    let alpha = match orientation.alpha() {
        Some(alpha) => Some(alpha),
        None if material.alpha_axis.is_some() => {
            Some(alpha_axis_from_turn(orientation.gamma(), 0.0))
        }
        None => None,
    };
    Some((gamma, alpha))
}

/// `material` with `orientation` applied under the render's own rules.
///
/// An isotropic material
/// ignores it, and only a biaxial one takes the Second Axis (the rules of
/// `pleochroma::render_setup::apply_material_overrides`, without its size step, so a scoring
/// scene that scores the unsized material keeps its scale).
#[must_use]
pub fn with_orientation(
    mut material: GemMaterial,
    orientation: Option<Orientation>,
) -> GemMaterial {
    if let Some((gamma, alpha)) = orientation
        && material.optical_character != OpticalCharacter::Isotropic
    {
        material.c_axis = gamma;
        if let Some(alpha) = alpha
            && material.biaxial_delta_beta_alpha.is_some()
        {
            material.alpha_axis = Some(alpha);
        }
    }
    // A color pattern that follows the Crystal Axis turns with the stone's axis in every scoring
    // scene too, not only in the live render (the same function the render context uses); it
    // reads the mode off the zones, so a material without a following pattern is untouched.
    follow_noted_pattern(&mut material);
    material
}

/// `material` carrying the Crystal Axis of `effects` ([`orientation_override`]); the material
/// itself when the design sets none.
#[must_use]
pub fn oriented_for_effects(material: GemMaterial, effects: &StoneRenderEffects) -> GemMaterial {
    let orientation = orientation_override(effects, &material);
    with_orientation(material, orientation)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn axis_on(tilt: f64, azimuth: f64, second: Option<f64>) -> StoneRenderEffects {
        StoneRenderEffects {
            c_axis_override_enabled: true,
            c_axis_tilt_deg: tilt,
            c_axis_azimuth_deg: azimuth,
            c_axis_second_azimuth_deg: second,
            ..StoneRenderEffects::default()
        }
    }

    fn built_in(name: &str) -> GemMaterial {
        GemMaterial::by_name(name).expect("a built-in material")
    }

    /// A design that never set an axis leaves every material exactly as it was.
    #[test]
    fn a_design_without_an_axis_changes_nothing() {
        let effects = StoneRenderEffects::default();
        for name in ["Diamond", "Sapphire", "Tourmaline", "Tanzanite"] {
            let material = built_in(name);
            assert_eq!(orientation_override(&effects, &material), None, "{name}");
            assert_eq!(
                oriented_for_effects(material.clone(), &effects),
                material,
                "{name}"
            );
        }
    }

    /// The axis of an enabled override is the slider vector, bit for bit.
    #[test]
    fn an_enabled_axis_is_the_slider_vector() {
        let effects = axis_on(30.0, 120.0, None);
        let (gamma, alpha) =
            orientation_override(&effects, &built_in("Sapphire")).expect("sapphire is anisotropic");
        let expected = CrystalAxis::slider_direction(30.0, 120.0);
        assert_eq!(
            gamma.to_array().map(f32::to_bits),
            expected.to_array().map(f32::to_bits)
        );
        assert_eq!(alpha, None);
        assert_eq!(
            oriented_for_effects(built_in("Sapphire"), &effects).c_axis,
            expected
        );
    }

    /// An isotropic material hides the axis: nothing reaches it.
    #[test]
    fn an_isotropic_material_ignores_the_axis() {
        let effects = axis_on(30.0, 120.0, Some(40.0));
        let diamond = built_in("Diamond");
        assert_eq!(orientation_override(&effects, &diamond), None);
        assert_eq!(oriented_for_effects(diamond.clone(), &effects), diamond);
    }

    /// The Second Axis reaches a biaxial material only, and is perpendicular to the axis.
    #[test]
    fn the_second_axis_reaches_a_biaxial_material_only() {
        let effects = axis_on(90.0, 0.0, Some(37.0));

        let tanzanite = oriented_for_effects(built_in("Tanzanite"), &effects);
        let alpha = tanzanite.alpha_axis.expect("tanzanite is biaxial");
        assert!(alpha.dot(tanzanite.c_axis).abs() < 1e-5);
        assert!((alpha.length() - 1.0).abs() < 1e-5);

        let sapphire = oriented_for_effects(built_in("Sapphire"), &effects);
        assert_eq!(
            sapphire.alpha_axis, None,
            "a uniaxial material has no alpha"
        );
        assert_eq!(sapphire.c_axis, CrystalAxis::slider_direction(90.0, 0.0));
    }

    /// With a turn of zero the Second Axis is the material's own completion.
    #[test]
    fn turn_zero_is_the_materials_own_completion() {
        let effects = axis_on(40.0, 10.0, Some(0.0));
        let tanzanite = oriented_for_effects(built_in("Tanzanite"), &effects);
        let alpha = tanzanite.alpha_axis.expect("tanzanite is biaxial");
        // The turn is taken about the canonical (f64) axis line, not the f32 slider vector, so
        // the two completions agree as lines to rounding, not bit for bit.
        let completion = alpha_axis_from_turn(tanzanite.c_axis, 0.0);
        assert!(
            alpha.dot(completion).abs() > 1.0 - 1e-6,
            "{alpha:?} vs {completion:?}"
        );
    }

    /// Acceptance 4: with no axis set, the size rule through the design is the plain size rule,
    /// bit for bit (a Tourmaline design without an axis scores as it always did); with an axis
    /// set, the stone is oriented.
    #[test]
    fn the_design_size_rule_is_plain_without_an_axis_and_oriented_with_one() {
        use crate::material_lookup::sized_material_for_optimize;
        use pleochroma::render_setup::{material_for_stone, material_for_stone_oriented};

        let mut design = crate::EditorSession::fresh().design;
        design.girdle_diameter_mm = Some(8.0);
        for name in ["Tourmaline", "Sapphire", "Tanzanite", "Diamond"] {
            let material = built_in(name);
            assert_eq!(
                sized_material_for_optimize(material.clone(), &design, &[]),
                material_for_stone(material.clone(), 8.0, &[]),
                "{name}: no axis"
            );
            assert_eq!(
                material_for_stone(material.clone(), 8.0, &[]),
                material_for_stone_oriented(material, 8.0, &[], None),
                "{name}: the plain rule is the oriented rule with none"
            );
        }

        design.render_effects = axis_on(0.0, 0.0, None);
        let tourmaline = built_in("Tourmaline");
        let oriented = sized_material_for_optimize(tourmaline.clone(), &design, &[]);
        assert_eq!(
            oriented.c_axis,
            Vec3::Y,
            "tilt 0 stands the axis through the table"
        );
        assert_ne!(oriented, material_for_stone(tourmaline, 8.0, &[]));
    }

    /// A pattern that follows the Crystal Axis is turned to the design's axis by every scoring
    /// scene; a pattern that does not follow is never touched.
    #[test]
    fn a_following_pattern_takes_the_designs_axis_in_the_scoring_scenes() {
        use pleochroma_cut_core::material::pattern::{PatternAxis, TemplateId, template};
        let zoned = |name: &str, axis: PatternAxis| {
            let mut pattern = template(TemplateId::Bicolor, axis);
            for at in pattern.unsolved_colors() {
                pattern.set_color_bands(at, vec![[520.0, 40.0, 0.3]]);
            }
            let mut material = built_in("Sapphire");
            material.name = name.to_owned();
            material.zoning = pattern.to_zoned().expect("converts");
            material
        };
        let effects = axis_on(90.0, 0.0, None);

        let fan = oriented_for_effects(
            zoned("Scoring Follow Fan", PatternAxis::FollowsCrystalAxis),
            &effects,
        );
        let frame = fan.zoning.as_ref().expect("zoned").frame;
        // A line: either end of it counts.
        assert!(
            frame
                .direction(glam::DVec3::Y)
                .dot(fan.c_axis.as_dvec3())
                .abs()
                > 1.0 - 1e-6,
            "the zones' axis lies on the design's Crystal Axis"
        );

        // The same name and a fixed pattern: the zones' own flag decides, not the name.
        let fixed =
            oriented_for_effects(zoned("Scoring Follow Fan", PatternAxis::ACROSS), &effects);
        assert_eq!(
            fixed.zoning,
            zoned("Scoring Follow Fan", PatternAxis::ACROSS).zoning,
            "a pattern that does not follow is not touched"
        );
    }

    /// The Second Axis is the same LINE whichever side of the table the slider vector points
    /// to: at tilt 90 an azimuth of 200 is the axis line of azimuth 20.
    #[test]
    fn the_second_axis_is_the_same_line_for_either_end_of_the_axis() {
        let tanzanite = built_in("Tanzanite");
        let (_, alpha_a) =
            orientation_override(&axis_on(90.0, 200.0, Some(30.0)), &tanzanite).expect("biaxial");
        let (_, alpha_b) =
            orientation_override(&axis_on(90.0, 20.0, Some(30.0)), &tanzanite).expect("biaxial");
        let (alpha_a, alpha_b) = (
            alpha_a.expect("a turn is set"),
            alpha_b.expect("a turn is set"),
        );
        assert!(
            alpha_a.dot(alpha_b).abs() > 1.0 - 1e-5,
            "{alpha_a:?} vs {alpha_b:?}"
        );
    }

    /// A design with an axis and no turn, on a material that holds a baked Second Axis, gets
    /// the completion of ITS axis, never the baked alpha of a different one.
    #[test]
    fn a_baked_second_axis_does_not_follow_a_different_design_axis() {
        let mut material = built_in("Tanzanite");
        material.alpha_axis = Some(alpha_axis_from_turn(Vec3::Y, 60.0));
        let effects = axis_on(90.0, 0.0, None);
        let resolved = oriented_for_effects(material, &effects);
        // The completion of the design's axis, as a line and to rounding (the turn is built about
        // the canonical axis line, which differs from the f32 slider vector in the last bits).
        let alpha = resolved.alpha_axis.expect("tanzanite is biaxial");
        let completion = alpha_axis_from_turn(resolved.c_axis, 0.0);
        assert!(
            alpha.dot(completion).abs() > 1.0 - 1e-6,
            "{alpha:?} vs {completion:?}"
        );
    }
}
