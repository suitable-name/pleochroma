//! [`AppSettings`]: the persisted application settings: render, camera, lighting,
//! remote and interface preferences.

pub use super::view_settings::{DEFAULT_LIVE_VIEW_MODE, DEFAULT_SOLID_VIEW_MODE};
use super::{
    head_shadow::{DEFAULT_HEAD_SHADOW_DEG, default_head_shadow_deg, deserialize_head_shadow_deg},
    is_zero_reflection,
    local_compute::LocalComputeTarget,
    remote_endpoint::{
        LegacyWorkerMigration, RemoteEndpoint, TiltVideoCompute, migrate_legacy_workers,
    },
    surface_glare::{DEFAULT_SURFACE_GLARE, default_surface_glare, deserialize_surface_glare},
    tilt_range::{default_tilt_average_range_deg, deserialize_tilt_average_range_deg},
    ui_preferences::{UiMode, deserialize_ui_scale_percent, normalize_ui_scale_percent},
    view_settings::default_live_view_mode,
    worker::{LiveComputeTarget, LocalPreviewScale, WorkerSettings},
};
use crate::bridge::export_thread::DEFAULT_TEMPLATE as DEFAULT_EXPORT_FILENAME_TEMPLATE;
use defaults::{
    default_editor_auto_solve_budget_ms, default_editor_dock_width,
    default_editor_inspector_height, default_export_filename_template, default_remote_batch_lanes,
    default_remote_preview_batch_size, deserialize_remote_batch_lanes,
    deserialize_remote_preview_batch_size,
};
use pleochroma::optics::{
    materials::{
        AbsorptionUnit,
        variants::{MAX_VARIANT_BANDS, VariantBandRow, VariantTensorRows},
    },
    raytracer::DEFAULT_POSE,
};
use pleochroma_cut_core::material::VariantChoice;
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;

// Defaults mirror `RenderContext::default()` and the Parameters panel's initializers,
// with one exception: `DEFAULT_LIGHTING_RIG` (the light tent) differs from
// `RenderContext::default()`'s `lighting_preset` (`LightingPreset::RingLights`).
// Startup overwrites the render context with the loaded settings, so that default only
// seeds the render thread. Camera yaw/pitch are in RADIANS (as `RenderContext`), light
// yaw/pitch in DEGREES (as the sliders); each field keeps the unit its main consumer
// uses (see `gui::startup_settings::apply_loaded_settings`).
/// `256`: the converged progressive sample count. `RenderContext::default()`'s
/// `target_samples` mirrors it.
pub const DEFAULT_TARGET_SAMPLES: u32 = 256;
/// Live render resolution -- matches `RenderContext::default()`'s `width`/`height`.
pub const DEFAULT_RENDER_WIDTH: u32 = 800;
/// Default render height for new settings.
pub const DEFAULT_RENDER_HEIGHT: u32 = 600;
/// Default max bounces for new settings: the raytracer's shared default.
pub use pleochroma::optics::raytracer::DEFAULT_MAX_BOUNCES;
/// Default exposure for new settings.
pub const DEFAULT_EXPOSURE: f32 = 1.0;
/// Default inclusion sigma s for new settings.
pub const DEFAULT_INCLUSION_SIGMA_S: f32 = 0.0;
/// Default c axis override enabled for new settings.
pub const DEFAULT_C_AXIS_OVERRIDE_ENABLED: bool = false;
/// Default c axis tilt deg for new settings.
pub const DEFAULT_C_AXIS_TILT_DEG: f32 = 0.0;
/// Default c axis azimuth deg for new settings.
pub const DEFAULT_C_AXIS_AZIMUTH_DEG: f32 = 0.0;
/// Default girdle frosted for new settings.
pub const DEFAULT_GIRDLE_FROSTED: bool = false;
/// Default edge rounding radius for new settings.
pub const DEFAULT_EDGE_ROUNDING_RADIUS: f32 = 0.0;
/// Default stone width mm for new settings.
pub const DEFAULT_STONE_WIDTH_MM: f32 = 0.0;
/// Default local preview scale for new settings.
pub const DEFAULT_LOCAL_PREVIEW_SCALE: LocalPreviewScale = LocalPreviewScale::Off;
/// Default live compute target: `LiveComputeTarget::Both`.
///
/// `Both` does nothing without a configured remote (`orchestrator::poll_tick` dispatches to a
/// remote only when `AppSettings::remote` is set), so it has an effect only once a remote is
/// configured.
pub const DEFAULT_LIVE_COMPUTE_TARGET: LiveComputeTarget = LiveComputeTarget::Both;
/// `CpuGpu` is the `LocalComputeTarget` default: hybrid CPU+GPU on a `gpu`-feature build,
/// CPU-only otherwise.
pub const DEFAULT_LOCAL_COMPUTE_TARGET: LocalComputeTarget = LocalComputeTarget::CpuGpu;
/// On by default: a configured remote's final-picture exports converge faster with the idle local
/// machine helping.
///
/// `export_thread::worker::final_picture::contribution_allowed`'s guards (compute target, matching
/// scene, HDR) keep it from changing what a picture looks like, only how fast it arrives.
pub const DEFAULT_CONTRIBUTE_TO_FINAL_PICTURE: bool = true;
/// How many pictures a catalogue preview or tilt batch keeps in flight on the remote at
/// once, for new settings. `4` keeps a coordinator with a couple of joined workers busy
/// without queueing far more pictures than it can start.
pub const DEFAULT_REMOTE_BATCH_LANES: u32 = 4;
/// The fewest remote batch lanes: one picture in flight at a time.
pub const MIN_REMOTE_BATCH_LANES: u32 = 1;
/// The most remote batch lanes. A coordinator caps its concurrent connections (64 by
/// default), and every lane holds one for as long as its picture renders.
pub const MAX_REMOTE_BATCH_LANES: u32 = 32;

/// `lanes` limited to `1..=32` ([`MIN_REMOTE_BATCH_LANES`] to [`MAX_REMOTE_BATCH_LANES`])
/// -- the one rule a loaded, edited or hand-written lane count passes through.
#[must_use]
pub fn clamp_remote_batch_lanes(lanes: u32) -> u32 {
    lanes.clamp(MIN_REMOTE_BATCH_LANES, MAX_REMOTE_BATCH_LANES)
}
/// Dispatchers a preview batch runs against a remote GPU worker when the user never chose
/// a lane count: each keeps two batches of [`DEFAULT_REMOTE_PREVIEW_BATCH_SIZE`] pictures
/// in flight on one persistent connection, so two connections already queue 40 pictures.
/// A CPU worker (or a coordinator) keeps [`DEFAULT_REMOTE_BATCH_LANES`].
pub const GPU_REMOTE_BATCH_LANES: u32 = 2;
/// Pictures per batched preview request, for new settings.
pub const DEFAULT_REMOTE_PREVIEW_BATCH_SIZE: u32 = 10;
/// The fewest pictures per batched preview request.
pub const MIN_REMOTE_PREVIEW_BATCH_SIZE: u32 = 1;
/// The most pictures per batched preview request (the protocol's `MAX_BATCH_ITEMS`).
pub const MAX_REMOTE_PREVIEW_BATCH_SIZE: u32 = 32;

/// The dispatcher count for a remote preview batch: `stored` when the user ever set a lane
/// count, [`GPU_REMOTE_BATCH_LANES`] for a GPU worker otherwise, else `stored`.
#[must_use]
pub const fn effective_remote_batch_lanes(stored: u32, user_set: bool, remote_is_gpu: bool) -> u32 {
    if remote_is_gpu && !user_set {
        GPU_REMOTE_BATCH_LANES
    } else {
        stored
    }
}

/// `size` limited to `1..=32` -- the one rule a loaded, edited or hand-written batch size
/// passes through.
#[must_use]
pub fn clamp_remote_preview_batch_size(size: u32) -> u32 {
    size.clamp(MIN_REMOTE_PREVIEW_BATCH_SIZE, MAX_REMOTE_PREVIEW_BATCH_SIZE)
}
/// Default light yaw deg for new settings.
pub const DEFAULT_LIGHT_YAW_DEG: f32 = 48.0;
/// Default light pitch deg for new settings: a near-overhead key, so the light tent's
/// 20-40 deg key cone covers the zenith and the table reads as a soft patch.
///
/// The rig's fill sits at `0.65 x` the key pitch clamped to `0.15..=1.2` rad, so the
/// fill is capped at 69 deg no matter how high the key goes.
pub const DEFAULT_LIGHT_PITCH_DEG: f32 = 72.0;
/// The light tent: the lit model whose ambient sits at middle grey with black cards for
/// facet contrast, so an undialled-in render already reads like a photograph.
pub const DEFAULT_LIGHTING_RIG: &str = "Light tent + black cards";

/// The surface the stone lies on: a lit tray seen around the stone and through its windows
/// (`EnvironmentSource::Studio::backdrop`, resolved by
/// `pleochroma::render_setup::Backdrop::resolve`).
///
/// The `index`/`from_index` logic lives once in `pleochroma::render_setup::Backdrop`, and
/// this type mirrors it variant for variant. The mirror carries the `serde` derives: serde is
/// optional in `pleochroma`, and enabling it for every consumer would grow its base
/// dependencies. The methods below convert and delegate.
///
/// Saved settings keep the choice they hold; only a fresh default is White.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Backdrop {
    /// The lighting's own ground, as lit (nothing below the girdle for the Grading tray).
    AsLit,
    /// An 18 % grey card.
    Grey,
    /// White paper (90 % reflectance), the default.
    #[default]
    White,
    /// Black velvet (2 % reflectance; appended last so stored names and indices stay valid).
    Black,
}

impl Backdrop {
    /// The tray radiance (at exposure 1) handed to `EnvironmentSource::with_backdrop`, for
    /// the scene's own lighting preset and light pose (radians); `0.0` for As Lit.
    #[must_use]
    pub fn resolve(
        self,
        preset: pleochroma::optics::raytracer::LightingPreset,
        light_yaw: f32,
        light_pitch: f32,
    ) -> f32 {
        self.to_pleochroma().resolve(preset, light_yaw, light_pitch)
    }

    /// Index into the Parameters panel's pill row (`SettingsModel.backdrop_index`).
    #[must_use]
    pub const fn index(self) -> i32 {
        self.to_pleochroma().index()
    }

    /// Inverse of [`Self::index`]; anything else is the default.
    #[must_use]
    pub const fn from_index(index: i32) -> Self {
        Self::from_pleochroma(pleochroma::render_setup::Backdrop::from_index(index))
    }

    /// Converts to the pure `pleochroma::render_setup` mirror this type's own
    /// `resolve`/`index` delegate to.
    const fn to_pleochroma(self) -> pleochroma::render_setup::Backdrop {
        match self {
            Self::AsLit => pleochroma::render_setup::Backdrop::AsLit,
            Self::Grey => pleochroma::render_setup::Backdrop::Grey,
            Self::White => pleochroma::render_setup::Backdrop::White,
            Self::Black => pleochroma::render_setup::Backdrop::Black,
        }
    }

    /// Inverse of [`Self::to_pleochroma`].
    const fn from_pleochroma(backdrop: pleochroma::render_setup::Backdrop) -> Self {
        match backdrop {
            pleochroma::render_setup::Backdrop::AsLit => Self::AsLit,
            pleochroma::render_setup::Backdrop::Grey => Self::Grey,
            pleochroma::render_setup::Backdrop::White => Self::White,
            pleochroma::render_setup::Backdrop::Black => Self::Black,
        }
    }
}
/// Default Live Render camera yaw for new settings. Deliberately NOT the raytracer's
/// shared `DEFAULT_POSE` (catalogue previews and pin tests keep that one).
pub const DEFAULT_CAMERA_YAW: f32 = 0.35;
/// Default Live Render camera pitch for new settings: about 66 deg, near face-up with the
/// table and the crown pattern both visible (a pure 90 deg view hides the crown).
pub const DEFAULT_LIVE_CAMERA_PITCH: f32 = 1.15;
/// Default camera pitch for new settings; see [`DEFAULT_LIVE_CAMERA_PITCH`].
pub const DEFAULT_CAMERA_PITCH: f32 = DEFAULT_LIVE_CAMERA_PITCH;
/// Default camera distance for new settings: the raytracer's shared default pose.
pub const DEFAULT_CAMERA_DISTANCE: f32 = DEFAULT_POSE.distance;
/// Default material for new settings.
pub const DEFAULT_MATERIAL: &str = "Diamond";
/// Square render size in pixels of the cached front/top catalogue thumbnails.
///
/// `160` keeps a card cheap to render and small enough for two side by side in the list. See
/// `bridge::preview_render`. It is a setting because speed against crispness is a matter of taste.
pub const DEFAULT_PREVIEW_SIZE: u32 = 160;
/// Samples per pixel of the cached preview thumbnails, paired with [`DEFAULT_PREVIEW_SIZE`].
pub const DEFAULT_PREVIEW_SPP: u32 = 256;
/// The Edit sub-tab's auto-solve budget in milliseconds.
///
/// After an edit, a design whose last solve took less than this is re-solved automatically
/// (debounced); see `pleochroma_editor::solve_policy::should_schedule_auto_solve`. `300` is well
/// above a small design's typical solve cost. `0` disables auto-solve: only the Solve button runs
/// it.
pub const DEFAULT_EDITOR_AUTO_SOLVE_BUDGET_MS: u32 = 300;
/// The Edit sub-tab's dock width in logical pixels.
///
/// See `EditorModel.dock_width` in `ui/models/editor.slint` and `gui::editor_layout` for the drag
/// clamp and persistence. `640.0` is the narrowest width at which the tier table shows every
/// column.
pub const DEFAULT_EDITOR_DOCK_WIDTH: f32 = 640.0;
/// The Edit sub-tab's inspector height in logical pixels (`EditorModel.inspector_height` in
/// `ui/models/editor.slint`).
///
/// `340.0` fits the Tier tab's title, its Angle, Meets, Name and Indices rows and the pinned
/// Save/Add row without scrolling.
pub const DEFAULT_EDITOR_INSPECTOR_HEIGHT: f32 = 340.0;

/// The persisted application settings: render, camera, lighting, remote and interface
/// preferences.
///
/// `#[serde(default)]` makes every field optional on load: a missing field takes its
/// default. Parse failures of the whole document are handled in `store::load_with_outcome`.
///
/// Unknown keys are ignored (no `deny_unknown_fields`), so a removed setting retires without
/// a migration and is dropped on the next save.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "mirrors the settings.toml file; the boolean fields are independent switches"
)]
pub struct AppSettings {
    /// Quality for the explicitly selected Ray-Traced view.
    #[serde(default)]
    pub ray_trace: super::RaySettings,
    /// The progressive-accumulation target sample count of the live view.
    pub target_samples: u32,
    /// Live render resolution, driving `RenderContext.width`/`.height` directly. The UI
    /// offers four pills (640x480, 800x600 default, 1280x720, 1920x1080) rather than
    /// tracking the viewport size, which Slint cannot report back to Rust. A hand-edited
    /// value outside the pills still loads and renders.
    pub render_width: u32,
    /// Render height in pixels.
    pub render_height: u32,
    /// Maximum number of ray bounces per path.
    pub max_bounces: u32,
    /// The Render tab's quality preset (Draft, Balanced, High or Custom). The three live-view
    /// values above keep their own fields; this is the persisted choice on top of them. An old
    /// file without the key reads Balanced and is shown as the preset its numbers match
    /// ([`Self::quality_preset_shown`]).
    #[serde(default)]
    pub quality_preset: super::QualityPreset,
    /// The samples, resolution and bounces Custom remembers; `None` until the user has made a
    /// mix of their own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_quality: Option<super::QualityValues>,
    /// Exposure multiplier applied when tone-mapping.
    pub exposure: f32,
    /// Light yaw in degrees.
    pub light_yaw_deg: f32,
    /// Light pitch in degrees.
    pub light_pitch_deg: f32,
    /// The lighting "rig" selection (e.g. "Gem Studio Ring Lights") -- what
    /// `RenderContext::lighting_preset` calls a "lighting preset". Named
    /// `lighting_rig` to stay unambiguous next to `LightingPreset` (the saveable
    /// bundle, which itself has a `lighting_rig` field referencing this).
    pub lighting_rig: String,
    /// Camera yaw.
    pub camera_yaw: f32,
    /// Camera pitch.
    pub camera_pitch: f32,
    /// Clockwise image-plane roll in radians. Older settings open without a roll.
    #[serde(default)]
    pub camera_roll: f32,
    /// Where the stone sits in the picture, `[x, y]` in image half-heights (x right, y up, in
    /// the picture's axes); `[0.0, 0.0]` is centred. Older settings open centred.
    #[serde(default)]
    pub camera_pan: [f32; 2],
    /// The Stone Pose, `[tilt, turn, spin]` in degrees: how the stone stands on the tray in the
    /// Live Render view. Stored with the camera pose; `[0.0, 0.0, 0.0]` is the stone as
    /// designed. Older settings open unposed.
    #[serde(default)]
    pub stone_pose: [f32; 3],
    /// Camera distance from the stone.
    pub camera_distance: f32,
    /// Name of the selected material.
    pub selected_material: String,
    /// The one remote this viewer renders with and browses the library of (a
    /// coordinator, rendering itself when started with `--render`). `None` renders
    /// locally only. See [`RemoteEndpoint`].
    #[serde(default)]
    pub remote: Option<RemoteEndpoint>,
    /// Multi-worker list of an old settings file, read (never written) only to migrate it:
    /// [`AppSettings::migrate_legacy_remote_workers`] moves its first entry into
    /// [`Self::remote`] and logs the rest as dropped. Always empty after a load.
    #[serde(rename = "remote_workers", default, skip_serializing)]
    pub legacy_remote_workers: Vec<WorkerSettings>,
    /// Whether the À-Trous denoiser is applied to the merged accumulation, regardless
    /// of which backend produced it -- a single toggle covering the whole image, never
    /// per-source, since denoising is nonlinear and can only be applied once to the
    /// fully merged result.
    #[serde(default = "default_denoise_enabled")]
    pub denoise_enabled: bool,
    /// The tray the stone lies on, lit by the lighting and seen around the stone and through
    /// its windows, in the live view and every export.
    #[serde(default)]
    pub backdrop: Backdrop,
    /// Scale of the white mirror image of the light on the table, `0.0..=1.0` (`1.0`,
    /// the default, is the unscaled render). Applies to the built-in lighting presets
    /// in the live view and every export, never to an HDR map. Limited when loaded
    /// (see [`super::clamp_surface_glare`]); a file without the key loads `1.0`.
    #[serde(
        default = "default_surface_glare",
        deserialize_with = "deserialize_surface_glare"
    )]
    pub surface_glare: f32,
    /// Angular radius in degrees of the viewer's head shadow on the lit lighting presets,
    /// `0.0..=30.0` (`0.0` is off, `16.0` the default: a head at arm's length). Live view
    /// and every export; the product-photography presets and an HDR map ignore it.
    /// Limited when loaded (see [`super::clamp_head_shadow_deg`]).
    #[serde(
        default = "default_head_shadow_deg",
        deserialize_with = "deserialize_head_shadow_deg"
    )]
    pub head_shadow_deg: f32,
    /// Whether the stone casts its spectral shadow onto the backdrop paper.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub stone_shadow: bool,
    /// Head-on reflectance of the glossy tray under the stone, `0.0..=1.0` (`0.0`, the
    /// default, is the matte tray). Studio lights with a backdrop only; limited wherever
    /// it is read into a [`super::LightingValues`].
    #[serde(default, skip_serializing_if = "is_zero_reflection")]
    pub tray_reflection: f32,
    /// Inclusion/subsurface scattering amount: the Henyey-Greenstein `sigma_s` applied via
    /// `GemMaterial::with_scattering_amount`. For the useful range (`0.05` to `3.0`) see the
    /// `scattering_sigma_s` field in `crates/pleochroma/src/optics/materials/mod.rs`. `0.0`
    /// (default) is off: the exact deterministic Beer-Lambert path.
    pub inclusion_sigma_s: f32,
    /// Crystal-axis orientation override -- off ("as cut") by default, leaving each
    /// material's own `GemMaterial::c_axis` untouched. When on, `c_axis_tilt_deg`/
    /// `c_axis_azimuth_deg` below replace it via `gui::optics::c_axis::angles_to_c_axis`.
    /// Disabled in the UI for an isotropic material, whose optic axis is physically
    /// meaningless.
    pub c_axis_override_enabled: bool,
    /// Tilt from the table normal (`+Y`), 0-90 degrees. `0.0` reproduces the default
    /// `c_axis` (`Vec3::Y`) of every built-in except Tourmaline and Aquamarine; `90.0`
    /// (azimuth `0.0`) reproduces their cut-orientation override (`Vec3::X`) to within
    /// `f32` precision.
    pub c_axis_tilt_deg: f32,
    /// Azimuth around `+Y`, 0-360 degrees.
    pub c_axis_azimuth_deg: f32,
    /// Frosted girdle finish. Off (default) leaves every facet `Polished`; on, the girdle
    /// band from `girdle::girdle_facet_finishes` renders `Frosted`.
    pub girdle_frosted: bool,
    /// Facet edge rounding radius in model units (the girdle radius is of order 1). `0.0`
    /// (default) is off. The slider's `0.03` ceiling is the largest radius the
    /// energy-conservation furnace test verifies.
    pub edge_rounding_radius: f32,
    /// Physical stone size: girdle width in millimetres to treat the active design as
    /// measuring -- see `RenderContext::stone_width_mm` for the full mechanism.
    /// `0.0` (default) is off: unscaled, this crate's pre-existing behaviour.
    pub stone_width_mm: f32,
    /// Local preview-then-settle rendering -- while the camera moves, trace at a
    /// fraction of `render_width`/`render_height`, then snap to full resolution once
    /// settled. `Off` (default) reproduces pre-existing behaviour -- see
    /// `bridge::render_thread::local_preview::effective_dimensions` for the mechanism.
    pub local_preview_scale: LocalPreviewScale,
    /// Live rendering's Local/Remote/Local+Remote choice -- see `LiveComputeTarget`.
    #[serde(default)]
    pub live_compute_target: LiveComputeTarget,
    /// Local live-rendering CPU/CPU+GPU/GPU choice -- see `LocalComputeTarget`.
    #[serde(default)]
    pub local_compute_target: LocalComputeTarget,
    /// The tilt video section's "Compute" pill (Local only / Remote only / Local + Remote),
    /// remembered across restarts. An absent key is Local + Remote; without a configured
    /// remote the video is local whatever this holds.
    #[serde(default)]
    pub tilt_video_compute_target: TiltVideoCompute,
    /// Whether this machine's idle CPU/GPU traces a share of a "final picture only" export or
    /// tilt frame alongside the remote, uploading its sum for the coordinator to fold in before
    /// tone-mapping. See [`DEFAULT_CONTRIBUTE_TO_FINAL_PICTURE`] and
    /// `export_thread::worker::final_picture::contribution_allowed` for when it applies.
    #[serde(default = "default_contribute_to_final_picture")]
    pub contribute_to_final_picture: bool,
    /// How many pictures a catalogue preview or tilt batch keeps in flight on the remote
    /// at once -- one remote dispatcher per lane, each rendering a whole picture. A
    /// coordinator with several workers wants more than one; see
    /// [`DEFAULT_REMOTE_BATCH_LANES`]. Limited to `1..=32` when loaded and when set (see
    /// [`clamp_remote_batch_lanes`]); a file without the key loads the default.
    #[serde(
        default = "default_remote_batch_lanes",
        deserialize_with = "deserialize_remote_batch_lanes"
    )]
    pub remote_batch_lanes: u32,
    /// Whether the user ever set [`Self::remote_batch_lanes`] ([`Self::set_remote_batch_lanes`]
    /// raises it). Until then a preview batch against a GPU worker uses
    /// [`GPU_REMOTE_BATCH_LANES`] dispatchers instead of the stored default, because the
    /// batched requests keep the GPU busy with few connections; a user's own choice is
    /// always honoured. A file without the key loads `false`.
    #[serde(default)]
    pub remote_batch_lanes_user_set: bool,
    /// How many pictures one batched preview request to a remote worker carries
    /// (protocol v24), limited to `1..=32` ([`clamp_remote_preview_batch_size`]); a file
    /// without the key loads [`DEFAULT_REMOTE_PREVIEW_BATCH_SIZE`].
    #[serde(
        default = "default_remote_preview_batch_size",
        deserialize_with = "deserialize_remote_preview_batch_size"
    )]
    pub remote_preview_batch_size: u32,
    /// The Rough planner's "Scan plan time limit" in seconds: a plan of a mesh rough (a
    /// scan) stops at this limit and shows the layouts found so far. `0` is no limit. See
    /// `crate::plan_limit`; a file without the key loads the default.
    #[serde(default = "default_plan_time_limit_secs")]
    pub plan_time_limit_secs: u32,
    /// The remembered answer to the question asked after an import about generating
    /// catalogue previews (`ask` shows the question every time).
    #[serde(default)]
    pub import_preview_choice: super::ImportPreviewChoice,
    /// How the viewer compresses the uploads it sends a coordinator (currently the
    /// `CONTRIBUTION` of a final-picture export): `"auto"` (default) follows the measured
    /// link speed, `"raw"`, `"lz4"`, `"zstd"` or `"zstd:LEVEL"` pin one encoding. A
    /// missing or unreadable value loads as `"auto"`.
    #[serde(
        default,
        deserialize_with = "pleochroma_net::messages::adaptive::PayloadChoice::deserialize_lenient"
    )]
    pub payload_encoding: pleochroma_net::messages::adaptive::PayloadChoice,
    /// HDR environment maps: path to the last-loaded Radiance `.hdr` file, or empty for "no
    /// map, use the studio rig" (the empty-string-means-off convention of `selected_material`).
    /// `gui::startup_settings::apply_loaded_settings` reloads it at startup via
    /// `load_env_map`. A failed load shows a toast and keeps the studio rig without clearing
    /// this field, so fixing the file and relaunching picks it back up.
    #[serde(default)]
    pub env_map_path: String,
    /// Yaw of the loaded HDR environment map in degrees, `0.0..360.0` (`0.0`, the default,
    /// is the map as loaded). Ignored without a map; loading a different map resets it.
    #[serde(default, skip_serializing_if = "is_zero_reflection")]
    pub env_map_yaw_deg: f32,
    /// Whether the HDR map turns with the camera, so the stone orbits under a map that
    /// stays fixed relative to the view. Off by default: the map is fixed in the world.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub env_map_follows_camera: bool,
    /// Square render size (pixels) for cached front/top design-catalogue preview
    /// thumbnails -- see `bridge::preview_render` and [`DEFAULT_PREVIEW_SIZE`].
    #[serde(default = "default_preview_size")]
    pub preview_size: u32,
    /// Stable thumbnail renderer; changing it makes existing thumbnails outdated once.
    #[serde(default)]
    pub thumbnail_renderer: pleochroma::renderer::engine::RenderEngine,
    /// The tray catalogue thumbnails are rendered on (White, Grey or Black); Grey by
    /// default, so a colorless stone stands apart from its background. Independent of the
    /// live view's [`Self::backdrop`]; changing it makes existing thumbnails outdated once.
    #[serde(default = "default_thumbnail_backdrop")]
    pub thumbnail_backdrop: Backdrop,
    /// Samples per pixel for the cached preview thumbnails -- see [`DEFAULT_PREVIEW_SPP`].
    #[serde(default = "default_preview_spp")]
    pub preview_spp: u32,
    /// The directory every high-resolution export writes into, chosen once via a
    /// native folder picker and remembered from then on. Empty means "not chosen
    /// yet" (same convention as `env_map_path`), which correctly triggers the
    /// one-time picker prompt on the next export.
    #[serde(default)]
    pub export_directory: String,
    /// The folder the `.asc` file and folder pickers open in, updated to whatever
    /// the cutter last imported from. Empty means "never imported", which leaves
    /// the picker at the OS default; importing one file at a time out of a single
    /// book folder is the common case, so re-navigating there every time was pure
    /// friction. Not shared with `export_directory`: designs are typically read
    /// from somewhere quite different from where renders are written.
    #[serde(default)]
    pub last_import_directory: String,
    /// The export filename template -- see `export_thread::filename_template` for
    /// the variable list and sanitising/collision behaviour. Defaults to the same
    /// template a fresh install gets (not an empty string, which would sanitise
    /// every file down to the "export" fallback name).
    #[serde(default = "default_export_filename_template")]
    pub export_filename_template: String,
    /// Whether the left-hand diagram-library panel is collapsed to a thin rail, to
    /// give the viewport more room.
    #[serde(default)]
    pub library_panel_collapsed: bool,
    /// Whether the Live Render and Edit viewports' mouse-gestures card has been shown once
    /// (it opens by itself until then). Not part of any "Reset This Tab".
    #[serde(default)]
    pub gestures_seen: bool,
    /// Whether the Live Render viewport's Face-Up figures card is folded to one line.
    /// Not part of any "Reset This Tab".
    #[serde(default)]
    pub figures_folded: bool,
    /// The solid preview's Solid/Path-Traced/Both/Diagram view-mode selector -- see
    /// [`DEFAULT_SOLID_VIEW_MODE`]. Still saved when the user changes it, but not
    /// applied at startup: the app always opens on the Live Render tab in Solid mode.
    #[serde(default)]
    pub solid_view_mode: u8,
    /// The Live Render tab's Solid/Path-traced view-mode selector -- see
    /// [`DEFAULT_LIVE_VIEW_MODE`]. Still saved when the user changes it, but not applied at
    /// startup: the Live Render tab always opens in Solid mode.
    #[serde(default = "default_live_view_mode")]
    pub live_view_mode: u8,
    /// The "Edit" sub-tab's auto-solve budget (milliseconds) -- see
    /// [`DEFAULT_EDITOR_AUTO_SOLVE_BUDGET_MS`].
    #[serde(default = "default_editor_auto_solve_budget_ms")]
    pub editor_auto_solve_budget_ms: u32,
    /// The Edit sub-tab's dock width -- see [`DEFAULT_EDITOR_DOCK_WIDTH`].
    #[serde(default = "default_editor_dock_width")]
    pub editor_dock_width: f32,
    /// The Edit sub-tab's inspector height -- see [`DEFAULT_EDITOR_INSPECTOR_HEIGHT`].
    #[serde(default = "default_editor_inspector_height")]
    pub editor_inspector_height: f32,
    /// Whether the Edit sub-tab's "DESIGN" section (`editor_design_settings.slint`)
    /// is collapsed.
    #[serde(default)]
    pub editor_settings_collapsed: bool,
    /// Whether the Edit sub-tab's "INSPECTOR" section (`editor_inspector.slint`) is
    /// collapsed.
    #[serde(default)]
    pub editor_inspector_collapsed: bool,
    /// Whether the Edit sub-tab's "TIERS" section (`editor_tier_table.slint`) is
    /// collapsed; the inspector then takes the freed space.
    #[serde(default)]
    pub editor_tier_table_collapsed: bool,
    /// Whether the Edit sub-tab's "GEAR REMAP" section (nested inside "DESIGN") is
    /// collapsed.
    #[serde(default)]
    pub editor_remap_collapsed: bool,
    /// Whether the user has changed the Edit sub-tab's layout (dock width, inspector height,
    /// or any section's collapsed state). `gui::window_sizing`'s per-screen inspector-collapse
    /// default applies only while this is `false`. `gui::editor_layout` sets it `true`.
    #[serde(default)]
    pub editor_layout_touched: bool,
    /// The Edit sub-tab's "Open Recent" list: up to [`MAX_RECENT_NATIVE_FILES`] `.psdd` design
    /// file paths, most recent first. Never a `.asc` and never a catalogue row. See
    /// [`Self::record_recent_native_file`].
    #[serde(default)]
    pub recent_native_files: Vec<String>,
    /// Keys of the `ConfirmActionDialog` prompts the user ticked "Don't ask again" on
    /// (`ui/components/confirm_action_dialog.slint`). A `BTreeSet`, not a hash set: no
    /// hash-map iteration in a decision path. Each call site owns a stable key (e.g.
    /// `"write_confirm.not_closed_solid"`); see `gui::editor::native_io::confirm`'s
    /// `confirm_keys`. A key silences only its own prompt.
    #[serde(default)]
    pub suppressed_confirmations: BTreeSet<String>,
    /// Which controls the interface shows: Simple or Advanced (one global switch, the
    /// header pill and the Preferences dialog). A file without the key is an existing
    /// install and loads [`UiMode::Advanced`] -- the interface exactly as it was; only a
    /// brand-new install ([`Self::default`]) starts in Simple. An unfamiliar word loads as
    /// Advanced too.
    #[serde(default, deserialize_with = "UiMode::deserialize_lenient")]
    pub ui_mode: UiMode,
    /// Whether the welcome tour has been shown (or skipped). A file without the key is an
    /// existing install and loads `true`: nobody who already uses the app gets a tour
    /// they did not ask for. A brand-new install starts `false`.
    #[serde(default = "default_first_run_tour_done")]
    pub first_run_tour_done: bool,
    /// The ids of the tutorials that have been finished (a `BTreeSet`, house rule: no
    /// hash-map iteration in a decision path). "Reset tutorial progress" empties it.
    #[serde(default)]
    pub tutorials_completed: BTreeSet<String>,
    /// The user-chosen interface scale in percent: `0` follows the operating system
    /// (Automatic), otherwise one of `UI_SCALE_CHOICES`. Applied once at start-up, before
    /// the first window exists, so a change needs a restart. A hand-edited value that is
    /// not offered loads as `0`.
    #[serde(default, deserialize_with = "deserialize_ui_scale_percent")]
    pub ui_scale_percent: u16,
    /// How far from table up the displayed tilt averages reach, in degrees each way: one of
    /// 15, 20, 30, 45, 60 or 90 (the default, every tilt). Compare and the Baseline figures
    /// follow it; the scoring tools always rate all tilts to 90. A missing key or a
    /// hand-edited value that is not offered loads as 90.
    #[serde(
        default = "default_tilt_average_range_deg",
        deserialize_with = "deserialize_tilt_average_range_deg"
    )]
    pub tilt_average_range_deg: u8,
    /// The high-contrast palette (`Theme.high-contrast`): near-black surfaces, white text
    /// and bright accents.
    #[serde(default)]
    pub high_contrast: bool,
    /// Larger drag handles and hit areas on the Solid viewport, for touch screens and pens.
    #[serde(default)]
    pub large_handles: bool,
    /// "Adaptive Sampling in the Viewport": while the view is still, samples move from clean
    /// tiles to noisy ones. Off by default, which keeps plain progressive accumulation.
    #[serde(default)]
    pub adaptive_viewport: bool,
    /// "Speed Up Still Views With a Path Cache": once the view stands still, the stone's paths
    /// are cached in memory and shaded from the cache. Off by default.
    #[serde(default)]
    pub path_cache_viewport: bool,
    /// "Instant Lighting Changes in the Ray-Traced View": a still Ray-Traced view keeps the
    /// light paths in memory, so a lighting or exposure change re-lights it at once. Off by
    /// default.
    #[serde(default)]
    pub instant_lighting: bool,
    /// "Beam Tracing in Still Views": once the Path-Traced view stands still, an eligible
    /// faceted stone is beam traced in the background, tile by tile. Off by default.
    /// Read and written in every build; only a build with the `beam` feature uses it, so a
    /// settings file keeps the choice through a standard build.
    #[serde(default)]
    pub beam_viewport: bool,
    /// "Instant Lighting Changes" for beam-traced still views: the beams stay in memory, so a
    /// lighting or exposure change re-lights them at once. Off by default; needs `beam_viewport`.
    /// Read and written in every build, as `beam_viewport`.
    #[serde(default)]
    pub beam_instant_lighting: bool,
    /// The most disk space kept path caches may use, in decimal gigabytes (the Path Cache Disk
    /// Limit of Preferences > Rendering). A settings file without the key loads
    /// [`DEFAULT_PATH_CACHE_DISK_LIMIT_GB`].
    #[serde(default = "default_path_cache_disk_limit_gb")]
    pub path_cache_disk_limit_gb: u32,
    /// The Solid viewport's Snap pill, remembered: `true` turns angle and depth snapping
    /// of the drag handles off.
    #[serde(default)]
    pub manipulate_snap_off: bool,
    /// The Slice tool's Symmetric pill, remembered: whether a new facet gets the whole
    /// symmetric orbit of its index. The Slice tool itself (`slice_mode`) is deliberately
    /// not remembered: it changes what a left-drag does.
    #[serde(default = "default_slice_symmetric")]
    pub slice_symmetric: bool,
    /// The Material Reference window's sort column, as the index of the column in the table
    /// header (0 is the name).
    #[serde(default)]
    pub material_reference_sort_column: u8,
    /// Whether the Material Reference window sorts descending.
    #[serde(default)]
    pub material_reference_sort_descending: bool,
    /// The Material Reference window's filter chip, as its index (0 is All).
    #[serde(default)]
    pub material_reference_filter: u8,
    /// The Material Reference window's search text.
    #[serde(default)]
    pub material_reference_query: String,
    /// Whether the Material Reference window's dispersion column shows the B-G scale (true) or
    /// the F-C scale (false).
    #[serde(default = "default_material_reference_dispersion_bg")]
    pub material_reference_dispersion_bg: bool,
    /// The Solid viewport's "Crystal Axis" toggle, remembered per user: whether the crystal axis
    /// (and the Second Axis of a biaxial stone) is drawn in the stone views. Off by default.
    /// It is also switched on for the session, without being saved, the first time the axis is
    /// edited or a planned stone with an axis is opened.
    #[serde(default)]
    pub show_crystal_axis: bool,
    /// The Stone tab's colour choice for a view that does not show the linked
    /// open design (a library design, or "Preview as..." chosen): the absorption
    /// triple `GemMaterial::with_body_color` takes, `None` for "Material default". A view
    /// setting only -- never written to a design or a catalogue entry. A value that is
    /// not three finite numbers loads as `None`; each channel is limited to zero up to
    /// [`MAX_RENDER_BODY_COLOR_CHANNEL`].
    #[serde(default, deserialize_with = "deserialize_render_body_color")]
    pub render_body_color_override: Option<[f32; 3]>,
    /// Band rows `[centre_nm, width_nm, amplitude_per_mm]` of the Live Render view color when
    /// it came from the L*C*h editor, saved beside [`Self::render_body_color_override`]. A view
    /// setting only. Rows that are not finite and positive are dropped on load, at most
    /// [`MAX_RENDER_BODY_COLOR_BANDS`] stay, and a list left empty loads as `None`. The rows are
    /// ignored while the triple is `None`.
    #[serde(
        default,
        deserialize_with = "deserialize_render_body_color_bands",
        skip_serializing_if = "Option::is_none"
    )]
    pub render_body_color_bands: Option<Vec<[f32; 3]>>,
    /// The Live Render view color as a color variant of the viewed material (a picked variant
    /// or a per-axis Custom Color), kept as a snapshot of its tensor so a pleochroic stone
    /// keeps every direction. It wins over [`Self::render_body_color_override`] and
    /// [`Self::render_body_color_bands`], which still render a file that carries only them.
    /// A view setting only. A value that cannot be used (see
    /// [`RenderBodyColorVariant::is_usable`]) loads as `None`. Read and written through
    /// [`Self::render_view_variant`] and [`Self::set_render_view_variant`].
    #[serde(
        default,
        deserialize_with = "deserialize_render_body_color_variant",
        skip_serializing_if = "Option::is_none"
    )]
    pub render_body_color_variant: Option<RenderBodyColorVariant>,
    /// The Render & Output tab's persisted output defaults (still export and tilt video),
    /// seeded into the export dialogs when they open and written back when an export
    /// starts. A file without the table loads the defaults.
    #[serde(default)]
    pub output: super::output_settings::OutputSettings,
    /// Whether the legacy app-level stone effects (`inclusion_sigma_s`, the crystal-axis
    /// trio, `girdle_frosted`, `edge_rounding_radius`, `stone_width_mm`) were already offered
    /// to a design; see [`Self::legacy_stone_defaults`]. A file without the key loads `false`,
    /// so the old values are offered once; a new install starts `true`. Those fields stay in
    /// the file as read-only values and are not read for rendering.
    #[serde(default = "default_migrated_stone_effects")]
    pub migrated_stone_effects: bool,
    /// Whether the Parameters dock (Stone / Lighting & Environment / Render & Output) is
    /// expanded, remembered across runs. A file without the key loads `false`: the dock starts
    /// folded into its thin rail.
    #[serde(default)]
    pub parameters_dock_open: bool,
    /// The Parameters dock's open tab: `0` Stone, `1` Lighting & Environment, `2` Render &
    /// Output (`ui/models/parameters.slint`). Any other value loads as the Stone tab
    /// (`gui::parameters_dock`).
    #[serde(default)]
    pub parameters_dock_tab: u8,
    /// Which folding cards of the Stone tab are open, one bit each (`ui/models/parameters.slint`,
    /// `stone_folds`): 1 Crystal Axis, 2 Girdle & Edges, 4 Clarity, 8 Structure (16 was the retired Gem Data card and is
    /// ignored). A file without
    /// the key loads `0`: all folded. Not part of any "Reset This Tab".
    #[serde(default)]
    pub stone_folds_open: u8,
    /// The new-version check: whether the user agreed to it, when it last ran and which
    /// version they skipped. A file without the table (every existing install, and a brand-new
    /// one) loads as "not asked yet", so the question is put once.
    #[serde(default)]
    pub update_check: pleochroma_update::UpdatePreferences,
    /// The cutter's faceting machine (scratch directions); never stored in a design.
    #[serde(skip_serializing_if = "super::FacetingMachineSetting::is_default")]
    pub faceting_machine: super::FacetingMachineSetting,
    /// The answer to the "RADV Shader Workaround" question (Linux, Mesa's AMD driver): `on`
    /// starts the program with `ACO_DEBUG` set so the driver's instruction scheduler fault
    /// cannot corrupt zoned stones, `off` never asks again. A file without the key, or with a
    /// value that is neither, is "not asked yet" (`None`) and the question is put to the user
    /// when the GPU turns out to be RADV. Read before the window exists, see
    /// `store::peek_radv_choice`; a change needs a restart.
    #[serde(
        default,
        deserialize_with = "super::RadvWorkaroundChoice::deserialize_lenient",
        skip_serializing_if = "Option::is_none"
    )]
    pub radv_scheduler_workaround: Option<super::RadvWorkaroundChoice>,
}

/// What a settings file WITHOUT `migrated_stone_effects` loads: `false`.
const fn default_migrated_stone_effects() -> bool {
    false
}

/// The largest absorption value one channel of
/// [`AppSettings::render_body_color_override`] may hold (the built-in presets stay under 3).
pub const MAX_RENDER_BODY_COLOR_CHANNEL: f32 = 10.0;

/// Reads [`AppSettings::render_body_color_override`] leniently: anything but three finite
/// numbers loads as `None` (so a hand-edited file cannot fail the whole settings load) and
/// each channel is limited to zero up to [`MAX_RENDER_BODY_COLOR_CHANNEL`].
fn deserialize_render_body_color<'de, D>(deserializer: D) -> Result<Option<[f32; 3]>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Lenient {
        Triple([f32; 3]),
        Other(serde::de::IgnoredAny),
    }
    Ok(match Option::<Lenient>::deserialize(deserializer)? {
        Some(Lenient::Triple(rgb)) if rgb.iter().all(|c| c.is_finite()) => {
            Some(rgb.map(|c| c.clamp(0.0, MAX_RENDER_BODY_COLOR_CHANNEL)))
        }
        _ => None,
    })
}

/// The most band rows [`AppSettings::render_body_color_bands`] keeps (the editor makes seven).
pub const MAX_RENDER_BODY_COLOR_BANDS: usize = 16;

/// Reads [`AppSettings::render_body_color_bands`] leniently: anything but a list of rows
/// loads as `None`, a row that is not three finite positive numbers is dropped, and a list
/// with no row left is `None` (so a hand-edited file cannot fail the whole settings load).
fn deserialize_render_body_color_bands<'de, D>(
    deserializer: D,
) -> Result<Option<Vec<[f32; 3]>>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Row {
        Triple([f32; 3]),
        Other(serde::de::IgnoredAny),
    }
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Lenient {
        Rows(Vec<Row>),
        Other(serde::de::IgnoredAny),
    }
    let Some(Lenient::Rows(rows)) = Option::<Lenient>::deserialize(deserializer)? else {
        return Ok(None);
    };
    let kept: Vec<[f32; 3]> = rows
        .into_iter()
        .filter_map(|row| match row {
            Row::Triple(values) if values.iter().all(|v| v.is_finite() && *v > 0.0) => Some(values),
            _ => None,
        })
        .take(MAX_RENDER_BODY_COLOR_BANDS)
        .collect();
    Ok((!kept.is_empty()).then_some(kept))
}

/// The longest key, label or material name [`AppSettings::render_body_color_variant`] keeps.
pub const MAX_RENDER_BODY_COLOR_VARIANT_TEXT: usize = 120;

/// The largest reference path (millimetres) [`AppSettings::render_body_color_variant`] keeps.
pub const MAX_RENDER_BODY_COLOR_VARIANT_PATH_MM: f32 = 1000.0;

/// What a stored view color variant's amplitudes are per (the spelling of
/// `AbsorptionUnit` in the settings file).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderBodyColorUnit {
    /// Per millimetre.
    PerMm,
    /// Per model unit, calibrated for a 7 mm stone.
    ModelUnit,
}

impl RenderBodyColorUnit {
    /// The spelling of `unit`.
    const fn of(unit: AbsorptionUnit) -> Self {
        match unit {
            AbsorptionUnit::PerMm => Self::PerMm,
            AbsorptionUnit::ModelUnit => Self::ModelUnit,
        }
    }

    /// The unit this spelling stands for.
    const fn unit(self) -> AbsorptionUnit {
        match self {
            Self::PerMm => AbsorptionUnit::PerMm,
            Self::ModelUnit => AbsorptionUnit::ModelUnit,
        }
    }
}

/// The settings file's form of the Live Render view color variant
/// ([`AppSettings::render_body_color_variant`]): a snapshot of the key, label, unit, reference
/// path and per-axis band rows (explicit band shape per row, an exact round trip of the tensor).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RenderBodyColorVariant {
    /// The variant's stable key (`"sapphire.padparadscha"`, `"user.<uuid>"`, `"custom"`).
    pub key: String,
    /// The display label (Title Case).
    pub label: String,
    /// The material the variant was picked for; a Custom Color fits any material of its rank.
    /// A file without the key loads an empty name.
    #[serde(default)]
    pub material: String,
    /// What the amplitudes are per.
    pub unit: RenderBodyColorUnit,
    /// The path (millimetres) the variant's face-up color is calibrated at.
    pub reference_path_mm: f32,
    /// The per-axis tensor as band rows.
    pub tensor: VariantTensorRows,
}

impl RenderBodyColorVariant {
    /// The stored form of `choice`, picked for `material`.
    #[must_use]
    pub fn from_choice(material: &str, choice: &VariantChoice) -> Self {
        Self {
            key: choice.key.clone(),
            label: choice.label.clone(),
            material: material.to_owned(),
            unit: RenderBodyColorUnit::of(choice.unit),
            reference_path_mm: choice.reference_path_mm,
            tensor: VariantTensorRows::from_tensor(&choice.absorption),
        }
    }

    /// The snapshot this stored form stands for (exact: `from_choice(..).to_choice()` equals the
    /// original, bit for bit).
    #[must_use]
    pub fn to_choice(&self) -> VariantChoice {
        VariantChoice {
            key: self.key.clone(),
            label: self.label.clone(),
            absorption: self.tensor.to_tensor(),
            unit: self.unit.unit(),
            reference_path_mm: self.reference_path_mm,
        }
    }

    /// Whether the renderer can use this record: a key and a label of at most
    /// [`MAX_RENDER_BODY_COLOR_VARIANT_TEXT`] characters, a material name within the same limit, a
    /// finite reference path above zero and at most
    /// [`MAX_RENDER_BODY_COLOR_VARIANT_PATH_MM`], and band rows that are finite with a positive
    /// centre and width and a nonnegative peak, at most `MAX_VARIANT_BANDS` per ray, with no ray
    /// empty while another is not (an all-empty tensor is a valid colorless color). A hand-edited
    /// value that fails this loads as no variant.
    #[must_use]
    pub fn is_usable(&self) -> bool {
        let text_ok = |text: &str| text.chars().count() <= MAX_RENDER_BODY_COLOR_VARIANT_TEXT;
        let row_ok = |row: &VariantBandRow| {
            [row.center_nm, row.width_nm, row.peak]
                .iter()
                .all(|v| v.is_finite())
                && row.center_nm > 0.0
                && row.width_nm > 0.0
                && row.peak >= 0.0
        };
        let mut rays: Vec<&[VariantBandRow]> =
            vec![self.tensor.o_ray.as_slice(), self.tensor.e_ray.as_slice()];
        if let Some(beta) = &self.tensor.beta_ray {
            rays.push(beta.as_slice());
        }
        let any_empty = rays.iter().any(|ray| ray.is_empty());
        let any_filled = rays.iter().any(|ray| !ray.is_empty());
        !self.key.trim().is_empty()
            && !self.label.trim().is_empty()
            && text_ok(&self.key)
            && text_ok(&self.label)
            && text_ok(&self.material)
            && self.reference_path_mm.is_finite()
            && self.reference_path_mm > 0.0
            && self.reference_path_mm <= MAX_RENDER_BODY_COLOR_VARIANT_PATH_MM
            && !(any_empty && any_filled)
            && rays
                .iter()
                .all(|ray| ray.len() <= MAX_VARIANT_BANDS && ray.iter().all(row_ok))
    }
}

/// Reads [`AppSettings::render_body_color_variant`] leniently: anything that is not a usable
/// record ([`RenderBodyColorVariant::is_usable`]) loads as `None`, so a hand-edited file cannot
/// fail the whole settings load.
fn deserialize_render_body_color_variant<'de, D>(
    deserializer: D,
) -> Result<Option<RenderBodyColorVariant>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum Lenient {
        Variant(RenderBodyColorVariant),
        Other(serde::de::IgnoredAny),
    }
    Ok(match Option::<Lenient>::deserialize(deserializer)? {
        Some(Lenient::Variant(variant)) if variant.is_usable() => Some(variant),
        _ => None,
    })
}

impl AppSettings {
    /// The remembered Live Render view color variant: the material it was picked for and the
    /// snapshot of its tensor (see [`Self::render_body_color_variant`]).
    #[must_use]
    pub fn render_view_variant(&self) -> Option<(String, VariantChoice)> {
        self.render_body_color_variant
            .as_ref()
            .map(|stored| (stored.material.clone(), stored.to_choice()))
    }

    /// Remembers `variant` (the material it was picked for, the snapshot) as the Live Render view
    /// color variant, or forgets it for `None`.
    pub fn set_render_view_variant(&mut self, variant: Option<(&str, &VariantChoice)>) {
        self.render_body_color_variant =
            variant.map(|(material, choice)| RenderBodyColorVariant::from_choice(material, choice));
    }
}

/// [`AppSettings::recent_native_files`]'s cap: up to ten recent files.
pub const MAX_RECENT_NATIVE_FILES: usize = 10;

const fn default_preview_size() -> u32 {
    DEFAULT_PREVIEW_SIZE
}

const fn default_preview_spp() -> u32 {
    DEFAULT_PREVIEW_SPP
}

const fn default_thumbnail_backdrop() -> Backdrop {
    Backdrop::Grey
}

const fn default_denoise_enabled() -> bool {
    true
}

/// What a settings file WITHOUT the `first_run_tour_done` key loads: `true`, because such
/// a file belongs to an existing install. [`AppSettings::default`] (a brand-new install)
/// says `false`.
const fn default_first_run_tour_done() -> bool {
    true
}

const fn default_slice_symmetric() -> bool {
    true
}

const fn default_material_reference_dispersion_bg() -> bool {
    true
}

const fn default_contribute_to_final_picture() -> bool {
    DEFAULT_CONTRIBUTE_TO_FINAL_PICTURE
}

/// The disk limit of kept path caches a new install and an old settings file start with: 50 GB.
pub const DEFAULT_PATH_CACHE_DISK_LIMIT_GB: u32 = 50;

/// The most disk space kept path caches may use, in gigabytes, limited to what the Preferences
/// field offers (1 to 100 000).
#[must_use]
pub const fn clamp_path_cache_disk_limit_gb(gb: u32) -> u32 {
    if gb < 1 {
        1
    } else if gb > 100_000 {
        100_000
    } else {
        gb
    }
}

const fn default_path_cache_disk_limit_gb() -> u32 {
    DEFAULT_PATH_CACHE_DISK_LIMIT_GB
}

const fn default_plan_time_limit_secs() -> u32 {
    crate::plan_limit::DEFAULT_LIMIT_SECS
}

mod actions;

mod defaults;

#[cfg(test)]
mod tests;
