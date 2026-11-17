//! The post-`WELCOME` request loop, shared by every render-capable connection whoever dialled it.
//!
//! A viewer connection `serve` accepted, and a `join`ed worker's outbound connection to its
//! coordinator. On the latter the roles invert at the application layer -- the worker is the
//! TLS/TCP client but the PROTOCOL server: it reads `ClientMessage`s and answers `StreamEvent`s,
//! exactly as here.

#[cfg(feature = "beam")]
use super::beam_tiles;
#[cfg(feature = "adaptive-sampling")]
use super::live_pass;
use super::{NO_RENDER_CAPACITY_CODE, batch, handle_non_render_message};
use crate::{
    assets::{self, AssetCache, Fetched, HdrRoute, HeldAsset},
    cli::ComputeMode,
    control::renders::{self, ActiveRenderStart},
    coordinator::{self, CapabilityWatch, ViewerSession},
    serve::library::LibraryHandle,
    stream_emit::{self, StreamOutcome, TimeoutRead, TimeoutWrite, is_stream_timeout},
    validate,
};
use pleochroma::renderer::gpu_backend::GpuBackend;
use pleochroma_net::{
    admin::status::RenderKind,
    framing::{FramingError, IDLE_READ_TIMEOUT},
    messages::{
        BatchRenderRequest, BeamTileRequest, ClientMessage, ErrorMsg, FinalImageRequest,
        JobRequest, LivePassRequest, NetError, PayloadEncoding, RenderRequest, RequestIntent,
        StreamEvent, TiltCurvesRequest, TransferMode, adaptive::PeerLink, error_codes,
    },
};
use std::{
    io::{Read, Write},
    sync::Arc,
};

/// `<- ERROR`/`TiltCurvesResponse::Error` code for a request whose embedded scene failed
/// [`validate::validate_scene`].
///
/// Shared verbatim between `RenderRequest` and `TiltCurvesRequest` validation failures even though
/// the reply envelope differs.
pub const VALIDATION_FAILED_CODE: u32 = error_codes::VALIDATION_FAILED;
/// `<- ERROR`/`TiltCurvesResponse::Error` code for a `pleochroma` panic caught by
/// `catch_unwind` on a validation-passing but pathological scene.
pub const TRACE_PANIC_CODE: u32 = error_codes::TRACE_PANIC;

/// Everything [`serve_requests`] needs for one connection.
pub struct RequestContext<'a> {
    /// CPU tracer threads (`0` = all cores).
    pub threads: usize,
    /// The process's shared GPU backend (disabled when this machine renders on the CPU
    /// only, or has no own lane at all).
    pub gpu: &'a Arc<GpuBackend>,
    /// The connection's library, opened by its first library request (never by any other
    /// kind of request), or `None` on a `join`ed worker's connection (the coordinator
    /// serves the library).
    pub db: Option<&'a LibraryHandle>,
    /// `--only-gpu`/`--only-cpu`/hybrid for the own lane.
    pub compute_mode: ComputeMode,
    /// The encoding negotiated in `WELCOME`: what a request sends when there is no
    /// [`Self::link`].
    pub payload_encoding: PayloadEncoding,
    /// The connection's adaptive-compression state, shared by every request on it.
    pub link: Option<&'a Arc<PeerLink>>,
    /// Whether this machine renders requests itself (`serve --render`, or any `join`ed
    /// worker). Without it (and without a [`Self::session`]) a `RenderRequest` is refused
    /// (see [`refuse_without_own_lane`]).
    pub own_lane: bool,
    /// A coordinator's viewer connection: requests execute through
    /// [`crate::coordinator`] (own lane directly, or a job over joined workers), and
    /// capacity changes are announced between requests. `None` on a `join`ed worker's
    /// connection and a plain single worker.
    pub session: Option<&'a ViewerSession>,
    /// The HDR asset cache: with it, requests whose scene names an HDR map are resolved
    /// first (`crate::assets::ensure_environment`); without it they are refused (see
    /// `crate::assets::hdr_route`).
    pub assets: Option<&'a AssetCache>,
    /// The durable job service: `None` on a server without an own render lane and on a
    /// `join`ed worker's connection, where job requests are refused.
    pub jobs: Option<&'a Arc<crate::jobs::JobService>>,
    /// The asking viewer's identity key (`certificate:<hex>`, `address:<ip>`, or empty for a
    /// viewer with neither), so its requests are listed as running renders in the coordinator
    /// status. `None` on a `join`ed worker's connection, whose chunks the coordinator lists.
    pub viewer: Option<&'a str>,
}

/// What [`render_start`] reads off a request.
struct Facts {
    kind: RenderKind,
    width: u32,
    height: u32,
    is_ray: bool,
    target: Option<u32>,
    /// The viewer's coordinator forwards it whole to a joined worker (when it has no lane).
    forwarded: bool,
    /// This connection serves the request at all (a refusal is not a render).
    served: bool,
}

/// The registry entry for `next`, or `None` when this is not a viewer's connection, the
/// request is not a render (a job is on the durable queue), or it is refused up front.
fn render_start(next: &NextRequest, ctx: &RequestContext<'_>) -> Option<ActiveRenderStart> {
    let viewer = ctx.viewer?;
    let lane = ctx.own_lane;
    let coordinated = ctx.session.is_some();
    let live_target =
        |samples: u32| (samples > 0 && samples < renders::OPEN_ENDED_SAMPLES).then_some(samples);
    let facts = match next {
        NextRequest::Render(r) => Facts {
            kind: match r.intent {
                RequestIntent::Interactive => RenderKind::Live,
                RequestIntent::Batch => RenderKind::FinalImage,
            },
            width: r.scene.width,
            height: r.scene.height,
            is_ray: r.scene.engine.is_ray(),
            target: live_target(r.samples),
            forwarded: false,
            served: lane || coordinated,
        },
        NextRequest::FinalImage(r) => Facts {
            kind: RenderKind::FinalImage,
            width: r.scene.width,
            height: r.scene.height,
            is_ray: r.scene.engine.is_ray(),
            target: Some(r.samples),
            forwarded: false,
            served: coordinated,
        },
        NextRequest::TiltCurves(r) => Facts {
            kind: RenderKind::TiltCurves,
            width: r.scene.width,
            height: r.scene.height,
            is_ray: r.scene.engine.is_ray(),
            target: None,
            forwarded: true,
            served: true,
        },
        NextRequest::LivePass(r) => Facts {
            kind: RenderKind::LivePass,
            width: r.scene.width,
            height: r.scene.height,
            is_ray: r.scene.engine.is_ray(),
            target: Some(r.samples),
            forwarded: true,
            served: lane || coordinated,
        },
        NextRequest::BeamTiles(r) => Facts {
            kind: RenderKind::BeamTiles,
            width: r.scene.width,
            height: r.scene.height,
            is_ray: r.scene.engine.is_ray(),
            target: None,
            forwarded: true,
            served: lane || coordinated,
        },
        NextRequest::Batch(r) => {
            let first = r.items.first()?;
            Facts {
                kind: RenderKind::Batch,
                width: first.scene.width,
                height: first.scene.height,
                is_ray: first.scene.engine.is_ray(),
                target: None,
                forwarded: false,
                served: lane,
            }
        }
        NextRequest::Job(_) => return None,
    };
    if !facts.served {
        return None;
    }
    let route = match ctx.session {
        Some(session) if session.coordinator.own().is_none() => {
            let joined = session
                .coordinator
                .registry()
                .map_or(0, |registry| registry.workers().len());
            renders::route_text(false, if facts.forwarded { 1 } else { joined })
        }
        _ => renders::route_text(true, 0),
    };
    let (device, fingerprint) = renders::viewer_identity(viewer);
    Some(ActiveRenderStart {
        device,
        fingerprint,
        kind: facts.kind,
        engine: renders::engine_text(facts.is_ray).to_owned(),
        width: facts.width,
        height: facts.height,
        target: facts.target,
        route,
    })
}

/// Writes the `UNSUPPORTED_REQUEST` reply for a well-formed request this server does
/// not implement yet -- the connection stays open for the next one.
fn refuse_unsupported<S: Write>(
    stream: &mut S,
    what: &str,
    request_id: u32,
) -> Result<(), NetError> {
    tracing::info!("refusing an unsupported request: {what}");
    pleochroma_net::messages::write_stream_event(
        stream,
        &StreamEvent::Error(ErrorMsg {
            code: error_codes::UNSUPPORTED_REQUEST,
            message: format!("{what} is not supported by this server (a plain worker)"),
            request_id: Some(request_id),
        }),
        None,
    )
}

/// The reply to a `RenderRequest` on a connection with no render lane and no
/// coordinator behind it (its `WELCOME.render` was `None`, so a well-behaved viewer
/// never sends one): `NO_RENDER_CAPACITY`. A coordinator's viewer connection never gets
/// here -- [`coordinator::serve_render`] words its own refusals.
fn refuse_without_own_lane<S: Write>(stream: &mut S, request_id: u32) -> Result<(), NetError> {
    let error = ErrorMsg {
        code: NO_RENDER_CAPACITY_CODE,
        message: format!(
            "RenderRequest (request_id={request_id}): this server has no render lane; its WELCOME \
             advertised no render capability"
        ),
        request_id: Some(request_id),
    };
    tracing::info!("{}", error.message);
    pleochroma_net::messages::write_stream_event(stream, &StreamEvent::Error(error), None)
}

/// The reply to a `LivePassRequest` in a build without the `adaptive-sampling` feature:
/// `UNSUPPORTED_REQUEST`, naming the missing feature, and the connection keeps serving. A zoning
/// payload sent ahead for the request is dropped (a coordinator already consumed it in
/// `attach_zoning_on_coordinator`).
#[cfg(not(feature = "adaptive-sampling"))]
fn refuse_live_pass_unbuilt<S: Write>(stream: &mut S, request_id: u32) -> Result<(), NetError> {
    let _ = crate::serve::zoning::take(request_id);
    let error = ErrorMsg {
        code: error_codes::UNSUPPORTED_REQUEST,
        message: format!(
            "LivePassRequest (request_id={request_id}): this server was built without adaptive \
             sampling (the `adaptive-sampling` feature)"
        ),
        request_id: Some(request_id),
    };
    tracing::info!("{}", error.message);
    pleochroma_net::messages::write_stream_event(stream, &StreamEvent::Error(error), None)
}

/// A `LivePassRequest` in a build with the `adaptive-sampling` feature: one chunk of an
/// adaptive pass. A coordinator with a lane of its own traces it there; without one it
/// forwards the request whole to one idle joined worker. A connection with a lane traces it
/// there, and any other connection refuses it with `NO_RENDER_CAPACITY`.
#[cfg(feature = "adaptive-sampling")]
fn serve_live_pass_request<S: Read + Write + TimeoutRead + TimeoutWrite>(
    stream: &mut S,
    request: LivePassRequest,
    ctx: &RequestContext<'_>,
) -> Result<(), NetError> {
    match ctx.session {
        Some(session) if session.coordinator.own().is_none() => {
            coordinator::serve_live_pass(stream, &request, session)
        }
        _ if ctx.own_lane || ctx.session.is_some() => {
            live_pass::serve_live_pass(stream, request, ctx)
        }
        _ => refuse_without_own_lane(stream, request.request_id),
    }
}

/// The reply to a `BeamTileRequest` in a build without the `beam` feature:
/// `UNSUPPORTED_REQUEST`, naming the missing feature, and the connection keeps serving. A
/// zoning payload sent ahead for the request is dropped (a coordinator already consumed it in
/// `attach_zoning_on_coordinator`).
#[cfg(not(feature = "beam"))]
fn refuse_beam_tiles_unbuilt<S: Write>(stream: &mut S, request_id: u32) -> Result<(), NetError> {
    let _ = crate::serve::zoning::take(request_id);
    let error = ErrorMsg {
        code: error_codes::UNSUPPORTED_REQUEST,
        message: format!(
            "BeamTileRequest (request_id={request_id}): this server was built without beam tracing \
             (the `beam` feature)"
        ),
        request_id: Some(request_id),
    };
    tracing::info!("{}", error.message);
    pleochroma_net::messages::write_stream_event(stream, &StreamEvent::Error(error), None)
}

/// A `BeamTileRequest` in a build with the `beam` feature, served like a live pass: a
/// coordinator without a lane of its own forwards it whole to one idle joined worker, a
/// connection with a lane traces it there, and any other connection refuses it with
/// `NO_RENDER_CAPACITY`.
#[cfg(feature = "beam")]
fn serve_beam_tile_request<S: Read + Write + TimeoutRead + TimeoutWrite>(
    stream: &mut S,
    request: BeamTileRequest,
    ctx: &RequestContext<'_>,
) -> Result<(), NetError> {
    match ctx.session {
        Some(session) if session.coordinator.own().is_none() => {
            coordinator::serve_beam_tiles(stream, &request, session)
        }
        _ if ctx.own_lane || ctx.session.is_some() => {
            beam_tiles::serve_beam_tiles(stream, request, ctx)
        }
        _ => refuse_without_own_lane(stream, request.request_id),
    }
}

/// The post-`WELCOME` request loop.
///
/// Dispatches `Cancel`/`Library`/`Ping` inline and runs each `RenderRequest` (validate, then
/// stream) and `TiltCurvesRequest` until the peer closes the connection.
///
/// Each `RenderRequest` usually comes from a fresh [`read_next_message`] call, but may
/// already be in hand if the client pipelined it ahead of the previous request's `DONE`
/// (handed back by [`stream_emit::run_stream`]). `TiltCurvesRequest` supports no
/// pipelining and goes straight to [`crate::serve::tilt::handle_tilt_curves_request`].
///
/// # Errors
///
/// Returns [`NetError`] for a transport-level failure. A validation failure or caught
/// tracing panic is NOT an error return -- both are reported to the peer as a
/// `StreamEvent::Error` and the loop continues.
pub fn serve_requests<S: Read + Write + TimeoutRead + TimeoutWrite>(
    stream: &mut S,
    ctx: &RequestContext<'_>,
) -> Result<(), NetError> {
    // A `RenderRequest` `run_stream` already pulled off the wire while streaming the
    // previous request (client-pipelined ahead of its `DONE`), to be processed next
    // iteration as if freshly read. `None` means read one instead.
    let mut pending_request: Option<RenderRequest> = None;
    // A coordinator viewer connection announces capacity changes between requests.
    let mut watch = ctx.session.and_then(CapabilityWatch::new);

    loop {
        let mut next = if let Some(r) = pending_request.take() {
            NextRequest::Render(r)
        } else if let Some(n) = read_next_message(stream, ctx.db, watch.as_mut())? {
            n
        } else {
            // The peer closing the connection is the normal end of this loop.
            return Ok(());
        };
        // A coordinator re-attaches the zones the viewer sent ahead of a picture request: its
        // own lane traces the scene as held, and its joined workers get them again
        // ahead of every chunk (`coordinator::lanes::chunk`).
        #[cfg(feature = "worker")]
        if ctx.session.is_some() && !attach_zoning_on_coordinator(stream, &mut next)? {
            continue;
        }
        // An HDR scene's map is resolved (and pinned for the whole request)
        // before the request is served at all.
        let hdr_pin = match prepare_hdr(stream, &next, ctx)? {
            Prepared::Serve(pin) => pin,
            Prepared::Skip(next_request) => {
                pending_request = next_request.map(|next| *next);
                continue;
            }
            Prepared::Closed => return Ok(()),
        };

        // The request is listed as a running render exactly as long as it is served: the guard
        // goes with this iteration, however it ends (done, cancelled, the viewer left, a failure).
        let _render_guard = render_start(&next, ctx).map(renders::begin);

        pending_request = match (next, ctx.session) {
            (NextRequest::Render(request), Some(session)) => {
                coordinator::serve_render(stream, request, session, hdr_pin.held.clone())?
            }
            (NextRequest::Render(request), None) if ctx.own_lane => {
                serve_one_render(stream, request, ctx)?
            }
            (NextRequest::Render(request), None) => {
                refuse_without_own_lane(stream, request.request_id)?;
                None
            }
            (NextRequest::FinalImage(request), Some(session)) => {
                coordinator::serve_final_image(stream, &request, session, hdr_pin.held.clone())?
            }
            (NextRequest::FinalImage(request), None) => {
                refuse_unsupported(
                    stream,
                    &format!("FinalImageRequest (request_id={})", request.request_id),
                    request.request_id,
                )?;
                None
            }
            (NextRequest::TiltCurves(tilt_request), Some(session)) => {
                coordinator::serve_tilt(stream, &tilt_request, session)?;
                None
            }
            // A single request/response call (its own validation, catch_unwind, reply
            // envelope) -- unlike the validate-then-stream dance of a render.
            (NextRequest::TiltCurves(tilt_request), None) => {
                crate::serve::tilt::handle_tilt_curves_request(stream, &tilt_request)?;
                None
            }
            // A batch of finished pictures, served on the own lane (a coordinator without one
            // refuses it, and the viewer falls back to single requests). It keeps serving
            // later batches that arrive while it runs, so it returns only once none is
            // unfinished.
            (NextRequest::Batch(batch_request), _) => {
                batch::serve_batches(stream, batch_request, ctx)?;
                None
            }
            // One chunk of an adaptive pass ([`serve_live_pass_request`]). Without the
            // `adaptive-sampling` feature every one is refused and the connection keeps serving.
            (NextRequest::LivePass(request), _) => {
                #[cfg(feature = "adaptive-sampling")]
                {
                    serve_live_pass_request(stream, *request, ctx)?;
                }
                #[cfg(not(feature = "adaptive-sampling"))]
                {
                    refuse_live_pass_unbuilt(stream, request.request_id)?;
                }
                None
            }
            // Whole beam tiles ([`serve_beam_tile_request`]). Without the `beam` feature every
            // one is refused and the connection keeps serving.
            (NextRequest::BeamTiles(request), _) => {
                #[cfg(feature = "beam")]
                {
                    serve_beam_tile_request(stream, *request, ctx)?;
                }
                #[cfg(not(feature = "beam"))]
                {
                    refuse_beam_tiles_unbuilt(stream, request.request_id)?;
                }
                None
            }
            // A durable job request, answered at once (the render itself runs on the job
            // executor, never on this connection). Its owner is the viewer identity.
            (NextRequest::Job(job_request), session) => {
                crate::jobs::serve_job(
                    stream,
                    *job_request,
                    ctx.jobs.map(|jobs| &**jobs),
                    session.map(|session| &*session.viewer),
                )?;
                None
            }
        };
        // The request is served, so its HDR map is unpinned.
        drop(hdr_pin);
    }
}

/// Installs the zones the viewer sent ahead of `next` on its scene, on a coordinator
/// connection; a plain worker does the same in [`serve_one_render`]. `Ok(false)` means the
/// payload did not fit the scene: the request was refused with `VALIDATION_FAILED` and must
/// not be served.
///
/// A batch is served on the own lane by `batch::serve_batches`, which attaches its own zones;
/// a tilt-curves request and a job never carry any.
#[cfg(feature = "worker")]
fn attach_zoning_on_coordinator<S: Write>(
    stream: &mut S,
    next: &mut NextRequest,
) -> Result<bool, NetError> {
    let (request_id, scene) = match next {
        NextRequest::Render(r) => (r.request_id, &mut r.scene),
        NextRequest::FinalImage(r) => (r.request_id, &mut r.scene),
        // The zones are installed on the scene here; the own lane then finds no payload left
        // (an unzoned request as far as it can tell) and a joined worker is sent them again.
        NextRequest::LivePass(r) => (r.request_id, &mut r.scene),
        // The same for beam tiles.
        NextRequest::BeamTiles(r) => (r.request_id, &mut r.scene),
        NextRequest::TiltCurves(_) | NextRequest::Batch(_) | NextRequest::Job(_) => {
            return Ok(true);
        }
    };
    let Err(message) = crate::serve::zoning::attach_to_render(request_id, scene) else {
        return Ok(true);
    };
    pleochroma_net::messages::write_stream_event(
        stream,
        &StreamEvent::Error(ErrorMsg {
            code: VALIDATION_FAILED_CODE,
            message,
            request_id: Some(request_id),
        }),
        None,
    )?;
    Ok(false)
}

/// Validates and streams one `RenderRequest` on the own lane; returns the client's
/// already-pipelined next `RenderRequest`, if `run_stream` pulled one off the wire.
fn serve_one_render<S: Read + Write + TimeoutRead + TimeoutWrite>(
    stream: &mut S,
    mut request: RenderRequest,
    ctx: &RequestContext<'_>,
) -> Result<Option<RenderRequest>, NetError> {
    // Display frames need the coordinator's tone-map and denoise path, which a plain worker
    // lacks.
    if request.stream.transfer_mode == TransferMode::DisplayOnly {
        refuse_unsupported(
            stream,
            &format!(
                "TransferMode::DisplayOnly (request_id={})",
                request.request_id
            ),
            request.request_id,
        )?;
        return Ok(None);
    }

    // Re-attach the zones the viewer sent ahead of this request (they are
    // `serde(skip)` in the scene). A payload that does not fit refuses the request.
    #[cfg(feature = "worker")]
    if let Err(message) =
        crate::serve::zoning::attach_to_render(request.request_id, &mut request.scene)
    {
        pleochroma_net::messages::write_stream_event(
            stream,
            &StreamEvent::Error(ErrorMsg {
                code: VALIDATION_FAILED_CODE,
                message,
                request_id: Some(request.request_id),
            }),
            None,
        )?;
        return Ok(None);
    }

    // `validate_stream_config` takes `&mut request.stream` (it clamps `cadence_ms` in
    // place) alongside `&request.scene` -- two disjoint field borrows.
    if let Err(msg) =
        validate::validate_request(&request.scene, request.first_sample, request.samples)
            .and_then(|()| validate::validate_stream_config(&mut request.stream, &request.scene))
    {
        pleochroma_net::messages::write_stream_event(
            stream,
            &StreamEvent::Error(ErrorMsg {
                code: if request.scene.engine.is_ray() {
                    error_codes::UNSUPPORTED_REQUEST
                } else {
                    VALIDATION_FAILED_CODE
                },
                message: msg,
                request_id: Some(request.request_id),
            }),
            None,
        )?;
        return Ok(None);
    }

    // Runs the tracer on its own thread (never touching `stream`) and this thread as
    // the emitter -- see `stream_emit`'s module docs. The tracer thread runs inside
    // `catch_unwind`, surfaced here as [`StreamOutcome::TracePanicked`].
    let started = std::time::Instant::now();
    let (outcome, next) = stream_emit::run_stream(
        stream,
        &request,
        ctx.threads,
        ctx.gpu,
        ctx.compute_mode,
        ctx.payload_encoding,
        ctx.link,
    )?;
    // The one line a request that SUCCEEDS leaves on the console: without it "went
    // quiet" cannot tell "stopped being asked" from "stopped answering".
    tracing::info!(
        request_id = request.request_id,
        intent = ?request.intent,
        size = %format_args!("{}x{}", request.scene.width, request.scene.height),
        samples = request.samples,
        route = "plain worker",
        elapsed = ?started.elapsed(),
        %outcome,
        "worker: served a render request"
    );
    let error = match outcome {
        StreamOutcome::Completed => None,
        StreamOutcome::TracePanicked => {
            tracing::warn!(
                "tracing panicked for a request that passed validation (request_id={}, first_sample={}, samples={})",
                request.request_id,
                request.first_sample,
                request.samples
            );
            Some(ErrorMsg {
                code: TRACE_PANIC_CODE,
                message: "internal error while tracing this request".to_string(),
                request_id: Some(request.request_id),
            })
        }
        // `stream_emit::run_stream` already stamped this error's `request_id`.
        StreamOutcome::Failed(error) => Some(error),
    };
    if let Some(error) = error {
        pleochroma_net::messages::write_stream_event(stream, &StreamEvent::Error(error), None)?;
    }
    Ok(next)
}

/// What a request holds while it is served: nothing (studio-lit), the decoded
/// map (a plain or joined worker), or the job's [`HeldAsset`] (a coordinator; it also
/// pins the own lane's decoded map, if any).
struct HdrPin {
    /// Only held (dropped when the request has been served).
    _map: Option<Arc<pleochroma::renderer::env_map::EnvironmentMap>>,
    /// A coordinator's job asset, handed to the job.
    held: Option<Arc<HeldAsset>>,
}

impl HdrPin {
    /// Nothing to hold (studio-lit, or refused elsewhere).
    const NONE: Self = Self {
        _map: None,
        held: None,
    };

    /// A decoded map, pinned.
    const fn map(map: Arc<pleochroma::renderer::env_map::EnvironmentMap>) -> Self {
        Self {
            _map: Some(map),
            held: None,
        }
    }

    /// A coordinator's held asset.
    const fn held(held: Arc<HeldAsset>) -> Self {
        Self {
            _map: None,
            held: Some(held),
        }
    }
}

/// What [`prepare_hdr`] decided for one request.
enum Prepared {
    /// Serve the request, holding this pin until it has been served.
    Serve(HdrPin),
    /// Do not serve it (refused, failed or cancelled -- the reply was already sent);
    /// serve this pipelined request next, if any.
    Skip(Option<Box<RenderRequest>>),
    /// The peer closed the connection.
    Closed,
}

/// Resolves the HDR map a render or final-image request names: the
/// policy first ([`assets::hdr_route`]), then [`assets::ensure_environment`] -- or, on a
/// coordinator's viewer connection, [`assets::ensure_held`] through the coordinator's
/// own cache (decoding only for its own lane). A request that fails validation is
/// passed through untouched -- its own path refuses it with the usual
/// `VALIDATION_FAILED`, and no asset is fetched for it.
fn prepare_hdr<S: Read + Write + TimeoutRead + TimeoutWrite>(
    stream: &mut S,
    next: &NextRequest,
    ctx: &RequestContext<'_>,
) -> Result<Prepared, NetError> {
    let (request_id, scene, first_sample, samples, cadence_ms) = match next {
        // Requests this connection refuses anyway (no own lane; a final picture on a
        // plain worker) are not worth an asset transfer.
        NextRequest::Render(_) if ctx.session.is_none() && !ctx.own_lane => {
            return Ok(Prepared::Serve(HdrPin::NONE));
        }
        NextRequest::FinalImage(_) if ctx.session.is_none() => {
            return Ok(Prepared::Serve(HdrPin::NONE));
        }
        // A batch item naming an HDR map fails by itself (`batch::check_item`); no asset is
        // fetched for a batch (nor for a tilt-curves request).
        // A live pass is studio-lit: one naming an HDR map is refused by its own validation.
        NextRequest::Batch(_)
        | NextRequest::TiltCurves(_)
        | NextRequest::Job(_)
        | NextRequest::LivePass(_)
        | NextRequest::BeamTiles(_) => {
            return Ok(Prepared::Serve(HdrPin::NONE));
        }
        NextRequest::Render(r) => (
            r.request_id,
            &r.scene,
            r.first_sample,
            r.samples,
            r.stream.cadence_ms,
        ),
        NextRequest::FinalImage(r) => (r.request_id, &r.scene, r.first_sample, r.samples, 1000),
    };
    let Some(hdr) = scene.hdr() else {
        return Ok(Prepared::Serve(HdrPin::NONE));
    };
    if validate::validate_request(scene, first_sample, samples).is_err() {
        return Ok(Prepared::Serve(HdrPin::NONE));
    }
    match assets::hdr_route(scene, ctx.assets, ctx.session) {
        HdrRoute::NotHdr => return Ok(Prepared::Serve(HdrPin::NONE)),
        HdrRoute::Refuse(error) => {
            tracing::info!("request {request_id}: {}", error.message);
            pleochroma_net::messages::write_stream_event(stream, &StreamEvent::Error(error), None)?;
            return Ok(Prepared::Skip(None));
        }
        HdrRoute::Serve => {}
    }
    let pending = assets::Pending {
        request_id,
        cadence_ms,
        hdr,
    };
    match (ctx.session, ctx.assets) {
        (Some(session), _) => {
            let coordinator = &session.coordinator;
            let Some(cache) = coordinator.assets() else {
                return Ok(Prepared::Serve(HdrPin::NONE)); // `hdr_route` refuses without one
            };
            let decode = coordinator.own().is_some();
            let fetched = assets::ensure_held(stream, cache, pending, decode)?;
            finish_prepare(stream, fetched, HdrPin::held)
        }
        (None, Some(cache)) => {
            let fetched = assets::ensure_environment(stream, cache, pending)?;
            finish_prepare(stream, fetched, HdrPin::map)
        }
        // `hdr_route` refuses without a cache.
        (None, None) => Ok(Prepared::Serve(HdrPin::NONE)),
    }
}

/// Turns a resolve's outcome into [`Prepared`], writing the error a failed one owes.
fn finish_prepare<S: Write, T>(
    stream: &mut S,
    fetched: Fetched<T>,
    pin: impl FnOnce(T) -> HdrPin,
) -> Result<Prepared, NetError> {
    Ok(match fetched {
        Fetched::Ready(value) => Prepared::Serve(pin(value)),
        Fetched::Failed(error) => {
            pleochroma_net::messages::write_stream_event(stream, &StreamEvent::Error(error), None)?;
            Prepared::Skip(None)
        }
        Fetched::Superseded(next_request) => Prepared::Skip(next_request),
        Fetched::Closed => Prepared::Closed,
    })
}

/// Answers a `PING` with the matching `PONG`, immediately.
pub fn write_pong<S: Write>(stream: &mut S, nonce: u64) -> Result<(), NetError> {
    pleochroma_net::messages::write_stream_event(stream, &StreamEvent::Pong { nonce }, None)
}

/// What [`read_next_message`] found once it stopped reading. `Cancel`/`Library`/`Ping`
/// are fully handled inline before this ever returns, so this enum only carries the
/// request kinds that end the read loop.
enum NextRequest {
    Render(RenderRequest),
    TiltCurves(TiltCurvesRequest),
    FinalImage(FinalImageRequest),
    Batch(BatchRenderRequest),
    Job(Box<JobRequest>),
    LivePass(Box<LivePassRequest>),
    BeamTiles(Box<BeamTileRequest>),
}

/// Reads the next message off `stream`: blocking, or -- on a coordinator viewer
/// connection (`watch`) -- polling so capacity changes go out while it waits (see
/// [`coordinator::read_message_watching`]). Dispatches `Cancel`/`Library`/`Ping` inline
/// and keeps reading; only a request ends the wait (see [`NextRequest`]).
///
/// # Errors
///
/// Returns [`NetError`] for a transport-level failure (including a connection that ends
/// inside a frame). `Ok(None)` (not an error) for a clean EOF between messages, and for
/// a connection that sent nothing for [`IDLE_READ_TIMEOUT`], which is closed with a
/// logged reason.
fn read_next_message<S: Read + Write + TimeoutRead>(
    stream: &mut S,
    db: Option<&LibraryHandle>,
    mut watch: Option<&mut CapabilityWatch>,
) -> Result<Option<NextRequest>, NetError> {
    loop {
        let read = if let Some(watch) = watch.as_deref_mut() {
            coordinator::read_message_watching(stream, watch)
        } else {
            // Arm the idle deadline for this wait: the emitter and the asset fetch
            // reset the socket's read timeout to blocking after they finish. A
            // `join`ed worker's connection has no database; its stream maps
            // "blocking" to its own, shorter coordinator-liveness deadline, which
            // must stay in force.
            let idle = db.is_some().then_some(IDLE_READ_TIMEOUT);
            let _ = stream.set_read_timeout(idle);
            pleochroma_net::messages::read_control_message(stream).map(Some)
        };
        let msg: ClientMessage = match read {
            Ok(Some(m)) => m,
            Ok(None) => return Ok(None),
            Err(NetError::Framing(FramingError::Io(e)))
                if e.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                return Ok(None);
            }
            Err(NetError::Framing(FramingError::Io(e))) if is_stream_timeout(&e) => {
                tracing::info!(
                    "closing a connection that sent nothing for {} s (idle timeout)",
                    IDLE_READ_TIMEOUT.as_secs()
                );
                return Ok(None);
            }
            Err(e) => return Err(e),
        };
        match msg {
            ClientMessage::RenderRequest(r) => return Ok(Some(NextRequest::Render(*r))),
            ClientMessage::TiltCurvesRequest(r) => {
                return Ok(Some(NextRequest::TiltCurves(*r)));
            }
            ClientMessage::FinalImageRequest(r) => {
                return Ok(Some(NextRequest::FinalImage(*r)));
            }
            ClientMessage::BatchRenderRequest(r) => return Ok(Some(NextRequest::Batch(*r))),
            ClientMessage::Job(request) => return Ok(Some(NextRequest::Job(request))),
            ClientMessage::LivePassRequest(request) => {
                return Ok(Some(NextRequest::LivePass(request)));
            }
            ClientMessage::BeamTileRequest(request) => {
                return Ok(Some(NextRequest::BeamTiles(request)));
            }
            ClientMessage::Ping { nonce } => write_pong(stream, nonce)?,
            // The zones of the request that follows: kept for it.
            #[cfg(feature = "worker")]
            ClientMessage::ZoningPayload(payload) => crate::serve::zoning::stash(*payload),
            // An asset nobody asked for: consume its payload frame.
            ClientMessage::Asset(header) => assets::discard_asset(stream, &header)?,
            // A stray contribution (e.g. arriving after this request's own DONE):
            // consume its payload frame to stay in sync.
            ClientMessage::Contribution(header) => {
                pleochroma_net::messages::discard_contribution_payload(stream, &header)?;
            }
            other @ (ClientMessage::Cancel(_) | ClientMessage::Library(_)) => {
                handle_non_render_message(stream, &other, db)?;
            }
        }
    }
}

#[cfg(all(test, feature = "worker"))]
mod zoning_tests {
    use super::*;
    use crate::serve::zoning::{ConnectionGuard, stash};
    use pleochroma_net::messages::{BatchReply, ZoningPayload};

    const fn batch(request_id: u32) -> NextRequest {
        NextRequest::Batch(BatchRenderRequest {
            request_id,
            reply: BatchReply::FinalPng,
            items: Vec::new(),
        })
    }

    fn render(request_id: u32) -> NextRequest {
        NextRequest::Render(RenderRequest {
            request_id,
            scene: crate::serve::tests::fixtures::tiny_scene(),
            first_sample: 0,
            samples: 1,
            stream: crate::serve::tests::fixtures::final_only(0),
            intent: pleochroma_net::messages::RequestIntent::Batch,
        })
    }

    /// A payload that does not fit the request's scene is answered with `VALIDATION_FAILED`
    /// naming the request, and its stash slot is consumed.
    #[test]
    fn an_unfitting_payload_on_a_coordinator_is_refused_loudly() {
        let _guard = ConnectionGuard::new();
        // No materials at all: `ZoningPayloadError::Empty`.
        stash(ZoningPayload {
            request_id: 12,
            materials: Vec::new(),
        });
        let mut out = Vec::new();
        assert!(!attach_zoning_on_coordinator(&mut out, &mut render(12)).unwrap());
        let (event, _) =
            pleochroma_net::messages::read_stream_event(&mut std::io::Cursor::new(out)).unwrap();
        let StreamEvent::Error(error) = event else {
            panic!("expected an error, got {event:?}");
        };
        assert_eq!(error.code, VALIDATION_FAILED_CODE);
        assert_eq!(error.request_id, Some(12));
        assert!(error.message.contains("request 12"), "{}", error.message);
        // The payload was consumed: asking again finds nothing and serves the request.
        let mut again = Vec::new();
        assert!(attach_zoning_on_coordinator(&mut again, &mut render(12)).unwrap());
        assert_eq!(again.len(), 0);
    }

    /// The zones the viewer sent are installed on the scene the coordinator then serves;
    /// nothing is written.
    ///
    /// The scene is per-millimetre, as every zoned scene the app sends is (applying a pattern
    /// switches the material to `PerMm`, zone lengths being millimetres): the fixture's
    /// Diamond is a model-unit material, which the payload check refuses on purpose.
    #[test]
    fn a_coordinator_attaches_the_zones_to_the_scene_it_serves() {
        let _guard = ConnectionGuard::new();
        let mut per_mm = crate::serve::tests::fixtures::tiny_scene();
        per_mm.material.absorption_unit = pleochroma::optics::materials::AbsorptionUnit::PerMm;
        let mut zoned = per_mm.clone();
        zoned.material =
            zoned
                .material
                .with_zoning(pleochroma::optics::zoning::ZonedAbsorption::new(
                    pleochroma::optics::zoning::ZoneAbsorption::per_mm(
                        pleochroma::optics::absorption::AbsorptionTensor::isotropic(vec![]),
                    ),
                ));
        stash(ZoningPayload::for_scene(5, &zoned).unwrap());
        let mut next = render(5);
        if let NextRequest::Render(request) = &mut next {
            request.scene = per_mm;
        }
        let mut out = Vec::new();
        assert!(attach_zoning_on_coordinator(&mut out, &mut next).unwrap());
        assert_eq!(out, Vec::<u8>::new());
        let NextRequest::Render(request) = next else {
            unreachable!("built as a render request");
        };
        assert_eq!(request.scene.material.zoning, zoned.material.zoning);
    }

    /// A request without a payload, and a batch (served on the own lane, which attaches its
    /// own zones), pass untouched.
    #[test]
    fn a_request_without_a_payload_passes_a_coordinator_untouched() {
        let _guard = ConnectionGuard::new();
        let mut out = Vec::new();
        assert!(attach_zoning_on_coordinator(&mut out, &mut render(3)).unwrap());
        stash(ZoningPayload {
            request_id: 4,
            materials: Vec::new(),
        });
        assert!(attach_zoning_on_coordinator(&mut out, &mut batch(4)).unwrap());
        assert_eq!(out.len(), 0);
    }
}

#[cfg(all(test, not(feature = "adaptive-sampling")))]
mod live_pass_refusal_tests {
    use super::*;
    use crate::serve::zoning::{ConnectionGuard, stash, take};
    use pleochroma_net::messages::ZoningPayload;

    /// A build without the `adaptive-sampling` feature refuses live passes.
    ///
    /// The reply to a `LivePassRequest` is `UNSUPPORTED_REQUEST` (code 5 on the wire) carrying the
    /// request id and naming the missing feature, exactly one frame is written, and the zoning
    /// payload sent ahead for the request is dropped.
    #[test]
    fn a_build_without_adaptive_sampling_refuses_a_live_pass_request_as_unsupported() {
        let _guard = ConnectionGuard::new();
        stash(ZoningPayload {
            request_id: 51,
            materials: Vec::new(),
        });
        let mut out = Vec::new();
        refuse_live_pass_unbuilt(&mut out, 51).unwrap();
        let mut frames = std::io::Cursor::new(out);
        let (event, payload) = pleochroma_net::messages::read_stream_event(&mut frames).unwrap();
        let StreamEvent::Error(error) = event else {
            panic!("expected an error, got {event:?}");
        };
        assert_eq!(error.code, error_codes::UNSUPPORTED_REQUEST);
        assert_eq!(error.code, 5, "UNSUPPORTED_REQUEST keeps its wire value");
        assert_eq!(error.request_id, Some(51));
        assert!(
            error.message.contains("adaptive-sampling"),
            "{}",
            error.message
        );
        assert!(error.message.contains("51"), "{}", error.message);
        assert!(payload.is_none());
        assert_eq!(
            usize::try_from(frames.position()).unwrap(),
            frames.get_ref().len(),
            "exactly one frame was written"
        );
        assert!(take(51).is_none(), "the zoning payload is dropped");
    }
}

#[cfg(all(test, not(feature = "beam")))]
mod beam_refusal_tests {
    use super::*;
    use crate::serve::zoning::{ConnectionGuard, stash, take};
    use pleochroma_net::messages::ZoningPayload;

    /// A build without `beam` refuses beam tiles.
    ///
    /// The reply to a `BeamTileRequest` is `UNSUPPORTED_REQUEST` (code 5 on the wire) carrying
    /// the request id and naming the missing feature, exactly one frame is written, and the
    /// zoning payload sent ahead for the request is dropped.
    #[test]
    fn a_build_without_beam_refuses_a_beam_tile_request_as_unsupported() {
        let _guard = ConnectionGuard::new();
        stash(ZoningPayload {
            request_id: 41,
            materials: Vec::new(),
        });
        let mut out = Vec::new();
        refuse_beam_tiles_unbuilt(&mut out, 41).unwrap();
        let mut frames = std::io::Cursor::new(out);
        let (event, payload) = pleochroma_net::messages::read_stream_event(&mut frames).unwrap();
        let StreamEvent::Error(error) = event else {
            panic!("expected an error, got {event:?}");
        };
        assert_eq!(error.code, error_codes::UNSUPPORTED_REQUEST);
        assert_eq!(error.code, 5, "UNSUPPORTED_REQUEST keeps its wire value");
        assert_eq!(error.request_id, Some(41));
        assert!(error.message.contains("beam"), "{}", error.message);
        assert!(error.message.contains("41"), "{}", error.message);
        assert!(payload.is_none());
        assert_eq!(
            usize::try_from(frames.position()).unwrap(),
            frames.get_ref().len(),
            "exactly one frame was written"
        );
        assert!(take(41).is_none(), "the zoning payload is dropped");
    }
}
