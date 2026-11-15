//! The Live View quality presets of the Render tab: Draft, Balanced, High and Custom.
//!
//! A preset is a name for three numbers: the target sample count, the render resolution and the
//! ray bounce cap. Picking Draft, Balanced or High sets all three. Touching any of the three
//! controls afterwards makes the preset Custom, and Custom remembers the values it was left with
//! ([`AppSettings::custom_quality`]), so picking Balanced and then Custom again brings the mix
//! back. The tab only shows the choice; this file is the one place that knows the numbers.
//!
//! The three values keep living where they always did (`AppSettings::target_samples`,
//! `render_width`/`render_height`, `max_bounces`), so the render loop, the exports and the
//! remote worker read what they read before. The preset is the persisted choice on top of them.

use super::app_settings::AppSettings;
use serde::{Deserialize, Serialize};

/// The preset choice of the Render tab. The discriminants are the tile indices of the UI
/// (`SettingsModel.quality_preset_index`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum QualityPreset {
    /// 64 samples, 640 x 480, 12 bounces.
    Draft,
    /// 256 samples, 800 x 600, 12 bounces: what a fresh install starts with.
    #[default]
    Balanced,
    /// 1024 samples, 1280 x 720, 24 bounces.
    High,
    /// The user's own mix, remembered in [`AppSettings::custom_quality`].
    Custom,
}

impl<'de> Deserialize<'de> for QualityPreset {
    /// Reads the `snake_case` names this type writes, and the display strings an older settings
    /// file carries ("High / Quality"): case-insensitive, judged by the text before the first
    /// `/`, with "fast" and "quality" as the old names of Draft and High. A spelling nobody
    /// knows loads as the default instead of rejecting the whole file; the shown preset is
    /// derived from the stored numbers anyway ([`AppSettings::quality_preset_shown`]).
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        let head = text.split('/').next().unwrap_or_default().trim();
        Ok(match head.to_ascii_lowercase().as_str() {
            "draft" | "fast" => Self::Draft,
            "high" | "quality" => Self::High,
            "custom" => Self::Custom,
            _ => Self::Balanced,
        })
    }
}

/// The three numbers a preset stands for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct QualityValues {
    /// Target sample count of the live view.
    pub samples: u32,
    /// Render width in pixels.
    pub width: u32,
    /// Render height in pixels.
    pub height: u32,
    /// Ray bounce cap.
    pub bounces: u32,
}

impl QualityPreset {
    /// Every preset, in tile order.
    pub const ALL: [Self; 4] = [Self::Draft, Self::Balanced, Self::High, Self::Custom];

    /// The tile index of this preset (0 Draft, 1 Balanced, 2 High, 3 Custom).
    #[must_use]
    pub const fn index(self) -> i32 {
        match self {
            Self::Draft => 0,
            Self::Balanced => 1,
            Self::High => 2,
            Self::Custom => 3,
        }
    }

    /// The preset of tile `index`; anything outside `0..=2` is Custom, which a tile can never
    /// send by mistake: the fixed presets are the ones that change values.
    #[must_use]
    pub const fn from_index(index: i32) -> Self {
        match index {
            0 => Self::Draft,
            1 => Self::Balanced,
            2 => Self::High,
            _ => Self::Custom,
        }
    }

    /// The name the tile and the toast show.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Draft => "Draft",
            Self::Balanced => "Balanced",
            Self::High => "High",
            Self::Custom => "Custom",
        }
    }

    /// The fixed values of Draft, Balanced and High; `None` for Custom.
    #[must_use]
    pub const fn fixed_values(self) -> Option<QualityValues> {
        match self {
            Self::Draft => Some(QualityValues {
                samples: 64,
                width: 640,
                height: 480,
                bounces: 12,
            }),
            Self::Balanced => Some(QualityValues {
                samples: 256,
                width: 800,
                height: 600,
                bounces: 12,
            }),
            Self::High => Some(QualityValues {
                samples: 1024,
                width: 1280,
                height: 720,
                bounces: 24,
            }),
            Self::Custom => None,
        }
    }
}

impl QualityValues {
    /// The live-view quality `settings` currently carries.
    #[must_use]
    pub const fn of(settings: &AppSettings) -> Self {
        Self {
            samples: settings.target_samples,
            width: settings.render_width,
            height: settings.render_height,
            bounces: settings.max_bounces,
        }
    }
}

impl AppSettings {
    /// The preset to show: the stored one while it still describes the live-view quality, else
    /// the fixed preset whose numbers match (a settings file from before the presets, or a
    /// hand-edited one), else Custom. Custom stays Custom whatever the numbers are.
    #[must_use]
    pub fn quality_preset_shown(&self) -> QualityPreset {
        let now = QualityValues::of(self);
        if self.quality_preset == QualityPreset::Custom
            || self.quality_preset.fixed_values() == Some(now)
        {
            return self.quality_preset;
        }
        QualityPreset::ALL
            .into_iter()
            .find(|preset| preset.fixed_values() == Some(now))
            .unwrap_or(QualityPreset::Custom)
    }

    /// Picks `preset`: Draft, Balanced and High write their numbers into the live-view quality;
    /// Custom writes the numbers it remembered (none yet: the current ones stay and become the
    /// remembered mix). Leaving Custom for a fixed preset keeps the mix it was left with.
    pub fn select_quality_preset(&mut self, preset: QualityPreset) {
        if self.quality_preset_shown() == QualityPreset::Custom {
            self.custom_quality = Some(QualityValues::of(self));
        }
        let values = match preset {
            QualityPreset::Custom => self.custom_quality,
            fixed => fixed.fixed_values(),
        };
        if let Some(values) = values {
            self.target_samples = values.samples;
            self.render_width = values.width;
            self.render_height = values.height;
            self.max_bounces = values.bounces;
        }
        self.quality_preset = preset;
        if preset == QualityPreset::Custom {
            self.custom_quality = Some(QualityValues::of(self));
        }
    }

    /// Records that the user touched the samples, the resolution or the bounces: the preset is
    /// Custom from now on and remembers the numbers as they are now. Call it after the value is
    /// written.
    pub const fn note_quality_edit(&mut self) {
        self.quality_preset = QualityPreset::Custom;
        self.custom_quality = Some(QualityValues::of(self));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::model::SettingsFile;

    #[test]
    fn the_indices_round_trip_and_the_balanced_numbers_are_the_defaults() {
        for preset in QualityPreset::ALL {
            assert_eq!(QualityPreset::from_index(preset.index()), preset);
        }
        assert_eq!(QualityPreset::from_index(-1), QualityPreset::Custom);
        let d = AppSettings::default();
        assert_eq!(
            QualityPreset::Balanced.fixed_values(),
            Some(QualityValues::of(&d))
        );
        assert_eq!(d.quality_preset, QualityPreset::Balanced);
        assert_eq!(d.quality_preset_shown(), QualityPreset::Balanced);
    }

    #[test]
    fn a_fixed_preset_sets_the_three_values() {
        let mut s = AppSettings::default();
        s.select_quality_preset(QualityPreset::High);
        assert_eq!(
            QualityValues::of(&s),
            QualityValues {
                samples: 1024,
                width: 1280,
                height: 720,
                bounces: 24
            }
        );
        assert_eq!(s.quality_preset_shown(), QualityPreset::High);
        s.select_quality_preset(QualityPreset::Draft);
        assert_eq!(s.target_samples, 64);
        assert_eq!((s.render_width, s.render_height), (640, 480));
        assert_eq!(s.max_bounces, 12);
    }

    #[test]
    fn touching_a_detail_makes_custom_and_custom_remembers_its_values() {
        let mut s = AppSettings::default();
        s.select_quality_preset(QualityPreset::Balanced);
        s.target_samples = 384;
        s.note_quality_edit();
        assert_eq!(s.quality_preset_shown(), QualityPreset::Custom);

        s.select_quality_preset(QualityPreset::Draft);
        assert_eq!(s.target_samples, 64);
        assert_eq!(s.quality_preset_shown(), QualityPreset::Draft);

        s.select_quality_preset(QualityPreset::Custom);
        assert_eq!(s.target_samples, 384, "the mix came back");
        assert_eq!((s.render_width, s.render_height), (800, 600));
        assert_eq!(s.quality_preset_shown(), QualityPreset::Custom);
    }

    #[test]
    fn custom_with_nothing_remembered_keeps_the_current_numbers() {
        let mut s = AppSettings::default();
        s.select_quality_preset(QualityPreset::High);
        s.select_quality_preset(QualityPreset::Custom);
        assert_eq!(s.target_samples, 1024);
        assert_eq!(s.quality_preset_shown(), QualityPreset::Custom);
        assert_eq!(s.custom_quality, Some(QualityValues::of(&s)));
    }

    #[test]
    fn an_old_file_shows_the_preset_its_numbers_match_or_custom() {
        let parsed: SettingsFile = toml::from_str(
            "[settings]\ntarget_samples = 64\nrender_width = 640\nrender_height = 480\n",
        )
        .expect("parse");
        assert_eq!(parsed.settings.quality_preset_shown(), QualityPreset::Draft);

        let parsed: SettingsFile =
            toml::from_str("[settings]\ntarget_samples = 384\n").expect("parse");
        assert_eq!(
            parsed.settings.quality_preset_shown(),
            QualityPreset::Custom
        );

        let parsed: SettingsFile = toml::from_str("[settings]\nexposure = 1.2\n").expect("parse");
        assert_eq!(
            parsed.settings.quality_preset_shown(),
            QualityPreset::Balanced
        );
    }

    #[test]
    fn the_choice_and_the_remembered_mix_survive_a_round_trip() {
        let mut settings = AppSettings {
            target_samples: 512,
            max_bounces: 64,
            ..Default::default()
        };
        settings.note_quality_edit();
        settings.select_quality_preset(QualityPreset::Draft);
        let file = SettingsFile {
            settings,
            ..SettingsFile::default()
        };
        let text = toml::to_string_pretty(&file).expect("serialises");
        let back: SettingsFile = toml::from_str(&text).expect("parses");
        assert_eq!(back.settings.quality_preset, QualityPreset::Draft);
        assert_eq!(back.settings.custom_quality, file.settings.custom_quality);
        let mut back = back.settings;
        back.select_quality_preset(QualityPreset::Custom);
        assert_eq!((back.target_samples, back.max_bounces), (512, 64));
    }
}
