//! Renders the two cached catalogue-preview images (front and top) of one design, and picks
//! which built-in [`GemMaterial`] preset to render it in.
//!
//! Storage lives in `pleochroma_vault::model::preview`, which stays `pleochroma`-free, so the
//! rendering and the `GemMaterial::all_materials()` -> `RiPresetCandidate` adaptation live
//! here. `gui::batch::preview` is the one caller: it renders each [`PreviewView`] with
//! [`render_view`] or [`render_view_remote_checked`], then saves both views with
//! `Database::save_preview_images`.
//!
//! # Camera convention: front = pitch 0, top = pitch 90 degrees
//!
//! `Camera::new`'s origin is `(d*cos(p)*sin(y), d*sin(p), d*cos(p)*cos(y))`, so at
//! `pitch = 90 deg` the camera sits directly overhead (`origin = (0, d, 0)`) for any `yaw`,
//! and at `pitch = 0` it lies on the horizontal plane. [`PreviewView::pitch`] uses the same
//! table-up pole as `pleochroma::color::metrics::evaluate_full_axis_profile_at_azimuth`, so
//! "top" means table-up, face-up everywhere.
//!
//! # Sizing: the chosen default
//!
//! `settings::model::{DEFAULT_PREVIEW_SIZE, DEFAULT_PREVIEW_SPP}` are 160x160 at 256 spp:
//! about 1.1 s per image on 16 cores, small enough for a diagram-list card and clean at that
//! size. Both are settings (`AppSettings::preview_size`/`preview_spp`), since they trade
//! quality for time.
//!
//! # Render path: GPU with a CPU scanline fallback, one full-resolution dispatch
//!
//! Each view is one call: [`GpuBackend::try_accumulate`] runs the whole `spp` budget in a
//! single dispatch, and `export_thread::batch::render_batch` (the CPU scanline tracer the
//! export path uses) takes over if the GPU declines. A preview is never split into batches;
//! cancellation and per-view panic isolation happen between items in `gui::batch::preview`.

use crate::{
    bridge::{
        export_thread::{self, SceneSnapshot},
        preview_wait::{
            PREVIEW_CANCEL_ACK_WAIT, PREVIEW_PROGRESS_STALL, PREVIEW_REMOTE_MAX_WALL,
            ProgressWatch, RemoteShortfall,
        },
        remote::{
            remote_can_render,
            remote_render::{self, RemoteRenderHandle, RemoteRenderRequest, RemoteUpdate},
        },
    },
    settings::WorkerSettings,
};
use glam::Vec3;
use pleochroma::{
    geometry::{plane::GpuFacetPlane, tool::ToolPrimitive},
    optics::{
        materials::GemMaterial,
        raytracer::{Camera, DEFAULT_FOV_DEG, DEFAULT_POSE, LightingPreset},
    },
    render_setup::Backdrop,
    renderer::{
        env_map::EnvironmentMap,
        gpu_backend::{GpuBackend, GpuSceneRef},
        tonemap::tonemap_to_rgba,
    },
};
use pleochroma_net::{SceneState, client::Accumulator, scene::effects::SceneEffects};
use pleochroma_vault::model::material_match::RiPresetCandidate;
use std::{
    io::Cursor,
    sync::{
        Arc, LazyLock, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, RecvTimeoutError},
    },
    time::{Duration, Instant},
};

mod cpu_permits;
mod fingerprint;
mod ray;
mod solid;
pub use ray::render_rgba_at_pose_ray;

pub use fingerprint::{
    AxisKey, CacheKind, EffectsKey, FingerprintMaterial, cache_fingerprint, frosting_hash,
    zones_hash,
};
pub use solid::render_view_solid_with_tools;

/// Refractive index is conventionally quoted at the sodium D line -- see
/// `pleochroma_vault::model::material_match`'s module doc comment for why every
/// [`RiPresetCandidate`] built here must evaluate `GemMaterial::dispersion` at exactly
/// this wavelength to match the convention a design's own stored (scraped)
/// `refractive_index` already uses.
///
/// `pub` for `gui::material_quality`'s `on_material_changed` handler, which uses this exact
/// constant rather than a bare `589.3`.
pub const SODIUM_D_NM: f32 = 589.3;

/// Evaluates `material`'s refractive index at the sodium D line ([`SODIUM_D_NM`]) --
/// the per-material conversion [`ri_candidates`] applies to every built-in preset,
/// factored out for a caller that only needs one material's RI. Used by
/// `gui::material_quality`'s `on_material_changed` handler to default the
/// RI-tolerance filter's centre to the material currently loaded in the viewport.
#[must_use]
pub fn material_ri_at_sodium_d(material: &GemMaterial) -> f64 {
    f64::from(material.dispersion.evaluate(SODIUM_D_NM))
}

/// Camera pose shared by both preview views -- see this module's doc comment for the
/// pitch convention. `yaw`/`distance`/`light_yaw`/`light_pitch`/`exposure` all match
/// `settings::model::app_settings::DEFAULT_CAMERA_*`/`DEFAULT_LIGHT_*` (the live
/// viewport's own fresh-install defaults), so a preview thumbnail frames and exposes like
/// a plain, undialled-in render of the design. The one deliberate difference is the
/// surround: see [`PREVIEW_LIGHTING_PRESET`] (white tray, 8 degree head shadow).
const PREVIEW_YAW: f32 = DEFAULT_POSE.yaw;
const PREVIEW_DISTANCE: f32 = DEFAULT_POSE.distance;
/// Light yaw of every preview and tilt sweep, `pub` for `gui::batch::tilt`.
///
/// It is fixed, not the live viewport's light: a filter over stored curves, such as "windowing
/// stays under 20% within +/-45 degrees", only means the same thing if every design was swept under
/// the same light.
pub const PREVIEW_LIGHT_YAW: f32 = 0.85;
/// Fixed light pitch used for previews.
pub const PREVIEW_LIGHT_PITCH: f32 = 0.95;
/// The lighting preset every stored tilt-curve set is scored under.
///
/// Used locally and remotely (in the `SceneState` a remote worker receives), so both lanes
/// describe the same lighting. The ISO hemisphere, a uniform lit dome, the grading standard
/// the optimizer's `CANONICAL_LIGHTING_PRESET` also uses, at the preview light pose. Its
/// label is part of the tilt-curve cache fingerprint, so curves scored under another preset
/// are stale.
pub const BATCH_TILT_LIGHTING_PRESET: LightingPreset = LightingPreset::IsoHemisphere;
/// Exposure of every catalogue preview. Part of the preview fingerprint
/// (`fingerprint::cache_fingerprint`), so changing it renders the previews again.
const PREVIEW_EXPOSURE: f32 = 1.0;
/// The lighting rig every catalogue preview renders under: a bright WHITE surround
/// ([`LightingPreset::WhiteTray`]: perfectly uniform walls and a bright floor, D65, no
/// black cards and no spot), so a stone's body color reads at its swatch value and the
/// windows of the top view are white instead of black. Chosen over `ShopLights`, whose
/// floor is dark (0.05) and whose pinpoint spark would put hot spots into a catalogue
/// comparison.
///
/// This is a catalogue decision only. The live viewport's default rig is `LightTent`
/// (`DEFAULT_LIGHTING_RIG`, "Light tent + black cards"): a photo tent is the right model
/// for looking at one stone, a white surround the right one for comparing many stones'
/// colors side by side. The Compare window's Traced mode renders under this same rig, so
/// a compared stone still reads like a thumbnail of itself.
const PREVIEW_LIGHTING_PRESET: LightingPreset = LightingPreset::WhiteTray;
/// Radius in degrees of the observer's head shadow in a catalogue preview (the live view's
/// default is 16): smaller, so the shadow does not black out the table of a stone seen
/// from above. Part of the preview fingerprint.
const PREVIEW_HEAD_SHADOW_DEG: f32 = 8.0;
/// The tray every catalogue preview renders on: the user's Thumbnail Backdrop
/// (`AppSettings::thumbnail_backdrop`, Grey by default, so a colorless stone stands apart
/// from it), as a [`Backdrop`] index. Held here, process-wide, so every preview path (local,
/// remote, Ray-Traced drafts, the Compare window) and the preview fingerprint read the same
/// choice; the settings wiring sets it at startup and on every change
/// ([`set_thumbnail_backdrop_index`]).
static THUMBNAIL_BACKDROP: std::sync::atomic::AtomicI32 =
    std::sync::atomic::AtomicI32::new(Backdrop::Grey.index());

/// Sets the tray catalogue previews render on (a [`Backdrop`] index). Changing it changes the
/// preview fingerprint, so stored previews become outdated.
pub fn set_thumbnail_backdrop_index(index: i32) {
    THUMBNAIL_BACKDROP.store(
        Backdrop::from_index(index).index(),
        std::sync::atomic::Ordering::Relaxed,
    );
}

/// The tray catalogue previews currently render on.
fn thumbnail_backdrop() -> Backdrop {
    Backdrop::from_index(THUMBNAIL_BACKDROP.load(std::sync::atomic::Ordering::Relaxed))
}

/// The name of the thumbnail tray in the preview fingerprint (with `TRAY_MODEL_VERSION`),
/// instead of a radiance, so a change of the light pose or the preview rig leaves the
/// fingerprint alone. A change of the tray model makes stored previews stale.
fn thumbnail_backdrop_name() -> &'static str {
    match thumbnail_backdrop() {
        Backdrop::AsLit => "as-lit",
        Backdrop::Grey => "grey",
        Backdrop::White => "white",
        Backdrop::Black => "black",
    }
}

/// The tray radiance every preview carries: the thumbnail tray resolved under
/// [`PREVIEW_LIGHTING_PRESET`] at the fixed preview light pose.
///
/// Every choice is resolved ONCE per process. The resolve integrates the rig's irradiance
/// (about 2-3 ms) and the library keeps a one-entry cache keyed by (preset, pose); the live
/// view, the exports and the preview workers resolve different keys on different threads, so
/// a per-preview call would evict the live view's entry on every thumbnail and make it pay
/// the integral again.
fn preview_backdrop() -> f32 {
    static TRAYS: LazyLock<[f32; 4]> = LazyLock::new(|| {
        [0, 1, 2, 3].map(|index| {
            Backdrop::from_index(index).resolve(
                PREVIEW_LIGHTING_PRESET,
                PREVIEW_LIGHT_YAW,
                PREVIEW_LIGHT_PITCH,
            )
        })
    });
    TRAYS[usize::try_from(thumbnail_backdrop().index()).unwrap_or(1)]
}
/// Surface glare of every preview (the live view's neutral value). Part of the preview
/// fingerprint, with the tray name: both decide pixels.
const PREVIEW_SURFACE_GLARE: f32 = 1.0;

/// Which of the two cached preview images a [`render_view`] call produces -- see this
/// module's doc comment for the pitch each maps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewView {
    /// Edge-on / profile view: `pitch = 0`, on the horizontal plane.
    Front,
    /// Table-up / face-up view: `pitch = 90 degrees`, straight overhead.
    Top,
}

impl PreviewView {
    #[must_use]
    const fn pitch(self) -> f32 {
        match self {
            Self::Front => 0.0,
            Self::Top => std::f32::consts::FRAC_PI_2,
        }
    }
}

/// Whether `material` carries no colour zones.
const fn is_unzoned(material: &GemMaterial) -> bool {
    material.zoning.is_none()
}

/// Builds the `RiPresetCandidate` list `pleochroma_vault::model::material_match::
/// pick_ri_preset` matches a design's own refractive index against -- one candidate per
/// `GemMaterial::all_materials()` entry, evaluated at the sodium D line (see
/// [`SODIUM_D_NM`]). The one place in this crate that performs this adaptation, since
/// `pleochroma_vault` is deliberately `pleochroma`-free.
///
/// A built-in with colour zones (the zoned Ametrine) is left out: the pick is a
/// write-once guess for a design that names no material, and a bicolour guess would make every
/// preview and tilt curve of such a design two-coloured.
#[must_use]
pub fn ri_candidates() -> Vec<RiPresetCandidate> {
    GemMaterial::all_materials()
        .into_iter()
        .filter(is_unzoned)
        .map(|material| {
            let refractive_index = material_ri_at_sodium_d(&material);
            RiPresetCandidate {
                name: material.name,
                refractive_index,
            }
        })
        .collect()
}

/// Everything one [`render_view`] call needs about the design being rendered, bundled
/// so that function's signature stays short. `planes`/`material` are already fully
/// resolved by the caller (`gui::batch::preview`); this module only renders what it's
/// given.
pub struct PreviewJob<'a> {
    /// Image-forming engine, independent of local/remote compute.
    pub engine: pleochroma::renderer::engine::EngineSpec,
    /// Facet planes of the stone.
    pub planes: &'a [GpuFacetPlane],
    /// The concave tools cut into `planes` (grooves, dimples), empty for a planar design:
    /// the stone is rendered WITH them, as the Compare window and the exports render it
    /// (`gui::editor::CataloguePlanes::render_tools`). The GPU traces them with the planes
    /// (`GpuBackend::try_accumulate_geom`, up to `GPU_MAX_TOOL_PRIMITIVES`); beyond that the
    /// CPU tracer does.
    pub tools: &'a [ToolPrimitive],
    /// The planes the design frosts: ascending indices into `planes`
    /// (`gui::editor::resolve_catalogue_planes_with_frosting`), traced `Frosted` locally and
    /// on a remote worker, as the live viewport and the exports trace them. Empty for a
    /// polished design.
    pub frosted: &'a [u32],
    /// The design's render physics (inclusions, play-of-color, facet surface finish, abraded
    /// edge) with the user's faceting machine; the default record for a design that sets none.
    /// Ignored by the Ray-Traced engine.
    pub effects: &'a SceneEffects,
    /// Gem material the stone is rendered with.
    pub material: &'a GemMaterial,
    /// Square render dimension in pixels: `AppSettings::preview_size` or a caller override.
    pub size: u32,
    /// Samples per pixel -- `AppSettings::preview_spp`.
    pub spp: u32,
    /// Bounce cap of the render. Fixed rather than a setting: see
    /// `gui::batch::preview::engine::PREVIEW_MAX_BOUNCES`.
    pub max_bounces: u32,
}

/// Renders `job`'s design from `view`'s camera pose, tone-maps it via
/// `pleochroma::renderer::tonemap::tonemap_to_rgba` (the same path every other sRGB
/// render in this app uses), and returns PNG bytes ready for
/// `Database::save_preview_images`.
///
/// The PNG encode step can theoretically fail on an internal buffer-size mismatch;
/// that case returns `None` rather than panicking, and `gui::batch::preview` treats it
/// like a caught panic for this one view. A genuine panic inside the tracer itself is
/// not caught here -- `gui::batch::preview::engine::render_item_local` already wraps each call
/// in `catch_unwind` at the single-item granularity its progress reports at.
#[must_use]
pub fn render_view(job: &PreviewJob<'_>, view: PreviewView, gpu: &GpuBackend) -> Option<Vec<u8>> {
    if job.engine.is_ray() {
        return ray::render_view_ray(job, view);
    }
    let scene = SceneSnapshot {
        camera_roll: 0.0,
        engine: pleochroma::renderer::engine::EngineSpec::default(),
        optical_diagnostics: None,
        yaw: PREVIEW_YAW,
        pitch: view.pitch(),
        distance: solid::preview_distance(job.planes),
        light_yaw: PREVIEW_LIGHT_YAW,
        light_pitch: PREVIEW_LIGHT_PITCH,
        material: job.material.clone(),
        lighting_preset: PREVIEW_LIGHTING_PRESET,
        max_bounces: job.max_bounces,
        exposure: PREVIEW_EXPOSURE,
        backdrop: preview_backdrop(),
        active_planes: job.planes.to_vec(),
        // A concave design's grooves and dimples; empty for a planar one.
        tools: job.tools.to_vec(),
        fluorescence: Arc::default(),
        // No frosted girdle for a catalogue thumbnail; the planes the design frosts are
        // traced frosted. Empty when nothing is frosted, which means every facet is Polished.
        facet_finishes: pleochroma::geometry::facet_finishes_with_frosted(
            job.planes,
            false,
            job.frosted,
        ),
        // Thumbnails use the analytic studio rig, never an HDR map: a batch must not depend
        // on whatever map the live viewport had loaded.
        env_map: None,
        hdr_orientation: pleochroma::renderer::env_map::HdrOrientation::default(),
        surface_glare: PREVIEW_SURFACE_GLARE,
        head_shadow_deg: PREVIEW_HEAD_SHADOW_DEG,
        stone_shadow: false,
        tray_reflection: 0.0,
        stone_closed: true,
        effects: job.effects.clone(),
    };
    let camera = Camera::new(scene.yaw, scene.pitch, scene.distance, DEFAULT_FOV_DEG)
        .with_roll(scene.camera_roll);
    let environment = scene
        .lighting_preset
        .studio(scene.exposure, scene.light_yaw, scene.light_pitch)
        .with_backdrop(scene.backdrop);
    let traced = scene.traced_material();
    let gpu_scene = GpuSceneRef {
        camera: &camera,
        width: job.size,
        height: job.size,
        planes: &scene.active_planes,
        facet_finishes: &scene.facet_finishes,
        material: &traced,
        max_bounces: scene.max_bounces,
        environment,
        estimator: None,
    };

    let pixel_count = (job.size as usize) * (job.size as usize);
    let mut accum = vec![Vec3::ZERO; pixel_count];
    // The tools ride with the planes to the GPU (`try_accumulate_geom`); a stone beyond the
    // kernels' tool limit declines there and is traced on the CPU, which reads them too.
    // The facet surfaces, machine and abraded edge travel with the request; the CPU fallback
    // reads them from the scene.
    let facet_surfaces = scene.facet_surfaces();
    let surface = scene.surface_scene(&facet_surfaces);
    if !gpu.try_accumulate_geom_surface(&gpu_scene, job.tools, &surface, 0, job.spp, &mut accum) {
        // Each fallback render takes its fair share of the core pool and spawns exactly that
        // many threads (see `cpu_permits`).
        let permits = cpu_permits::acquire_share();
        export_thread::batch::render_batch_with_threads(
            (job.size, job.size),
            job.spp,
            0,
            &camera,
            &scene,
            &mut accum,
            permits.threads(),
        );
    }

    // The one shared tone-map + PNG step (`1.0 / spp` scale, untagged sRGB): a remote
    // worker's `BATCH_ITEM_DONE` calls the very same function, so a preview rendered
    // there is byte-identical to this one from the same sum.
    pleochroma::render_setup::encode_preview_png(job.size, job.size, &accum, job.spp)
}

/// Renders `planes` minus the concave `tools` in `material` on the CPU at an arbitrary
/// orbit pose and rectangular size, and returns the tone-mapped RGBA8 buffer (row-major,
/// four bytes per pixel) instead of PNG bytes.
///
/// The visual compare window's "Traced" mode (`gui::editor::compare::render`) is the
/// caller: it needs the SAME lighting rig, backdrop, exposure, field of view and
/// tone-mapping every catalogue preview uses (so a compared stone reads like a
/// thumbnail of itself), but at the window's own shared camera pose rather than one
/// of the two fixed [`PreviewView`] poses, and straight into a `slint::Image`
/// rather than a PNG. `camera_pose` is `(yaw, pitch, distance, roll)` in exactly
/// `Camera::new`'s convention -- the same one the solid rasterizer projects with,
/// so a traced frame lines up with the solid frame rendered at the same pose.
///
/// CPU only ([`export_thread::batch::render_batch`], the tracer [`render_view`]
/// falls back to): the compare window must never compete with the live viewport for
/// the shared GPU adapter. A zero `size` component returns an empty buffer.
/// `frosted` lists the planes the design frosts (indices into `planes`, ascending), traced
/// `Frosted` like the live viewport and the exports do; empty for a design without frosting.
#[must_use]
#[expect(
    clippy::too_many_arguments,
    reason = "the pose render's seven parameters plus the frosted planes; a bundling struct would only rename them"
)]
pub fn render_rgba_at_pose(
    planes: &[GpuFacetPlane],
    tools: &[ToolPrimitive],
    frosted: &[u32],
    material: &GemMaterial,
    camera_pose: (f32, f32, f32, f32),
    size: (u32, u32),
    spp: u32,
    max_bounces: u32,
) -> Vec<u8> {
    let (width, height) = size;
    if width == 0 || height == 0 {
        return Vec::new();
    }
    let (yaw, pitch, distance, roll) = camera_pose;
    let spp = spp.max(1);
    let scene = SceneSnapshot {
        camera_roll: roll,
        engine: pleochroma::renderer::engine::EngineSpec::default(),
        optical_diagnostics: None,
        yaw,
        pitch,
        distance,
        light_yaw: PREVIEW_LIGHT_YAW,
        light_pitch: PREVIEW_LIGHT_PITCH,
        material: material.clone(),
        lighting_preset: PREVIEW_LIGHTING_PRESET,
        max_bounces,
        exposure: PREVIEW_EXPOSURE,
        backdrop: preview_backdrop(),
        active_planes: planes.to_vec(),
        // The compare window's concave tools (grooves, dimples), empty for a planar
        // stone (see `gui::editor::compare`).
        tools: tools.to_vec(),
        fluorescence: Arc::default(),
        // Every facet Polished (the analytic studio rig, as `render_view`'s own snapshot) except
        // the planes the design frosts; empty when it frosts none.
        facet_finishes: pleochroma::geometry::facet_finishes_with_frosted(planes, false, frosted),
        env_map: None,
        hdr_orientation: pleochroma::renderer::env_map::HdrOrientation::default(),
        surface_glare: PREVIEW_SURFACE_GLARE,
        head_shadow_deg: PREVIEW_HEAD_SHADOW_DEG,
        stone_shadow: false,
        tray_reflection: 0.0,
        stone_closed: true,
        effects: SceneEffects::default(),
    };
    let camera = Camera::new(yaw, pitch, distance, DEFAULT_FOV_DEG).with_roll(roll);
    let mut accum = vec![Vec3::ZERO; (width as usize) * (height as usize)];
    export_thread::batch::render_batch(width, height, spp, 0, &camera, &scene, &mut accum);
    tonemap_to_rgba(&accum, 1.0 / spp as f32)
}

/// The request id of every preview remote dispatch. Each dispatch opens its own one-shot
/// connection ([`remote_render::spawn_remote_render`]), so no request is pipelined behind
/// another on the same socket.
const PREVIEW_REQUEST_ID: u32 = 1;

/// The environment every catalogue thumbnail is rendered under: always the analytic
/// studio rig (see `render_view`'s `env_map: None`), never an HDR map.
const PREVIEW_ENV_MAP: Option<&Arc<EnvironmentMap>> = None;

/// The remote counterpart of [`render_view`]: dispatches `job`'s view as one
/// `RenderRequest` covering the whole `job.spp` budget in a single request (a preview
/// is small enough to never need an export's batched-request chunking) against
/// `worker`, blocking until it finishes, fails, or `cancel` is observed.
///
/// Returns an error on any kind of shortfall -- connection failure, worker rejection,
/// cancellation, a missed progress/wall deadline, or fewer samples than asked for -- so
/// `gui::batch::preview`'s remote lane can decide what's next: under
/// `LiveComputeTarget::Both` it requeues the item for a guaranteed local retry via
/// [`render_view`]; under `RemoteOnly` the failure is final and surfaced. A partial remote
/// result is discarded and the whole image retraced locally: a preview is cheap to redo,
/// unlike an export, which keeps the worker's partial samples.
///
/// Every shortfall comes back as a [`RemoteShortfall`] naming the reason. Deadlines: the wait
/// gives up (after sending a `CANCEL` and allowing [`PREVIEW_CANCEL_ACK_WAIT`] for its
/// acknowledgement) when the reported `samples_done` has not advanced for
/// [`PREVIEW_PROGRESS_STALL`], or the request has run past
/// [`PREVIEW_REMOTE_MAX_WALL`].
///
/// Deadlines are needed because the transport's own liveness rule is silence-based and a
/// coordinator heartbeats every couple of seconds whether or not its tracer advances: a
/// wedged remote would otherwise park the batch's remote lane on one item forever, and a
/// Cancel could never complete either. Every shortfall is logged at `warn!` with the
/// view, so a failing remote shows in `pleochroma-cut.log`.
///
/// # Errors
///
/// A [`RemoteShortfall`] naming why no image was produced: a refused, failed or
/// unsupported request, a cancel, a missed deadline, a disconnected update channel, a
/// short sample count, or a PNG encode failure.
pub fn render_view_remote_checked(
    job: &PreviewJob<'_>,
    view: PreviewView,
    worker: &WorkerSettings,
    cancel: &AtomicBool,
) -> Result<Vec<u8>, RemoteShortfall> {
    let result = dispatch_remote_view(job, view, worker, cancel);
    if let Err(shortfall) = &result {
        tracing::warn!(?view, %shortfall, "remote preview render fell short");
    }
    result
}

/// The scene a remote worker traces for `job`'s `view` -- the same lighting, camera and
/// backdrop [`render_view`] uses locally, shared by the single-request path and the
/// batch path ([`remote_batch_item`]).
fn remote_scene(job: &PreviewJob<'_>, view: PreviewView) -> SceneState {
    // The material's own inclusions are not in its serde bytes; they ride in the record, as
    // `scene_snapshot` sends them. The plain Ray-Traced engine ignores the record.
    let mut effects = job.effects.clone();
    if !job.engine.is_ray() {
        effects.material_inclusions =
            pleochroma_net::scene::material_inclusions::MaterialInclusions::of_material(
                job.material,
            );
    }
    SceneState {
        engine: job.engine,
        width: job.size,
        height: job.size,
        yaw: PREVIEW_YAW,
        camera_roll: 0.0,
        stone_shadow: false,
        tray_reflection: 0.0,
        // Thumbnails are lit by the studio rig, so there is no map to turn.
        hdr_orientation: pleochroma::renderer::env_map::HdrOrientation::default(),
        estimator: pleochroma::optics::raytracer::default_estimator(),
        pitch: view.pitch(),
        distance: solid::preview_distance(job.planes),
        light_yaw: PREVIEW_LIGHT_YAW,
        light_pitch: PREVIEW_LIGHT_PITCH,
        exposure: PREVIEW_EXPOSURE,
        max_bounces: job.max_bounces,
        lighting_preset: PREVIEW_LIGHTING_PRESET,
        material: job.material.clone(),
        planes: job.planes.to_vec(),
        girdle_frosted: false,
        backdrop: preview_backdrop(),
        environment: pleochroma_net::scene::SceneEnvironment::Studio,
        surface_glare: PREVIEW_SURFACE_GLARE,
        tools: job.tools.to_vec(),
        fluorescence: pleochroma::optics::fluorescence::Fluorescence::default(),
        head_shadow_deg: PREVIEW_HEAD_SHADOW_DEG,
        frosted_planes: job.frosted.to_vec(),
        effects,
    }
}

/// One picture of a `BatchRenderRequest`: `job`'s `view` over the whole
/// `job.spp` budget, answered by the worker with a finished PNG. `item_id` is the item's
/// index within its batch.
///
/// # Errors
///
/// [`RemoteShortfall::Failed`] when the shared remote-render rule refuses (never, for the
/// studio-lit catalogue thumbnails).
pub fn remote_batch_item(
    item_id: u32,
    job: &PreviewJob<'_>,
    view: PreviewView,
) -> Result<pleochroma_net::messages::BatchItem, RemoteShortfall> {
    if let Err(refusal) = remote_can_render(PREVIEW_ENV_MAP, false) {
        return Err(RemoteShortfall::Failed(format!(
            "remote render refused: {refusal:?}"
        )));
    }
    Ok(pleochroma_net::messages::BatchItem {
        item_id,
        scene: remote_scene(job, view),
        first_sample: 0,
        samples: job.spp,
        width: job.size,
        height: job.size,
    })
}

/// The body of [`render_view_remote_checked`], separated so the single `warn!` covers
/// every early return.
fn dispatch_remote_view(
    job: &PreviewJob<'_>,
    view: PreviewView,
    worker: &WorkerSettings,
    cancel: &AtomicBool,
) -> Result<Vec<u8>, RemoteShortfall> {
    // The shared remote-render rule (`bridge::remote::guard`). Catalogue thumbnails
    // always use the analytic studio rig -- `render_view`'s snapshot hard-codes
    // `env_map: None` -- so this never refuses today; it is asked anyway so a future
    // per-design environment cannot silently start mixing lighting between the local
    // and remote lanes of a batch.
    if let Err(refusal) = remote_can_render(PREVIEW_ENV_MAP, false) {
        return Err(RemoteShortfall::Failed(format!(
            "remote render refused: {refusal:?}"
        )));
    }
    let scene = remote_scene(job, view);
    let accumulator = Arc::new(Mutex::new(Accumulator::new(job.size, job.size)));
    let (tx, rx) = mpsc::channel::<RemoteUpdate>();
    let handle = remote_render::spawn_remote_render(
        RemoteRenderRequest {
            worker: worker.clone(),
            request_id: PREVIEW_REQUEST_ID,
            scene,
            first_sample: 0,
            samples: job.spp,
            width: job.size,
            height: job.size,
            intent: pleochroma_net::messages::RequestIntent::Batch,
            display_only: false,
        },
        Arc::clone(&accumulator),
        move |update| {
            let _ = tx.send(update);
        },
    );

    wait_for_remote(&rx, &handle, cancel)?;

    let acc = accumulator
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let done = acc.samples_done();
    if done < job.spp {
        return Err(RemoteShortfall::ShortSamples {
            done,
            wanted: job.spp,
        });
    }
    let png =
        pleochroma::render_setup::encode_preview_png(job.size, job.size, acc.buffer(), job.spp);
    drop(acc);
    png.ok_or_else(|| RemoteShortfall::Failed("could not encode the preview PNG".to_owned()))
}

/// What one [`RemoteUpdate`] means for the wait in [`wait_for_remote`].
enum Step {
    /// The request ended: `Ok` for a completed `Done`, `Err` for everything else terminal.
    Terminal(Result<(), RemoteShortfall>),
    /// A `samples_done` report to feed the [`ProgressWatch`].
    Progress(u32),
    /// Nothing the wait acts on (connection notices, preview frames, capability news).
    Ignore,
}

/// Sorts one update into a [`Step`]. Only `Progress` and `Frame` carry a `samples_done`
/// worth watching: a `DisplayFrame` repeats a count the preceding `Frame` already
/// reported, and a `Preview` carries none.
fn classify(update: RemoteUpdate) -> Step {
    match update {
        RemoteUpdate::Done {
            cancelled: true, ..
        } => Step::Terminal(Err(RemoteShortfall::Cancelled)),
        RemoteUpdate::Done {
            cancelled: false, ..
        } => Step::Terminal(Ok(())),
        RemoteUpdate::Failed { message, .. } => {
            Step::Terminal(Err(RemoteShortfall::Failed(message)))
        }
        RemoteUpdate::Unsupported { message, .. } => {
            Step::Terminal(Err(RemoteShortfall::Unsupported(message)))
        }
        RemoteUpdate::Progress { samples_done, .. } | RemoteUpdate::Frame { samples_done, .. } => {
            Step::Progress(samples_done)
        }
        RemoteUpdate::Connected { .. }
        | RemoteUpdate::Preview { .. }
        | RemoteUpdate::DisplayFrame { .. }
        | RemoteUpdate::FinalImage { .. }
        | RemoteUpdate::CapabilityChanged { .. } => Step::Ignore,
    }
}

/// Blocks until the remote request reaches a terminal update, forwarding `cancel` as a
/// `CANCEL` and enforcing the progress-stall and wall-clock deadlines. On a deadline the
/// request is cancelled and given [`PREVIEW_CANCEL_ACK_WAIT`] to acknowledge before the
/// shortfall is returned; a batch Cancel gets the same bound, so a wedged remote cannot
/// keep Cancel from completing.
fn wait_for_remote(
    rx: &mpsc::Receiver<RemoteUpdate>,
    handle: &RemoteRenderHandle,
    cancel: &AtomicBool,
) -> Result<(), RemoteShortfall> {
    let mut watch = ProgressWatch::new(Instant::now());
    let mut cancelled_at: Option<Instant> = None;
    loop {
        if cancelled_at.is_none() && cancel.load(Ordering::Relaxed) {
            handle.cancel();
            cancelled_at = Some(Instant::now());
        }
        match rx.recv_timeout(Duration::from_millis(100)) {
            Ok(update) => match classify(update) {
                Step::Terminal(outcome) => return outcome,
                Step::Progress(samples_done) => watch.observe(samples_done, Instant::now()),
                Step::Ignore => {}
            },
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Err(RemoteShortfall::Disconnected),
        }
        let now = Instant::now();
        if cancelled_at.is_some_and(|at| now.duration_since(at) > PREVIEW_CANCEL_ACK_WAIT) {
            return Err(RemoteShortfall::Cancelled);
        }
        if let Some(shortfall) = watch.verdict(now, PREVIEW_PROGRESS_STALL, PREVIEW_REMOTE_MAX_WALL)
        {
            handle.cancel();
            await_cancel_ack(rx);
            return Err(shortfall);
        }
    }
}

/// Waits at most [`PREVIEW_CANCEL_ACK_WAIT`] for any terminal update after a `CANCEL`
/// was sent, discarding it -- the caller already knows why it gave up. Returns early when
/// the update channel closes, which means the one-shot thread has ended too.
fn await_cancel_ack(rx: &mpsc::Receiver<RemoteUpdate>) {
    let deadline = Instant::now() + PREVIEW_CANCEL_ACK_WAIT;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return;
        }
        match rx.recv_timeout(remaining) {
            Ok(update) => {
                if matches!(classify(update), Step::Terminal(_)) {
                    return;
                }
            }
            Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => return,
        }
    }
}

/// PNG-encodes `rgba` (`width * height * 4` bytes) into memory. The preview path's
/// counterpart of the `Srgb` branch of `export_thread::tonemap_png`, without an ICC profile or
/// file I/O: a cached preview is always sRGB and is stored as bytes, never written to a path.
fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Option<Vec<u8>> {
    let image = image::RgbaImage::from_raw(width, height, rgba.to_vec())?;
    let mut bytes = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
        .ok()?;
    Some(bytes)
}

#[cfg(test)]
#[path = "preview_render_tests.rs"]
mod tests;
