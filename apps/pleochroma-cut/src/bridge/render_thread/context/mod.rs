//! The `RenderContext`/`FrameInputs` state: the shared, live-mutated render
//! configuration the GUI writes into and the render loop reads a per-frame snapshot
//! from ([`frame`]), plus material resolution
//! ([`materials`]).

mod cancellation;
mod defaults;
mod effects;
mod frame;
mod materials;
pub mod scene_identity;
#[cfg(test)]
mod tests;
mod view_color;
mod view_pattern;

pub use cancellation::RenderContextGuard;
pub use frame::load_env_map;
pub(super) use frame::{FrameInputs, snapshot_frame_inputs};
pub(super) use materials::resolve_frame_material;
pub use materials::{
    MaterialOverrides, MaterialSources, apply_material_overrides, resolve_material,
    resolve_material_with_override,
};
use scene_identity::SceneIdentity;
pub use view_color::{ColorTarget, ViewColor};
pub use view_pattern::ViewColorPattern;

use crate::{
    bridge::sample_cursor::LiveEpoch,
    settings::model::{LiveComputeTarget, LocalComputeTarget, LocalPreviewScale},
};
use glam::Vec3;
use pleochroma::{
    geometry::{plane::GpuFacetPlane, tool::ToolPrimitive},
    optics::{
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{LightingPreset, surface::FacetingMachine},
    },
    render_setup::StonePose,
    renderer::env_map::{EnvironmentMap, HdrOrientation},
};
use pleochroma_net::scene::effects::SceneEffects;
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

/// Everything the render thread needs to draw one frame.
#[expect(
    clippy::struct_excessive_bools,
    reason = "shared render-thread state; the flags are independent switches set by different callbacks"
)]
pub struct RenderContext {
    /// Image renderer and deterministic anti-aliasing options, separate from compute.
    pub engine: pleochroma::renderer::engine::EngineSpec,
    /// Live render resolution, set by the Parameters panel's resolution pills
    /// (`gui::render::quality`). Limited to fixed choices (640x480 ... 1920x1080) because
    /// Slint cannot report a widget's rendered size to Rust. This is always the configured
    /// resolution: `local_preview_scale` and `camera_moving` shadow a reduced copy per frame
    /// (see `local_preview::effective_dimensions`) and never change it, so export and remote
    /// sizing ignore preview scaling. A change resets accumulation on the next frame.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Camera yaw.
    pub yaw: f32,
    /// Camera pitch.
    pub pitch: f32,
    /// Clockwise rotation of the image plane, radians; independent of orbit/zoom.
    pub camera_roll: f32,
    /// Where the stone sits in the picture, `[x, y]` in image half-heights (x right, y up,
    /// in the picture's axes after the roll); `[0.0, 0.0]` is centred. Applied with
    /// `Camera::with_pan` after the roll.
    pub camera_pan: [f32; 2],
    /// Camera distance from the stone.
    pub distance: f32,
    /// Light yaw.
    pub light_yaw: f32,
    /// Light pitch.
    pub light_pitch: f32,
    /// Name of the selected material.
    pub material_name: String,
    /// A fully-resolved material that, when set, takes priority over `material_name` in
    /// [`resolve_material_with_override`]. A by-name lookup cannot represent a design with
    /// no material name or unlisted RI overrides. The writer is
    /// `gui::editor::view::inspector::sync_viewport_material_link`. `None` (default) means
    /// by-name resolution only.
    pub material_override: Option<GemMaterial>,
    /// The Live Render toolbar's view-only body colour ([`ViewColor`]: an absorption triple, see
    /// `GemMaterial::with_body_color`, plus the L*C*h editor's seven-band rows when it came
    /// from there); `None` is "Material default". Only applies while
    /// the view is NOT showing the linked open design -- there the design's own colour
    /// override rides in [`Self::material_override`] and this stays dormant. Never written
    /// to a design. Read through [`Self::effective_view_body_color`] and
    /// [`Self::tinted_material_override`]; written by `gui::render::render_body_color`.
    pub view_body_color: Option<ViewColor>,
    /// The current preview stone's color pattern, scoped to its geometry owner and material.
    /// Use [`Self::set_view_color_pattern`] to change it; it is never written to a material.
    pub view_color_pattern: Option<ViewColorPattern>,
    /// Mirror of `ViewportModel.viewport_material_linked` (the view follows the design's
    /// material), kept by `gui::render::render_body_color` so the render pipeline can tell a
    /// design-driven view from a free one. `true` (following the design) is the default.
    pub material_linked: bool,
    /// Lighting preset the render uses.
    pub lighting_preset: LightingPreset,
    /// The tray the stone lies on -- `AppSettings::backdrop`; resolved with
    /// `Backdrop::resolve` at `lighting_preset` and the light pose wherever it is used.
    pub backdrop: crate::settings::model::Backdrop,
    /// Surface-glare scale, `0.0..=1.0` (`AppSettings::surface_glare`): `1.0` is the
    /// unscaled render, `0.0` hides the mirror image of the light on the table (first
    /// surface event only). Analytic presets only -- an HDR map ignores it. Scene
    /// identity includes it, so a change restarts accumulation (local and remote).
    pub surface_glare: f32,
    /// Head-shadow radius in degrees, `0.0..=30.0` (`AppSettings::head_shadow_deg`, `0.0` = off,
    /// default `16.0`): the viewer's head hiding the sky on the lit presets. An HDR map and the
    /// product-photography presets ignore it. Scene identity includes it, so a change restarts
    /// accumulation (local and remote).
    pub head_shadow_deg: f32,
    /// Whether the stone casts its spectral shadow and colored light onto the backdrop paper
    /// (`AppSettings::stone_shadow`). Studio lights with a lit backdrop only; an HDR map, the
    /// contrast light and the ultraviolet lamps decline it with a note. Scene identity includes
    /// it, so a change restarts accumulation (local and remote); the map itself is built in the
    /// background (`tray_shadow_worker`).
    pub stone_shadow: bool,
    /// Head-on reflectance of the glossy tray under the stone, `0.0..=1.0`
    /// (`AppSettings::tray_reflection`, `0.0` = off). Studio lights with a backdrop only;
    /// other lighting declines it with a note. Scene identity includes it, so a change
    /// restarts accumulation (local and remote); the plane is cheap and resolved once per scene
    /// generation (`TrayMirrorPlan` in the render worker).
    pub tray_reflection: f32,
    /// The stone's pose on the tray (`AppSettings::stone_pose`): tilt, turn and spin in degrees,
    /// the identity being the stone as designed. Part of the Live Render camera pose and read
    /// only through [`Self::effective_stone_pose`], which gives the identity while the Edit tab
    /// is shown; the render loop applies it to the planes, tools and material it traces, and
    /// the exports and render jobs carry it as it is (`SceneSnapshot::stone_pose`).
    pub stone_pose: StonePose,
    /// Whether the Live Render tab is the one shown (`render_view_tab == 0`), kept by
    /// `gui::render::render_visibility::recompute_tab_visible`. The Edit tab never shows the
    /// stone posed, and its traced modes share this render loop, so the loop learns the tab here.
    pub live_tab_shown: bool,
    /// Whether a pose drag or a pose slider is held: the Stone Shadow starts no new build while
    /// it is, and for 250 ms after it is released. Written by the Live Render viewport's pose
    /// gesture and the Lighting tab's pose sliders. Not a scene field: `scene_identity` ignores
    /// it.
    pub pose_drag_held: bool,
    /// Progressive-accumulation target for the live image. The render loop stops once local
    /// plus (while combining) the live epoch's remote samples reach it, and a settled
    /// epoch's shared cursor covers exactly `[0, target_samples)`. Passed through
    /// `EngineSpec::target_samples` each frame.
    pub target_samples: u32,
    /// Maximum number of ray bounces per path.
    pub max_bounces: u32,
    /// Exposure multiplier applied when tone-mapping.
    pub exposure: f32,
    /// Inclusion/subsurface scattering amount, applied via
    /// `GemMaterial::with_scattering_amount`. `0.0` (default) is off; useful range is
    /// `0.05` (barely perceptible) to `3.0` (milky) -- see `scattering_sigma_s`.
    pub inclusion_sigma_s: f32,
    /// Crystal-axis orientation override: `Some(axis)` replaces the resolved
    /// material's `c_axis` (skipped for isotropic materials); `None` (default, "as
    /// cut") leaves it untouched. Already resolved to a `Vec3` --
    /// `gui::optics::c_axis::angles_to_c_axis` converts the Parameters panel's
    /// tilt/azimuth sliders here.
    pub c_axis_override: Option<Vec3>,
    /// The Second Axis (the alpha direction of a biaxial stone) that goes with
    /// [`Self::c_axis_override`]: `Some(alpha)` replaces a biaxial material's own alpha axis
    /// (skipped for uniaxial and isotropic ones); `None` (default) leaves it untouched. Set
    /// from the design's stone effects together with the Crystal Axis
    /// (`pleochroma_editor::stone_orientation::orientation_override`); part of the scene
    /// identity, so a change restarts accumulation (local and remote).
    pub alpha_axis_override: Option<Vec3>,
    /// Bruted (frosted) girdle finish toggle: `true` renders the identified girdle
    /// band as `FacetFinish::Frosted` instead of `Polished` -- see
    /// `GirdleFinishCache`. `false` (default) is the all-polished path.
    pub girdle_frosted: bool,
    /// Facet edge (meet-point) rounding radius, via `GemMaterial::with_edge_rounding`.
    /// `0.0` (default) is off (sharp edges). See `edge_rounding_radius` in
    /// `crates/pleochroma/src/optics/materials/mod.rs` for units/range.
    pub edge_rounding_radius: f32,
    /// The render physics (inclusions, play-of-color, facet surfaces, abraded edge); the
    /// default record is "none" and renders as before. See `context::effects`.
    pub scene_effects: SceneEffects,
    /// The whole stone's non-ideal facet surfaces, by plane ([`Self::claim_active_surfaces`]).
    pub surface_table: Vec<(GpuFacetPlane, u32)>,
    /// The user's faceting machine (`AppSettings::faceting_machine`), written into
    /// [`Self::scene_effects`] while the stone has a surface finish.
    pub faceting_machine: FacetingMachine,
    /// Physical stone size: girdle width in mm for absorption/scattering. `0.0`
    /// (default) is off. When positive, scales by typed mm divided by the design's
    /// measured girdle width, so the value persists across `active_planes` owner
    /// changes (catalogue browse, editor solve completion) and can scale for a
    /// different stone. See [`Self::claim_active_planes`], which logs this case.
    pub stone_width_mm: f32,
    /// The active design's facet planes, and the catalogue's custom materials.
    ///
    /// `Arc<Vec<..>>`, not a bare `Vec`: `snapshot_frame_inputs` reads both out of this
    /// struct on every render-loop iteration (~every 16ms), and `GemMaterial` carries
    /// `String`s/`Vec`s of its own, so a deep clone of either field is real,
    /// non-trivial work repeated 60x/second for data that is usually unchanged frame
    /// to frame. An `Arc` clone there is one atomic increment instead. A writer that
    /// needs to mutate in place (rather than replace wholesale) goes through
    /// `Arc::make_mut` (see `gui::optics::custom_materials`), which only pays for an
    /// actual deep copy on the rare frame where a render-loop snapshot is still
    /// outstanding.
    pub active_planes: Arc<Vec<GpuFacetPlane>>,
    /// The frosted facet planes of the active design: indices into [`Self::active_planes`],
    /// ascending (`Design::frosted_planes_from_solved`). Empty for every design without
    /// frosting, which keeps its render exactly as it was; the girdle toggle
    /// ([`Self::girdle_frosted`]) is independent and adds to it.
    ///
    /// Cleared by every claim of the planes ([`Self::claim_active_geometry`]), so a writer
    /// that knows nothing of frosting shows a polished stone; a writer that does follows its
    /// claim with [`Self::claim_active_frosted`]. An `Arc` for the same reason `active_planes`
    /// is one.
    pub active_frosted: Arc<Vec<u32>>,
    /// The concave tools that cut into [`Self::active_planes`], in cutting order; empty
    /// for every planar design (the overwhelmingly common case, so the default and every
    /// writer that knows nothing about tools leave the stone convex).
    ///
    /// Lives beside the planes rather than inside them because the planes are the convex
    /// polyhedron the tools are subtracted from, and the GPU, the guide pass and the
    /// stone-width measurement all want that polyhedron alone. Written only through
    /// [`Self::claim_active_geometry`] (or [`Self::claim_active_planes`], which clears
    /// it) so the two can never describe different stones. An `Arc` for the same reason
    /// `active_planes` is one: it is cloned into every frame's inputs.
    pub active_tools: Arc<Vec<ToolPrimitive>>,
    /// `(concave tier, placement)` of each of [`Self::active_tools`], parallel to it:
    /// bookkeeping the solid preview needs to name a tool's facet when a camera drag
    /// re-issues the geometry (`gui::render::camera_lighting`). Not scene-defining, so
    /// it is not part of the scene identity.
    pub active_placements: Vec<(usize, usize)>,
    /// The gear tooth count and reference angle of the design whose planes are in
    /// `active_planes`. Writers set it together with the planes.
    ///
    /// A camera-drag `Reproject` request carries no `Design`, so it takes the gear info
    /// from here. Without it the solid preview would reuse the gear info of its last
    /// replan, which may belong to another design. `None` leaves the solid preview's
    /// last gear info untouched, as for the built-in placeholder cut.
    pub design_gear: Option<(u32, f32)>,
    /// Which writer last claimed the `active_planes` slot. Arbitration uses it, see
    /// [`Self::may_claim_active_planes`]. Writes go through [`Self::claim_active_planes`].
    pub planes_owner: PlanesOwner,
    /// The editor's design generation counter (`EditorState` keeps the one `Arc` for its whole
    /// life), registered by the editor's setup; `None` before that.
    pub editor_generation: Option<Arc<AtomicU64>>,
    /// The editor generation that was current when a Library selection last claimed the slot;
    /// `None` while anything else owns it. See [`Self::library_took_over_since`].
    pub catalogue_over_generation: Option<u64>,
    /// Why the design cannot be traced honestly, as a cutter-facing message. `None` when
    /// tracing is valid. Set when a design has no material name and its refractive index
    /// matches no built-in preset, or when it names a material that is neither a built-in
    /// nor a saved custom material. The render is refused rather than substituting Diamond.
    /// Shown in the viewport banner (`ViewportModel.trace_refusal`). `SceneSnapshot::capture`,
    /// the remote dispatch tick and the tilt-video export also refuse while this is set.
    pub material_unresolved: Option<String>,
    /// The editor's viewport material while a Library selection holds the planes slot,
    /// restored when the editor claims the slot back. Without it, a Library row with no
    /// material would leave its refusal in [`Self::material_unresolved`] and tracing
    /// suspended.
    pub editor_material_stash: Option<MaterialStash>,
    /// User-defined gem materials.
    pub custom_materials: Arc<Vec<GemMaterial>>,
    /// Name -> specific gravity for custom catalogue materials. A side channel to
    /// [`Self::custom_materials`], not on `GemMaterial`, since SG does not affect optical
    /// tracing. Kept in lock-step by `gui::optics::custom_materials` and the startup load.
    /// `Arc<Vec<..>>` for cheap cloning into per-frame snapshots.
    pub custom_material_specific_gravity: Arc<Vec<(String, f64)>>,
    /// Names of the custom materials whose active color mode is Physics (a recipe renders
    /// them). A side channel to [`Self::custom_materials`] like the specific gravity above,
    /// since `GemMaterial` has no color-mode field. Kept in lock-step by
    /// `gui::optics::custom_materials`. Read through [`Self::physics_color`].
    pub custom_material_physics: Arc<Vec<String>>,
    /// The fluorescent emitters of the custom materials whose color mode is Physics,
    /// resolved from their recipes (`ColorMode::fluorescence`). A side channel beside
    /// [`Self::custom_material_physics`], never inside `GemMaterial`, so every other
    /// material renders unchanged. Only materials with at least one emitter are listed.
    /// Read through [`Self::active_fluorescence`].
    pub custom_material_fluorescence: Arc<Vec<(String, Arc<Fluorescence>)>>,
    /// A native file whose top-level color was edited while its physics recipe was still
    /// stored: the material the recipe would restore, held until the user picks "keep
    /// physics recipe" or "use edited color" (`PhysicscolorModel.color_conflict_*`).
    /// The recipe's fluorescence rides along (see [`Self::custom_material_fluorescence`]).
    pub pending_color_choice: Option<(String, GemMaterial, Fluorescence)>,
    /// Shutdown signal for the render thread. Setting this `false` ends the loop
    /// *permanently* -- never reuse this as a pause mechanism; see `paused`.
    pub running: bool,
    /// Whether the context changed since the last render.
    pub dirty: bool,
    /// User-initiated pause/stop control, independent of `tab_visible`: both suspend
    /// rendering when off, but switching tabs must never clear an explicit pause, and
    /// pausing must never look like a tab-visibility change.
    pub paused: bool,
    /// Automatic suspend when the rendered image isn't visible anywhere: combines the
    /// UI's `active_tab`, the Live Render/Edit sub-tab, each sub-tab's view mode and
    /// the open Retarget dialog -- see `gui::render::render_visibility`, the single
    /// place that computes this flag. Not user-facing on its own.
    pub tab_visible: bool,
    /// Whether the À-Trous denoiser is applied to the tone-mapped output -- see
    /// `AppSettings::denoise_enabled`. `true` by default. Independent of
    /// `remote_active`: this is about WHETHER to denoise, not which backend produced
    /// the samples.
    pub denoise_enabled: bool,
    /// One-shot "redraw the current image now" request, set by the global denoise
    /// toggle (`gui::remote::worker_callbacks::setup_denoise_toggle_callback`) so
    /// flipping it takes effect immediately even when accumulation has already
    /// converged. Consumed (read-and-cleared) alongside `dirty` in
    /// `snapshot_frame_inputs`, and treated like `remote_advanced` by the render
    /// loop: forces exactly one display cycle (bypassing both the "already converged"
    /// early sleep and `DENOISE_MIN_INTERVAL`'s cadence gate) without resetting
    /// accumulation or tracing a single new sample -- toggling denoise must re-tonemap
    /// the SAME accumulated buffer, not restart convergence. Does nothing on its own
    /// while `RenderContext::effective_live_target` is `RemoteOnly`, since that mode
    /// suspends this loop entirely -- see the toggle callback's own doc comment for
    /// how that case redraws instead (`orchestrator::tick::update::redraw_from_epoch`,
    /// called directly from the callback).
    pub redisplay_requested: bool,
    /// Set by the remote-rendering orchestrator while a settled epoch's remote work
    /// owns (part of) the displayed image. Distinct from `paused`: driven by the
    /// handoff state machine, not the user, and never observable as a user pause.
    ///
    /// What `true` does depends on `live_compute_target`: for
    /// [`LiveComputeTarget::RemoteOnly`] it suspends local tracing (see
    /// `SuspensionFlags`); for [`LiveComputeTarget::Both`] local keeps tracing,
    /// claiming its per-frame ranges from [`live_epoch`](Self::live_epoch), and the
    /// display folds the epoch's remote contribution into the shown image (see
    /// `should_combine_remote`).
    ///
    /// Stays `true` past a completed remote contribution: the settled image keeps
    /// combining. Cleared only through [`Self::release_remote`] (every discard path:
    /// drag, failure in `RemoteOnly`, a compute-target change) or the render loop's own
    /// `resolve_remote_ownership` release on a non-drag scene change.
    pub remote_active: bool,
    /// The current settle's shared sample budget and merged remote contribution --
    /// see `bridge::sample_cursor::live`. Set together with `remote_active`/`dirty`
    /// in one locked mutation by the orchestrator's `start_remote_render`; cleared
    /// together with `remote_active` by [`Self::release_remote`]. Never carried from
    /// one epoch into the next: a new settle always installs a fresh one.
    ///
    /// `Arc`: the same instance the orchestrator's remote lane merges finished chunks
    /// into; the render thread only claims ranges from it and reads its sums.
    pub live_epoch: Option<Arc<LiveEpoch>>,
    /// A COUNT, not a `bool`, of the high-resolution render jobs currently in flight --
    /// the batch preview, the batch tilt-curve sweep, and a fanned-out hi-res export
    /// queue each increment this on start and decrement it on every exit path (success,
    /// error, cancel, or an unwinding `Drop` guard). A single shared `bool` would
    /// let one job's own cleanup re-enable local tracing while a SECOND job (e.g. a
    /// tilt batch finishing mid-export) is still running. Suspends local tracing like
    /// `paused`/`tab_visible`/`remote_active` (and contending for `GpuBackend`'s shared
    /// `Mutex`) whenever this is above zero -- see [`Self::export_active`] for the
    /// `bool` view every read site still wants. Must never be observable as a
    /// user-visible pause -- flipping `paused` instead would corrupt its restore.
    pub export_active_count: u32,
    /// Live rendering's Local/Remote/Local+Remote choice -- see
    /// `LiveComputeTarget`. Read fresh by `orchestrator::poll_tick` at every settle
    /// (so a change applies on the NEXT settle) and every frame by the render loop.
    pub live_compute_target: LiveComputeTarget,
    /// Final-picture live transfer: the current epoch's remote request asks for finished,
    /// denoised display frames (`TransferMode::DisplayOnly`) instead of float deltas.
    /// 8-bit frames cannot be merged with local samples, so while this holds a `Both`
    /// epoch behaves exactly like `RemoteOnly` (see [`Self::effective_live_target`]).
    /// Set by the orchestrator's `start_remote_render` in the same locked mutation as
    /// `remote_active`/`live_epoch`, cleared with them by [`Self::clear_remote_state`].
    pub live_display_only: bool,
    /// Local live-rendering CPU/CPU+GPU/GPU choice -- see `LocalComputeTarget`. Read
    /// fresh every frame by `accumulate_frame_samples`, so a settings change applies
    /// immediately with no restart. `CpuGpu` (default) is the hybrid
    /// behaviour -- see that function's doc comment for how.
    pub local_compute_target: LocalComputeTarget,
    /// Local preview-then-settle rendering: resolution reduction while the camera is
    /// moving. `Off` (default) makes `local_preview::effective_dimensions` return
    /// `width`/`height` unchanged regardless of `camera_moving`.
    pub local_preview_scale: LocalPreviewScale,
    /// "Adaptive Sampling in the Viewport": while the view is still, clean tiles stop
    /// receiving samples (see `render_thread::adaptive_view`). Off by default. A change
    /// must set `dirty` so the accumulation restarts.
    pub adaptive_viewport: bool,
    /// "Speed Up Still Views With a Path Cache": once a still view has refined for
    /// `render_thread::viewport_cache::VIEWPORT_CACHE_AFTER_SPP` samples, its stone is shaded
    /// from an in-memory path cache (see `render_thread::viewport_cache`). Off by default. A
    /// change must set `dirty` so the accumulation restarts.
    pub path_cache_viewport: bool,
    /// "Instant Lighting Changes in the Ray-Traced View": a still Ray-Traced view keeps the
    /// stone's light paths in memory (see `render_thread::reshade_view`), and a lighting or
    /// exposure change re-lights them at once instead of tracing again. Off by default. A
    /// change must set `dirty`.
    pub instant_lighting: bool,
    /// "Beam Tracing in Still Views" (see `render_thread::still_beams`). Off by default. A
    /// change must set `dirty`.
    #[cfg(feature = "beam")]
    pub beam_viewport: bool,
    /// "Instant Lighting Changes" for beam tracing: a lighting-only change of a beam-traced
    /// still view re-lights the kept beams instead of tracing again. Off by default.
    #[cfg(feature = "beam")]
    pub beam_instant_lighting: bool,
    /// Whether the camera is CURRENTLY moving -- mirrors
    /// `HandoffState::Previewing`, written once per poll tick from the same
    /// `HandoffMachine` that drives the remote handoff, so there's one definition of
    /// "settled" app-wide. Meant to be mutually exclusive with `remote_active`;
    /// `resolve_remote_ownership` releases `remote_active` on the same camera-drag
    /// `ctx.dirty = true` write that will make this go `true` on the next poll tick.
    /// The render loop still ANDs this with `!remote_active` before applying
    /// `local_preview_scale` as a belt-and-suspenders guard.
    pub camera_moving: bool,
    /// Whether a mouse button is currently held for an orbit, pan or light drag in either
    /// viewport -- written by `ViewportModel.camera_drag_begin`/`camera_drag_end`
    /// (`gui::render::camera_lighting::setup_camera_drag_callbacks`), cleared by
    /// `gui::render::render_visibility::recompute_tab_visible` on every tab/view switch.
    /// Read only by the remote orchestrator's poll tick, which refuses to settle (leave
    /// `HandoffState::Previewing`, so `camera_moving` and the reduced preview
    /// resolution persist, and no remote handoff is dispatched) while this holds --
    /// see `poll::POSE_SETTLE_DEBOUNCE`. Also read by the render loop, which counts the
    /// held drag as camera motion only once the scene changed under it (a plain press
    /// keeps the converged image). Not a scene field: `scene_identity` ignores it.
    pub camera_drag_held: bool,
    /// HDR environment maps: `Some(map)` replaces the analytic studio rig with a loaded
    /// Radiance `.hdr` panorama as the render loop's `EnvironmentSource` (see
    /// `pleochroma::renderer::env_map`). `None` (default) is the studio rig.
    ///
    /// `Arc`, not a bare `EnvironmentMap`: `snapshot_frame_inputs` clones this field every
    /// frame, and a decoded panorama can be tens of megabytes.
    /// Written by the environment-map load and clear callbacks and by the lighting presets.
    pub env_map: Option<Arc<EnvironmentMap>>,
    /// Yaw of the HDR map in radians, `0..TAU` (`0.0` is the map as loaded). Read only
    /// while [`Self::env_map`] is `Some`; the light controls turn it then. Written by
    /// `gui::render::camera_lighting`, the map loader (which resets it) and the settings.
    pub hdr_yaw: f32,
    /// Whether the HDR map turns with the camera: the map's yaw then gets the camera yaw
    /// added, so the map stays fixed relative to the view and the stone orbits under it.
    pub hdr_follows_camera: bool,
    /// The content-derived scene generation counter behind
    /// [`Self::scene_generation`] -- see [`scene_identity`]. Never written directly;
    /// `Default::default()` is the only sensible initial value.
    pub scene_identity: SceneIdentity,
    /// Cancellation shared by render and display work for the current scene/activity.
    /// Snapshots reuse it until invalidated; a cancelled token is never reset.
    pub frame_cancel: Arc<AtomicBool>,
}

/// The viewport material state a Library selection replaces and the editor takes back:
/// see [`RenderContext::editor_material_stash`].
#[derive(Debug, Clone)]
pub struct MaterialStash {
    /// `RenderContext::material_name` at the time.
    pub name: String,
    /// `RenderContext::material_override` at the time.
    pub material_override: Option<GemMaterial>,
    /// `RenderContext::material_unresolved` at the time.
    pub unresolved: Option<String>,
}

/// Tags which subsystem last claimed `RenderContext::active_planes`/`design_gear`.
/// See [`RenderContext::claim_active_planes`] for the
/// intended write path and [`RenderContext::planes_owner`] for why this exists.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PlanesOwner {
    /// Nobody has claimed the slot since startup -- still the built-in
    /// placeholder cut [`RenderContext::default`] seeds `active_planes` with.
    #[default]
    Builtin,
    /// The editor's currently loaded/edited design, tagged with `EditorState`'s
    /// own edit generation counter so a superseded write (e.g. a background
    /// auto-solve that finished after a newer edit started) can be told apart
    /// from the current one.
    Editor {
        /// `EditorState`'s generation counter at claim time.
        generation: u64,
    },
    /// A catalogue entry, tagged by its row id so a delete-reset can
    /// check whether the row it just deleted is the one that owns the slot before
    /// resetting it.
    Catalogue {
        /// The catalogue row's database id.
        entry_id: i64,
    },
}

/// The pose the render loop and the scene identity use: `pose` clamped while the Live Render
/// tab is shown, the identity on the Edit tab (`live_tab_shown` false), which always shows the
/// stone as designed.
#[must_use]
pub fn live_render_pose(live_tab_shown: bool, pose: StonePose) -> StonePose {
    if live_tab_shown {
        pose.clamped()
    } else {
        StonePose::IDENTITY
    }
}

impl RenderContext {
    /// [`Self::stone_pose`] as the Live Render tab shows it ([`live_render_pose`]): the traced
    /// picture, the shadow, the mirror, the Solid view's camera and the scene identity read this,
    /// not the raw field.
    #[must_use]
    pub fn effective_stone_pose(&self) -> StonePose {
        live_render_pose(self.live_tab_shown, self.stone_pose)
    }

    /// Whether a write tagged `new_owner` may overwrite the slot's current owner. Every
    /// writer uses this one rule.
    ///
    /// Two `Editor` claims arbitrate by generation, so a background auto-solve that was
    /// already stale when it finished cannot overwrite a newer edit's planes. Every other
    /// claim is allowed, including a catalogue selection over an `Editor` owner: picking a
    /// Library row is an explicit request, and the editor takes the slot back with its next
    /// load or edit.
    #[must_use]
    pub const fn may_claim_active_planes(&self, new_owner: PlanesOwner) -> bool {
        match (self.planes_owner, new_owner) {
            (
                PlanesOwner::Editor {
                    generation: current,
                },
                PlanesOwner::Editor { generation: new },
            ) => new >= current,
            _ => true,
        }
    }

    /// The write path for `active_planes`, `design_gear` and `planes_owner` together.
    /// Returns `false` and leaves every field untouched when
    /// [`Self::may_claim_active_planes`] refuses the claim, so a caller can tell a stale
    /// write from an accepted one.
    ///
    /// Does NOT set `dirty`: callers set it with their own request, so a refused claim
    /// needs no extra handling.
    pub fn claim_active_planes(
        &mut self,
        planes: Arc<Vec<GpuFacetPlane>>,
        design_gear: Option<(u32, f32)>,
        owner: PlanesOwner,
    ) -> bool {
        // A writer that hands over planes alone knows of no tools, so the stone it
        // shows is the flat one: carrying a previous design's tools over would cut a
        // different stone's notches into it.
        self.claim_active_geometry(planes, Arc::new(Vec::new()), Vec::new(), design_gear, owner)
    }

    /// [`Self::claim_active_planes`] for a stone that may have concave tools: the
    /// planes and tools are replaced together under the same arbitration, so a render
    /// never sees one design's planes with another's tools. `tools` is empty for a
    /// planar design.
    pub fn claim_active_geometry(
        &mut self,
        planes: Arc<Vec<GpuFacetPlane>>,
        tools: Arc<Vec<ToolPrimitive>>,
        placements: Vec<(usize, usize)>,
        design_gear: Option<(u32, f32)>,
        owner: PlanesOwner,
    ) -> bool {
        if !self.may_claim_active_planes(owner) {
            return false;
        }
        if self.planes_owner != owner {
            self.view_color_pattern = None;
        }
        // `stone_width_mm` survives an owner change. It may belong to the new owner or be
        // left over from the previous one, and a reset would also clear a width the cutter
        // means to keep, so this only logs.
        if self.stone_width_mm > 0.0 && self.planes_owner != owner {
            tracing::warn!(
                stone_width_mm = self.stone_width_mm,
                previous_owner = ?self.planes_owner,
                new_owner = ?owner,
                "active_planes' owner is changing while stone_width_mm is still set; \
                 absorption/scattering scale may now be computed against the wrong \
                 stone's measured width"
            );
        }
        self.swap_editor_material(owner);
        self.active_planes = planes;
        // The frosting named planes of the previous stone: a new claim starts polished.
        // Kept as the same `Arc` when it is already empty, so re-claiming a design without
        // frosting is no scene change.
        if !self.active_frosted.is_empty() {
            self.active_frosted = Arc::new(Vec::new());
        }
        // The surface finish names planes of the previous stone too.
        self.claim_active_surfaces(Vec::new());
        // Kept as the same `Arc` when both the old and new set are empty, so a planar
        // design re-claimed on every edit does not look like a scene change to
        // `scene_identity` (which compares `Arc` identity).
        if !(self.active_tools.is_empty() && tools.is_empty()) {
            self.active_tools = tools;
        }
        self.active_placements = placements;
        self.design_gear = design_gear;
        self.planes_owner = owner;
        self.catalogue_over_generation =
            matches!(owner, PlanesOwner::Catalogue { .. }).then(|| {
                self.editor_generation
                    .as_ref()
                    .map_or(0, |generation| generation.load(Ordering::Relaxed))
            });
        true
    }

    /// Whether a Library selection took the slot after the editor's design reached
    /// `started_generation`. A background solve of that design that finishes now must leave
    /// the Library row on the viewport. The editor's next load or edit has a newer
    /// generation and claims the slot back.
    #[must_use]
    pub fn library_took_over_since(&self, started_generation: u64) -> bool {
        matches!(self.planes_owner, PlanesOwner::Catalogue { .. })
            && self
                .catalogue_over_generation
                .is_some_and(|floor| started_generation <= floor)
    }

    /// Sets the frosted planes of the stone [`Self::claim_active_geometry`] just claimed
    /// (indices into [`Self::active_planes`], ascending). Call it right after an accepted
    /// claim, under the same lock. The `Arc` is kept when the list is unchanged, so an edit
    /// that moves no frosted plane is no scene change. Does NOT set `dirty`, like the claim.
    pub fn claim_active_frosted(&mut self, frosted: Vec<u32>) {
        if *self.active_frosted != frosted {
            self.active_frosted = Arc::new(frosted);
        }
    }

    /// Keeps the editor's viewport material across a Library selection: stashed when a
    /// catalogue claim takes the slot from the editor (before the row writes its own),
    /// restored when the editor claims it back.
    fn swap_editor_material(&mut self, new_owner: PlanesOwner) {
        match (self.planes_owner, new_owner) {
            (PlanesOwner::Editor { .. }, PlanesOwner::Catalogue { .. }) => {
                self.editor_material_stash = Some(MaterialStash {
                    name: self.material_name.clone(),
                    material_override: self.material_override.clone(),
                    unresolved: self.material_unresolved.clone(),
                });
            }
            (PlanesOwner::Catalogue { .. }, PlanesOwner::Editor { .. }) => {
                self.restore_editor_material();
            }
            _ => {}
        }
    }

    /// Puts back the viewport material the editor had before a Library selection took
    /// the slot (see [`Self::editor_material_stash`]); does nothing when none is stashed.
    pub fn restore_editor_material(&mut self) {
        if let Some(stash) = self.editor_material_stash.take() {
            self.material_name = stash.name;
            self.material_override = stash.material_override;
            self.material_unresolved = stash.unresolved;
            self.dirty = true;
        }
    }

    /// Whether the planes in `active_planes` come from an older edit than
    /// `current_generation`. [`PlanesOwner::Editor`] carries the generation the planes were
    /// claimed at, so this is a direct comparison. It is `false` when the editor does not own
    /// the slot, since staleness only applies to editor edits.
    ///
    /// This only computes the flag. A caller passes the live `EditorState::generation` and
    /// shows the result in the viewport.
    #[must_use]
    pub const fn traced_planes_are_stale(&self, current_generation: u64) -> bool {
        matches!(
            self.planes_owner,
            PlanesOwner::Editor { generation } if generation != current_generation
        )
    }

    /// Looks up `name`'s specific gravity in [`Self::custom_material_specific_gravity`],
    /// ignoring ASCII case like the other name lookups (see [`resolve_material`]). `None`
    /// when no custom material by that name has a recorded SG.
    #[must_use]
    pub fn custom_specific_gravity(&self, name: &str) -> Option<f64> {
        self.custom_material_specific_gravity
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(name))
            .map(|(_, sg)| *sg)
    }

    /// Whether the material being traced (the editor's override when set, else the selected
    /// name) is a custom material whose active color mode is Physics. It is read from the
    /// explicit [`Self::custom_material_physics`] list, not from the material's bands (a
    /// pure-host recipe has none). The render thread, export and the remote scene all use
    /// this answer. It also selects the default stone width (`effective_stone_width_mm`).
    #[must_use]
    pub fn physics_color(&self) -> bool {
        let active = self
            .material_override
            .as_ref()
            .map_or(self.material_name.as_str(), |m| m.name.as_str());
        self.custom_material_physics
            .iter()
            .any(|n| n.eq_ignore_ascii_case(active))
    }

    /// The `n_d` of the material being traced (the override when set, else the by-name
    /// lookup), or `0.0` when it does not resolve. The denoiser's path signature refracts at
    /// it, so every guide-buffer key carries it. Read-only: the view tint and the material
    /// overrides never change the dispersion model.
    #[must_use]
    pub fn active_n_d(&self) -> f32 {
        if let Some(material) = &self.material_override {
            return material.dispersion.n_d();
        }
        resolve_material_with_override(
            &GemMaterial::all_materials(),
            &self.custom_materials,
            None,
            &self.material_name,
        )
        .map_or(0.0, |material| material.dispersion.n_d())
    }

    /// The exposure the renderers get: [`Self::exposure`] (the cutter's compensation) on top of
    /// the incident-light meter of the lighting rig at the light pose
    /// (`pleochroma::optics::raytracer::metered_exposure`), so every rig shows white paper at the
    /// same level. A loaded HDR map is its own light, unmetered: the plain exposure scales its
    /// radiance (`EnvironmentSource::HdrMap::exposure`). The live frame, exports, render jobs,
    /// the remote scene and the file name placeholder `{exposure}` all read this one value.
    #[must_use]
    pub fn effective_exposure(&self) -> f32 {
        if self.env_map.is_some() {
            self.exposure
        } else {
            pleochroma::optics::raytracer::metered_exposure(
                self.exposure,
                self.lighting_preset,
                self.light_yaw,
                self.light_pitch,
            )
        }
    }

    /// How the loaded HDR map is turned: [`Self::hdr_yaw`] and [`Self::hdr_follows_camera`]
    /// as the one value the renderers take (`HdrOrientation::rotation_for` at the camera
    /// yaw gives the rotation). Meaningless without a map.
    #[must_use]
    pub const fn hdr_orientation(&self) -> HdrOrientation {
        HdrOrientation {
            yaw: self.hdr_yaw,
            follows_camera: self.hdr_follows_camera,
        }
    }

    /// The fluorescence of the material being traced: the emitters of its physics recipe, or
    /// `None` for every other material (built-in, fantasy, or a recipe without emitters). The
    /// render thread, export, the remote scene and the tilt sweeps all read this one answer
    /// beside `physics_color()`; `None` is `Fluorescence::none()` and changes nothing.
    #[must_use]
    pub fn active_fluorescence(&self) -> Option<Arc<Fluorescence>> {
        if !self.physics_color() {
            return None;
        }
        let active = self
            .material_override
            .as_ref()
            .map_or(self.material_name.as_str(), |m| m.name.as_str());
        self.custom_material_fluorescence
            .iter()
            .find(|(n, _)| n.eq_ignore_ascii_case(active))
            .map(|(_, f)| Arc::clone(f))
    }

    /// Records the fluorescence of the custom material `name` (an empty one removes the
    /// entry): the companion of [`Self::set_custom_material_physics`].
    pub fn set_custom_material_fluorescence(&mut self, name: &str, fluorescence: Fluorescence) {
        let list = Arc::make_mut(&mut self.custom_material_fluorescence);
        list.retain(|(n, _)| !n.eq_ignore_ascii_case(name));
        if !fluorescence.is_empty() {
            list.push((name.to_string(), Arc::new(fluorescence)));
        }
    }

    /// Records whether the custom material `name` renders from a physics recipe.
    pub fn set_custom_material_physics(&mut self, name: &str, physics: bool) {
        let list = Arc::make_mut(&mut self.custom_material_physics);
        list.retain(|n| !n.eq_ignore_ascii_case(name));
        if physics {
            list.push(name.to_string());
        }
    }

    /// Whether ANY high-resolution render job currently suspends local tracing -- the
    /// `bool` view of [`Self::export_active_count`] every existing reader wants.
    #[must_use]
    pub const fn export_active(&self) -> bool {
        self.export_active_count > 0
    }

    /// Releases every piece of live remote state together -- `remote_active` and the
    /// epoch (its cursor, remote sums and in-flight chunk accumulator all live inside
    /// [`LiveEpoch`]) -- and marks the frame `dirty` so local tracing restarts from a
    /// clean buffer. The one function every discard path calls (drag, a failure in
    /// `RemoteOnly`, a live-compute-target change): local samples claimed from the
    /// released epoch's cursor are not a prefix, so continuing to accumulate on top of
    /// them with the plain `accum_samples` offset would re-trace indices.
    pub fn release_remote(&mut self) {
        self.clear_remote_state();
        self.dirty = true;
    }

    /// [`Self::release_remote`] without touching `dirty` -- for the render loop's own
    /// release, which happens on a frame that is ALREADY resetting its accumulation
    /// because of the `dirty` it just consumed (setting it again would throw away one
    /// more frame for nothing, and clearing it could swallow a callback's fresh one).
    pub fn clear_remote_state(&mut self) {
        self.remote_active = false;
        self.live_epoch = None;
        self.live_display_only = false;
    }

    /// The live compute target the render loop and the orchestrator actually act on:
    /// [`Self::live_compute_target`], except that a final-picture epoch
    /// ([`Self::live_display_only`]) turns `Both` into `RemoteOnly` -- local tracing
    /// pauses after the handoff and the remote's display frames are the image.
    #[must_use]
    pub const fn effective_live_target(&self) -> LiveComputeTarget {
        effective_live_target(self.live_compute_target, self.live_display_only)
    }
}

/// [`RenderContext::effective_live_target`]'s pure rule: a final-picture epoch
/// (`display_only`) cannot combine 8-bit remote frames with local samples, so `Both`
/// acts as `RemoteOnly`; every other combination is unchanged.
#[must_use]
pub const fn effective_live_target(
    target: LiveComputeTarget,
    display_only: bool,
) -> LiveComputeTarget {
    match (target, display_only) {
        (LiveComputeTarget::Both, true) => LiveComputeTarget::RemoteOnly,
        (other, _) => other,
    }
}
