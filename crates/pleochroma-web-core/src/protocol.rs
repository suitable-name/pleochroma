//! The messages between the page and its Web Workers.
//!
//! Every message is a [`ToWorker`] or [`FromWorker`] encoded with postcard
//! ([`encode_to_worker`] / [`decode_from_worker`] and their mirrors) and posted as a
//! TRANSFERRED `ArrayBuffer`, so no copy is made on the way.
//!
//! # Handshake
//!
//! 1. A Worker starts, installs its `onmessage` handler and posts
//!    [`FromWorker::Loaded`] (messages posted to a Worker before its handler exists
//!    would be lost, so the host waits for this).
//! 2. The host sends [`ToWorker::Init`] with its [`PROTOCOL_VERSION`] and the Worker's
//!    role and index.
//! 3. The Worker checks the version and answers [`FromWorker::Ready`], or
//!    [`FromWorker::Error`] on a mismatch (a stale cached Worker script next to a new
//!    page, say).
//!
//! # Render Workers
//!
//! [`ToWorker::HdrMap`] and [`ToWorker::SetScene`] (or [`ToWorker::SetGeometryScene`], the
//! same scene with concave tools; build either with [`ToWorker::scene`]) are cached by the
//! Worker. [`ToWorker::TraceChunk`] traces against the cached scene and answers
//! [`FromWorker::ChunkResult`], or [`FromWorker::ChunkDropped`] when the Worker has no
//! scene with that id (so the host never waits for a chunk that will not come). The
//! host drops results whose `scene_id` is not current.
//!
//! A chunk is traced in row groups (finer interleaved partitions). The host may name a
//! `blob:` URL for it with [`ToWorker::WatchCancel`] (`job_id` = the chunk's scene id)
//! just before the [`ToWorker::TraceChunk`], and revoke the URL when the chunk is no
//! longer wanted (the scene was replaced); the Worker looks at it between row groups and
//! answers [`FromWorker::ChunkAborted`] instead of finishing.
//!
//! [`ToWorker::Picture`] turns a finished (or settled) sum of the cached scene into
//! pixels off the page's thread -- the settled live view's denoise, or an export's
//! tone map + PNG encode ([`PictureKind`]) -- and answers [`FromWorker::Picture`] or
//! [`FromWorker::PictureFailed`]. The host keeps these on a Worker of their own, so a
//! picture never takes a render partition away from tracing.
//!
//! # The solve Worker
//!
//! [`ToWorker::Solve`] answers [`FromWorker::SolveResult`]. An Optimize or Retarget
//! search, and a tilt sweep, stream [`FromWorker::Progress`] messages (at most five a
//! second) before their result. The current-view metrics job is short and streams
//! nothing.
//!
//! The analysis Worker also takes [`ToWorker::HdrMap`] and [`ToWorker::ClearHdr`], the
//! same messages the render Workers get, and answers [`FromWorker::HdrLoaded`] or
//! [`FromWorker::HdrError`]. A metrics job whose `hdr_id` names the map the Worker holds
//! is scored under it; any other is scored under its lighting preset, and the result's
//! `scored_under` says which it was. The tilt sweep and the searches are scored under the
//! preset, as on the desktop.
//!
//! A Worker cannot read a message while it computes, so [`ToWorker::Cancel`] stops the jobs
//! still queued behind the running one and raises a mark that the running job polls when
//! its cancel URL cannot be probed (see [`crate::worker::WorkerHandler`]). A running search
//! is cancelled cooperatively through [`ToWorker::WatchCancel`]'s URL, which the Worker
//! polls between tier decisions and which answers with the best partial result; the
//! host terminates the Worker only when that does not answer in time. A browser that
//! refuses the synchronous request the poll makes (a strict content-security policy) is
//! told apart from a revoked URL: the job then simply runs on, and is ended by the
//! host's own deadline.

use pleochroma::geometry::tool::ToolPrimitive;
use serde::{Deserialize, Serialize};

use crate::{
    scene::SceneSpec,
    solve::{SolveRequest, SolveResponse},
};

/// Version of the message encoding, checked on [`ToWorker::Init`].
///
/// Postcard has no field defaults, so any change to a message's bytes is a wire change and
/// must bump this. A Worker on another version answers `Init` with an error, which keeps a
/// stale cached Worker script from decoding a newer message. The pinned-bytes tests
/// `wire_format_is_pinned_to_worker` and `wire_format_is_pinned_from_worker` must be updated
/// with each bump.
pub const PROTOCOL_VERSION: u32 = 33;

/// What [`ToWorker::Picture`] makes of a sum (see `crate::display`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PictureKind {
    /// The settled live view: `denoise_and_tonemap_frame`, RGBA8.
    DenoisedLive,
    /// A finished export: PNG bytes (`display::export_png`).
    Png {
        /// `display::EXPORT_COLOR_SPACES` index.
        color_space: i32,
        /// Filter with the live view's denoiser first.
        denoise: bool,
    },
}

/// What a Worker is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WorkerRole {
    /// Traces render chunks.
    Render,
    /// Runs solve jobs.
    Solve,
    /// Traces GPU passes and compiles GPU pipelines.
    Gpu,
}

/// Page to Worker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ToWorker {
    /// Assigns the role; see the module's "Handshake".
    Init {
        /// The host's [`PROTOCOL_VERSION`].
        protocol_version: u32,
        /// The role.
        role: WorkerRole,
        /// The Worker's index in its pool (a render Worker's partition).
        worker_index: u32,
    },
    /// Replaces the cached scene.
    SetScene {
        /// The scene's id; results carry it back.
        scene_id: u64,
        /// The scene.
        spec: SceneSpec,
    },
    /// Decodes and caches an HDR map (replacing any previous one), on a render Worker or
    /// a solve-role Worker (the analysis Worker scores the metrics under it).
    HdrMap {
        /// The map's id, named by `SceneSpec::hdr_id` and `MetricsParams::hdr_id`.
        id: u64,
        /// The `.hdr` file's bytes.
        bytes: Vec<u8>,
    },
    /// Frees the cached HDR map.
    ClearHdr,
    /// Traces one chunk of the cached scene.
    TraceChunk {
        /// The scene the host means; a mismatch is dropped.
        scene_id: u64,
        /// The partition (first pixel).
        first_pixel: u32,
        /// The partition count.
        stride: u32,
        /// First sample index.
        sample_offset: u32,
        /// Samples per pixel.
        spp: u32,
    },
    /// Runs a solve job.
    Solve {
        /// The job's id; the result carries it back.
        job_id: u64,
        /// The design as a self-contained native file (`solve::design_to_toml`).
        design_toml: String,
        /// What to do.
        request: SolveRequest,
    },
    /// Cancels every solve job with an id up to and including `job_id`: queued jobs are
    /// skipped, and the Worker's message-cancel mark is raised, which a running job
    /// polls when it cannot probe its cancel URL (see [`crate::worker::WorkerHandler`]).
    /// The host sends it with every graceful cancel of a solve-role job.
    Cancel {
        /// The newest job id to cancel.
        job_id: u64,
    },
    /// Makes pixels from a sum of the cached scene (render Workers).
    Picture {
        /// The scene the sum belongs to; a mismatch fails.
        scene_id: u64,
        /// Samples per pixel in `sums`.
        sample_count: u32,
        /// The full-frame running sum, row-major.
        sums: Vec<[f32; 3]>,
        /// What to make.
        kind: PictureKind,
    },
    /// Names the URL a Worker polls, WHILE it computes job `job_id`, to learn that the
    /// page cancelled the job (sent just before the job's [`ToWorker::Solve`], or, on a
    /// render Worker, just before the [`ToWorker::TraceChunk`] of scene `job_id`).
    ///
    /// A Worker cannot read a message while it computes, so this is a side channel: the
    /// page hands over a `blob:` URL and revokes it to cancel; the Worker's poll of the
    /// URL then fails, its search stops between tier decisions (a chunk between row
    /// groups) and answers with the best result so far. See
    /// [`crate::worker::WorkerHandler::handle_probed`].
    WatchCancel {
        /// The job the URL belongs to (a render chunk's scene id); any other job
        /// ignores it.
        job_id: u64,
        /// The URL to poll.
        url: String,
    },
    /// Compiles GPU kernels in order and reports each.
    GpuCompile {
        /// Material class (0 generic, 1 isotropic, 2 uniaxial, 3 biaxial).
        class: u32,
        /// Kernel names to compile in order.
        kernels: Vec<String>,
        /// Soft deadline in milliseconds (stop compiling new kernels if breached).
        soft_ms: u32,
        /// Hard deadline in milliseconds (terminate worker if breached).
        hard_ms: u32,
    },
    /// Traces one full pass of the cached scene on the GPU.
    TraceFullPass {
        /// The scene the host means; a mismatch is dropped.
        scene_id: u64,
        /// The pass id.
        pass_id: u64,
        /// First sample index.
        sample_offset: u32,
        /// Samples per pixel.
        spp: u32,
    },
    /// [`Self::SetScene`] for a concave stone: the scene is `spec`'s planes minus `tools`.
    ///
    /// Sent instead of `SetScene` only when there are tools (build one with [`Self::scene`]).
    /// Appended as the last variant, so the bytes of every other message are unchanged. A
    /// render Worker builds it with
    /// [`GeometrySceneSpec::build`](crate::scene::GeometrySceneSpec::build), which refuses
    /// an invalid tool or more than `MAX_TOOL_PRIMITIVES`; the GPU Worker additionally
    /// declines more than `GPU_MAX_TOOL_PRIMITIVES` for itself.
    SetGeometryScene {
        /// The scene's id; results carry it back.
        scene_id: u64,
        /// The scene's planes, material, camera, lighting and frame.
        spec: SceneSpec,
        /// The resolved cutting tools, in cutting order (never empty on the wire).
        tools: Vec<ToolPrimitive>,
    },
    /// Selects the renderer of the cached scene without changing `SceneSpec`'s bytes.
    SetEngine {
        /// The scene the renderer choice belongs to.
        scene_id: u64,
        /// Renderer and deterministic AA options.
        engine: pleochroma::renderer::engine::EngineSpec,
    },
    /// Applies the design's render physics (inclusions, play-of-color, surface finish) to the
    /// cached scene without changing `SceneSpec`'s bytes. Sent after the scene, and only when
    /// the design sets something (`EffectsSpec::is_none` is false). Render Workers only.
    SetEffects {
        /// The scene the record belongs to.
        scene_id: u64,
        /// The design's render physics.
        effects: crate::scene::EffectsSpec,
    },
}

impl ToWorker {
    /// The message that gives a Worker the scene `spec` minus `tools`: [`Self::SetScene`] when
    /// there are no tools (byte-identical to before concave stones), else
    /// [`Self::SetGeometryScene`].
    #[must_use]
    pub fn scene(scene_id: u64, spec: SceneSpec, tools: &[ToolPrimitive]) -> Self {
        if tools.is_empty() {
            Self::SetScene { scene_id, spec }
        } else {
            Self::SetGeometryScene {
                scene_id,
                spec,
                tools: tools.to_vec(),
            }
        }
    }
}

/// Worker to page.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[expect(
    clippy::large_enum_variant,
    reason = "the postcard wire format pins variant order and payload layout (protocol/tests.rs); \
              `SolveResult` carries one finished answer by value and a message is built once per \
              job, so boxing it would only move the allocation and change every consumer"
)]
pub enum FromWorker {
    /// The Worker's handler is installed; send `Init`.
    Loaded {
        /// The Worker's own [`PROTOCOL_VERSION`].
        protocol_version: u32,
    },
    /// `Init` accepted.
    Ready {
        /// The role taken.
        role: WorkerRole,
        /// The index given.
        worker_index: u32,
    },
    /// A traced chunk.
    ChunkResult {
        /// The scene it was traced for.
        scene_id: u64,
        /// The partition.
        first_pixel: u32,
        /// The partition count.
        stride: u32,
        /// First sample index.
        sample_offset: u32,
        /// Samples per pixel.
        spp: u32,
        /// One summed XYZ radiance per partition pixel, in pixel order.
        sums: Vec<[f32; 3]>,
        /// Tracing time measured in the Worker (`performance.now()`).
        elapsed_ms: f64,
    },
    /// A chunk the Worker could not trace (no scene, or a different one).
    ChunkDropped {
        /// The scene the chunk asked for.
        scene_id: u64,
        /// The partition.
        first_pixel: u32,
        /// First sample index.
        sample_offset: u32,
    },
    /// The scene could not be built; chunks for it will be dropped.
    SceneError {
        /// The scene.
        scene_id: u64,
        /// Why.
        message: String,
    },
    /// An HDR map is decoded and cached.
    HdrLoaded {
        /// The map.
        id: u64,
        /// Width in texels.
        width: u32,
        /// Height in texels.
        height: u32,
    },
    /// An HDR map failed to decode.
    HdrError {
        /// The map.
        id: u64,
        /// Why.
        message: String,
    },
    /// Progress of a long job (an Optimize or Retarget search, or a tilt sweep).
    Progress {
        /// The job.
        job_id: u64,
        /// Human-readable stage.
        message: String,
        /// Completion fraction when known.
        fraction: Option<f32>,
    },
    /// A finished solve job.
    SolveResult {
        /// The job.
        job_id: u64,
        /// The answer.
        response: SolveResponse,
        /// Time the job took in the Worker.
        elapsed_ms: f64,
    },
    /// Anything else that went wrong (bad message, version mismatch, wrong role).
    Error {
        /// Why.
        message: String,
    },
    /// A finished [`ToWorker::Picture`].
    Picture {
        /// The scene.
        scene_id: u64,
        /// Samples per pixel it was made from.
        sample_count: u32,
        /// What was made.
        kind: PictureKind,
        /// RGBA8 for [`PictureKind::DenoisedLive`], PNG file bytes for
        /// [`PictureKind::Png`].
        bytes: Vec<u8>,
        /// Time it took in the Worker.
        elapsed_ms: f64,
    },
    /// A [`ToWorker::Picture`] that could not be made.
    PictureFailed {
        /// The scene.
        scene_id: u64,
        /// What was asked for.
        kind: PictureKind,
        /// Why.
        message: String,
    },
    /// A chunk the Worker stopped before it was done, because the page revoked its
    /// cancel URL (see [`ToWorker::WatchCancel`]). The host revokes a chunk's URL only
    /// once it has replaced the chunk's scene, so the partition has no result and none
    /// is wanted; the Worker is free for the new scene at once.
    ChunkAborted {
        /// The scene the chunk was traced for.
        scene_id: u64,
        /// The partition.
        first_pixel: u32,
        /// First sample index.
        sample_offset: u32,
    },
    /// Reports adapter limits and capabilities after GPU worker initialisation.
    GpuAdapter {
        /// Adapter vendor name.
        vendor: String,
        /// Architecture name.
        architecture: String,
        /// Device name.
        device: String,
        /// Human-readable description.
        description: String,
        /// Backend name (e.g. Vulkan, Metal, D3D12).
        backend: String,
        /// Maximum storage buffers per stage.
        max_storage_buffers_per_stage: u32,
        /// Maximum storage buffer binding size in bytes.
        max_storage_binding: u64,
        /// Maximum buffer size in bytes.
        max_buffer: u64,
    },
    /// WebGPU is unavailable or does not satisfy minimum requirements.
    GpuUnavailable {
        /// Why WebGPU could not be used.
        reason: String,
    },
    /// A GPU kernel pipeline compiled successfully.
    GpuCompiled {
        /// Kernel name.
        kernel: String,
        /// Material class.
        class: u32,
        /// Compilation time in milliseconds.
        ms: f64,
    },
    /// A GPU kernel pipeline failed to compile or exceeded budget.
    GpuCompileFailed {
        /// Kernel name.
        kernel: String,
        /// Material class.
        class: u32,
        /// Failure reason.
        reason: String,
        /// Compilation time in milliseconds before failure/timeout.
        ms: f64,
    },
    /// A finished full-frame GPU render pass.
    FullPassResult {
        /// The scene it was traced for.
        scene_id: u64,
        /// The pass id.
        pass_id: u64,
        /// First sample index.
        sample_offset: u32,
        /// Samples per pixel.
        spp: u32,
        /// One summed XYZ radiance per pixel of the entire frame.
        sums: Vec<[f32; 3]>,
        /// Tracing time measured in the Worker (`performance.now()`).
        elapsed_ms: f64,
    },
    /// Device lost or unrecoverable GPU error.
    GpuLost {
        /// Reason for device loss.
        reason: String,
    },
}

/// Encodes a page-to-Worker message.
///
/// # Errors
///
/// postcard's error text (in practice unreachable for these types).
pub fn encode_to_worker(message: &ToWorker) -> Result<Vec<u8>, String> {
    postcard::to_allocvec(message).map_err(|e| format!("encoding a worker message: {e}"))
}

/// Decodes a page-to-Worker message.
///
/// # Errors
///
/// When the bytes are not a valid [`ToWorker`].
pub fn decode_to_worker(bytes: &[u8]) -> Result<ToWorker, String> {
    postcard::from_bytes(bytes).map_err(|e| format!("decoding a worker message: {e}"))
}

/// Encodes a Worker-to-page message.
///
/// # Errors
///
/// postcard's error text (in practice unreachable for these types).
pub fn encode_from_worker(message: &FromWorker) -> Result<Vec<u8>, String> {
    postcard::to_allocvec(message).map_err(|e| format!("encoding a worker reply: {e}"))
}

/// Decodes a Worker-to-page message.
///
/// # Errors
///
/// When the bytes are not a valid [`FromWorker`].
pub fn decode_from_worker(bytes: &[u8]) -> Result<FromWorker, String> {
    postcard::from_bytes(bytes).map_err(|e| format!("decoding a worker reply: {e}"))
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod geometry_tests;
