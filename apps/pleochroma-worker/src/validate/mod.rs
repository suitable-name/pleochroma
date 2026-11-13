//! Input validation shared by both subcommands.
//!
//! `serve` accepts caller-supplied geometry and `render` a caller-supplied scene file;
//! neither should hand attacker- or fat-finger-controlled numbers straight to
//! `pleochroma` unchecked. [`validate_scene`] catches the geometry/material half
//! (non-finite or degenerate plane normals, implausible refractive indices, runaway
//! bounce counts); [`validate_request`] catches the size/count half.

use glam::Vec3;
use pleochroma_net::SceneState;

/// Hard cap on `width * height` for a single scene, shared by `render`'s CLI dimensions
/// and `serve`'s per-request `SceneState`.
///
/// 7680x4320 (8K UHD) keeps a traced radiance buffer
/// (`width * height * 12` bytes, see `pleochroma_net::radiance::BYTES_PER_PIXEL`) at
/// about 379 MiB, under [`pleochroma_net::framing::MAX_FRAME_LEN`] (512 MiB), so a `serve`
/// reply for a maximum-sized scene fits in one `FRAME` message.
pub const MAX_PIXELS: u32 = 7680 * 4320;

/// Hard cap on a single `RenderRequest.samples` value in `serve`.
///
/// Caps one `serve` request's `samples`, which is a single batch of a larger total. Without
/// it, a malicious or buggy `RenderRequest` could hold a worker's CPU before it replies, a
/// denial-of-service risk rather than a memory one. The local CLI cap,
/// [`render_cmd::MAX_CLI_SAMPLES`](crate::render_cmd::MAX_CLI_SAMPLES), is far larger.
///
/// Defined in `pleochroma_dispatch`, which sizes its chunks against it.
pub use pleochroma_dispatch::DEFAULT_MAX_CHUNK_SAMPLES as MAX_SAMPLES_PER_REQUEST;

/// Hard cap on `SceneState::max_bounces`.
///
/// Must stay at or above the GUI's top bounce rung, `MAX_EXPORT_BOUNCES` in
/// `apps/pleochroma-cut/src/bridge/export_thread/params.rs`. A lower cap would make a
/// 128-bounce "Local + Remote" export receive an error it never shows, leaving the job at
/// 0% GPU.
///
/// The cap is cheap: `crates/pleochroma/examples/bounce_cost.rs` measured only a
/// 1.27-1.38x wall-time increase, since almost no paths go deep (0.03% reach bounce 128).
/// It is not a meaningful `DoS` amplification lever.
pub const MAX_BOUNCES: u32 = 128;

/// Hard cap on `SceneState::backdrop`, the tray radiance at exposure 1.
///
/// The brightest plausible tray, a white tray (albedo 0.9) under the brightest Studio rig,
/// is about 8; 16 leaves a factor of two and still refuses a corrupt or hostile 1e30 emitter.
/// `0.0` (As Lit) is the floor.
pub const MAX_BACKDROP: f32 = 16.0;

/// Plausible refractive-index bounds, checked at the sodium D line and at both ends of
/// the visible spectrum (see [`validate_scene`]).
///
/// No real gem material here exceeds about 2.9 (moissanite, rutile). The bounds are looser
/// so a legitimate exotic material is not rejected, yet a Sellmeier or Cauchy fit gone to
/// NaN, negative or far off is caught before it reaches `trace_spectral_ray`.
pub const MIN_PLAUSIBLE_RI: f32 = 1.0;
/// Largest refractive index accepted as plausible.
pub const MAX_PLAUSIBLE_RI: f32 = 6.0;

/// Hard cap on `scene.planes.len()`.
///
/// A plausible cut has well under a thousand facets. Without this cap, a huge plane list
/// could make `build_plane_soa` and the GPU encoding spend unbounded time and memory before
/// any sample is traced.
pub const MAX_PLANES: usize = 4096;

/// Validates a fully-resolved [`SceneState`].
///
/// It requires finite, non-degenerate facet normals, a plausible refractive index across the
/// visible spectrum, finite scalars, and bounded `max_bounces`, backdrop and tray reflection.
///
/// The Stone Shadow and the Tray Reflection each need a studio light with a backdrop. An HDR
/// environment, As Lit or the lamp presets are refused here with the feature's own message.
/// Whether the stone is closed enough is known only once the map is built, so the request
/// path decides that (`crate::assets::resolve_tray`).
///
/// `width`/`height` are checked against [`MAX_PIXELS`] by the callers (`render_cmd`, `serve`),
/// since the `render` flags override the dimensions in a `scene.json`.
///
/// # Errors
///
/// Returns a human-readable message describing the first thing wrong with `scene`.
pub fn validate_scene(scene: &SceneState) -> Result<(), String> {
    if scene.stone_shadow && !scene.lit_by_studio_backdrop() {
        return Err("Stone shadow needs a studio light with a backdrop.".to_owned());
    }
    if scene.tray_reflection > 0.0 && !scene.lit_by_studio_backdrop() {
        return Err("Tray reflection needs a studio light with a backdrop.".to_owned());
    }
    if scene.engine.is_ray() {
        if !pleochroma::renderer::engine::AA_TAP_CHOICES.contains(&scene.engine.ray.aa_taps) {
            return Err("Ray-Traced AA taps must be 1, 4, 8 or 16".to_owned());
        }
        if scene.hdr().is_none() && scene.lighting_preset.is_uv_lamp() {
            return Err(pleochroma::renderer::whitted::WhittedDecline::UvLamp
                .message()
                .to_owned());
        }
        if !scene.fluorescence.is_empty() {
            return Err(pleochroma::renderer::whitted::WhittedDecline::Fluorescence
                .message()
                .to_owned());
        }
    }
    for (name, v) in [
        ("yaw", scene.yaw),
        ("camera_roll", scene.camera_roll),
        ("pitch", scene.pitch),
        ("light_yaw", scene.light_yaw),
        ("light_pitch", scene.light_pitch),
        ("exposure", scene.exposure),
        ("backdrop", scene.backdrop),
        ("surface_glare", scene.surface_glare),
        ("head_shadow_deg", scene.head_shadow_deg),
        ("tray_reflection", scene.tray_reflection),
        ("hdr_orientation.yaw", scene.hdr_orientation.yaw),
    ] {
        if !v.is_finite() {
            return Err(format!("scene.{name} must be finite (got {v})"));
        }
    }
    // The tray's head-on reflectance `R0`: a fraction, 0 (matte tray) to 1 (mirror).
    if !(0.0..=1.0).contains(&scene.tray_reflection) {
        return Err(format!(
            "scene.tray_reflection must be between 0 and 1 (got {})",
            scene.tray_reflection
        ));
    }
    // Tray radiance at exposure 1; see `MAX_BACKDROP`.
    if !(0.0..=MAX_BACKDROP).contains(&scene.backdrop) {
        return Err(format!(
            "scene.backdrop must be between 0 and {MAX_BACKDROP} (got {})",
            scene.backdrop
        ));
    }
    if !scene.distance.is_finite() || scene.distance <= 0.0 {
        return Err(format!(
            "scene.distance must be finite and positive (got {})",
            scene.distance
        ));
    }
    if scene.max_bounces == 0 || scene.max_bounces > MAX_BOUNCES {
        return Err(format!(
            "scene.max_bounces must be between 1 and {MAX_BOUNCES} (got {})",
            scene.max_bounces
        ));
    }

    check_planes_and_tools(scene)?;

    // Fluorescent emitters: at most `MAX_EMITTERS`, each with at most `MAX_BANDS` excitation
    // and emission bands, all values finite, quantum yields in [0, 1].
    scene
        .fluorescence
        .validate()
        .map_err(|e| format!("scene.fluorescence is invalid: {e}"))?;

    check_material_scalars(scene)?;

    // `renderer::buffers::GpuGemMaterial` flattens each eigenmode's `Vec<AbsorptionBand>`
    // into a fixed-capacity `[GpuAbsorptionBand; MAX_ABSORPTION_BANDS]` array for GPU
    // encoding. Reject an oversized `Vec` here rather than silently truncating it the
    // first time the GPU path encodes it.
    check_absorption_band_caps(scene)?;

    // Sodium D line (589.3nm, the conventional n_d reference) plus both ends of the
    // visible spectrum -- a fit that looks fine at n_d can still blow up (or be masked
    // by `DispersionModel::evaluate`'s `n2.max(1.0)` clamp) at the edge of its range.
    for lambda_nm in [380.0f32, 589.3, 780.0] {
        let n = scene.material.dispersion.evaluate(lambda_nm);
        if !n.is_finite() || !(MIN_PLAUSIBLE_RI..=MAX_PLAUSIBLE_RI).contains(&n) {
            return Err(format!(
                "scene.material's refractive index at {lambda_nm}nm is implausible: {n} (expected {MIN_PLAUSIBLE_RI}..={MAX_PLAUSIBLE_RI})"
            ));
        }
    }

    validate_environment(scene)
}

/// Checks the scalar properties of the material: the axis and birefringence values are finite and
/// the absorption path scale is finite and positive.
///
/// # Errors
///
/// Returns a message naming the first material value that is non-finite or out of range.
fn check_material_scalars(scene: &SceneState) -> Result<(), String> {
    if !scene.material.c_axis.is_finite() {
        return Err(format!(
            "scene.material.c_axis is non-finite: {}",
            scene.material.c_axis
        ));
    }
    if !scene.material.birefringence_delta.is_finite() {
        return Err(format!(
            "scene.material.birefringence_delta is non-finite: {}",
            scene.material.birefringence_delta
        ));
    }
    if let Some(delta) = scene.material.biaxial_delta_beta_alpha
        && !delta.is_finite()
    {
        return Err(format!(
            "scene.material.biaxial_delta_beta_alpha is non-finite: {delta}"
        ));
    }
    // This scale (see `GemMaterial::absorption_path_scale`) multiplies every interior
    // path length before Beer-Lambert absorption; zero/negative collapses or inverts
    // path lengths, NaN poisons every absorbed sample.
    if !scene.material.absorption_path_scale.is_finite()
        || scene.material.absorption_path_scale <= 0.0
    {
        return Err(format!(
            "scene.material.absorption_path_scale must be finite and positive (got {})",
            scene.material.absorption_path_scale
        ));
    }
    Ok(())
}

/// Checks the facet planes and the concave tools of `scene`.
///
/// # Errors
///
/// Returns a message naming the first plane or tool that is empty, oversized or malformed.
fn check_planes_and_tools(scene: &SceneState) -> Result<(), String> {
    if scene.planes.is_empty() {
        return Err("scene.planes must not be empty".to_string());
    }
    if scene.planes.len() > MAX_PLANES {
        return Err(format!(
            "scene.planes has {} facet(s), exceeding the maximum of {MAX_PLANES}",
            scene.planes.len()
        ));
    }
    for (i, plane) in scene.planes.iter().enumerate() {
        let normal = Vec3::from_array(plane.normal);
        if !normal.is_finite() {
            return Err(format!(
                "scene.planes[{i}].normal is non-finite: {:?}",
                plane.normal
            ));
        }
        if normal.length() <= 1e-6 {
            return Err(format!(
                "scene.planes[{i}].normal is degenerate (near-zero length): {:?}",
                plane.normal
            ));
        }
        if !plane.d.is_finite() {
            return Err(format!("scene.planes[{i}].d is non-finite: {}", plane.d));
        }
    }

    // Concave tools. The count bound is the shared cap the kernel's tool scan is sized for.
    // Each primitive uses the desktop's own validator, so a worker never feeds the tracer a
    // tool with a non-finite, degenerate or out-of-range field.
    if scene.tools.len() > pleochroma::geometry::MAX_TOOL_PRIMITIVES {
        return Err(format!(
            "scene.tools has {} primitive(s), exceeding the maximum of {}",
            scene.tools.len(),
            pleochroma::geometry::MAX_TOOL_PRIMITIVES
        ));
    }
    for (i, tool) in scene.tools.iter().enumerate() {
        tool.validate()
            .map_err(|e| format!("scene.tools[{i}] is invalid: {e}"))?;
    }
    // Frosted planes: at most one entry per plane, so the list is bounded. An index past the
    // planes names nothing and is not an error.
    if scene.frosted_planes.len() > scene.planes.len() {
        return Err(format!(
            "scene.frosted_planes has {} entries for {} facet(s)",
            scene.frosted_planes.len(),
            scene.planes.len()
        ));
    }
    // Render physics: inclusion and structure choices, facet surface words, the sender's
    // machine and the abraded edge.
    scene.effects.validate(scene.planes.len())?;
    Ok(())
}

/// Rejects an absorption band list longer than the GPU encoding's fixed-size array.
///
/// # Errors
///
/// Returns a message naming the oversized ray when a list exceeds the cap.
fn check_absorption_band_caps(scene: &SceneState) -> Result<(), String> {
    for (mode_name, bands) in [
        ("o_ray", &scene.material.absorption.o_ray),
        ("e_ray", &scene.material.absorption.e_ray),
    ]
    .into_iter()
    .chain(
        scene
            .material
            .absorption
            .beta_ray
            .as_ref()
            .filter(|_| scene.engine.is_ray())
            .map(|bands| ("beta_ray", bands)),
    ) {
        if bands.len() > pleochroma::renderer::buffers::MAX_ABSORPTION_BANDS {
            return Err(format!(
                "scene.material.absorption.{mode_name} has {} band(s), exceeding the GPU \
                 encoding's cap of {} (see renderer::buffers::MAX_ABSORPTION_BANDS)",
                bands.len(),
                pleochroma::renderer::buffers::MAX_ABSORPTION_BANDS
            ));
        }
    }
    Ok(())
}

/// Validates the scene's HDR environment, if any.
///
/// Its declared size must be within `pleochroma::renderer::env_map::HdrLimits::DEFAULT`.
/// This is checked before the map's bytes are requested, so an over-limit map costs no
/// transfer.
///
/// # Errors
///
/// A human-readable message when the declared size is zero or over the limits.
pub fn validate_environment(scene: &SceneState) -> Result<(), String> {
    let Some(hdr) = scene.hdr() else {
        return Ok(());
    };
    let limits = pleochroma::renderer::env_map::HdrLimits::DEFAULT;
    if hdr.width == 0 || hdr.height == 0 || !limits.admits(hdr.width, hdr.height) {
        return Err(format!(
            "scene.environment declares a {}x{} HDR map; it must be non-empty and within {}x{} \
             texels ({} MiB decoded)",
            hdr.width,
            hdr.height,
            limits.max_width,
            limits.max_height,
            limits.max_decoded_bytes / (1024 * 1024)
        ));
    }
    Ok(())
}

/// Validates one `serve` [`pleochroma_net::messages::RenderRequest`].
///
/// Checks the embedded scene (via [`validate_scene`]), the scene's own dimensions
/// against [`MAX_PIXELS`], the requested sample count against
/// [`MAX_SAMPLES_PER_REQUEST`], and that `first_sample + samples` doesn't overflow
/// `u32` -- an overflow would wrap the absolute sample numbering the seed formula
/// depends on, which must never repeat within one accumulation.
///
/// # Errors
///
/// Returns a human-readable message describing the first thing wrong with the request.
pub fn validate_request(scene: &SceneState, first_sample: u32, samples: u32) -> Result<(), String> {
    if scene.width == 0 || scene.height == 0 {
        return Err(format!(
            "scene dimensions must be positive (got {}x{})",
            scene.width, scene.height
        ));
    }
    let pixels = u64::from(scene.width) * u64::from(scene.height);
    if pixels > u64::from(MAX_PIXELS) {
        return Err(format!(
            "scene dimensions {}x{} ({pixels} px) exceed the maximum of {MAX_PIXELS} px",
            scene.width, scene.height
        ));
    }
    validate_scene(scene)?;
    if scene.engine.is_ray()
        && first_sample
            .checked_add(samples)
            .is_none_or(|end| end > u32::from(scene.engine.ray.aa_taps))
    {
        return Err("Ray-Traced request exceeds its fixed AA tap range".to_owned());
    }

    if samples == 0 {
        return Err("samples must be positive".to_string());
    }
    if samples > MAX_SAMPLES_PER_REQUEST {
        return Err(format!(
            "samples per request must be <= {MAX_SAMPLES_PER_REQUEST} (got {samples})"
        ));
    }
    // Beyond plain overflow, leave room below `u32::MAX` for one more chunk. Downstream chunk
    // bounds and the coordinator's accounting add a chunk's worth without re-checking this.
    if first_sample
        .checked_add(samples)
        .is_none_or(|end| end > u32::MAX - MAX_SAMPLES_PER_REQUEST)
    {
        return Err(format!(
            "first_sample + samples must not overflow u32 and must leave headroom below \
             u32::MAX for chunking (first_sample={first_sample}, samples={samples})"
        ));
    }
    Ok(())
}

/// Why a [`pleochroma_net::messages::LivePassRequest`] is refused.
#[cfg(feature = "adaptive-sampling")]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LivePassRefusal {
    /// A well-formed request this server does not trace: the Ray-Traced engine (it has no noise
    /// to converge) or an HDR environment (a live pass is studio-lit). Answered with
    /// `UNSUPPORTED_REQUEST`.
    Unsupported(String),
    /// The request breaks a rule (the scene, the sample range or the live-tile set). Answered
    /// with `VALIDATION_FAILED`.
    Invalid(String),
}

#[cfg(feature = "adaptive-sampling")]
impl LivePassRefusal {
    /// The human-readable reason.
    #[must_use]
    pub fn message(&self) -> &str {
        match self {
            Self::Unsupported(message) | Self::Invalid(message) => message,
        }
    }

    /// The `ErrorMsg::code` the refusal is answered with.
    #[must_use]
    pub const fn code(&self) -> u32 {
        match self {
            Self::Unsupported(_) => pleochroma_net::messages::error_codes::UNSUPPORTED_REQUEST,
            Self::Invalid(_) => pleochroma_net::messages::error_codes::VALIDATION_FAILED,
        }
    }
}

/// Validates one [`pleochroma_net::messages::LivePassRequest`] and returns the live tiles it
/// names on its scene's frame.
///
/// The scene and the sample range are checked as for a `RenderRequest` ([`validate_request`]),
/// the live-tile set against the frame and
/// [`pleochroma_net::messages::MAX_LIVE_PASS_PIXELS`].
///
/// # Errors
///
/// [`LivePassRefusal`], `Unsupported` for a Ray-Traced or HDR-lit scene and `Invalid` for
/// anything else.
#[cfg(feature = "adaptive-sampling")]
pub fn validate_live_pass(
    request: &pleochroma_net::messages::LivePassRequest,
) -> Result<pleochroma::renderer::adaptive::LiveTiles, LivePassRefusal> {
    if request.scene.engine.is_ray() {
        return Err(LivePassRefusal::Unsupported(
            "a live pass is for the path-traced renderer: the Ray-Traced engine has no noise to \
             converge"
                .to_owned(),
        ));
    }
    if request.scene.hdr().is_some() {
        return Err(LivePassRefusal::Unsupported(
            "a live pass is studio-lit; an HDR environment is not served in a live pass".to_owned(),
        ));
    }
    validate_request(&request.scene, request.first_sample, request.samples)
        .map_err(LivePassRefusal::Invalid)?;
    request
        .live_tiles()
        .map_err(|e| LivePassRefusal::Invalid(e.to_string()))
}

#[cfg(feature = "beam")]
/// Why a [`pleochroma_net::messages::BeamTileRequest`] is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BeamTileRefusal {
    /// A well-formed request this server does not render: the Ray-Traced engine (beams need the
    /// path-traced residual) or an HDR environment (the beams are studio-lit). Answered with
    /// `UNSUPPORTED_REQUEST`.
    Unsupported(String),
    /// The request breaks a rule (the scene, the sample range, the settings or the tile set).
    /// Answered with `VALIDATION_FAILED`.
    Invalid(String),
}

#[cfg(feature = "beam")]
impl BeamTileRefusal {
    /// The human-readable reason.
    #[must_use]
    pub fn message(&self) -> &str {
        match self {
            Self::Unsupported(message) | Self::Invalid(message) => message,
        }
    }

    /// The `ErrorMsg::code` the refusal is answered with.
    #[must_use]
    pub const fn code(&self) -> u32 {
        match self {
            Self::Unsupported(_) => pleochroma_net::messages::error_codes::UNSUPPORTED_REQUEST,
            Self::Invalid(_) => pleochroma_net::messages::error_codes::VALIDATION_FAILED,
        }
    }
}

#[cfg(feature = "beam")]
/// Validates one [`pleochroma_net::messages::BeamTileRequest`] and returns the tiles it names on
/// its scene's frame.
///
/// The scene and first sample are checked as for [`validate_request`]. The settings and tile
/// set are checked by `BeamTileRequest::live_tiles`, which applies `BeamSettings::validate`
/// (spectral nodes 8..=1024 in eights, positive finite tolerances, fragment depth within the
/// GPU clamp) and [`pleochroma_net::messages::MAX_BEAM_TILE_PIXELS`]. The tile edge must equal
/// the adaptive tile, since the beam tile is the adaptive tile. `adaptive.max_spp` is capped at
/// [`MAX_SAMPLES_PER_REQUEST`], and `sample_offset + max_spp` must leave headroom below
/// `u32::MAX`. `beam.residual_min_spp` may exceed `max_spp`; the residual clamps it.
///
/// # Errors
///
/// [`BeamTileRefusal`], `Unsupported` for a Ray-Traced or HDR-lit scene and `Invalid` for anything
/// else.
pub fn validate_beam_tiles(
    request: &pleochroma_net::messages::BeamTileRequest,
) -> Result<pleochroma::renderer::adaptive::LiveTiles, BeamTileRefusal> {
    if request.scene.engine.is_ray() {
        return Err(BeamTileRefusal::Unsupported(
            pleochroma::renderer::accel_beam::BeamOff::RayTracedResidual
                .message()
                .to_owned(),
        ));
    }
    if request.scene.hdr().is_some() {
        return Err(BeamTileRefusal::Unsupported(
            "beam tiles are studio-lit; an HDR environment is not served in beam tiles".to_owned(),
        ));
    }
    // One sample checks the scene and the offset; the residual range is checked below.
    validate_request(&request.scene, request.sample_offset, 1).map_err(BeamTileRefusal::Invalid)?;
    // The residual loop has no other bound, so the live-pass sample cap applies.
    if request.adaptive.max_spp > MAX_SAMPLES_PER_REQUEST {
        return Err(BeamTileRefusal::Invalid(format!(
            "max_spp must be <= {MAX_SAMPLES_PER_REQUEST} (got {})",
            request.adaptive.max_spp
        )));
    }
    // `beam.residual_min_spp` above `max_spp` is clamped by `tile_residual::residual_settings`,
    // not refused, since the default would reject small jobs.
    if request
        .sample_offset
        .checked_add(request.adaptive.max_spp)
        .is_none_or(|end| end > u32::MAX - MAX_SAMPLES_PER_REQUEST)
    {
        return Err(BeamTileRefusal::Invalid(format!(
            "sample_offset + max_spp must not overflow u32 and must leave headroom below \
             u32::MAX (sample_offset={}, max_spp={})",
            request.sample_offset, request.adaptive.max_spp
        )));
    }
    let live = request
        .live_tiles()
        .map_err(|e| BeamTileRefusal::Invalid(e.to_string()))?;
    if live.tile() != request.adaptive.tile {
        return Err(BeamTileRefusal::Invalid(format!(
            "the beam tile is the adaptive tile: the tile set has edge {} but the adaptive \
             settings have {}",
            live.tile(),
            request.adaptive.tile
        )));
    }
    Ok(live)
}

/// Validates the [`pleochroma_net::messages::StreamConfig`] half of a `serve`
/// [`pleochroma_net::messages::RenderRequest`].
///
/// [`validate_request`] covers the rest of the request. This also clamps
/// [`pleochroma_net::messages::StreamConfig::cadence_ms`] up to the advertised floor.
///
/// # `cadence_ms` clamping
///
/// A `cadence_ms` below [`crate::stream_emit::MIN_CADENCE_FLOOR_MS`] is raised to it in place,
/// so `stream` is taken `&mut`. `0` does not mean "as fast as possible": with a full-scale
/// preview under `FinalOnly`, the emitter would write the whole preview on every
/// `EMITTER_POLL` tick (about 20 ms) instead of once per cadence.
///
/// # Full-scale preview under `FinalOnly`
///
/// Rejected outright. `FinalOnly` sends `FRAME` once at the end, so the emitter's skip of a
/// redundant full-scale `PREVIEW` (which fires only when a `FRAME` delta went out on the same
/// tick) never applies, and the preview would repeat the whole frame on every cadence tick.
/// `LiveProgressive` is fine, because its `FRAME` deltas trigger that skip.
///
/// # Errors
///
/// Returns a human-readable message if a configured preview has a zero `width`/`height`,
/// more pixels than the scene it's a reduced-resolution preview of, or is exactly the
/// scene's own resolution under `TransferMode::FinalOnly`.
pub fn validate_stream_config(
    stream: &mut pleochroma_net::messages::StreamConfig,
    scene: &SceneState,
) -> Result<(), String> {
    if stream.cadence_ms < crate::stream_emit::MIN_CADENCE_FLOOR_MS {
        stream.cadence_ms = crate::stream_emit::MIN_CADENCE_FLOOR_MS;
    }

    let Some(preview) = stream.preview else {
        return Ok(());
    };
    if preview.width == 0 || preview.height == 0 {
        return Err(format!(
            "stream.preview dimensions must be positive (got {}x{})",
            preview.width, preview.height
        ));
    }
    let preview_pixels = u64::from(preview.width) * u64::from(preview.height);
    let scene_pixels = u64::from(scene.width) * u64::from(scene.height);
    if preview_pixels > scene_pixels {
        return Err(format!(
            "stream.preview {}x{} ({preview_pixels} px) must not exceed the scene's own \
             {}x{} ({scene_pixels} px) -- a preview is a REDUCED-resolution snapshot",
            preview.width, preview.height, scene.width, scene.height
        ));
    }
    if stream.transfer_mode == pleochroma_net::messages::TransferMode::FinalOnly
        && preview.width == scene.width
        && preview.height == scene.height
    {
        return Err(format!(
            "stream.preview {}x{} must not equal the scene's own resolution under FinalOnly \
             transfer mode -- a full-scale PREVIEW would duplicate the single FRAME reply on \
             every cadence tick; request a smaller preview, drop it, or use LiveProgressive \
             instead",
            preview.width, preview.height
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests;
