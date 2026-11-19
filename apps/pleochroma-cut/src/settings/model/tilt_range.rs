//! `AppSettings::tilt_average_range_deg`: how far from table up the displayed tilt averages
//! reach, its default and the rule every loaded or edited value passes through.

use pleochroma_editor::metric_deltas::{
    DEFAULT_DISPLAY_TILT_WINDOW_DEG, normalize_display_tilt_window_deg,
};

/// The default of a new or older settings file: every tilt, to 90 degrees each way.
pub(super) const fn default_tilt_average_range_deg() -> u8 {
    DEFAULT_DISPLAY_TILT_WINDOW_DEG
}

/// Reads `AppSettings::tilt_average_range_deg` as a signed integer and normalizes it, so a
/// hand-edited value that is not offered loads as 90 instead of failing the file.
pub(super) fn deserialize_tilt_average_range_deg<'de, D>(deserializer: D) -> Result<u8, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let raw = <i64 as serde::Deserialize>::deserialize(deserializer)?;
    Ok(normalize_display_tilt_window_deg(raw))
}

#[cfg(test)]
mod tests {
    use crate::settings::model::{AppSettings, SettingsFile};

    /// A settings document holding only `[settings]` plus `lines`.
    fn parse(lines: &str) -> SettingsFile {
        toml::from_str(&format!("[settings]\n{lines}\n")).expect("settings parse")
    }

    #[test]
    fn the_default_is_ninety_and_a_file_without_the_key_loads_it() {
        assert_eq!(AppSettings::default().tilt_average_range_deg, 90);
        assert_eq!(
            parse("exposure = 1.2").settings.tilt_average_range_deg,
            90,
            "an older file has no key"
        );
        let mut existing = AppSettings::default();
        existing.treat_as_existing_install();
        assert_eq!(existing.tilt_average_range_deg, 90);
    }

    #[test]
    fn each_offered_range_round_trips_through_toml() {
        for degrees in [15_u8, 20, 30, 45, 60, 90] {
            let mut file = SettingsFile::default();
            file.settings.tilt_average_range_deg = degrees;
            let text = toml::to_string_pretty(&file).expect("serialize");
            assert!(
                text.contains(&format!("tilt_average_range_deg = {degrees}")),
                "{text}"
            );
            let back: SettingsFile = toml::from_str(&text).expect("deserialize");
            assert_eq!(back.settings.tilt_average_range_deg, degrees);
            assert_eq!(back, file);
        }
    }

    #[test]
    fn a_hand_edited_range_that_is_not_offered_loads_as_ninety() {
        for (written, expected) in [
            ("15", 15),
            ("45", 45),
            ("60", 60),
            ("90", 90),
            ("0", 90),
            ("10", 90),
            ("-30", 90),
            ("91", 90),
            ("256", 90),
            ("9999999999", 90),
        ] {
            let settings = parse(&format!("tilt_average_range_deg = {written}")).settings;
            assert_eq!(settings.tilt_average_range_deg, expected, "wrote {written}");
        }
    }

    #[test]
    fn setting_the_range_goes_through_the_same_rule() {
        let mut settings = AppSettings::default();
        settings.set_tilt_average_range_deg(30);
        assert_eq!(settings.tilt_average_range_deg, 30);
        settings.set_tilt_average_range_deg(45);
        assert_eq!(settings.tilt_average_range_deg, 45);
        settings.set_tilt_average_range_deg(25);
        assert_eq!(settings.tilt_average_range_deg, 90);
        settings.set_tilt_average_range_deg(-1);
        assert_eq!(settings.tilt_average_range_deg, 90);
    }
}
