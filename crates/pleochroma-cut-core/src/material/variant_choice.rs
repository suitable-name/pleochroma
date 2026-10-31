//! [`VariantChoice`]: the snapshot of a color variant a design carries.
//!
//! A design that picks a named color variant ("Sapphire / Padparadscha") copies the variant's
//! whole per-axis absorption tensor into its [`super::MaterialSelection`], so the design renders
//! the same without the variant library, on another machine and in the browser, and does not
//! change when a saved variant is later edited or deleted.

use pleochroma::optics::{
    absorption::AbsorptionTensor,
    materials::{AbsorptionUnit, variants::ColorVariant},
};

/// The `key` of a color the cutter authored in the Custom editor rather than picked from a
/// material's variant list.
pub const CUSTOM_VARIANT_KEY: &str = "custom";

/// The label of a color the cutter authored in the Custom editor.
pub const CUSTOM_VARIANT_LABEL: &str = "Custom Color";

/// A color variant as one design stores it: the full absorption tensor, its unit and the stone
/// width it was calibrated for, plus the name it was picked under.
///
/// Applying it replaces only the host's absorption tensor and unit (see
/// [`super::MaterialSelection::apply_overrides_at_width`]); every other property of the
/// material, including its pleochroism rank, is the host's own.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct VariantChoice {
    /// The variant's stable key: `"sapphire.padparadscha"` (built-in), `"user.<uuid>"` (saved)
    /// or [`CUSTOM_VARIANT_KEY`].
    pub key: String,
    /// The display label (Title Case): `"Padparadscha"`.
    pub label: String,
    /// The per-axis absorption tensor, at most `MAX_ABSORPTION_BANDS` bands per ray.
    pub absorption: AbsorptionTensor,
    /// What the tensor's amplitudes are per: millimetre or model unit.
    pub unit: AbsorptionUnit,
    /// The STONE WIDTH in millimetres the tensor's numbers are calibrated for: 7 for a built-in
    /// variant, the width of the stone a Custom Color was solved on. It is a width, never a light
    /// path: every swatch and the `.psdd` fallback color of the variant are evaluated at
    /// `variants::face_up_swatch_path(unit, reference_path_mm)`, so a reader shows the swatch the
    /// cutter saw. The render ignores the field (it sizes the path from the stone itself).
    pub reference_path_mm: f32,
}

impl VariantChoice {
    /// The snapshot of `variant`: key, label, tensor, unit and reference path, copied.
    #[must_use]
    pub fn from_variant(variant: &ColorVariant) -> Self {
        Self {
            key: variant.key.clone(),
            label: variant.label.clone(),
            absorption: variant.absorption.clone(),
            unit: variant.unit,
            reference_path_mm: variant.reference_path_mm,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::{
        MaterialSelection, built_in_material_by_exact_name,
        pattern::{ColorPattern, PatternAxis, TemplateId, apply_color_pattern, template},
    };
    use pleochroma::optics::{
        absorption::AbsorptionBand,
        materials::{GemMaterial, variants::VariantProvenance},
    };

    /// A pleochroic variant (distinct o and e sets) in the given unit.
    fn choice(unit: AbsorptionUnit) -> VariantChoice {
        VariantChoice {
            key: "sapphire.test".to_owned(),
            label: "Test Pink".to_owned(),
            absorption: AbsorptionTensor::uniaxial(
                vec![AbsorptionBand::energy(550.0, 1500.5, 0.35)],
                vec![AbsorptionBand::new(430.0, 40.0, 0.12)],
            ),
            unit,
            reference_path_mm: 6.0,
        }
    }

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

    #[test]
    fn a_snapshot_copies_every_field_of_its_variant() {
        let source = choice(AbsorptionUnit::PerMm);
        let variant = ColorVariant {
            key: source.key.clone(),
            label: source.label.clone(),
            material: "Sapphire".to_owned(),
            absorption: source.absorption.clone(),
            unit: source.unit,
            reference_path_mm: source.reference_path_mm,
            provenance: VariantProvenance::User,
        };
        assert_eq!(VariantChoice::from_variant(&variant), source);
    }

    #[test]
    fn a_variant_replaces_only_the_absorption_and_keeps_the_hosts_pleochroism() {
        let host = GemMaterial::sapphire();
        let chosen = choice(AbsorptionUnit::PerMm);
        let selection = MaterialSelection::none().with_color_variant(Some(chosen.clone()));
        let result = selection.apply_overrides(host.clone());
        assert_eq!(
            result,
            host.clone()
                .with_color_variant_tensor(chosen.absorption.clone(), chosen.unit)
        );
        assert_eq!(result.absorption, chosen.absorption);
        assert_eq!(result.absorption_unit, AbsorptionUnit::PerMm);
        assert!(result.absorption.is_pleochroic);
        assert_eq!(result.name, host.name);
        assert_eq!(result.dispersion, host.dispersion);
        assert_eq!(result.birefringence_delta, host.birefringence_delta);
    }

    #[test]
    fn a_per_model_unit_variant_sets_that_unit() {
        let chosen = choice(AbsorptionUnit::ModelUnit);
        let result = MaterialSelection::none()
            .with_color_variant(Some(chosen))
            .apply_overrides(GemMaterial::sapphire());
        assert_eq!(result.absorption_unit, AbsorptionUnit::ModelUnit);
    }

    #[test]
    fn a_variant_beats_the_legacy_bands_and_triple() {
        let host = GemMaterial::sapphire();
        let chosen = choice(AbsorptionUnit::PerMm);
        // Built by hand: the builders never leave a variant next to the legacy fields.
        let selection = MaterialSelection {
            body_color_override: Some([0.2, 0.4, 2.8]),
            body_color_bands_override: Some(vec![[550.0, 60.0, 0.3]]),
            absorption_path_scale_override: Some(1.0),
            color_variant_override: Some(chosen.clone()),
            ..MaterialSelection::none()
        };
        assert_eq!(
            selection.apply_overrides(host.clone()),
            host.with_color_variant_tensor(chosen.absorption, chosen.unit)
        );
    }

    #[test]
    fn a_stone_pattern_beats_a_variant() {
        let host = GemMaterial::sapphire();
        let pattern = solved_bicolor();
        let both = MaterialSelection::none()
            .with_color_pattern(Some(pattern.clone()))
            .with_color_variant(Some(choice(AbsorptionUnit::PerMm)));
        let patterned = MaterialSelection::none().with_color_pattern(Some(pattern.clone()));
        assert_eq!(
            both.apply_overrides_at_width(host.clone(), 9.0),
            patterned.apply_overrides_at_width(host.clone(), 9.0)
        );
        assert_eq!(
            both.apply_overrides_at_width(host.clone(), 9.0),
            apply_color_pattern(host, &pattern, 9.0).expect("the pattern applies")
        );
    }

    #[test]
    fn a_zoned_material_ignores_a_variant() {
        let zoned = GemMaterial::ametrine();
        assert!(zoned.zoning.is_some(), "the premise: Ametrine is zoned");
        let selection =
            MaterialSelection::none().with_color_variant(Some(choice(AbsorptionUnit::PerMm)));
        assert_eq!(selection.apply_overrides(zoned.clone()), zoned);
    }

    #[test]
    fn a_variant_leaves_a_biaxial_host_trichroic_when_the_variant_is() {
        let host = built_in_material_by_exact_name("Alexandrite").expect("a built-in");
        let chosen = VariantChoice {
            absorption: AbsorptionTensor::biaxial(
                vec![AbsorptionBand::new(640.0, 44.4, 0.31)],
                vec![AbsorptionBand::new(540.0, 30.0, 0.17)],
                vec![AbsorptionBand::new(460.0, 39.9, 0.55)],
            ),
            ..choice(AbsorptionUnit::PerMm)
        };
        let result = MaterialSelection::none()
            .with_color_variant(Some(chosen.clone()))
            .apply_overrides(host);
        assert!(result.absorption.beta_ray.is_some());
        assert_eq!(result.absorption, chosen.absorption);
    }

    #[test]
    fn setting_a_variant_clears_the_legacy_colors_and_keeps_the_pattern() {
        let pattern = solved_bicolor();
        let legacy = MaterialSelection::none()
            .with_color_pattern(Some(pattern.clone()))
            .with_body_color_bands(
                Some([0.2, 0.4, 2.8]),
                Some(vec![[550.0, 60.0, 0.3]]),
                Some(1.5),
            );
        let chosen = choice(AbsorptionUnit::PerMm);
        let set = legacy.with_color_variant(Some(chosen.clone()));
        assert_eq!(set.color_variant_override, Some(chosen));
        assert_eq!(set.body_color_override, None);
        assert_eq!(set.body_color_bands_override, None);
        assert_eq!(set.absorption_path_scale_override, None);
        assert_eq!(set.color_pattern_override, Some(pattern));
        // `None` clears the variant and leaves the (already cleared) legacy fields alone.
        assert_eq!(set.with_color_variant(None).color_variant_override, None);
    }

    #[test]
    fn setting_a_legacy_color_clears_the_variant_and_keeps_the_pattern() {
        let pattern = solved_bicolor();
        let with_variant = MaterialSelection::none()
            .with_color_pattern(Some(pattern.clone()))
            .with_color_variant(Some(choice(AbsorptionUnit::PerMm)));
        let triple = with_variant.clone().with_body_color(Some([0.2, 0.4, 2.8]));
        assert_eq!(triple.color_variant_override, None);
        assert_eq!(triple.body_color_override, Some([0.2, 0.4, 2.8]));
        assert_eq!(triple.color_pattern_override, Some(pattern.clone()));
        let bands = with_variant.with_body_color_bands(
            Some([0.2, 0.4, 2.8]),
            Some(vec![[550.0, 60.0, 0.3]]),
            Some(1.5),
        );
        assert_eq!(bands.color_variant_override, None);
        assert_eq!(
            bands.body_color_bands_override,
            Some(vec![[550.0, 60.0, 0.3]])
        );
        assert_eq!(bands.color_pattern_override, Some(pattern));
    }

    #[test]
    fn debug_prints_a_selection_without_a_variant_exactly_as_before() {
        assert_eq!(
            format!("{:?}", MaterialSelection::none()),
            "MaterialSelection { name: None, specific_gravity_override: None, \
             refractive_index_override: None }"
        );
        let with_variant =
            MaterialSelection::none().with_color_variant(Some(choice(AbsorptionUnit::PerMm)));
        assert!(format!("{with_variant:?}").contains("color_variant_override: VariantChoice"));
    }

    #[test]
    fn a_variant_counts_as_a_recolor_and_names_itself() {
        let plain = MaterialSelection::none();
        assert!(!plain.has_body_color_override());
        assert_eq!(plain.color_label(), None);
        let with_variant = plain
            .clone()
            .with_color_variant(Some(choice(AbsorptionUnit::PerMm)));
        assert!(with_variant.has_body_color_override());
        assert_eq!(with_variant.color_label().as_deref(), Some("Test Pink"));
        // The static legacy label stays the legacy triple's.
        assert_eq!(with_variant.body_color_label(), None);
        let yellow = plain.with_body_color(Some([0.2, 0.4, 2.8]));
        assert_eq!(yellow.color_label().as_deref(), yellow.body_color_label());
    }
}
