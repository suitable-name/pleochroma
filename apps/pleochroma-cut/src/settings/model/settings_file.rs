//! [`SettingsFile`]: the full on-disk document, plus its lighting-preset CRUD.

use super::{
    app_settings::AppSettings,
    lighting_preset::{LightingPreset, built_in_presets},
};
use pleochroma_cut_core::rough_plan::locate::RigProfile;
use serde::{Deserialize, Serialize};

/// The full on-disk document: current settings plus the lighting-preset library and the
/// camera rigs of the Rough planner's "Locate inclusion from photos".
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SettingsFile {
    /// Application settings.
    pub settings: AppSettings,
    /// Lighting presets.
    pub presets: Vec<LightingPreset>,
    /// The named camera rigs (views, refractive indices, calibration). An additive section:
    /// a settings file without it has none and still loads, and a file without
    /// rigs is written without the section's entries.
    pub rig_profiles: Vec<RigProfile>,
}

impl Default for SettingsFile {
    fn default() -> Self {
        Self {
            settings: AppSettings::default(),
            presets: built_in_presets(),
            rig_profiles: Vec::new(),
        }
    }
}

impl SettingsFile {
    /// Adds any built-in preset missing from `self.presets` (matched by name), and refreshes
    /// an already-present one whose `built_in` flag is still `true` from the current
    /// [`built_in_presets`] table. The refresh keeps a stored built-in from holding stale
    /// values after the table changes.
    ///
    /// Every field is refreshed except `export_usable`, the one per-install choice on a
    /// built-in (see [`LightingPreset::export_usable`]). A user preset (`built_in: false`),
    /// or one renamed away from every built-in name, is never touched: only an exact name
    /// match against a `built_in: true` entry refreshes. Idempotent.
    pub fn ensure_built_in_presets(&mut self) {
        for builtin in built_in_presets() {
            if let Some(existing) = self.presets.iter_mut().find(|p| p.name == builtin.name) {
                if existing.built_in {
                    let export_usable = existing.export_usable;
                    *existing = LightingPreset {
                        export_usable,
                        ..builtin
                    };
                }
            } else {
                self.presets.push(builtin);
            }
        }
    }

    /// Creates a new user preset, or overwrites an existing user (non-built-in) preset
    /// of the same name. Refuses to shadow a built-in.
    pub fn upsert_user_preset(&mut self, preset: LightingPreset) -> Result<(), String> {
        let trimmed = preset.name.trim();
        if trimmed.is_empty() {
            return Err("Preset name cannot be empty.".to_string());
        }
        if let Some(existing) = self.presets.iter().position(|p| p.name == trimmed) {
            if self.presets[existing].built_in {
                return Err(format!(
                    "'{trimmed}' is a built-in preset and cannot be overwritten."
                ));
            }
            self.presets[existing] = preset;
        } else {
            self.presets.push(preset);
        }
        Ok(())
    }

    /// Renames a user preset, failing when the name is taken or protected.
    pub fn rename_preset(&mut self, old_name: &str, new_name: &str) -> Result<(), String> {
        let new_name = new_name.trim();
        if new_name.is_empty() {
            return Err("Preset name cannot be empty.".to_string());
        }
        if self
            .presets
            .iter()
            .any(|p| p.name == new_name && p.name != old_name)
        {
            return Err(format!("A preset named '{new_name}' already exists."));
        }
        let preset = self
            .presets
            .iter_mut()
            .find(|p| p.name == old_name)
            .ok_or_else(|| format!("Preset '{old_name}' not found."))?;
        if preset.built_in {
            return Err(format!(
                "'{old_name}' is a built-in preset and cannot be renamed."
            ));
        }
        preset.name = new_name.to_string();
        Ok(())
    }

    /// Flips a preset's `export_usable` flag -- the Parameters panel's checkbox next to
    /// each preset row. Unlike `rename_preset`/`delete_preset`, this is NOT refused
    /// for a built-in: its name/existence is protected, but whether it's useful
    /// enough for an export fan-out is purely a matter of taste.
    pub fn set_preset_export_usable(&mut self, name: &str, usable: bool) -> Result<(), String> {
        let preset = self
            .presets
            .iter_mut()
            .find(|p| p.name == name)
            .ok_or_else(|| format!("Preset '{name}' not found."))?;
        preset.export_usable = usable;
        Ok(())
    }

    /// Deletes a user preset by name.
    pub fn delete_preset(&mut self, name: &str) -> Result<(), String> {
        let idx = self
            .presets
            .iter()
            .position(|p| p.name == name)
            .ok_or_else(|| format!("Preset '{name}' not found."))?;
        if self.presets[idx].built_in {
            return Err(format!(
                "'{name}' is a built-in preset and cannot be deleted."
            ));
        }
        self.presets.remove(idx);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    impl SettingsFile {
        /// Looks up a lighting preset by name; the model tests' read accessor.
        #[must_use]
        pub fn find_preset(&self, name: &str) -> Option<&LightingPreset> {
            self.presets.iter().find(|p| p.name == name)
        }
    }

    /// A stored "Dramatic Spotlight" with the old White tray comes back Black on load, while a
    /// user preset keeps its own tray.
    #[test]
    fn ensure_built_in_presets_brings_the_dramatic_spotlight_tray_to_old_files() {
        let mut old = built_in_presets()
            .into_iter()
            .find(|p| p.name == "Dramatic Spotlight")
            .expect("the Dramatic Spotlight built-in exists");
        old.backdrop = crate::settings::model::Backdrop::White;
        let mut mine = old.clone();
        mine.name = "My Spotlight".to_string();
        mine.built_in = false;
        let mut file = SettingsFile {
            settings: AppSettings::default(),
            presets: vec![old, mine],
            rig_profiles: Vec::new(),
        };
        file.ensure_built_in_presets();
        assert_eq!(
            file.find_preset("Dramatic Spotlight").map(|p| p.backdrop),
            Some(crate::settings::model::Backdrop::Black)
        );
        assert_eq!(
            file.find_preset("My Spotlight").map(|p| p.backdrop),
            Some(crate::settings::model::Backdrop::White)
        );
    }

    #[test]
    fn ensure_built_in_presets_adds_a_missing_one() {
        let mut file = SettingsFile {
            settings: AppSettings::default(),
            presets: Vec::new(),
            rig_profiles: Vec::new(),
        };
        file.ensure_built_in_presets();
        assert_eq!(file.presets.len(), built_in_presets().len());
    }

    /// A built-in preset whose stored definition differs from the current one picks up the
    /// current definition on load, while keeping the per-install `export_usable` choice.
    #[test]
    fn ensure_built_in_presets_refreshes_a_stale_built_in_but_keeps_export_usable() {
        let current = built_in_presets()
            .into_iter()
            .next()
            .expect("at least one built-in preset exists");
        let mut stale = current.clone();
        stale.light_yaw_deg = 999.0; // deliberately wrong -- simulates a since-changed value
        stale.export_usable = true; // the one user customization to preserve

        let mut file = SettingsFile {
            settings: AppSettings::default(),
            presets: vec![stale],
            rig_profiles: Vec::new(),
        };
        file.ensure_built_in_presets();

        let refreshed = file
            .find_preset(&current.name)
            .expect("the built-in is still present under its own name");
        assert_eq!(refreshed.light_yaw_deg, current.light_yaw_deg);
        assert!(
            refreshed.export_usable,
            "export_usable must survive the refresh"
        );
    }

    /// A "Presentation" stored by an earlier version (a lighting mood without the shadow, the
    /// reflection or the product shot) comes back as the product shot on load, keeping its
    /// `export_usable` choice, while a user preset of its own keeps its values.
    #[test]
    fn ensure_built_in_presets_turns_a_stored_presentation_into_the_product_shot() {
        let mut old = built_in_presets()
            .into_iter()
            .find(|p| p.name == "Presentation")
            .expect("the Presentation built-in exists");
        old.stone_shadow = false;
        old.tray_reflection = 0.0;
        old.product_shot = false;
        old.export_usable = true;
        let mut mine = old.clone();
        mine.name = "My Presentation".to_string();
        mine.built_in = false;
        let mut file = SettingsFile {
            settings: AppSettings::default(),
            presets: vec![old, mine],
            rig_profiles: Vec::new(),
        };
        file.ensure_built_in_presets();
        let refreshed = file
            .find_preset("Presentation")
            .expect("the built-in is still there");
        assert!(refreshed.product_shot);
        assert!(refreshed.stone_shadow);
        assert_eq!(
            refreshed.tray_reflection.to_bits(),
            crate::settings::model::PRESENTATION_TRAY_REFLECTION.to_bits()
        );
        assert!(refreshed.export_usable, "the per-install choice survives");
        let mine = file
            .find_preset("My Presentation")
            .expect("the user preset is untouched");
        assert!(!mine.product_shot && !mine.stone_shadow);
        assert_eq!(mine.tray_reflection.to_bits(), 0);
    }

    /// A settings file without the rig section still loads, with no rigs.
    #[test]
    fn a_settings_file_without_rig_profiles_loads_with_none() {
        let parsed: SettingsFile = toml::from_str("[settings]\nexposure = 1.1\n").expect("parse");
        assert_eq!(parsed.rig_profiles.len(), 0);
    }

    /// A rig with an uncalibrated and a calibrated profile survives the TOML round trip, and the
    /// rest of the document is untouched by the rig section.
    #[test]
    fn rig_profiles_round_trip_through_toml() {
        use pleochroma_cut_core::rough_plan::locate::{Projection, RigProfile};

        let views = RigProfile::side_layout(
            150.0,
            30.0,
            Projection::Pinhole { focal_px: 4000.0 },
            [4000, 3000],
        );
        let mut ortho = RigProfile::new("Macro rig", views.clone(), 1.54);
        ortho.surround_n = 1.33;
        let mut file = SettingsFile::default();
        file.settings.exposure = 1.5;
        file.rig_profiles = vec![RigProfile::new("Bench", views, 1.76), ortho];

        let text = toml::to_string_pretty(&file).expect("serialize");
        let parsed: SettingsFile = toml::from_str(&text).expect("deserialize");
        assert_eq!(parsed, file);
        assert_eq!(parsed.rig_profiles.len(), 2);
        assert_eq!(parsed.rig_profiles[0].views.len(), 8);
    }

    /// A user preset (`built_in: false`) is never overwritten, even under a built-in's name.
    /// `upsert_user_preset` refuses to create such a collision; this guards the refresh path.
    #[test]
    fn ensure_built_in_presets_never_touches_a_same_named_user_preset() {
        let current = built_in_presets()
            .into_iter()
            .next()
            .expect("at least one built-in preset exists");
        let mut user_owned = current.clone();
        user_owned.built_in = false;
        user_owned.light_yaw_deg = 42.0;

        let mut file = SettingsFile {
            settings: AppSettings::default(),
            presets: vec![user_owned],
            rig_profiles: Vec::new(),
        };
        file.ensure_built_in_presets();

        let unchanged = file
            .find_preset(&current.name)
            .expect("the user preset is still present");
        assert_eq!(unchanged.light_yaw_deg, 42.0);
        assert!(!unchanged.built_in);
    }
}
