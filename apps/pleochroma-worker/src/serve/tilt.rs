//! `TILT_CURVES`: a single-request, single-response analytic tilt-performance sweep.
//!
//! Contrast with `RENDER`'s progressive stream (`crate::stream_emit`). The wire types are
//! documented in `pleochroma_net::messages::tilt`.
//!
//! [`handle_tilt_curves_request`] is the one entry point, called from
//! `super::connection`'s worker-build request loop where `RenderRequest` instead calls
//! `crate::stream_emit::run_stream`.
//!
//! # Runs inline, unlike `RenderRequest`
//!
//! `stream_emit::run_stream` runs tracing on its own thread so a slow `write()` never
//! stalls sample production. A `TiltCurvesRequest` writes one ~8,688-byte reply at the
//! end, so there is nothing to stall and a thread split would buy nothing. The sweep runs
//! single-threaded (about 340 ms per axis; see the call site).
//!
//! # Cancellation: 4 checkpoints, ~340ms worst-case latency
//!
//! [`poll_for_cancel`] runs once before each axis, not in a continuous loop, because there
//! is no progress to report between axes. A `CANCEL` arriving mid-axis is seen at the next
//! checkpoint, so latency is at most one axis (~340 ms).
//!
//! # `ComputeMode` plays no role here
//!
//! `evaluate_full_axis_profile_at_azimuth` is a CPU-only analytic sweep with no GPU
//! kernel; `--only-gpu`/`--only-cpu` have nothing to change, so
//! [`handle_tilt_curves_request`] deliberately takes no `ComputeMode` parameter.
//!
//! # Frosting
//!
//! The sweep honours the scene's facet finishes: `girdle_frosted` and `frosted_planes` are
//! resolved by `crate::render_core::resolve_facet_finishes`, the function a `RENDER` of the
//! same scene uses, and handed to the sweep as a `FinishedStone`. A frosted design's curves
//! therefore differ from its polished ones; a scene with neither field set sweeps exactly as
//! it always did.
//!
//! # No pipelining
//!
//! Only one `TiltCurvesRequest` may be outstanding per connection (unlike
//! `RenderRequest`, which `run_stream` accepts pipelined as an implicit cancel).
//! [`poll_for_cancel`] logs and drops anything else pipelined mid-computation. A client
//! wanting concurrent tilt-curve requests opens more than one connection.

use crate::{stream_emit::TimeoutRead, validate};
use pleochroma_net::messages::{
    AxisTiltCurves, ErrorMsg, NetError, TILT_CURVE_AXIS_COUNT, TiltCurvesRequest,
    TiltCurvesResponse, TiltCurvesResult,
};
use std::{
    io::{Read, Write},
    time::Duration,
};

/// How long [`poll_for_cancel`] waits, per checkpoint, for a `CANCEL` already in the
/// socket's receive buffer.
///
/// Short and non-looping: it runs once per checkpoint, so it only needs to catch what
/// arrived during the ~340 ms since the previous one.
const CANCEL_POLL_TIMEOUT: Duration = Duration::from_millis(5);

/// What one [`poll_for_cancel`] check found.
pub enum CancelPoll {
    /// Nothing pending within the poll window.
    Pending,
    /// A `CANCEL` for this exact `request_id`.
    Cancelled,
    /// The peer closed the connection.
    Closed,
}

/// Checks whether a `CANCEL` for `request_id` is already sitting on `stream`'s socket
/// buffer, without blocking longer than [`CANCEL_POLL_TIMEOUT`].
///
/// Mirrors `crate::stream_emit`'s private `poll_for_client_message`. The length prefix's
/// first byte is read with a raw `read()`, not `read_exact`, which leaves the bytes read on
/// an error unspecified, so a timeout can only land before a message starts. Once a byte
/// is seen, the rest is read under `FRAME_REMAINDER_TIMEOUT` (bounded by
/// `MAX_CONTROL_FRAME_LEN`), where a timeout is a protocol error. The first byte's timeout
/// is classified by [`crate::stream_emit::is_stream_timeout`], since a TLS stream can
/// report a timeout as `WriteZero`.
///
/// Anything besides a matching `CANCEL` (a stale `CANCEL`, or any other pipelined
/// `ClientMessage`) is logged and dropped as [`CancelPoll::Pending`]: `TILT_CURVES`
/// supports no pipelining. A dropped `ASSET` or `CONTRIBUTION` has its payload frame
/// consumed too, so the stream stays in sync.
///
/// # Errors
///
/// Returns [`NetError`] for a transport-level failure other than the tolerated timeout.
pub fn poll_for_cancel<S: Read + TimeoutRead>(
    stream: &mut S,
    request_id: u32,
) -> Result<CancelPoll, NetError> {
    stream
        .set_read_timeout(Some(CANCEL_POLL_TIMEOUT))
        .map_err(|e| NetError::Framing(pleochroma_net::framing::FramingError::Io(e)))?;

    let mut len_bytes = [0u8; pleochroma_net::framing::LEN_PREFIX_BYTES];
    let n = match stream.read(&mut len_bytes) {
        Ok(0) => return Ok(CancelPoll::Closed),
        Ok(n) => n,
        Err(e) if crate::stream_emit::is_stream_timeout(&e) => {
            return Ok(CancelPoll::Pending);
        }
        Err(e) => {
            return Err(NetError::Framing(
                pleochroma_net::framing::FramingError::Io(e),
            ));
        }
    };

    // At least one byte of a message has arrived -- commit to reading the rest of it
    // under a bounded timeout, not `None` (see this function's own doc comment).
    stream
        .set_read_timeout(Some(crate::stream_emit::FRAME_REMAINDER_TIMEOUT))
        .map_err(|e| NetError::Framing(pleochroma_net::framing::FramingError::Io(e)))?;
    let payload = pleochroma_net::framing::read_frame_continuing(
        stream,
        &len_bytes[..n],
        pleochroma_net::framing::MAX_CONTROL_FRAME_LEN,
    )?;

    let msg: pleochroma_net::messages::ClientMessage =
        pleochroma_net::messages::decode_control_frame(&payload)?;
    Ok(match msg {
        pleochroma_net::messages::ClientMessage::Cancel(c) if c.request_id == request_id => {
            CancelPoll::Cancelled
        }
        // An unrequested payload-carrying message: its payload frame follows and must be
        // consumed, or it would be misread as the next message.
        pleochroma_net::messages::ClientMessage::Asset(header) => {
            crate::assets::discard_asset(stream, &header)?;
            CancelPoll::Pending
        }
        pleochroma_net::messages::ClientMessage::Contribution(header) => {
            pleochroma_net::messages::discard_contribution_payload(stream, &header)?;
            CancelPoll::Pending
        }
        // Not served mid-computation; a `PutAsset`'s payload frame is still consumed.
        pleochroma_net::messages::ClientMessage::Job(job) => {
            crate::jobs::ignore_in_flight(stream, &job)?;
            CancelPoll::Pending
        }
        // Zones of a request pipelined behind this one are kept for it; the sweep ignores zones.
        #[cfg(feature = "worker")]
        pleochroma_net::messages::ClientMessage::ZoningPayload(payload) => {
            crate::serve::zoning::stash(*payload);
            CancelPoll::Pending
        }
        other => {
            tracing::debug!(
                "ignoring a message pipelined while request_id={request_id}'s tilt-curves \
                 computation was in flight (TILT_CURVES has no mid-request pipelining in this \
                 phase): {other:?}"
            );
            CancelPoll::Pending
        }
    })
}

/// Handles one `TiltCurvesRequest`: computes all [`TILT_CURVE_AXIS_COUNT`] axes and writes
/// one [`TiltCurvesResponse`], never a stream.
///
/// See the module docs for the inline and cancellation designs.
///
/// Validates with [`validate::validate_scene`], as `RenderRequest` does. A rejected scene
/// gets [`TiltCurvesResponse::Error`] with `super::connection::VALIDATION_FAILED_CODE`.
///
/// Each axis runs inside `catch_unwind`, so a panic on a validation-passing scene is
/// reported as [`TiltCurvesResponse::Error`] with `super::connection::TRACE_PANIC_CODE`
/// instead of dropping the connection.
///
/// Restores `stream`'s read timeout to `None` (blocking) on every return path.
/// [`poll_for_cancel`] leaves a short timeout applied, and the connection's next
/// `read_message` expects a blocking read. This wrapper does the restore once, around
/// [`handle_tilt_curves_request_inner`], which returns early from several places.
///
/// # Errors
///
/// Returns [`NetError`] for a transport-level failure while polling for `CANCEL` or
/// writing the reply. A validation failure or caught panic is NOT an error return: both
/// are reported as `TiltCurvesResponse::Error` and this returns `Ok(())`.
///
/// # Panics
///
/// Does not panic. The array conversion after the loop is unreachable, because exactly
/// `TILT_CURVE_AXIS_COUNT` axes are pushed; a debug assertion ties that count to the
/// azimuth table.
pub fn handle_tilt_curves_request<S: Read + Write + TimeoutRead>(
    stream: &mut S,
    request: &TiltCurvesRequest,
) -> Result<(), NetError> {
    let result = handle_tilt_curves_request_inner(stream, request);
    // Best-effort, like `serve::apply_handshake_timeout`: a failed `set_read_timeout`
    // call is logged neither here nor there -- the inner result (the meaningful outcome
    // either way) is returned regardless, and a socket that can't have its timeout
    // changed will surface that on the connection's very next read.
    let _ = stream.set_read_timeout(None);
    result
}

/// The `TILT_CURVES` work that [`handle_tilt_curves_request`] wraps.
///
/// The timeout restore lives in the wrapper, not here.
fn handle_tilt_curves_request_inner<S: Read + Write + TimeoutRead>(
    stream: &mut S,
    request: &TiltCurvesRequest,
) -> Result<(), NetError> {
    if let Err(message) = validate::validate_scene(&request.scene) {
        return pleochroma_net::messages::write_message(
            stream,
            &TiltCurvesResponse::Error(ErrorMsg {
                code: super::connection::VALIDATION_FAILED_CODE,
                message,
                request_id: Some(request.request_id),
            }),
        );
    }

    debug_assert_eq!(
        pleochroma::color::metrics::PROFILE_AZIMUTHS_DEG.len(),
        TILT_CURVE_AXIS_COUNT,
        "pleochroma's PROFILE_AZIMUTHS_DEG and pleochroma-net's independently-pinned \
         TILT_CURVE_AXIS_COUNT must agree -- see pleochroma_net::messages::tilt's module doc \
         comment for why the two are pinned separately rather than sharing one constant"
    );

    let mut axes: Vec<AxisTiltCurves> = Vec::with_capacity(TILT_CURVE_AXIS_COUNT);
    let scene = &request.scene;
    // The curves are scored under the scene's own lighting preset and light pose, the
    // radiance the same scene renders with (the stone's optics never see the exposure).
    let environment =
        scene
            .lighting_preset
            .studio(scene.exposure, scene.light_yaw, scene.light_pitch);
    // The finishes the render of this scene would use (girdle toggle plus authored frosting;
    // empty for a polished design), so a frosted design's curves are the frosted stone's.
    let finishes = crate::render_core::resolve_facet_finishes(scene);

    for (axis_index, &azimuth_deg) in pleochroma::color::metrics::PROFILE_AZIMUTHS_DEG
        .iter()
        .enumerate()
    {
        match poll_for_cancel(stream, request.request_id)? {
            CancelPoll::Cancelled => {
                tracing::debug!(
                    "request_id={}: CANCEL honored after {} of {TILT_CURVE_AXIS_COUNT} axes",
                    request.request_id,
                    axes.len(),
                );
                return pleochroma_net::messages::write_message(
                    stream,
                    &TiltCurvesResponse::Cancelled {
                        request_id: request.request_id,
                    },
                );
            }
            CancelPoll::Closed => return Ok(()),
            CancelPoll::Pending => {}
        }

        let axis_result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            // `Some(1)`: this request holds no `render_core` permits, so a wider fan-out
            // could oversubscribe the process-wide pool. Threading needs a
            // permit-acquiring helper in `render_core`.
            pleochroma::color::metrics::evaluate_full_axis_profile_at_azimuth_finished_threaded(
                pleochroma::color::metrics::FinishedStone::new(
                    pleochroma::geometry::StoneGeometry {
                        planes: &scene.planes,
                        tools: &scene.tools,
                    },
                    &finishes,
                ),
                &scene.material,
                azimuth_deg,
                environment,
                Some(1),
            )
        }));

        if let Ok((brilliance_pct, extinction_pct, windowing_pct)) = axis_result {
            axes.push(AxisTiltCurves::from_arrays(
                brilliance_pct,
                extinction_pct,
                windowing_pct,
            ));
        } else {
            tracing::warn!(
                "tilt-curves computation panicked for a request that passed validation \
                 (request_id={}, axis_index={axis_index}, azimuth_deg={azimuth_deg})",
                request.request_id,
            );
            return pleochroma_net::messages::write_message(
                stream,
                &TiltCurvesResponse::Error(ErrorMsg {
                    code: super::connection::TRACE_PANIC_CODE,
                    message: "internal error while computing tilt-performance curves".to_string(),
                    request_id: Some(request.request_id),
                }),
            );
        }
    }

    let axes: [AxisTiltCurves; TILT_CURVE_AXIS_COUNT] = axes.try_into().unwrap_or_else(|v: Vec<_>| {
        unreachable!(
            "exactly TILT_CURVE_AXIS_COUNT={TILT_CURVE_AXIS_COUNT} axes were pushed above, got {}",
            v.len()
        )
    });

    pleochroma_net::messages::write_message(
        stream,
        &TiltCurvesResponse::Curves(Box::new(TiltCurvesResult {
            request_id: request.request_id,
            axes,
        })),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma::{
        geometry::cuts::StandardGemCuts,
        optics::{materials::GemMaterial, raytracer::LightingPreset},
    };
    use pleochroma_net::{SceneState, messages::write_message};
    use std::io::Cursor;

    /// A minimal `Read + Write + TimeoutRead` double over two in-memory buffers,
    /// mirroring `serve::tests::fixtures::DuplexHalf` (kept separate to avoid reaching
    /// across that module boundary). `closed` distinguishes "peer hung up" (`Ok(0)`
    /// even with a timeout active) from "nothing pending yet" (`WouldBlock`/`TimedOut`),
    /// since a bare `timeout_active` flag alone can't tell them apart.
    struct DuplexHalf {
        in_: Cursor<Vec<u8>>,
        out: Vec<u8>,
        timeout_active: bool,
        closed: bool,
    }

    impl DuplexHalf {
        const fn new(input: Vec<u8>) -> Self {
            Self {
                in_: Cursor::new(input),
                out: Vec::new(),
                timeout_active: false,
                closed: false,
            }
        }
    }

    impl Read for DuplexHalf {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let n = self.in_.read(buf)?;
            if n == 0 && !self.closed && self.timeout_active {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WouldBlock,
                    "DuplexHalf: no more scripted input (yet)",
                ));
            }
            Ok(n)
        }
    }

    impl Write for DuplexHalf {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.out.write(buf)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl TimeoutRead for DuplexHalf {
        fn set_read_timeout(&mut self, duration: Option<Duration>) -> std::io::Result<()> {
            self.timeout_active = duration.is_some();
            Ok(())
        }
    }

    fn valid_scene() -> SceneState {
        SceneState {
            engine: pleochroma::renderer::engine::EngineSpec::default(),
            width: 4,
            height: 4,
            yaw: 0.4,
            camera_roll: 0.0,
            stone_shadow: false,
            tray_reflection: 0.0,
            hdr_orientation: pleochroma::renderer::env_map::HdrOrientation::default(),
            estimator: pleochroma::optics::raytracer::Estimator::Branching,
            pitch: 0.3,
            distance: 3.0,
            light_yaw: 0.85,
            light_pitch: 0.95,
            exposure: 1.0,
            max_bounces: 4,
            lighting_preset: LightingPreset::Daylight,
            material: GemMaterial::diamond(),
            planes: StandardGemCuts::standard_round_brilliant(),
            girdle_frosted: false,
            backdrop: 0.0,
            environment: pleochroma_net::scene::SceneEnvironment::Studio,
            surface_glare: 1.0,
            tools: Vec::new(),
            fluorescence: pleochroma::optics::fluorescence::Fluorescence::default(),
            head_shadow_deg: 16.0,
            frosted_planes: Vec::new(),
            effects: pleochroma_net::scene::effects::SceneEffects::default(),
        }
    }

    fn read_response(out: &[u8]) -> TiltCurvesResponse {
        let mut cursor = Cursor::new(out.to_vec());
        pleochroma_net::messages::read_message(&mut cursor).unwrap()
    }

    #[test]
    fn rejects_an_invalid_scene_with_the_shared_validation_failed_code() {
        let mut scene = valid_scene();
        scene.planes.clear(); // fails validate_scene: planes must not be empty
        let request = TiltCurvesRequest {
            request_id: 1,
            scene,
        };
        let mut duplex = DuplexHalf::new(Vec::new());

        handle_tilt_curves_request(&mut duplex, &request).unwrap();

        let response = read_response(&duplex.out);
        match response {
            TiltCurvesResponse::Error(ErrorMsg { code, message, .. }) => {
                assert_eq!(code, super::super::connection::VALIDATION_FAILED_CODE);
                assert!(message.contains("planes"), "{message}");
            }
            other => panic!("expected TiltCurvesResponse::Error, got {other:?}"),
        }
    }

    #[test]
    fn a_matching_cancel_already_on_the_wire_is_honored_before_any_axis_computes() {
        let request = TiltCurvesRequest {
            request_id: 5,
            scene: valid_scene(),
        };
        // Already sitting in the input buffer; the first checkpoint must catch it.
        let mut cancel_bytes = Vec::new();
        write_message(
            &mut cancel_bytes,
            &pleochroma_net::messages::ClientMessage::Cancel(pleochroma_net::messages::Cancel {
                request_id: 5,
            }),
        )
        .unwrap();
        let mut duplex = DuplexHalf::new(cancel_bytes);

        handle_tilt_curves_request(&mut duplex, &request).unwrap();

        let response = read_response(&duplex.out);
        assert_eq!(response, TiltCurvesResponse::Cancelled { request_id: 5 });
    }

    #[test]
    fn a_cancel_for_a_different_request_id_is_ignored_and_the_request_completes() {
        let request = TiltCurvesRequest {
            request_id: 5,
            scene: valid_scene(),
        };
        let mut stale_cancel_bytes = Vec::new();
        write_message(
            &mut stale_cancel_bytes,
            &pleochroma_net::messages::ClientMessage::Cancel(pleochroma_net::messages::Cancel {
                request_id: 999,
            }),
        )
        .unwrap();
        let mut duplex = DuplexHalf::new(stale_cancel_bytes);

        handle_tilt_curves_request(&mut duplex, &request).unwrap();

        let response = read_response(&duplex.out);
        match response {
            TiltCurvesResponse::Curves(result) => {
                assert_eq!(result.request_id, 5);
                assert_eq!(result.axes.len(), TILT_CURVE_AXIS_COUNT);
            }
            other => panic!("expected TiltCurvesResponse::Curves, got {other:?}"),
        }
    }

    #[test]
    fn a_closed_connection_writes_nothing() {
        let request = TiltCurvesRequest {
            request_id: 1,
            scene: valid_scene(),
        };
        let mut duplex = DuplexHalf::new(Vec::new());
        duplex.closed = true; // simulates a peer that already hung up

        handle_tilt_curves_request(&mut duplex, &request).unwrap();
        assert!(
            duplex.out.is_empty(),
            "a peer that already closed the connection gets no reply written"
        );
    }

    /// Every return path must restore the socket's read timeout to `None`
    /// (blocking) before returning, since `poll_for_cancel` leaves a short timeout
    /// applied and this connection's next `read_message` call assumes a blocking read.
    /// Covers the three paths [`handle_tilt_curves_request_inner`] can take: a normal
    /// completion (this test), an invalid scene, and an honored `CANCEL`.
    #[test]
    fn read_timeout_is_restored_to_blocking_after_a_normal_completion() {
        let request = TiltCurvesRequest {
            request_id: 9,
            scene: valid_scene(),
        };
        let mut duplex = DuplexHalf::new(Vec::new());

        handle_tilt_curves_request(&mut duplex, &request).unwrap();

        assert!(
            !duplex.timeout_active,
            "the read timeout must be restored to None (blocking) once this request is done"
        );
    }

    #[test]
    fn read_timeout_is_restored_to_blocking_after_a_validation_failure() {
        let mut scene = valid_scene();
        scene.planes.clear();
        let request = TiltCurvesRequest {
            request_id: 10,
            scene,
        };
        let mut duplex = DuplexHalf::new(Vec::new());

        handle_tilt_curves_request(&mut duplex, &request).unwrap();

        assert!(!duplex.timeout_active);
    }

    #[test]
    fn read_timeout_is_restored_to_blocking_after_an_honored_cancel() {
        let request = TiltCurvesRequest {
            request_id: 11,
            scene: valid_scene(),
        };
        let mut cancel_bytes = Vec::new();
        write_message(
            &mut cancel_bytes,
            &pleochroma_net::messages::ClientMessage::Cancel(pleochroma_net::messages::Cancel {
                request_id: 11,
            }),
        )
        .unwrap();
        let mut duplex = DuplexHalf::new(cancel_bytes);

        handle_tilt_curves_request(&mut duplex, &request).unwrap();

        assert!(!duplex.timeout_active);
    }

    /// The curves a worker computes for `scene`.
    fn curves_of(scene: SceneState) -> Box<TiltCurvesResult> {
        let request = TiltCurvesRequest {
            request_id: 3,
            scene,
        };
        let mut duplex = DuplexHalf::new(Vec::new());
        handle_tilt_curves_request(&mut duplex, &request).unwrap();
        match read_response(&duplex.out) {
            TiltCurvesResponse::Curves(result) => result,
            other => panic!("expected TiltCurvesResponse::Curves, got {other:?}"),
        }
    }

    /// A polished scene is swept exactly as the plain-geometry sweep does (an empty finish
    /// list changes nothing), and the frosting of the scene -- the girdle toggle or authored
    /// frosted planes -- reaches the sweep: the curves of a frosted scene differ.
    #[test]
    fn a_frosted_scene_gets_different_curves_than_the_polished_one() {
        let polished_scene = valid_scene();
        let scene = &polished_scene;
        let (brilliance, extinction, windowing) =
            pleochroma::color::metrics::evaluate_full_axis_profile_at_azimuth_geom_threaded(
                pleochroma::geometry::StoneGeometry::planes_only(&scene.planes),
                &scene.material,
                pleochroma::color::metrics::PROFILE_AZIMUTHS_DEG[0],
                scene
                    .lighting_preset
                    .studio(scene.exposure, scene.light_yaw, scene.light_pitch),
                Some(1),
            );
        let polished = curves_of(valid_scene());
        assert_eq!(
            polished.axes[0],
            AxisTiltCurves::from_arrays(brilliance, extinction, windowing),
            "a polished scene is the plain sweep"
        );

        let mut girdle = valid_scene();
        girdle.girdle_frosted = true;
        assert_ne!(
            curves_of(girdle).axes,
            polished.axes,
            "a frosted girdle changes the curves"
        );

        let mut authored = valid_scene();
        authored.frosted_planes = (0..u32::try_from(authored.planes.len()).unwrap()).collect();
        assert_ne!(
            curves_of(authored).axes,
            polished.axes,
            "authored frosting changes the curves"
        );
    }

    /// Pins that `poll_for_cancel` tolerates a `WriteZero` (which a real TLS stream can
    /// surface for a socket timeout even from a read) exactly like `WouldBlock`/
    /// `TimedOut`, rather than tearing the connection down.
    struct WriteZeroOnFirstRead;

    impl Read for WriteZeroOnFirstRead {
        fn read(&mut self, _buf: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::new(
                std::io::ErrorKind::WriteZero,
                "scripted: TLS complete_io() swallowed an inner write timeout",
            ))
        }
    }

    impl TimeoutRead for WriteZeroOnFirstRead {
        fn set_read_timeout(&mut self, _duration: Option<Duration>) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn poll_for_cancel_tolerates_a_write_zero_on_the_first_byte() {
        let mut stream = WriteZeroOnFirstRead;
        let result = poll_for_cancel(&mut stream, 1);
        assert!(
            matches!(result, Ok(CancelPoll::Pending)),
            "a WriteZero on the very first read must be tolerated as \"nothing pending \
             yet\", not a fatal protocol error"
        );
    }
}
