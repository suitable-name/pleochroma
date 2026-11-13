//! Client of the remote live passes, built around [`LivePassClient`].
//!
//! [`LivePassClient`] sends one chunk of an adaptive pass (a sample range for a set of live
//! tiles) to a remote worker and returns the pass total and the odd-sample total of every
//! live pixel.
//!
//! # One connection, one request at a time
//!
//! The client owns one mutual-TLS connection (opened by [`LivePassClient::connect`]) for a
//! whole render. A pass is a request and one reply: `PROGRESS` heartbeats while the worker
//! traces, then `LIVE_PASS` with a raw payload frame, `ERROR`, or `DONE { cancelled: true }`
//! after a `CANCEL`. [`LivePassClient::trace`] reads the socket itself, in short slices, so a
//! raised `cancel` goes out at once. A silent worker is given up on after the deadlines
//! (30 s for the first sign of life, 8 s after, 10 s for a cancel to be confirmed).
//!
//! # Limits
//!
//! A request names at most `MAX_LIVE_PASS_PIXELS` live pixels. A bigger share is split into
//! several requests on the same connection, in tile order, and the replies are joined.
//!
//! # A failed pass is not a failed render
//!
//! [`LivePassFailure`] says only that the remote could not trace these tiles. The caller
//! traces them on its own lane and stops offering the remote any (see `worker::core::adaptive`).

use super::{
    capability::RemoteCapability,
    dispatch::{CANCEL_WAIT_TIMEOUT, liveness_deadline},
};
use crate::bridge::remote::remote_render::{RemoteStream, connect_and_handshake};
use glam::Vec3;
use pleochroma::renderer::adaptive::LiveTiles;
use pleochroma_net::{
    SceneState,
    framing::{self, LEN_PREFIX_BYTES, MAX_CONTROL_FRAME_LEN, MAX_FRAME_LEN},
    messages::{
        Cancel, ClientMessage, LivePassHeader, LivePassRequest, LiveTileSet, MAX_LIVE_PASS_PIXELS,
        StreamEvent, ZoningPayload, decode_control_frame, decode_live_sums,
        request_id_with_path_cache, write_message, write_zoning_payload,
    },
};
use std::{
    fmt,
    io::{Read, Write},
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

/// How often the reply loop looks at `cancel` between read attempts.
pub(super) const POLL: Duration = Duration::from_millis(100);

/// How long the rest of a frame may take once its first byte has arrived (the bound the
/// one-shot connection uses): a frame already in flight never finishing is a broken connection.
const FRAME_REMAINDER_TIMEOUT: Duration = Duration::from_secs(5);

/// A connection the live-pass client can poll: `Read + Write` plus a read timeout.
pub trait LiveStream: Read + Write {
    /// Sets the read timeout (`None` blocks).
    ///
    /// # Errors
    ///
    /// Whatever the transport's own call returns.
    fn set_read_poll(&mut self, timeout: Option<Duration>) -> std::io::Result<()>;
}

impl LiveStream for RemoteStream {
    fn set_read_poll(&mut self, timeout: Option<Duration>) -> std::io::Result<()> {
        self.sock.set_read_timeout(timeout)
    }
}

/// Why a live pass produced nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LivePassFailure {
    /// The worker refused or failed the request (`ERROR`); the connection is still in sync.
    Refused {
        /// The worker's `ErrorMsg::code`.
        code: u32,
        /// The worker's message.
        message: String,
    },
    /// The connection can no longer be used: it closed, went silent, sent an undecodable or
    /// inconsistent reply, or a write failed.
    Broken(String),
    /// `cancel` was raised and the worker confirmed it (`DONE { cancelled: true }`).
    Cancelled,
}

impl fmt::Display for LivePassFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused { code, message } => {
                write!(
                    f,
                    "the worker refused the live pass (code {code}): {message}"
                )
            }
            Self::Broken(why) => write!(f, "{why}"),
            Self::Cancelled => write!(f, "the live pass was cancelled"),
        }
    }
}

/// `true` for the errors a read timeout produces (a TLS stream reports it as `WriteZero`).
fn is_timeout(error: &std::io::Error) -> bool {
    matches!(
        error.kind(),
        std::io::ErrorKind::WouldBlock
            | std::io::ErrorKind::TimedOut
            | std::io::ErrorKind::WriteZero
    )
}

/// One `StreamEvent` with the raw payload frame that followed it, if any.
pub(super) type EventWithPayload = (StreamEvent, Option<Vec<u8>>);

/// Reads one `StreamEvent` and its payload frame, waiting at most `poll` for the first byte.
///
/// `Ok(None)` means nothing arrived in time.
pub(super) fn read_event<S: LiveStream>(
    stream: &mut S,
    poll: Duration,
) -> Result<Option<EventWithPayload>, String> {
    stream
        .set_read_poll(Some(poll))
        .map_err(|e| format!("could not arm the read deadline: {e}"))?;
    let mut prefix = [0_u8; LEN_PREFIX_BYTES];
    let read = match stream.read(&mut prefix) {
        Ok(0) => return Err("the worker closed the connection".to_owned()),
        Ok(read) => read,
        Err(e) if is_timeout(&e) => return Ok(None),
        Err(e) => return Err(format!("reading the worker's reply failed: {e}")),
    };
    stream
        .set_read_poll(Some(FRAME_REMAINDER_TIMEOUT))
        .map_err(|e| format!("could not arm the read deadline: {e}"))?;
    let header = framing::read_frame_continuing(stream, &prefix[..read], MAX_CONTROL_FRAME_LEN)
        .map_err(|e| format!("reading the worker's reply failed: {e}"))?;
    let event: StreamEvent = decode_control_frame(&header)
        .map_err(|e| format!("the worker sent an undecodable reply: {e}"))?;
    let payload = match event.payload_len() {
        Some(expected) => {
            let bytes = framing::read_frame_bounded(stream, expected.min(MAX_FRAME_LEN))
                .map_err(|e| format!("reading the worker's payload failed: {e}"))?;
            if u32::try_from(bytes.len()) != Ok(expected) {
                return Err(format!(
                    "the worker declared a payload of {expected} bytes and sent {}",
                    bytes.len()
                ));
            }
            Some(bytes)
        }
        None => None,
    };
    Ok(Some((event, payload)))
}

/// Splits `live` into runs of consecutive tiles of at most `max_pixels` pixels each. A tile is
/// never split, so a tile over the limit forms a run of its own.
pub(super) fn split_by_pixels(live: &LiveTiles, max_pixels: u64) -> Vec<LiveTiles> {
    let mut chunks = Vec::new();
    let mut current: Vec<u32> = Vec::new();
    let mut pixels = 0_u64;
    let flush = |tiles: &mut Vec<u32>, chunks: &mut Vec<LiveTiles>| {
        if !tiles.is_empty() {
            chunks.push(LiveTiles::from_indices(
                live.width(),
                live.height(),
                live.tile(),
                std::mem::take(tiles),
            ));
        }
    };
    for &tile in live.indices() {
        let (x0, y0, x1, y1) = live.tile_rect(tile);
        let size = u64::from(x1 - x0) * u64::from(y1 - y0);
        if !current.is_empty() && pixels + size > max_pixels {
            flush(&mut current, &mut chunks);
            pixels = 0;
        }
        current.push(tile);
        pixels += size;
    }
    flush(&mut current, &mut chunks);
    chunks
}

/// One connection to a remote worker for the live passes of a render. See the module doc.
pub struct LivePassClient<S: LiveStream = RemoteStream> {
    stream: S,
    /// Whether the worker's `WELCOME` advertised the zoning capability.
    takes_zoning: bool,
    /// The id of the next request; unique on this connection.
    next_request_id: u32,
    /// Ask the worker to shade the stone from a path cache of its own (the flag bit of every
    /// request id, `pleochroma_net::messages::PATH_CACHE_FLAG`).
    path_cache: bool,
}

impl LivePassClient {
    /// Connects to the worker `capability` describes and checks it can render.
    ///
    /// # Errors
    ///
    /// Why the connection or handshake failed, or that the server renders nothing.
    pub fn connect(capability: &RemoteCapability) -> Result<Self, String> {
        let (stream, welcome) =
            connect_and_handshake(&capability.worker).map_err(|e| e.to_string())?;
        if welcome.render.is_none() {
            return Err(
                "the remote serves the design library only: it has no render capacity".to_owned(),
            );
        }
        Ok(Self::with_stream(stream, welcome.zoning))
    }
}

impl<S: LiveStream> LivePassClient<S> {
    /// A client over an already handshaken `stream`. `takes_zoning` is the `WELCOME` zoning bit.
    #[must_use]
    pub const fn with_stream(stream: S, takes_zoning: bool) -> Self {
        Self {
            stream,
            takes_zoning,
            next_request_id: 1,
            path_cache: false,
        }
    }

    /// Sets whether the worker shades from its own path cache: every later request carries
    /// the flag. Only the flag is sent; the worker builds and keeps the cache.
    #[must_use]
    pub const fn with_path_cache(mut self, path_cache: bool) -> Self {
        self.path_cache = path_cache;
        self
    }

    /// Traces the samples `first_sample..first_sample + samples` of `scene` for the pixels of
    /// `live` and returns `(pass total, odd-sample total)` per live pixel. The order is that of
    /// `pleochroma_net::messages::gather_live`: tiles ascending, row-major inside a tile.
    ///
    /// # Errors
    ///
    /// A [`LivePassFailure`]; nothing is returned for a pass that failed part way.
    pub fn trace(
        &mut self,
        scene: &SceneState,
        (first_sample, samples): (u32, u32),
        live: &LiveTiles,
        cancel: &AtomicBool,
    ) -> Result<Vec<(Vec3, Vec3)>, LivePassFailure> {
        let mut pixels = Vec::with_capacity(usize::try_from(live.pixel_count()).unwrap_or(0));
        for chunk in split_by_pixels(live, MAX_LIVE_PASS_PIXELS) {
            pixels.extend(self.request(scene, (first_sample, samples), &chunk, cancel)?);
        }
        Ok(pixels)
    }

    /// One request and its reply.
    fn request(
        &mut self,
        scene: &SceneState,
        (first_sample, samples): (u32, u32),
        live: &LiveTiles,
        cancel: &AtomicBool,
    ) -> Result<Vec<(Vec3, Vec3)>, LivePassFailure> {
        let request_id = request_id_with_path_cache(self.next_request_id, self.path_cache);
        self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
        let zones = ZoningPayload::for_scene(request_id, scene);
        if zones.is_some() && !self.takes_zoning {
            return Err(LivePassFailure::Refused {
                code: pleochroma_net::messages::error_codes::UNSUPPORTED_REQUEST,
                message: "the worker does not take colour zones, so a zoned stone is not sent to \
                          it"
                .to_owned(),
            });
        }
        let broken = |e: pleochroma_net::messages::NetError| {
            LivePassFailure::Broken(format!("sending the live pass failed: {e}"))
        };
        // The zones go first (they are `serde(skip)` in the scene), then the request.
        if let Some(zones) = &zones {
            write_zoning_payload(&mut self.stream, zones).map_err(broken)?;
        }
        write_message(
            &mut self.stream,
            &ClientMessage::LivePassRequest(Box::new(LivePassRequest {
                request_id,
                scene: scene.clone(),
                first_sample,
                samples,
                live: LiveTileSet::from_live(live),
            })),
        )
        .map_err(broken)?;

        let (header, payload) = self.await_reply(request_id, cancel)?;
        let expected_pixels = live.pixel_count();
        if header.first_sample != first_sample
            || header.samples != samples
            || u64::from(header.live_pixels) != expected_pixels
        {
            return Err(LivePassFailure::Broken(format!(
                "the worker answered another pass: samples {}+{} for {} pixels, asked {first_sample}+{samples} for {expected_pixels}",
                header.first_sample, header.samples, header.live_pixels
            )));
        }
        decode_live_sums(&payload, header.live_pixels as usize)
            .map_err(|e| LivePassFailure::Broken(format!("the worker's payload is bad: {e}")))
    }

    /// Waits for the terminal reply of `request_id`, sending `CANCEL` when `cancel` is raised.
    fn await_reply(
        &mut self,
        request_id: u32,
        cancel: &AtomicBool,
    ) -> Result<(LivePassHeader, Vec<u8>), LivePassFailure> {
        let mut last_event = Instant::now();
        let mut seen_event = false;
        let mut cancel_sent_at: Option<Instant> = None;
        loop {
            if cancel_sent_at.is_none() && cancel.load(Ordering::Relaxed) {
                write_message(
                    &mut self.stream,
                    &ClientMessage::Cancel(Cancel { request_id }),
                )
                .map_err(|e| LivePassFailure::Broken(format!("sending CANCEL failed: {e}")))?;
                cancel_sent_at = Some(Instant::now());
            }
            let read = read_event(&mut self.stream, POLL).map_err(LivePassFailure::Broken)?;
            let Some((event, payload)) = read else {
                if let Some(sent) = cancel_sent_at {
                    if sent.elapsed() > CANCEL_WAIT_TIMEOUT {
                        return Err(LivePassFailure::Broken(
                            "the worker never confirmed the cancel".to_owned(),
                        ));
                    }
                } else if last_event.elapsed() > liveness_deadline(seen_event, 0) {
                    return Err(LivePassFailure::Broken(format!(
                        "the worker was silent for {:.0?}",
                        last_event.elapsed()
                    )));
                }
                continue;
            };
            last_event = Instant::now();
            seen_event = true;
            match event {
                StreamEvent::LivePass(header) if header.request_id == request_id => {
                    return Ok((header, payload.unwrap_or_default()));
                }
                StreamEvent::Done(done) if done.request_id == request_id => {
                    return Err(LivePassFailure::Cancelled);
                }
                StreamEvent::Error(error) if error.request_id.is_none_or(|id| id == request_id) => {
                    return Err(LivePassFailure::Refused {
                        code: error.code,
                        message: error.message,
                    });
                }
                // A heartbeat, or a late event of an earlier request.
                _ => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bridge::{
        export_thread::{remote::scene_state_from_snapshot, scene_snapshot::SceneSnapshot},
        render_thread::RenderContext,
    };
    use pleochroma::optics::{
        absorption::AbsorptionTensor,
        zoning::{Zone, ZoneAbsorption, ZoneShape, ZonedAbsorption},
    };
    use pleochroma_net::messages::{
        ErrorMsg, Progress, encode_live_sums, error_codes, read_message, write_stream_event,
    };
    use std::{io::Cursor, sync::Mutex};

    /// A scripted worker: `input` is what it replies, `output` collects what the client wrote.
    struct Scripted {
        input: Cursor<Vec<u8>>,
        output: Vec<u8>,
    }

    impl Read for Scripted {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.input.read(buf)
        }
    }

    impl Write for Scripted {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.output.write(buf)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl LiveStream for Scripted {
        fn set_read_poll(&mut self, _timeout: Option<Duration>) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn scripted(events: &[(StreamEvent, Option<Vec<u8>>)]) -> LivePassClient<Scripted> {
        let mut input = Vec::new();
        for (event, payload) in events {
            write_stream_event(&mut input, event, payload.as_deref()).unwrap();
        }
        LivePassClient::with_stream(
            Scripted {
                input: Cursor::new(input),
                output: Vec::new(),
            },
            true,
        )
    }

    fn scene() -> SceneState {
        let snapshot = SceneSnapshot::capture(&Mutex::new(RenderContext::default()))
            .expect("the default material resolves");
        scene_state_from_snapshot(&snapshot, 24, 16, snapshot.yaw, snapshot.pitch)
    }

    fn pixels(count: u16) -> Vec<(Vec3, Vec3)> {
        (0..count)
            .map(|i| {
                let f = f32::from(i);
                (Vec3::new(f, f + 1.0, f + 2.0), Vec3::new(f * 0.5, 0.0, 1.0))
            })
            .collect()
    }

    fn pass_event(
        request_id: u32,
        first_sample: u32,
        samples: u32,
        count: u16,
    ) -> (StreamEvent, Option<Vec<u8>>) {
        let payload = encode_live_sums(&pixels(count));
        (
            StreamEvent::LivePass(LivePassHeader {
                request_id,
                first_sample,
                samples,
                live_pixels: u32::from(count),
                payload_len: u32::try_from(payload.len()).unwrap(),
            }),
            Some(payload),
        )
    }

    fn live() -> LiveTiles {
        // Two 8 x 8 tiles of the 24 x 16 frame.
        LiveTiles::from_indices(24, 16, 8, vec![1, 4])
    }

    fn never() -> AtomicBool {
        AtomicBool::new(false)
    }

    fn written_requests(client: &LivePassClient<Scripted>) -> Vec<ClientMessage> {
        let mut cursor = Cursor::new(client.stream.output.clone());
        let mut messages = Vec::new();
        while let Ok(message) = read_message::<_, ClientMessage>(&mut cursor) {
            messages.push(message);
        }
        messages
    }

    #[test]
    fn a_pass_sends_its_request_and_returns_the_replys_pixels_after_a_heartbeat() {
        let mut client = scripted(&[
            (
                StreamEvent::Progress(Progress {
                    request_id: 1,
                    samples_done: 0,
                }),
                None,
            ),
            pass_event(1, 33, 8, 128),
        ]);
        let got = client
            .trace(&scene(), (33, 8), &live(), &never())
            .expect("the scripted worker answers");
        assert_eq!(got.len(), 128);
        assert_eq!(got, pixels(128));

        let sent = written_requests(&client);
        let [ClientMessage::LivePassRequest(request)] = sent.as_slice() else {
            panic!("exactly one request, got {sent:?}");
        };
        assert_eq!(request.request_id, 1);
        assert_eq!((request.first_sample, request.samples), (33, 8));
        assert_eq!(request.live, LiveTileSet::from_live(&live()));
        assert_eq!(request.scene, scene());
    }

    #[test]
    fn requests_on_one_connection_have_distinct_ids() {
        let mut client = scripted(&[pass_event(1, 0, 8, 128), pass_event(2, 8, 8, 128)]);
        client.trace(&scene(), (0, 8), &live(), &never()).unwrap();
        client.trace(&scene(), (8, 8), &live(), &never()).unwrap();
        let ids: Vec<u32> = written_requests(&client)
            .iter()
            .map(|m| match m {
                ClientMessage::LivePassRequest(r) => r.request_id,
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        assert_eq!(ids, [1, 2]);
    }

    fn zoned_scene() -> SceneState {
        let mut scene = scene();
        let zoning = || ZoneAbsorption::per_mm(AbsorptionTensor::isotropic(vec![]));
        let mut zones = ZonedAbsorption::new(zoning());
        zones.zones.push(Zone {
            shape: ZoneShape::HalfSpace {
                normal: glam::DVec3::X,
                offset: 0.0,
            },
            absorption: zoning(),
        });
        scene.material = scene.material.with_zoning(zones);
        scene
    }

    #[test]
    fn the_zones_of_a_zoned_stone_go_ahead_of_its_request() {
        let mut client = scripted(&[pass_event(1, 0, 8, 128)]);
        client
            .trace(&zoned_scene(), (0, 8), &live(), &never())
            .unwrap();
        let sent = written_requests(&client);
        assert!(
            matches!(sent.as_slice(), [
                ClientMessage::ZoningPayload(payload),
                ClientMessage::LivePassRequest(request),
            ] if payload.request_id == 1 && request.request_id == 1),
            "{sent:?}"
        );
    }

    #[test]
    fn a_zoned_stone_is_not_sent_to_a_worker_without_the_capability() {
        let mut client = scripted(&[]);
        client.takes_zoning = false;
        let failure = client
            .trace(&zoned_scene(), (0, 8), &live(), &never())
            .unwrap_err();
        assert!(
            matches!(failure, LivePassFailure::Refused { code, .. } if code == error_codes::UNSUPPORTED_REQUEST),
            "{failure:?}"
        );
        assert!(client.stream.output.is_empty(), "nothing was written");
    }

    #[test]
    fn a_refusal_a_cancel_and_a_closed_connection_are_told_apart() {
        let refused = (
            StreamEvent::Error(ErrorMsg {
                code: error_codes::VALIDATION_FAILED,
                message: "no".to_owned(),
                request_id: Some(1),
            }),
            None,
        );
        let failure = scripted(&[refused])
            .trace(&scene(), (0, 8), &live(), &never())
            .unwrap_err();
        assert_eq!(
            failure,
            LivePassFailure::Refused {
                code: error_codes::VALIDATION_FAILED,
                message: "no".to_owned()
            }
        );

        let cancelled = (
            StreamEvent::Done(pleochroma_net::messages::Done {
                request_id: 1,
                cancelled: true,
                stats: pleochroma_net::messages::Stats {
                    samples_done: 0,
                    requested_cadence_ms: 0,
                    effective_cadence_ms: 0,
                    reclaimed_samples: 0,
                },
            }),
            None,
        );
        assert_eq!(
            scripted(&[cancelled])
                .trace(&scene(), (0, 8), &live(), &never())
                .unwrap_err(),
            LivePassFailure::Cancelled
        );

        let failure = scripted(&[])
            .trace(&scene(), (0, 8), &live(), &never())
            .unwrap_err();
        assert!(
            matches!(&failure, LivePassFailure::Broken(why) if why.contains("closed")),
            "{failure:?}"
        );
    }

    #[test]
    fn a_reply_that_does_not_fit_the_request_breaks_the_connection() {
        // Right id, wrong pixel count.
        let failure = scripted(&[pass_event(1, 0, 8, 64)])
            .trace(&scene(), (0, 8), &live(), &never())
            .unwrap_err();
        assert!(matches!(failure, LivePassFailure::Broken(_)), "{failure:?}");
        // Another pass's samples.
        let failure = scripted(&[pass_event(1, 8, 8, 128)])
            .trace(&scene(), (0, 8), &live(), &never())
            .unwrap_err();
        assert!(matches!(failure, LivePassFailure::Broken(_)), "{failure:?}");
        // A late reply of an earlier request is ignored; the real one still lands.
        let got = scripted(&[pass_event(77, 0, 8, 128), pass_event(1, 0, 8, 128)])
            .trace(&scene(), (0, 8), &live(), &never())
            .unwrap();
        assert_eq!(got.len(), 128);
    }

    #[test]
    fn a_raised_cancel_goes_out_as_a_cancel_before_the_reply_is_read() {
        let cancelled = AtomicBool::new(true);
        let mut client = scripted(&[(
            StreamEvent::Done(pleochroma_net::messages::Done {
                request_id: 1,
                cancelled: true,
                stats: pleochroma_net::messages::Stats {
                    samples_done: 0,
                    requested_cadence_ms: 0,
                    effective_cadence_ms: 0,
                    reclaimed_samples: 0,
                },
            }),
            None,
        )]);
        assert_eq!(
            client
                .trace(&scene(), (0, 8), &live(), &cancelled)
                .unwrap_err(),
            LivePassFailure::Cancelled
        );
        let sent = written_requests(&client);
        assert!(
            matches!(sent.last(), Some(ClientMessage::Cancel(c)) if c.request_id == 1),
            "{sent:?}"
        );
    }

    #[test]
    fn a_big_share_is_split_into_runs_of_whole_tiles_in_order() {
        let all = LiveTiles::all(40, 24, 8); // 5 x 3 tiles of 64 pixels
        let runs = split_by_pixels(&all, 200);
        let lens: Vec<usize> = runs.iter().map(|r| r.indices().len()).collect();
        assert_eq!(lens, [3, 3, 3, 3, 3], "three 64-pixel tiles fit under 200");
        let joined: Vec<u32> = runs.iter().flat_map(|r| r.indices().to_vec()).collect();
        assert_eq!(joined, all.indices());
        // A limit below one tile still puts one tile in each run.
        assert_eq!(split_by_pixels(&all, 10).len(), 15);
        // Everything fits one run under the real limit.
        assert_eq!(split_by_pixels(&all, MAX_LIVE_PASS_PIXELS).len(), 1);
        assert_eq!(
            split_by_pixels(&LiveTiles::from_indices(8, 8, 8, vec![]), 100),
            [] as [LiveTiles; 0]
        );
    }

    #[test]
    fn a_client_that_asks_for_a_path_cache_flags_every_request_id_and_reads_the_replies_by_it() {
        use pleochroma_net::messages::PATH_CACHE_FLAG;
        let (first, second) = (1 | PATH_CACHE_FLAG, 2 | PATH_CACHE_FLAG);
        let mut client = scripted(&[pass_event(first, 0, 8, 128), pass_event(second, 8, 8, 128)])
            .with_path_cache(true);
        client.trace(&scene(), (0, 8), &live(), &never()).unwrap();
        client.trace(&scene(), (8, 8), &live(), &never()).unwrap();
        let sent = written_requests(&client);
        let flags: Vec<(u32, bool)> = sent
            .iter()
            .map(|m| match m {
                ClientMessage::LivePassRequest(r) => (r.request_id, r.path_cache()),
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        assert_eq!(flags, [(first, true), (second, true)]);

        // Without the ask the requests are the plain ones: ids 1 and 2, no flag.
        let mut plain = scripted(&[pass_event(1, 0, 8, 128)]).with_path_cache(false);
        plain.trace(&scene(), (0, 8), &live(), &never()).unwrap();
        let plain_ids: Vec<(u32, bool)> = written_requests(&plain)
            .iter()
            .map(|m| match m {
                ClientMessage::LivePassRequest(r) => (r.request_id, r.path_cache()),
                other => panic!("unexpected {other:?}"),
            })
            .collect();
        assert_eq!(plain_ids, [(1, false)]);
    }

    #[test]
    fn a_flagged_zoned_request_keys_its_zones_by_the_flagged_id() {
        use pleochroma_net::messages::PATH_CACHE_FLAG;
        let id = 1 | PATH_CACHE_FLAG;
        let mut client = scripted(&[pass_event(id, 0, 8, 128)]).with_path_cache(true);
        client
            .trace(&zoned_scene(), (0, 8), &live(), &never())
            .unwrap();
        let sent = written_requests(&client);
        assert!(
            matches!(sent.as_slice(), [
                ClientMessage::ZoningPayload(payload),
                ClientMessage::LivePassRequest(request),
            ] if payload.request_id == id && request.request_id == id),
            "{sent:?}"
        );
    }
}
