//! The stored answer about the RADV shader workaround: [`RadvWorkaroundChoice`], the three
//! states of `AppSettings::radv_scheduler_workaround` (`on`, `off`, or no key at all: the
//! user was not asked yet) and the pure rules the settings file, the start-up code and the
//! Preferences switch share.
//!
//! What the workaround is, and why it needs a restart, is in
//! `pleochroma::renderer::gpu_driver`. No Slint here, so every rule is unit-tested directly.

use serde::{Deserialize, Deserializer, Serialize};

/// The user's answer to "use the RADV shader workaround?". Not answered is `None` in the
/// settings (the key is absent from the file), so the question is asked.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RadvWorkaroundChoice {
    /// Start with the workaround (`ACO_DEBUG` set, see `pleochroma::renderer::gpu_driver`).
    On,
    /// Never use it and never ask again.
    Off,
}

impl RadvWorkaroundChoice {
    /// The choice for the Preferences switch.
    #[must_use]
    pub const fn from_on(on: bool) -> Self {
        if on { Self::On } else { Self::Off }
    }

    /// Whether the workaround is chosen.
    #[must_use]
    pub const fn is_on(self) -> bool {
        matches!(self, Self::On)
    }

    /// Reads the text of the settings file: `on` or `off` (any capitalisation, spaces
    /// around); anything else -- an unfamiliar word a newer build wrote -- is "not answered",
    /// so the user is asked again rather than the file failing to load.
    #[must_use]
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim();
        if text.eq_ignore_ascii_case("on") {
            Some(Self::On)
        } else if text.eq_ignore_ascii_case("off") {
            Some(Self::Off)
        } else {
            None
        }
    }

    /// `serde` hook: [`Self::parse`] on a string value, anything that is not a string reads
    /// as "not answered" too, so a hand-edited file never fails to load over this key.
    ///
    /// # Errors
    ///
    /// Never fails on the value itself; only an error of the deserializer is passed on.
    pub fn deserialize_lenient<'de, D>(deserializer: D) -> Result<Option<Self>, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum Lenient {
            Text(String),
            Other(serde::de::IgnoredAny),
        }
        Ok(match Option::<Lenient>::deserialize(deserializer)? {
            Some(Lenient::Text(text)) => Self::parse(&text),
            Some(Lenient::Other(_)) | None => None,
        })
    }

    /// The `[settings] radv_scheduler_workaround` value in the settings document `text`
    /// (`None` when the key is absent, not a string, not `on`/`off`, or the document does not
    /// parse). Used at start-up, before the normal settings load (`store::peek_radv_choice`).
    #[must_use]
    pub fn in_document(text: &str) -> Option<Self> {
        toml::from_str::<toml::Table>(text)
            .ok()?
            .get("settings")?
            .as_table()?
            .get("radv_scheduler_workaround")?
            .as_str()
            .and_then(Self::parse)
    }
}

/// Whether the "RADV Shader Workaround" question should be put to the user now.
///
/// * `choice`: the stored answer; only an unanswered setting asks.
/// * `is_radv`: whether the GPU adapter the program acquired is Mesa's RADV driver.
/// * `already_active`: whether the workaround is in force in this process anyway (the user
///   set `ACO_DEBUG=nosched` themselves, or an earlier answer applied it at start-up).
/// * `linux`: RADV exists on Linux only.
#[must_use]
pub const fn should_ask(
    choice: Option<RadvWorkaroundChoice>,
    is_radv: bool,
    already_active: bool,
    linux: bool,
) -> bool {
    choice.is_none() && is_radv && !already_active && linux
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::model::{AppSettings, SettingsFile};

    fn parse_file(lines: &str) -> SettingsFile {
        toml::from_str(&format!("[settings]\n{lines}\n")).expect("settings parse")
    }

    #[test]
    fn the_setting_is_unanswered_by_default_and_in_an_old_file() {
        assert_eq!(AppSettings::default().radv_scheduler_workaround, None);
        let old = parse_file("exposure = 1.2");
        assert_eq!(old.settings.radv_scheduler_workaround, None);
    }

    #[test]
    fn on_and_off_are_read_in_any_case_and_anything_else_is_unanswered() {
        for (written, expected) in [
            ("\"on\"", Some(RadvWorkaroundChoice::On)),
            ("\"off\"", Some(RadvWorkaroundChoice::Off)),
            ("\" On \"", Some(RadvWorkaroundChoice::On)),
            ("\"OFF\"", Some(RadvWorkaroundChoice::Off)),
            ("\"sometimes\"", None),
            ("\"\"", None),
            ("true", None),
            ("1", None),
            ("[1, 2]", None),
        ] {
            let file = parse_file(&format!("radv_scheduler_workaround = {written}"));
            assert_eq!(
                file.settings.radv_scheduler_workaround, expected,
                "wrote {written}"
            );
        }
    }

    #[test]
    fn the_choice_round_trips_through_toml_and_unanswered_writes_no_key() {
        for (choice, written) in [
            (RadvWorkaroundChoice::On, "on"),
            (RadvWorkaroundChoice::Off, "off"),
        ] {
            let mut file = SettingsFile::default();
            file.settings.radv_scheduler_workaround = Some(choice);
            let text = toml::to_string_pretty(&file).expect("serialize");
            assert!(
                text.contains(&format!("radv_scheduler_workaround = \"{written}\"")),
                "{text}"
            );
            let parsed: SettingsFile = toml::from_str(&text).expect("deserialize");
            assert_eq!(parsed, file);
        }
        let text = toml::to_string_pretty(&SettingsFile::default()).expect("serialize");
        assert!(!text.contains("radv_scheduler_workaround"), "{text}");
    }

    #[test]
    fn the_start_up_peek_reads_only_that_key() {
        assert_eq!(
            RadvWorkaroundChoice::in_document("[settings]\nradv_scheduler_workaround = \"on\"\n"),
            Some(RadvWorkaroundChoice::On)
        );
        assert_eq!(
            RadvWorkaroundChoice::in_document(
                "[settings]\nexposure = 2.0\nradv_scheduler_workaround = \"off\"\n"
            ),
            Some(RadvWorkaroundChoice::Off)
        );
        for text in [
            "",
            "not toml at all = = =",
            "[settings]\nexposure = 1.0\n",
            "[settings]\nradv_scheduler_workaround = true\n",
            "[settings]\nradv_scheduler_workaround = \"maybe\"\n",
            "radv_scheduler_workaround = \"on\"\n",
            "[other]\nradv_scheduler_workaround = \"on\"\n",
        ] {
            assert_eq!(RadvWorkaroundChoice::in_document(text), None, "{text:?}");
        }
    }

    #[test]
    fn the_switch_maps_to_the_choice_and_back() {
        assert_eq!(
            RadvWorkaroundChoice::from_on(true),
            RadvWorkaroundChoice::On
        );
        assert_eq!(
            RadvWorkaroundChoice::from_on(false),
            RadvWorkaroundChoice::Off
        );
        assert!(RadvWorkaroundChoice::On.is_on());
        assert!(!RadvWorkaroundChoice::Off.is_on());
    }

    #[test]
    fn only_an_unanswered_radv_run_without_the_workaround_on_linux_is_asked() {
        let ask = |choice, is_radv, active, linux| should_ask(choice, is_radv, active, linux);
        assert!(ask(None, true, false, true));
        // Answered either way: never again.
        assert!(!ask(Some(RadvWorkaroundChoice::On), true, false, true));
        assert!(!ask(Some(RadvWorkaroundChoice::Off), true, false, true));
        // Not RADV, already in force, or not Linux: nothing to ask.
        assert!(!ask(None, false, false, true));
        assert!(!ask(None, true, true, true));
        assert!(!ask(None, true, false, false));
    }
}
