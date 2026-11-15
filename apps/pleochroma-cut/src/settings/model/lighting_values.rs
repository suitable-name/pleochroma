//! [`LightingValues`]: the one struct that carries every scene/lighting field.
//!
//! The live state (`RenderContext`), a design's own lighting (the library's
//! `design_lighting` JSON), the named presets ([`super::LightingPreset`]) and the render-job
//! scene capture all convert to and from it, so no surface can silently lack a field the
//! others carry.
//!
//! No dependency on Slint, the render thread or the GUI: pure data plus pure logic. The
//! pieces that need the render context or the GUI (`from_live`, the rig lookup, the disk
//! pin) are further inherent `impl` blocks in
//! `gui::render::design_lighting::stored`.

use super::{
    app_settings::{AppSettings, Backdrop},
    clamp_camera_pan, clamp_tray_reflection,
    head_shadow::{DEFAULT_HEAD_SHADOW_DEG, clamp_head_shadow_deg},
    is_zero_reflection,
    surface_glare::{DEFAULT_SURFACE_GLARE, clamp_surface_glare},
};
use pleochroma::render_setup::StonePose;
use serde::{Deserialize, Serialize};

/// The stored format version. Raise it only when a field changes meaning; a new optional
/// field does not need it (an older build ignores keys it does not know).
pub const FORMAT_VERSION: u32 = 1;

/// The light's elevation limits in radians: the ones the live light controls use.
pub const PITCH_RANGE_RAD: (f32, f32) = (0.15, 1.55);

/// The exposure limits: the ones the live exposure slider uses.
pub const EXPOSURE_RANGE: (f32, f32) = (0.2, 5.0);

const fn format_version() -> u32 {
    FORMAT_VERSION
}

const fn full_glare() -> f32 {
    DEFAULT_SURFACE_GLARE
}

const fn default_head_shadow() -> f32 {
    DEFAULT_HEAD_SHADOW_DEG
}

/// Every scene/lighting field, as one value.
///
/// Every field has a serde default, so a row that lacks a field (a design's stored lighting
/// without a head shadow, say) loads with the current default for it, and keys a newer build
/// added are ignored. The camera pose is left out of the serialised form when absent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LightingValues {
    /// The format this row was written in, see [`FORMAT_VERSION`].
    #[serde(default = "format_version")]
    pub version: u32,
    /// The lighting rig's label, as `LightingPreset::label` writes it.
    pub lighting_rig: String,
    /// Light yaw in degrees, `0..360`.
    pub light_yaw_deg: f32,
    /// Light pitch (elevation) in degrees.
    pub light_pitch_deg: f32,
    /// Exposure multiplier.
    pub exposure: f32,
    /// Scale of the white mirror image of the light on the table, `0.0..=1.0`.
    #[serde(default = "full_glare")]
    pub surface_glare: f32,
    /// The lit tray the stone lies on (resolved at this lighting's own rig and pose).
    #[serde(default)]
    pub backdrop: Backdrop,
    /// Head-shadow radius in degrees, `0.0..=30.0` (`0.0` is off).
    #[serde(default = "default_head_shadow")]
    pub head_shadow_deg: f32,
    /// Whether the stone casts its spectral shadow onto the backdrop paper.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stone_shadow: bool,
    /// Head-on reflectance of the glossy tray under the stone, `0.0..=1.0` (`0.0` is off).
    #[serde(default, skip_serializing_if = "is_zero_reflection")]
    pub tray_reflection: f32,
    /// The HDR environment map in use, if any. `None` means the studio rig.
    #[serde(default)]
    pub env_map_path: Option<String>,
    /// Yaw of the HDR map in degrees, `0..360` (`0.0` is the map as loaded).
    #[serde(default, skip_serializing_if = "is_zero_reflection")]
    pub env_map_yaw_deg: f32,
    /// Whether the HDR map turns with the camera.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub env_map_follows_camera: bool,
    /// Camera yaw in radians, if a pose is carried (always together with
    /// [`Self::camera_pitch`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_yaw: Option<f32>,
    /// Camera pitch in radians, if a pose is carried.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_pitch: Option<f32>,
    /// Clockwise image-plane roll in radians. An older recorded camera pose uses zero.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_roll: Option<f32>,
    /// Where the stone sits in the picture, `[x, y]` in image half-heights (x right, y up, in
    /// the picture's axes). An older recorded camera pose is centred.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_pan: Option<[f32; 2]>,
    /// The Stone Pose `[tilt, turn, spin]` in degrees, if carried. It is written only together
    /// with a camera pose; a pose without it stands the stone as designed. See
    /// [`Self::pose_stone_pose`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stone_pose: Option<[f32; 3]>,
    /// Camera distance from the stone, if carried.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub camera_distance: Option<f32>,
}

impl Default for LightingValues {
    /// The app's built-in defaults (what [`AppSettings::default`] holds).
    fn default() -> Self {
        Self::from_app_settings(&AppSettings::default())
    }
}

impl LightingValues {
    /// The app's normal lighting, as the settings file holds it. No camera pose.
    #[must_use]
    pub fn from_app_settings(app: &AppSettings) -> Self {
        Self {
            version: FORMAT_VERSION,
            lighting_rig: app.lighting_rig.clone(),
            light_yaw_deg: app.light_yaw_deg,
            light_pitch_deg: app.light_pitch_deg,
            exposure: app.exposure,
            surface_glare: app.surface_glare,
            backdrop: app.backdrop,
            head_shadow_deg: app.head_shadow_deg,
            stone_shadow: app.stone_shadow,
            tray_reflection: app.tray_reflection,
            env_map_path: (!app.env_map_path.is_empty()).then(|| app.env_map_path.clone()),
            env_map_yaw_deg: app.env_map_yaw_deg,
            env_map_follows_camera: app.env_map_follows_camera,
            camera_yaw: None,
            camera_pitch: None,
            camera_roll: None,
            camera_pan: None,
            stone_pose: None,
            camera_distance: None,
        }
        .sanitised()
    }

    /// Reads a stored row, limiting every value the way the live controls do.
    ///
    /// # Errors
    ///
    /// The parser's message when `text` is not the stored format (the rig is required).
    pub fn from_json(text: &str) -> Result<Self, String> {
        serde_json::from_str::<Self>(text)
            .map(Self::sanitised)
            .map_err(|err| err.to_string())
    }

    /// The row as JSON text.
    ///
    /// # Errors
    ///
    /// The serialiser's message (practically unreachable for this plain struct).
    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|err| err.to_string())
    }

    /// These values with each one inside the range the live controls allow, and anything
    /// that is not a number replaced by the app's default. A hand-edited or damaged row
    /// then still shows something sensible instead of feeding NaN to the tracer.
    #[must_use]
    pub fn sanitised(mut self) -> Self {
        let defaults = AppSettings::default();
        self.light_yaw_deg =
            finite_or(self.light_yaw_deg, defaults.light_yaw_deg).rem_euclid(360.0);
        self.light_pitch_deg =
            clamp_pitch_deg(finite_or(self.light_pitch_deg, defaults.light_pitch_deg));
        self.exposure =
            finite_or(self.exposure, defaults.exposure).clamp(EXPOSURE_RANGE.0, EXPOSURE_RANGE.1);
        self.surface_glare = clamp_surface_glare(self.surface_glare);
        self.head_shadow_deg = clamp_head_shadow_deg(self.head_shadow_deg);
        self.tray_reflection = clamp_tray_reflection(self.tray_reflection);
        self.env_map_path = self.env_map_path.filter(|path| !path.trim().is_empty());
        // Adding `0.0` turns a negative zero into the positive one the settings skip.
        self.env_map_yaw_deg = finite_or(self.env_map_yaw_deg, 0.0).rem_euclid(360.0) + 0.0;
        self.camera_yaw = self.camera_yaw.filter(|v| v.is_finite());
        self.camera_pitch = self.camera_pitch.filter(|v| v.is_finite());
        self.camera_roll = self.camera_roll.filter(|v| v.is_finite());
        self.camera_pan = self
            .camera_pan
            .filter(|pan| pan.iter().all(|v| v.is_finite()))
            .map(clamp_camera_pan);
        self.stone_pose = self
            .stone_pose
            .filter(|pose| pose.iter().all(|v| v.is_finite()))
            .map(|pose| StonePose::from_array(pose).to_array());
        self.camera_distance = self.camera_distance.filter(|v| v.is_finite() && *v > 0.0);
        self
    }

    /// A complete recorded camera orientation. Older poses without roll retain
    /// their original unrolled view; lighting-only values carry no orientation.
    #[must_use]
    pub const fn camera_pose(&self) -> Option<(f32, f32, f32)> {
        match (self.camera_yaw, self.camera_pitch) {
            (Some(yaw), Some(pitch)) => {
                let roll = match self.camera_roll {
                    Some(roll) if roll.is_finite() => roll,
                    _ => 0.0,
                };
                Some((yaw, pitch, roll))
            }
            _ => None,
        }
    }

    /// The pan that goes with [`Self::camera_pose`]: the recorded one, or centred for an older
    /// pose without one. Lighting-only values carry none, so applying them keeps the camera.
    #[must_use]
    pub const fn pose_pan(&self) -> Option<[f32; 2]> {
        if self.camera_pose().is_none() {
            return None;
        }
        Some(match self.camera_pan {
            Some(pan) if pan[0].is_finite() && pan[1].is_finite() => pan,
            _ => [0.0, 0.0],
        })
    }

    /// The Stone Pose that goes with [`Self::camera_pose`]: the recorded one, or the stone as
    /// designed (`[0.0, 0.0, 0.0]`) for a pose recorded without one. Lighting-only values carry
    /// none (`None`), so applying them keeps the current Stone Pose. The returned array is the
    /// stored one; the caller limits it with `StonePose::from_array`.
    #[must_use]
    pub const fn pose_stone_pose(&self) -> Option<[f32; 3]> {
        if self.camera_pose().is_none() {
            return None;
        }
        Some(match self.stone_pose {
            Some(pose) if pose[0].is_finite() && pose[1].is_finite() && pose[2].is_finite() => pose,
            _ => [0.0, 0.0, 0.0],
        })
    }

    /// The Stone Pose that stands after applying these values to a view whose current pose is
    /// `current`: the values' own pose when they carry a camera pose (identity when that pose
    /// records none), `current` when they are lighting-only.
    #[cfg(test)]
    #[must_use]
    pub const fn stone_pose_after_apply(&self, current: [f32; 3]) -> [f32; 3] {
        match self.pose_stone_pose() {
            Some(pose) => pose,
            None => current,
        }
    }

    /// The Stone Pose to store with a camera pose saved from a view posed `pose`: `None` for the
    /// stone as designed (the stored row then reads exactly as before), the pose otherwise.
    #[must_use]
    pub fn recorded_stone_pose(pose: [f32; 3]) -> Option<[f32; 3]> {
        let pose = StonePose::from_array(pose);
        (!pose.is_identity()).then_some(pose.to_array())
    }

    /// Writes these values into the lighting fields of `app`. The camera is not written:
    /// the persisted camera pose is the window's own state, not part of the lighting, and the
    /// Stone Pose, which is stored with it, is left alone too.
    pub fn write_into(&self, app: &mut AppSettings) {
        app.lighting_rig.clone_from(&self.lighting_rig);
        app.light_yaw_deg = self.light_yaw_deg;
        app.light_pitch_deg = self.light_pitch_deg;
        app.exposure = self.exposure;
        app.surface_glare = self.surface_glare;
        app.backdrop = self.backdrop;
        app.head_shadow_deg = self.head_shadow_deg;
        app.stone_shadow = self.stone_shadow;
        app.tray_reflection = self.tray_reflection;
        app.env_map_path = self.env_map_path.clone().unwrap_or_default();
        app.env_map_yaw_deg = self.env_map_yaw_deg;
        app.env_map_follows_camera = self.env_map_follows_camera;
    }
}

const fn finite_or(value: f32, fallback: f32) -> f32 {
    if value.is_finite() { value } else { fallback }
}

/// `degrees` limited to the elevation range, left exactly as it is when it already fits.
const fn clamp_pitch_deg(degrees: f32) -> f32 {
    degrees.clamp(
        PITCH_RANGE_RAD.0.to_degrees(),
        PITCH_RANGE_RAD.1.to_degrees(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Stone Shadow toggle survives the settings file, a saved lighting and a per-design
    /// lighting row, and writes back into the settings.
    #[test]
    fn stone_shadow_survives_global_saved_and_per_design_lighting() {
        let app = AppSettings {
            stone_shadow: true,
            ..AppSettings::default()
        };
        let restored: AppSettings =
            toml::from_str(&toml::to_string(&app).expect("write settings")).expect("read settings");
        let mut preset = crate::settings::model::lighting_preset::built_in_presets()
            .into_iter()
            .find(|p| !p.stone_shadow)
            .expect("a built-in without the Stone Shadow");
        preset.set_values(&LightingValues::from_app_settings(&restored));
        let row =
            LightingValues::from_json(&preset.values().to_json().expect("write design lighting"))
                .expect("read design lighting");
        let mut applied = AppSettings::default();
        row.write_into(&mut applied);
        assert!(applied.stone_shadow);
    }

    /// The Tray Reflection survives the settings file, a saved lighting and a per-design
    /// lighting row, writes back into the settings, and an off value leaves the row's JSON
    /// exactly as before.
    #[test]
    fn tray_reflection_survives_global_saved_and_per_design_lighting() {
        let app = AppSettings {
            tray_reflection: 0.04,
            ..AppSettings::default()
        };
        let restored: AppSettings =
            toml::from_str(&toml::to_string(&app).expect("write settings")).expect("read settings");
        let mut preset = crate::settings::model::lighting_preset::built_in_presets().remove(0);
        preset.set_values(&LightingValues::from_app_settings(&restored));
        let json = preset.values().to_json().expect("write design lighting");
        let row = LightingValues::from_json(&json).expect("read design lighting");
        let mut applied = AppSettings::default();
        row.write_into(&mut applied);
        assert_eq!(applied.tray_reflection.to_bits(), 0.04f32.to_bits());
        let off = LightingValues::default().to_json().expect("serialises");
        assert!(!off.contains("tray_reflection"), "{off}");
    }

    /// The reflection is limited to `0..=1` and a damaged value reads as off.
    #[test]
    fn tray_reflection_is_limited_and_damage_reads_as_off() {
        let wild = LightingValues {
            tray_reflection: 7.0,
            ..LightingValues::default()
        }
        .sanitised();
        assert_eq!(wild.tray_reflection.to_bits(), 1.0f32.to_bits());
        let damaged = LightingValues {
            tray_reflection: f32::NAN,
            ..LightingValues::default()
        }
        .sanitised();
        assert_eq!(damaged.tray_reflection.to_bits(), 0.0f32.to_bits());
    }

    /// A row without a head shadow reads the current default for it.
    #[test]
    fn a_row_without_a_head_shadow_reads_the_default() {
        let json = r#"{"lighting_rig":"ISO hemisphere","light_yaw_deg":10.0,
                       "light_pitch_deg":30.0,"exposure":1.0}"#;
        let values = LightingValues::from_json(json).expect("loads");
        assert_eq!(
            values.head_shadow_deg.to_bits(),
            DEFAULT_HEAD_SHADOW_DEG.to_bits()
        );
        assert_eq!(values.camera_yaw, None);
    }

    /// The camera pose never reaches the stored JSON unless it is carried.
    #[test]
    fn an_absent_camera_pose_is_not_serialised() {
        let json = LightingValues::default().to_json().expect("serialises");
        assert!(!json.contains("camera"), "{json}");
        assert!(json.contains("head_shadow_deg"), "{json}");
    }

    /// A row with only the rig, the light, the exposure and a map reads glare, backdrop and head
    /// shadow at their defaults.
    #[test]
    fn the_oldest_row_reads_every_newer_field_at_its_default() {
        let json = r#"{"lighting_rig":"Gem Studio Ring Lights","light_yaw_deg":48.0,
                       "light_pitch_deg":54.0,"exposure":1.0,
                       "env_map_path":"C:/env/studio.hdr"}"#;
        let values = LightingValues::from_json(json).expect("loads");
        assert_eq!(values.version, FORMAT_VERSION);
        assert!(!values.stone_shadow);
        assert_eq!(values.tray_reflection.to_bits(), 0.0f32.to_bits());
        assert_eq!(
            values.surface_glare.to_bits(),
            DEFAULT_SURFACE_GLARE.to_bits()
        );
        assert_eq!(values.backdrop, Backdrop::default());
        assert_eq!(
            values.head_shadow_deg.to_bits(),
            DEFAULT_HEAD_SHADOW_DEG.to_bits()
        );
        assert_eq!(values.env_map_path.as_deref(), Some("C:/env/studio.hdr"));
        assert_eq!((values.camera_yaw, values.camera_pitch), (None, None));
    }

    /// A row written by this build reads back as itself, and the keys a newer build might
    /// add are ignored rather than refused.
    #[test]
    fn a_row_round_trips_and_ignores_keys_it_does_not_know() {
        let values = LightingValues {
            surface_glare: 0.5,
            backdrop: Backdrop::White,
            head_shadow_deg: 8.0,
            stone_shadow: true,
            tray_reflection: 0.5,
            camera_yaw: Some(0.25),
            camera_pitch: Some(0.75),
            camera_roll: Some(1.5),
            camera_pan: Some([0.25, -0.5]),
            camera_distance: Some(2.5),
            ..LightingValues::default()
        };
        let json = values.to_json().expect("serialises");
        assert_eq!(LightingValues::from_json(&json), Ok(values.clone()));
        let newer = json.replacen('{', r#"{"a_future_field":[1,2,3],"#, 1);
        assert_eq!(LightingValues::from_json(&newer), Ok(values));
    }

    /// A recorded view brings its pan, an older recorded view without one centres the stone,
    /// and lighting-only values carry no pan, so applying them keeps the camera.
    #[test]
    fn the_pan_goes_with_the_recorded_pose_and_an_older_pose_is_centred() {
        let panned = LightingValues {
            camera_yaw: Some(0.25),
            camera_pitch: Some(0.75),
            camera_pan: Some([0.4, -0.3]),
            ..LightingValues::default()
        };
        assert_eq!(panned.pose_pan(), Some([0.4, -0.3]));

        let older = r#"{"lighting_rig":"ISO hemisphere","light_yaw_deg":10.0,
                        "light_pitch_deg":30.0,"exposure":1.0,
                        "camera_yaw":0.25,"camera_pitch":0.75,"camera_roll":0.5}"#;
        let older = LightingValues::from_json(older).expect("loads");
        assert_eq!(older.camera_pan, None);
        assert_eq!(older.pose_pan(), Some([0.0, 0.0]));

        let lighting_only = LightingValues {
            camera_pan: Some([0.4, -0.3]),
            ..LightingValues::default()
        };
        assert_eq!(lighting_only.pose_pan(), None);
        let json = LightingValues::default().to_json().expect("serialises");
        assert!(!json.contains("camera_pan"), "{json}");
    }

    /// A damaged pan reads as none (the recorded pose is then centred) and one beyond what a
    /// renderer takes is limited to it.
    #[test]
    fn a_damaged_pan_reads_as_none_and_a_wild_one_is_limited() {
        let pose = |pan| {
            LightingValues {
                camera_yaw: Some(0.25),
                camera_pitch: Some(0.75),
                camera_pan: Some(pan),
                ..LightingValues::default()
            }
            .sanitised()
        };
        let damaged = pose([f32::NAN, 0.5]);
        assert_eq!(damaged.camera_pan, None);
        assert_eq!(damaged.pose_pan(), Some([0.0, 0.0]));
        let max = pleochroma::optics::raytracer::MAX_CAMERA_PAN;
        assert_eq!(pose([40.0, -0.5]).camera_pan, Some([max, -0.5]));
    }

    /// The Stone Pose goes with the recorded camera pose only: lighting-only values carry none
    /// (the current pose stands), a pose recorded without one stands the stone as designed.
    #[test]
    fn the_stone_pose_goes_with_the_recorded_camera_pose() {
        let posed = LightingValues {
            camera_yaw: Some(0.25),
            camera_pitch: Some(0.75),
            stone_pose: Some([90.0, 30.0, 10.0]),
            ..LightingValues::default()
        };
        assert_eq!(posed.pose_stone_pose(), Some([90.0, 30.0, 10.0]));
        assert_eq!(
            posed.stone_pose_after_apply([5.0, 5.0, 5.0]),
            [90.0, 30.0, 10.0]
        );

        // Rule 2: a camera pose without a stone pose sets the stone as designed.
        let unposed = LightingValues {
            stone_pose: None,
            ..posed.clone()
        };
        assert_eq!(unposed.pose_stone_pose(), Some([0.0, 0.0, 0.0]));
        assert_eq!(
            unposed.stone_pose_after_apply([90.0, 30.0, 10.0]),
            [0.0, 0.0, 0.0]
        );

        // Rule 1: values without a camera pose keep the current stone pose.
        let lighting_only = LightingValues {
            camera_yaw: None,
            camera_pitch: None,
            ..posed
        };
        assert_eq!(lighting_only.pose_stone_pose(), None);
        assert_eq!(
            lighting_only.stone_pose_after_apply([90.0, 30.0, 10.0]),
            [90.0, 30.0, 10.0]
        );
    }

    /// The pose is serialised only when carried, survives the stored JSON, is limited like the
    /// sliders, and a damaged one reads as none (identity with a camera pose).
    #[test]
    fn the_stone_pose_round_trips_is_limited_and_is_omitted_when_absent() {
        let json = LightingValues::default().to_json().expect("serialises");
        assert!(!json.contains("stone_pose"), "{json}");

        let values = LightingValues {
            camera_yaw: Some(0.25),
            camera_pitch: Some(0.75),
            stone_pose: Some([90.0, 30.0, 10.0]),
            ..LightingValues::default()
        };
        let json = values.to_json().expect("serialises");
        assert!(json.contains("stone_pose"), "{json}");
        assert_eq!(LightingValues::from_json(&json), Ok(values));

        let pose = |stone_pose| {
            LightingValues {
                camera_yaw: Some(0.25),
                camera_pitch: Some(0.75),
                stone_pose: Some(stone_pose),
                ..LightingValues::default()
            }
            .sanitised()
        };
        assert_eq!(
            pose([400.0, 370.0, -10.0]).stone_pose,
            Some([180.0, 10.0, 350.0])
        );
        let damaged = pose([f32::NAN, 0.0, 0.0]);
        assert_eq!(damaged.stone_pose, None);
        assert_eq!(damaged.pose_stone_pose(), Some([0.0, 0.0, 0.0]));
    }

    /// Only a posed stone is recorded with a saved view, so an unposed save reads as before.
    #[test]
    fn only_a_posed_stone_is_recorded_with_a_saved_view() {
        assert_eq!(LightingValues::recorded_stone_pose([0.0, 0.0, 0.0]), None);
        assert_eq!(
            LightingValues::recorded_stone_pose([90.0, 30.0, 0.0]),
            Some([90.0, 30.0, 0.0])
        );
    }

    /// The camera fields are not lighting: neither the app's own values nor writing values back
    /// touch the Stone Pose.
    #[test]
    fn the_stone_pose_is_not_lighting() {
        let mut app = AppSettings {
            stone_pose: [90.0, 30.0, 10.0],
            ..AppSettings::default()
        };
        assert_eq!(LightingValues::from_app_settings(&app).stone_pose, None);
        let values = LightingValues {
            camera_yaw: Some(0.25),
            camera_pitch: Some(0.75),
            stone_pose: Some([10.0, 20.0, 30.0]),
            ..LightingValues::default()
        };
        values.write_into(&mut app);
        assert_eq!(app.stone_pose, [90.0, 30.0, 10.0]);
    }

    /// The map rotation and its camera switch survive the settings file, a saved lighting
    /// and a per-design row, write back, and the unturned default leaves the JSON as before.
    #[test]
    fn env_map_rotation_survives_global_saved_and_per_design_lighting() {
        let app = AppSettings {
            env_map_yaw_deg: 125.0,
            env_map_follows_camera: true,
            ..AppSettings::default()
        };
        let restored: AppSettings =
            toml::from_str(&toml::to_string(&app).expect("write settings")).expect("read settings");
        let mut preset = crate::settings::model::lighting_preset::built_in_presets().remove(0);
        preset.set_values(&LightingValues::from_app_settings(&restored));
        let json = preset.values().to_json().expect("write design lighting");
        let row = LightingValues::from_json(&json).expect("read design lighting");
        let mut applied = AppSettings::default();
        row.write_into(&mut applied);
        assert_eq!(applied.env_map_yaw_deg.to_bits(), 125.0f32.to_bits());
        assert!(applied.env_map_follows_camera);
        let off = LightingValues::default().to_json().expect("serialises");
        assert!(!off.contains("env_map_yaw_deg"), "{off}");
        assert!(!off.contains("env_map_follows_camera"), "{off}");
    }

    /// The yaw wraps into `0..360` and a damaged value reads as the unturned map.
    #[test]
    fn env_map_yaw_wraps_and_damage_reads_as_unturned() {
        let wrapped = LightingValues {
            env_map_yaw_deg: -90.0,
            ..LightingValues::default()
        }
        .sanitised();
        assert_eq!(wrapped.env_map_yaw_deg.to_bits(), 270.0f32.to_bits());
        let damaged = LightingValues {
            env_map_yaw_deg: f32::NAN,
            ..LightingValues::default()
        }
        .sanitised();
        assert_eq!(damaged.env_map_yaw_deg.to_bits(), 0.0f32.to_bits());
    }

    #[test]
    fn the_head_shadow_is_limited_like_the_slider() {
        let wild = LightingValues {
            head_shadow_deg: 80.0,
            ..LightingValues::default()
        }
        .sanitised();
        assert_eq!(wild.head_shadow_deg.to_bits(), 30.0f32.to_bits());
    }
}
