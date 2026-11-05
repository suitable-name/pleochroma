//! [`LightingPreset`]: a named, user-saveable snapshot of a full viewport "view" --
//! lighting rig plus camera pose -- plus the built-in presets shipped with the app.

use super::{
    app_settings::Backdrop,
    head_shadow::{default_head_shadow_deg, deserialize_head_shadow_deg},
    lighting_values::LightingValues,
    surface_glare::{default_surface_glare, deserialize_surface_glare},
};
use serde::{Deserialize, Serialize};

/// A named, user-saveable snapshot of the viewport's "view": the lighting rig, the HDR
/// environment map (if any), and optionally the camera pose.
///
/// # Camera pose is part of the view, deliberately
///
/// A preset captures the full view -- lighting rig AND camera pose -- so it can be
/// recalled as a complete, reproducible shot, not just a lighting mood.
/// [`camera_yaw`]/[`camera_pitch`] below hold that pose, recorded here so a future
/// reader doesn't "fix" it back to lighting-only on the assumption that switching
/// moods shouldn't reposition the camera.
///
/// # Why the camera fields are `Option`, not plain `f32`
///
/// Data compatibility: a preset saved before these fields existed has no camera data,
/// and defaulting to `0.0` would fling the stone to yaw=0/pitch=0 on next apply. `None`
/// means "no pose recorded" (predates the fields, or is a built-in); `Some` means a
/// real pose captured by the "Save as preset" viewport button.
/// `gui::lighting_presets::setup_apply_lighting_preset_callback` restores the camera
/// only when both are `Some`, otherwise leaves the current pose untouched. Radians,
/// matching `RenderContext::yaw`/`pitch`.
///
/// # `env_map_path`: `None` means "no HDR map"
///
/// `Some(path)` means a map was loaded when this preset was saved; applying it loads
/// that map. `None` covers both "predates this field" and "no map was active" -- either
/// way it means no map: a saved lighting is a complete look, so applying one without a
/// map clears any loaded map and the lighting rig it names comes back, exactly as choosing
/// a rig does (`gui::render::lighting_presets::apply_environment`). A map that is named but
/// cannot be reloaded (the file moved) leaves the current environment untouched.
///
/// # `export_usable`
///
/// Marks this preset as offered in the export dialog's preset fan-out list. Off by
/// default so an existing preset doesn't silently start appearing in every export.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LightingPreset {
    /// Display name of the preset.
    pub name: String,
    /// Built-in presets ship with the app and cannot be renamed or deleted -- see
    /// `SettingsFile::rename_preset` / `delete_preset`, which both refuse to act on one.
    #[serde(default)]
    pub built_in: bool,
    /// Light yaw in degrees.
    pub light_yaw_deg: f32,
    /// Light pitch in degrees.
    pub light_pitch_deg: f32,
    /// Exposure multiplier applied when tone-mapping.
    pub exposure: f32,
    /// Name of the lighting rig the preset selects.
    pub lighting_rig: String,
    /// Camera distance from the stone.
    pub camera_distance: f32,
    /// Camera yaw, in radians -- see this type's doc comment for why `Option`.
    #[serde(default)]
    pub camera_yaw: Option<f32>,
    /// Camera pitch, in radians -- always written/read alongside `camera_yaw` as a
    /// pair (both `Some` or both `None`).
    #[serde(default)]
    pub camera_pitch: Option<f32>,
    /// Clockwise image-plane roll, radians; an old complete camera pose uses zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_roll: Option<f32>,
    /// Loaded HDR environment map path, if one was active when saved -- see this
    /// type's doc comment for the `None`-means-"leave it alone" semantics.
    #[serde(default)]
    pub env_map_path: Option<String>,
    /// Whether this preset is offered in the export dialog's preset fan-out list.
    #[serde(default)]
    pub export_usable: bool,
    /// Scale of the white mirror image of the light, `0.0..=1.0` (see
    /// `AppSettings::surface_glare`). A preset saved before this field existed reads the
    /// default `1.0` (the unscaled render).
    #[serde(
        default = "default_surface_glare",
        deserialize_with = "deserialize_surface_glare"
    )]
    pub surface_glare: f32,
    /// The lit tray the stone lies on. A preset saved before this field existed reads the
    /// default backdrop (White since 2026-10-20); a preset that stored a choice keeps it.
    #[serde(default)]
    pub backdrop: Backdrop,
    /// Head-shadow radius in degrees, `0.0..=30.0` (see `AppSettings::head_shadow_deg`). A
    /// preset saved before this field existed reads the default `16.0`.
    #[serde(
        default = "default_head_shadow_deg",
        deserialize_with = "deserialize_head_shadow_deg"
    )]
    pub head_shadow_deg: f32,
}

impl LightingPreset {
    /// The scene/lighting fields of this preset as the one shared [`LightingValues`] struct
    /// (the HDR map path is the preset's, the camera pose is carried when both halves
    /// are present).
    ///
    /// The camera distance travels with the pose only. A lighting-only preset leaves the
    /// camera where it is, zoom included: a distance saved with the stone that was on
    /// screen then is the wrong zoom for any other design.
    #[must_use]
    pub fn values(&self) -> LightingValues {
        LightingValues {
            version: crate::settings::model::lighting_values::FORMAT_VERSION,
            lighting_rig: self.lighting_rig.clone(),
            light_yaw_deg: self.light_yaw_deg,
            light_pitch_deg: self.light_pitch_deg,
            exposure: self.exposure,
            surface_glare: self.surface_glare,
            backdrop: self.backdrop,
            head_shadow_deg: self.head_shadow_deg,
            env_map_path: self.env_map_path.clone(),
            camera_yaw: self.camera_yaw,
            camera_pitch: self.camera_pitch,
            camera_roll: self.camera_roll,
            camera_distance: self
                .camera_yaw
                .and(self.camera_pitch)
                .map(|_| self.camera_distance),
        }
    }

    /// Writes the lighting fields of `values` into this preset. The camera pose is
    /// written only when `values` carries a complete pair (yaw and pitch); an absent
    /// camera distance leaves the preset's own distance alone.
    #[allow(
        dead_code,
        reason = "the inverse of values(); the save paths build presets field by field and the tests use it"
    )]
    pub fn set_values(&mut self, values: &LightingValues) {
        self.lighting_rig.clone_from(&values.lighting_rig);
        self.light_yaw_deg = values.light_yaw_deg;
        self.light_pitch_deg = values.light_pitch_deg;
        self.exposure = values.exposure;
        self.surface_glare = values.surface_glare;
        self.backdrop = values.backdrop;
        self.head_shadow_deg = values.head_shadow_deg;
        self.env_map_path.clone_from(&values.env_map_path);
        if let (Some(yaw), Some(pitch)) = (values.camera_yaw, values.camera_pitch) {
            self.camera_yaw = Some(yaw);
            self.camera_pitch = Some(pitch);
            self.camera_roll = values.camera_roll;
        }
        if let Some(distance) = values.camera_distance {
            self.camera_distance = distance;
        }
    }

    /// The default name offered for a newly saved lighting: `"<rig> - <date>"`, so it no
    /// longer collides with a rig's own name. `date` is the caller's `YYYY-MM-DD` text.
    #[must_use]
    pub fn default_save_name(rig_label: &str, date: &str) -> String {
        format!("{rig_label} \u{2013} {date}")
    }
}

/// The built-in presets shipped with the app, kept to a small set that cannot be
/// deleted. Three carry their own tray: "Studio Softbox" a grey card, "Dramatic Spotlight"
/// (Fable decision 2026-10-20) and "Presentation" black velvet; the others use the default
/// White. Every built-in but "Presentation" is a viewing setup to judge a stone by;
/// "Presentation" is the showcase look of a jeweler's photo (2026-11-03). Names double as their stable identity -- `SettingsFile::ensure_built_in_presets`
/// matches on `name` to decide whether a loaded file already has one.
///
/// `env_map_path` is `None` and `export_usable` is `false` for every built-in, and
/// `camera_yaw`/`camera_pitch` are `None` for every built-in except "Grading tray"
/// (a view, not just a mood: it recalls the near face-up pose a grader looks from):
/// applying a lighting mood must only change the lighting, never reposition the camera
/// or swap in an environment map. They also default out of the export fan-out list.
///
/// `exposure` is the compensation on top of the rig's incident-light meter
/// (`RenderContext::effective_exposure`): `1.0` shows white paper as every rig shows it,
/// above is brighter, below darker. Each value was chosen from renders of an emerald and a
/// sapphire under the rig (2026-11-02). `camera_distance` only matters with a camera pose: the
/// Grading tray frames the stone on screen (Fit) when it is applied, the moods keep the zoom.
#[must_use]
#[allow(
    clippy::too_many_lines,
    reason = "a flat data table of struct literals, one per built-in preset"
)]
pub fn built_in_presets() -> Vec<LightingPreset> {
    vec![
        LightingPreset {
            name: "Studio Softbox".to_string(),
            built_in: true,
            light_yaw_deg: 48.0,
            light_pitch_deg: 54.0,
            exposure: 1.0,
            lighting_rig: "Gem Studio Ring Lights".to_string(),
            camera_distance: 2.4,
            camera_yaw: None,
            camera_pitch: None,
            camera_roll: None,
            env_map_path: None,
            export_usable: false,
            surface_glare: crate::settings::model::DEFAULT_SURFACE_GLARE,
            // Ring lights on white paper clip; a ring-light product shot sits on a grey card.
            backdrop: crate::settings::model::Backdrop::Grey,
            head_shadow_deg: crate::settings::model::DEFAULT_HEAD_SHADOW_DEG,
        },
        LightingPreset {
            name: "Daylight Bright".to_string(),
            built_in: true,
            light_yaw_deg: 30.0,
            light_pitch_deg: 65.0,
            exposure: 1.3,
            // D65 is 6500K, not 5500K -- `LightingPreset::from_label` parses both labels
            // identically, for compatibility with presets saved under the mislabelled string.
            lighting_rig: "D65 Daylight (6500K)".to_string(),
            camera_distance: 2.2,
            camera_yaw: None,
            camera_pitch: None,
            camera_roll: None,
            env_map_path: None,
            export_usable: false,
            surface_glare: crate::settings::model::DEFAULT_SURFACE_GLARE,
            backdrop: crate::settings::model::Backdrop::default(),
            head_shadow_deg: crate::settings::model::DEFAULT_HEAD_SHADOW_DEG,
        },
        LightingPreset {
            name: "Dramatic Spotlight".to_string(),
            built_in: true,
            light_yaw_deg: 300.0,
            light_pitch_deg: 25.0,
            exposure: 0.7,
            lighting_rig: "Dramatic Dark Spotlight".to_string(),
            camera_distance: 2.6,
            camera_yaw: None,
            camera_pitch: None,
            camera_roll: None,
            env_map_path: None,
            export_usable: false,
            surface_glare: crate::settings::model::DEFAULT_SURFACE_GLARE,
            // The dark look needs black velvet; the user can still pick another pill.
            backdrop: crate::settings::model::Backdrop::Black,
            head_shadow_deg: crate::settings::model::DEFAULT_HEAD_SHADOW_DEG,
        },
        LightingPreset {
            name: "Light Tent".to_string(),
            built_in: true,
            light_yaw_deg: 48.0,
            light_pitch_deg: 72.0,
            exposure: 1.0,
            lighting_rig: "Light tent + black cards".to_string(),
            camera_distance: 2.4,
            camera_yaw: None,
            camera_pitch: None,
            camera_roll: None,
            env_map_path: None,
            export_usable: false,
            surface_glare: crate::settings::model::DEFAULT_SURFACE_GLARE,
            backdrop: crate::settings::model::Backdrop::default(),
            head_shadow_deg: crate::settings::model::DEFAULT_HEAD_SHADOW_DEG,
        },
        // The colour-grading view: the D65 hemisphere with the observer's head shadow,
        // seen face-up. The one built-in that carries a camera pose (yaw and pitch must
        // both be `Some`; the apply callback clamps pitch to +-1.48, so 1.48 is the
        // steepest view it can restore). Yaw 0 keeps the stone's length along the picture
        // (0.35 turned it askew for nothing: the dome looks the same from every azimuth).
        // The head shadow stays the 16 degree default the optimizer scores with: the HUD
        // figures are scored with the viewport's shadow, and under this view they must be
        // the standard figures (a 24 degree shadow drew more face-up pattern, 2026-11-02,
        // but its HUD figures would no longer match Optimize).
        LightingPreset {
            name: "Grading tray".to_string(),
            built_in: true,
            light_yaw_deg: 48.0,
            light_pitch_deg: 72.0,
            exposure: 1.0,
            lighting_rig: pleochroma::optics::raytracer::LightingPreset::IsoHemisphere
                .label()
                .to_string(),
            camera_distance: 2.4,
            camera_yaw: Some(0.0),
            camera_pitch: Some(1.48),
            camera_roll: None,
            env_map_path: None,
            export_usable: false,
            surface_glare: crate::settings::model::DEFAULT_SURFACE_GLARE,
            backdrop: crate::settings::model::Backdrop::default(),
            head_shadow_deg: crate::settings::model::DEFAULT_HEAD_SHADOW_DEG,
        },
        // Exposure 1.5: metered for the sunlit paper, the stone (which sees mostly the sky)
        // read dark; half a stop more shows it as a sunlit stone looks, with the paper still
        // just white.
        LightingPreset {
            name: "Daylight Sun".to_string(),
            built_in: true,
            light_yaw_deg: 30.0,
            light_pitch_deg: 55.0,
            exposure: 1.5,
            lighting_rig: "Daylight sky + direct sun".to_string(),
            camera_distance: 2.4,
            camera_yaw: None,
            camera_pitch: None,
            camera_roll: None,
            env_map_path: None,
            export_usable: false,
            surface_glare: crate::settings::model::DEFAULT_SURFACE_GLARE,
            backdrop: crate::settings::model::Backdrop::default(),
            head_shadow_deg: crate::settings::model::DEFAULT_HEAD_SHADOW_DEG,
        },
        // Window daylight: one broad soft source low in the sky (30 degrees = 0.52 rad, in
        // the 25-35 degree band the preset is designed for), a dim room around it.
        LightingPreset {
            name: "Window daylight".to_string(),
            built_in: true,
            light_yaw_deg: 40.0,
            light_pitch_deg: 30.0,
            exposure: 1.2,
            lighting_rig: pleochroma::optics::raytracer::LightingPreset::WindowDaylight
                .label()
                .to_string(),
            camera_distance: 2.4,
            camera_yaw: None,
            camera_pitch: None,
            camera_roll: None,
            env_map_path: None,
            export_usable: false,
            surface_glare: crate::settings::model::DEFAULT_SURFACE_GLARE,
            backdrop: crate::settings::model::Backdrop::default(),
            head_shadow_deg: crate::settings::model::DEFAULT_HEAD_SHADOW_DEG,
        },
        // The showcase look of a jeweler's photo, for pictures that show a stone off rather
        // than judge it: the studio ring lights at their Studio Softbox pose over black
        // velvet, half a stop above the meter. Chosen 2026-11-03 from renders of an
        // emerald, a sapphire and a tanzanite: on black the ring lights give the most
        // saturated color and crisp sparkle (a higher ring flooded the table, a lower one
        // showed grey windows, shop lights looked flat, the sun rigs were too dark on
        // velvet), and +0.5 EV lifts a dark stone without clipping a light one. A mood: the
        // camera stays where it is.
        LightingPreset {
            name: "Presentation".to_string(),
            built_in: true,
            light_yaw_deg: 48.0,
            light_pitch_deg: 54.0,
            exposure: 1.4,
            lighting_rig: "Gem Studio Ring Lights".to_string(),
            camera_distance: 2.4,
            camera_yaw: None,
            camera_pitch: None,
            camera_roll: None,
            env_map_path: None,
            export_usable: false,
            surface_glare: crate::settings::model::DEFAULT_SURFACE_GLARE,
            backdrop: crate::settings::model::Backdrop::Black,
            head_shadow_deg: crate::settings::model::DEFAULT_HEAD_SHADOW_DEG,
        },
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A settings file serialized before `camera_yaw`/`camera_pitch`/`env_map_path`/
    /// `export_usable` existed must still deserialize, with every new field defaulted.
    #[test]
    fn a_preset_predating_the_view_fields_still_loads_with_them_defaulted() {
        let old_toml = r#"
            name = "Old Preset"
            built_in = false
            light_yaw_deg = 10.0
            light_pitch_deg = 20.0
            exposure = 1.0
            lighting_rig = "Gem Studio Ring Lights"
            camera_distance = 2.4
        "#;
        let preset: LightingPreset =
            toml::from_str(old_toml).expect("a pre-view-fields preset must still parse");
        assert_eq!(preset.camera_yaw, None);
        assert_eq!(preset.camera_pitch, None);
        assert_eq!(preset.env_map_path, None);
        assert!(!preset.export_usable);
    }

    /// A preset saved before glare, backdrop and head shadow joined the saved lighting reads
    /// the current defaults for all three, and its `values()` carry them.
    #[test]
    fn a_preset_predating_glare_backdrop_and_head_shadow_reads_their_defaults() {
        let old_toml = r#"
            name = "Before the full look"
            light_yaw_deg = 10.0
            light_pitch_deg = 20.0
            exposure = 1.0
            lighting_rig = "Gem Studio Ring Lights"
            camera_distance = 2.4
            camera_yaw = 0.6
            camera_pitch = 0.45
            env_map_path = "C:/env/studio.hdr"
            export_usable = true
        "#;
        let preset: LightingPreset = toml::from_str(old_toml).expect("an old preset parses");
        assert_eq!(
            preset.surface_glare.to_bits(),
            crate::settings::model::DEFAULT_SURFACE_GLARE.to_bits()
        );
        assert_eq!(preset.backdrop, Backdrop::default());
        assert_eq!(
            preset.head_shadow_deg.to_bits(),
            crate::settings::model::DEFAULT_HEAD_SHADOW_DEG.to_bits()
        );
        // What it did carry is untouched.
        assert!(preset.export_usable);
        assert_eq!(preset.env_map_path.as_deref(), Some("C:/env/studio.hdr"));
        let values = preset.values();
        assert_eq!(
            values.surface_glare.to_bits(),
            preset.surface_glare.to_bits()
        );
        assert_eq!(values.backdrop, preset.backdrop);
        assert_eq!(values.camera_yaw, Some(0.6));
        assert_eq!(values.camera_pose(), Some((0.6, 0.45, 0.0)));
        assert_eq!(values.camera_distance, Some(2.4));
    }

    /// The zoom belongs to a captured view: a preset with a camera pose carries its distance,
    /// a lighting-only one (every built-in mood) carries none, so applying it keeps the zoom.
    #[test]
    fn the_distance_travels_with_the_camera_pose_only() {
        for preset in built_in_presets() {
            let values = preset.values();
            assert_eq!(
                values.camera_distance.is_some(),
                values.camera_pose().is_some(),
                "{}",
                preset.name
            );
        }
        let mut shot = built_in_presets().remove(0);
        shot.camera_yaw = Some(0.2);
        shot.camera_pitch = Some(0.4);
        shot.camera_distance = 3.1;
        assert_eq!(shot.values().camera_distance, Some(3.1));
    }

    /// The whole look goes into a preset and comes back out of it through `LightingValues`.
    #[test]
    fn a_saved_lighting_carries_the_whole_look_through_lighting_values() {
        let mut preset = built_in_presets().remove(0);
        let values = LightingValues {
            lighting_rig: "Daylight sky + direct sun".to_string(),
            light_yaw_deg: 100.0,
            light_pitch_deg: 40.0,
            exposure: 1.5,
            surface_glare: 0.25,
            backdrop: Backdrop::White,
            head_shadow_deg: 4.0,
            env_map_path: None,
            camera_yaw: Some(0.1),
            camera_pitch: Some(0.2),
            camera_roll: None,
            camera_distance: Some(3.0),
            ..LightingValues::default()
        };
        preset.set_values(&values);
        assert_eq!(preset.values(), values);
    }

    /// A newly saved lighting is named after its rig and the day, so it never reads like the
    /// rig itself.
    #[test]
    fn the_default_name_of_a_saved_lighting_is_the_rig_and_the_date() {
        let name = LightingPreset::default_save_name("Light tent + black cards", "2026-10-12");
        assert_eq!(name, "Light tent + black cards \u{2013} 2026-10-12");
        assert_ne!(name, "Light tent + black cards");
    }

    /// A preset saved by the new "Save as preset" viewport button round-trips its full
    /// captured view -- camera pose and HDR map included -- through serialization.
    #[test]
    fn a_full_view_preset_round_trips_through_toml() {
        let preset = LightingPreset {
            name: "My Shot".to_string(),
            built_in: false,
            light_yaw_deg: 48.0,
            light_pitch_deg: 54.0,
            exposure: 1.0,
            lighting_rig: "Gem Studio Ring Lights".to_string(),
            camera_distance: 2.4,
            camera_yaw: Some(0.6),
            camera_pitch: Some(0.45),
            camera_roll: Some(std::f32::consts::FRAC_PI_2),
            env_map_path: Some("C:/env/studio.hdr".to_string()),
            export_usable: true,
            surface_glare: crate::settings::model::DEFAULT_SURFACE_GLARE,
            backdrop: crate::settings::model::Backdrop::default(),
            head_shadow_deg: crate::settings::model::DEFAULT_HEAD_SHADOW_DEG,
        };
        let toml_str = toml::to_string_pretty(&preset).expect("must serialize");
        let round_tripped: LightingPreset = toml::from_str(&toml_str).expect("must deserialize");
        assert_eq!(preset, round_tripped);
    }

    /// The "Presentation" built-in shows a stone off: the studio ring lights over black velvet,
    /// brighter than the meter, without moving the camera.
    #[test]
    fn the_presentation_view_is_ring_lights_on_black_velvet_above_the_meter() {
        let preset = built_in_presets()
            .into_iter()
            .find(|p| p.name == "Presentation")
            .expect("the Presentation built-in must exist");
        assert!(preset.built_in);
        assert_eq!(
            preset.lighting_rig,
            pleochroma::optics::raytracer::LightingPreset::RingLights.label()
        );
        assert_eq!(preset.backdrop, Backdrop::Black);
        assert!(preset.exposure > 1.0, "exposure {}", preset.exposure);
        assert!(preset.values().camera_pose().is_none());
    }

    /// The "Window daylight" built-in exists, selects the window rig and keeps the key low
    /// (0.45 to 0.6 rad, i.e. 25 to 35 degrees).
    #[test]
    fn window_daylight_view_preset_exists_with_a_low_light_pitch() {
        let preset = built_in_presets()
            .into_iter()
            .find(|p| p.name == "Window daylight")
            .expect("the Window daylight built-in must exist");
        assert!(preset.built_in);
        assert_eq!(
            preset.lighting_rig,
            pleochroma::optics::raytracer::LightingPreset::WindowDaylight.label()
        );
        let pitch_rad = preset.light_pitch_deg.to_radians();
        assert!(
            (0.45..=0.6).contains(&pitch_rad),
            "light pitch {pitch_rad} rad"
        );
    }

    /// Every built-in ships with no env data and is not export-usable by default, and
    /// none carries a camera pose except "Grading tray" (which carries both halves).
    #[test]
    fn built_in_presets_carry_no_camera_or_env_data_and_are_not_export_usable() {
        for preset in built_in_presets() {
            if preset.name == "Grading tray" {
                assert_eq!(preset.camera_yaw, Some(0.0));
                assert_eq!(preset.camera_pitch, Some(1.48));
                assert_eq!(
                    preset.head_shadow_deg.to_bits(),
                    crate::settings::model::DEFAULT_HEAD_SHADOW_DEG.to_bits(),
                    "the grading view shows the figures the optimizer scores"
                );
                assert_eq!(
                    preset.lighting_rig,
                    pleochroma::optics::raytracer::LightingPreset::IsoHemisphere.label()
                );
            } else {
                assert_eq!(
                    preset.camera_yaw, None,
                    "{}: only the grading tray carries a camera pose",
                    preset.name
                );
                assert_eq!(
                    preset.camera_pitch, None,
                    "{}: only the grading tray carries a camera pose",
                    preset.name
                );
            }
            assert_eq!(
                preset.env_map_path, None,
                "{}: built-ins must not carry an HDR map",
                preset.name
            );
            assert!(
                !preset.export_usable,
                "{}: built-ins must default out of the export fan-out list",
                preset.name
            );
            let tray = match preset.name.as_str() {
                "Studio Softbox" => Backdrop::Grey,
                "Dramatic Spotlight" | "Presentation" => Backdrop::Black,
                _ => Backdrop::White,
            };
            assert_eq!(preset.backdrop, tray, "{}: built-in tray", preset.name);
        }
    }
}
