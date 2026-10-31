//! Validation of per-stone patterns crossing the page/Worker boundary.

use pleochroma::optics::materials::GemMaterial;
use pleochroma_cut_core::material::pattern::{ColorPattern, apply_color_pattern};
use serde::{Deserialize, Deserializer, de::Error};

/// Reads the optional pattern tag, including `None`, then checks the stored data.
pub fn deserialize<'de, D>(deserializer: D) -> Result<Option<ColorPattern>, D::Error>
where
    D: Deserializer<'de>,
{
    let pattern = Option::<ColorPattern>::deserialize(deserializer)?;
    if let Some(pattern) = &pattern {
        apply_color_pattern(GemMaterial::diamond(), pattern, 7.0).map_err(D::Error::custom)?;
    }
    Ok(pattern)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{scene::DesignMaterialOverrides, solve::MaterialSelectionData};
    use pleochroma::optics::{
        absorption::{AbsorptionBand, AbsorptionTensor},
        materials::variants::built_in_variants,
    };
    use pleochroma_cut_core::material::pattern::{
        PATTERN_FORMAT_VERSION, PatternAxis, PatternColor, PatternTensorRows, TemplateId,
        ZoneColor, template,
    };

    fn solved(id: TemplateId) -> ColorPattern {
        let mut pattern = template(id, PatternAxis::TABLE_NORMAL);
        for at in pattern.unsolved_colors() {
            pattern.set_color_bands(at, vec![[520.0, 40.0, 0.25]]);
        }
        pattern
    }

    /// Both positional wire records carry `pattern` and read it back equal.
    fn assert_crosses_both_records(pattern: &ColorPattern) {
        let scene = DesignMaterialOverrides {
            color_pattern_override: Some(pattern.clone()),
            ..DesignMaterialOverrides::default()
        };
        let bytes = postcard::to_allocvec(&scene).expect("encode scene");
        assert_eq!(
            postcard::from_bytes::<DesignMaterialOverrides>(&bytes).expect("decode scene"),
            scene
        );
        let selection = MaterialSelectionData {
            color_pattern_override: Some(pattern.clone()),
            ..MaterialSelectionData::default()
        };
        let bytes = postcard::to_allocvec(&selection).expect("encode selection");
        assert_eq!(
            postcard::from_bytes::<MaterialSelectionData>(&bytes).expect("decode selection"),
            selection
        );
    }

    #[test]
    fn each_pattern_template_crosses_both_positional_wire_records() {
        for id in TemplateId::ALL {
            assert_crosses_both_records(&solved(id));
        }
    }

    /// A biaxial rows set: three distinct band sets, so the beta set is present.
    fn trichroic_rows() -> PatternTensorRows {
        PatternTensorRows::from_tensor(&AbsorptionTensor::biaxial(
            vec![AbsorptionBand::new(640.0, 44.4, 0.31)],
            vec![AbsorptionBand::new(540.0, 30.0, 0.17)],
            vec![AbsorptionBand::new(460.0, 39.9, 0.55)],
        ))
    }

    #[test]
    fn variant_and_trichroic_zone_colors_cross_both_positional_wire_records() {
        let yellow = built_in_variants("Sapphire")
            .into_iter()
            .find(|variant| variant.key == "sapphire.yellow")
            .expect("the seed variant");
        // A uniaxial variant (no beta set: the Option tag is None).
        let mut pattern = solved(TemplateId::Bicolor);
        pattern.zones[0].color = PatternColor::from_variant(&yellow);
        assert!(matches!(
            pattern.zones[0].color,
            PatternColor::Variant { .. }
        ));
        let pattern = pattern.with_required_version();
        assert_eq!(pattern.version, PATTERN_FORMAT_VERSION);
        assert_crosses_both_records(&pattern);

        // A variant with a beta set (the Option tag is Some).
        let mut with_beta = solved(TemplateId::Bicolor);
        with_beta.zones[0].color = PatternColor::Variant {
            key: "user.beta".to_owned(),
            label: "Beta Test".to_owned(),
            absorption: trichroic_rows(),
        };
        let with_beta = with_beta.with_required_version();
        assert_crosses_both_records(&with_beta);

        // Three picked colors on the base zone.
        let zone = |centre: f32| ZoneColor::solved([50.0, 30.0, 100.0], vec![[centre, 40.0, 0.25]]);
        let mut trichroic = solved(TemplateId::Bicolor);
        trichroic.base.color = PatternColor::Trichroic {
            alpha: zone(640.0),
            beta: zone(540.0),
            gamma: zone(460.0),
        };
        let trichroic = trichroic.with_required_version();
        assert_eq!(trichroic.version, PATTERN_FORMAT_VERSION);
        assert_crosses_both_records(&trichroic);
    }

    #[test]
    fn absent_patterns_keep_an_explicit_tag_and_json_defaults() {
        let scene = DesignMaterialOverrides::default();
        let bytes = postcard::to_allocvec(&scene).expect("encode");
        assert_eq!(bytes, vec![0, 0, 0, 0, 0, 0]);
        assert_eq!(
            postcard::from_bytes::<DesignMaterialOverrides>(&bytes).expect("decode"),
            scene
        );
        let json = r#"{"refractive_index_override":null,"body_color_override":null}"#;
        assert_eq!(
            serde_json::from_str::<DesignMaterialOverrides>(json).expect("old JSON"),
            scene
        );
    }

    #[test]
    fn an_invalid_pattern_is_rejected_before_the_worker_uses_it() {
        let mut pattern = solved(TemplateId::Bicolor);
        pattern.version = PATTERN_FORMAT_VERSION + 1;
        let scene = DesignMaterialOverrides {
            color_pattern_override: Some(pattern),
            ..DesignMaterialOverrides::default()
        };
        let bytes = postcard::to_allocvec(&scene).expect("encode");
        assert!(postcard::from_bytes::<DesignMaterialOverrides>(&bytes).is_err());
    }
}
