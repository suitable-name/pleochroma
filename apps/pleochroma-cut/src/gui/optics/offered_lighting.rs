//! Which lighting presets this build offers: the two UV lamps belong to the
//! `physical-color` feature ([`PHYSICS_COLOR_UI`]), like the fluorescence UI they light.
//!
//! The data layer still knows every preset (a scene, a job or a design file may name a UV
//! lamp), so the gate sits where a preset enters the desktop app from outside the combo box:
//! the settings file, a saved lighting preset, a design's own lighting and the combo's index.
//! A UV lamp reaching a build without the feature falls back to the editor default rig
//! ([`DEFAULT_LIGHTING_RIG`]), never an error.

use pleochroma::optics::LightingPreset;

use crate::{gui::optics::physics_state::PHYSICS_COLOR_UI, settings::model::DEFAULT_LIGHTING_RIG};

/// `preset` itself, or the editor default rig when it is a UV lamp and `uv_offered` is false.
#[must_use]
pub fn gate_preset(preset: LightingPreset, uv_offered: bool) -> LightingPreset {
    if uv_offered || !preset.is_uv_lamp() {
        preset
    } else {
        LightingPreset::from_label(DEFAULT_LIGHTING_RIG)
    }
}

/// [`gate_preset`] for this build ([`PHYSICS_COLOR_UI`]).
#[must_use]
pub fn offered(preset: LightingPreset) -> LightingPreset {
    gate_preset(preset, PHYSICS_COLOR_UI)
}

/// The preset a stored label names in this build: `LightingPreset::from_label`, gated.
#[must_use]
pub fn offered_from_label(label: &str) -> LightingPreset {
    offered(LightingPreset::from_label(label))
}

/// The preset the combo's index names in this build: `LightingPreset::from_index`, gated.
#[must_use]
pub fn offered_from_index(index: i32) -> LightingPreset {
    offered(LightingPreset::from_index(index))
}

/// The `KitRigThumb` kind of `preset`: 0-12 are the visible-light rigs in `LightingPreset::ALL`
/// order, 13 and 14 the two UV lamps. Feeds `ViewportModel.lighting_kinds`.
#[must_use]
pub const fn rig_kind(preset: LightingPreset) -> i32 {
    match preset {
        LightingPreset::LightTent => 0,
        LightingPreset::IsoHemisphere => 1,
        LightingPreset::WhiteTray => 2,
        LightingPreset::ShopLights => 3,
        LightingPreset::WindowDaylight => 4,
        LightingPreset::DaylightDome => 5,
        LightingPreset::DaylightSun => 6,
        LightingPreset::Daylight => 7,
        LightingPreset::Incandescent => 8,
        LightingPreset::IlluminantA => 9,
        LightingPreset::RingLights => 10,
        LightingPreset::DarkSpotlight => 11,
        LightingPreset::Aset => 12,
        LightingPreset::UvLamp365 => 13,
        LightingPreset::UvLamp395 => 14,
    }
}

/// The rig menu / gallery group of `preset`: 0 lit model, 1 studio, 2 analysis, 3 UV. Feeds
/// `ViewportModel.lighting_groups`.
#[must_use]
pub const fn rig_group(preset: LightingPreset) -> i32 {
    match preset {
        LightingPreset::LightTent
        | LightingPreset::IsoHemisphere
        | LightingPreset::WhiteTray
        | LightingPreset::ShopLights
        | LightingPreset::WindowDaylight
        | LightingPreset::DaylightDome
        | LightingPreset::DaylightSun => 0,
        LightingPreset::Daylight
        | LightingPreset::Incandescent
        | LightingPreset::IlluminantA
        | LightingPreset::RingLights
        | LightingPreset::DarkSpotlight => 1,
        LightingPreset::Aset => 2,
        LightingPreset::UvLamp365 | LightingPreset::UvLamp395 => 3,
    }
}

/// The short name of `preset` for the chip, the rig card and the gallery tiles. Feeds
/// `ViewportModel.lighting_short_names`.
#[must_use]
pub const fn rig_short_name(preset: LightingPreset) -> &'static str {
    match preset {
        LightingPreset::LightTent => "Light Tent",
        LightingPreset::IsoHemisphere => "Grading Tray",
        LightingPreset::WhiteTray => "White Tray",
        LightingPreset::ShopLights => "Jewellery Shop",
        LightingPreset::WindowDaylight => "Window Daylight",
        LightingPreset::DaylightDome => "Sky, No Sun",
        LightingPreset::DaylightSun => "Sky + Sun",
        LightingPreset::Daylight => "D65 Daylight",
        LightingPreset::Incandescent => "Incandescent",
        LightingPreset::IlluminantA => "Incandescent A",
        LightingPreset::RingLights => "Ring Lights",
        LightingPreset::DarkSpotlight => "Dark Spotlight",
        LightingPreset::Aset => "Contrast View",
        LightingPreset::UvLamp365 => "UV 365 nm",
        LightingPreset::UvLamp395 => "UV 395 nm",
    }
}

/// One line about `preset` for the rig card. Feeds `ViewportModel.lighting_blurbs`.
#[must_use]
pub const fn rig_blurb(preset: LightingPreset) -> &'static str {
    match preset {
        LightingPreset::LightTent => "Soft light, black cards",
        LightingPreset::IsoHemisphere => "Even D65 dome with a head shadow",
        LightingPreset::WhiteTray => "Lit from below, like a light table",
        LightingPreset::ShopLights => "Bright walls plus pinpoint spots",
        LightingPreset::WindowDaylight => "One soft window in a dim room",
        LightingPreset::DaylightDome => "Clear sky all around, no sun",
        LightingPreset::DaylightSun => "Clear sky with a small bright sun",
        LightingPreset::Daylight => "D65 daylight with pinpoint lights",
        LightingPreset::Incandescent => "Warm 3200 K bulbs",
        LightingPreset::IlluminantA => "Illuminant A, 2856 K, for color change",
        LightingPreset::RingLights => "Ring lights at 5000 K, as in studio photos",
        LightingPreset::DarkSpotlight => "One hard spot in a dark room",
        LightingPreset::Aset => "Colored by exit angle to show light return",
        LightingPreset::UvLamp365 => "365 nm ultraviolet lamp, dark room",
        LightingPreset::UvLamp395 => "395 nm ultraviolet lamp, dark room",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rig_tables_cover_every_preset_in_all_order() {
        // Kinds follow the UI order and are distinct.
        for (i, preset) in LightingPreset::ALL.iter().enumerate() {
            assert_eq!(rig_kind(*preset), i32::try_from(i).unwrap());
            assert_eq!(rig_kind(*preset), preset.index());
        }
        let groups: Vec<i32> = LightingPreset::ALL.iter().map(|p| rig_group(*p)).collect();
        assert_eq!(groups.len(), LightingPreset::ALL.len());
        assert!(groups.iter().all(|g| (0..=3).contains(g)));
        assert!(
            groups.windows(2).all(|w| w[0] <= w[1]),
            "groups are contiguous"
        );
        for preset in LightingPreset::ALL {
            assert_eq!(rig_group(preset) == 3, preset.is_uv_lamp());
            assert!(!rig_short_name(preset).is_empty());
            assert!(!rig_blurb(preset).is_empty());
        }
        let names: std::collections::HashSet<&str> = LightingPreset::ALL
            .iter()
            .map(|p| rig_short_name(*p))
            .collect();
        assert_eq!(
            names.len(),
            LightingPreset::ALL.len(),
            "short names are distinct"
        );
    }

    #[test]
    fn studio_rigs_are_group_one() {
        use pleochroma::optics::raytracer::LightingModel;
        for preset in LightingPreset::ALL {
            if !preset.is_uv_lamp() && preset.model() == LightingModel::Studio {
                assert_eq!(rig_group(preset), 1);
            }
        }
    }

    #[test]
    fn uv_lamps_fall_back_to_the_default_rig_without_the_feature() {
        let default = LightingPreset::from_label(DEFAULT_LIGHTING_RIG);
        assert_eq!(gate_preset(LightingPreset::UvLamp365, false), default);
        assert_eq!(gate_preset(LightingPreset::UvLamp395, false), default);
        assert!(!default.is_uv_lamp());
    }

    #[test]
    fn uv_lamps_are_kept_with_the_feature() {
        assert_eq!(
            gate_preset(LightingPreset::UvLamp365, true),
            LightingPreset::UvLamp365
        );
        assert_eq!(
            gate_preset(LightingPreset::UvLamp395, true),
            LightingPreset::UvLamp395
        );
    }

    #[test]
    fn visible_light_presets_are_never_changed() {
        for preset in LightingPreset::ALL {
            if !preset.is_uv_lamp() {
                assert_eq!(gate_preset(preset, false), preset);
                assert_eq!(gate_preset(preset, true), preset);
            }
        }
    }

    #[test]
    fn stored_labels_and_indices_follow_the_build() {
        let uv_label = LightingPreset::UvLamp365.label();
        let uv_index = LightingPreset::UvLamp395.index();
        let default = LightingPreset::from_label(DEFAULT_LIGHTING_RIG);
        let expect = |uv: LightingPreset| if PHYSICS_COLOR_UI { uv } else { default };
        assert_eq!(
            offered_from_label(uv_label),
            expect(LightingPreset::UvLamp365)
        );
        assert_eq!(
            offered_from_index(uv_index),
            expect(LightingPreset::UvLamp395)
        );
        assert_eq!(offered_from_index(9999), LightingPreset::from_index(9999));
    }
}
