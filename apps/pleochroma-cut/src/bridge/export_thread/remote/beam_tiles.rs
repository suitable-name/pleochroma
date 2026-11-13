//! The remote client for beam tiles: [`BeamTileClient`] sends whole tiles of a beam-traced frame
//! to a remote worker or coordinator and returns their finished pixels, beam part and residual.
//!
//! # One connection, one request at a time
//!
//! Like the live-pass client (`live_pass`), the client owns one mutual-TLS connection for a whole
//! render. A request gets `PROGRESS` heartbeats, then `BEAM_TILES` with a payload, or `ERROR`.
//! [`BeamTileClient::render`] reads the socket in short slices. A raised `cancel` sends `CANCEL`
//! and returns at once. The render is discarded, so the worker's `DONE` is not awaited and the
//! connection is not reused. A silent worker is given up on after the usual deadlines. A `DONE`
//! nobody asked for is a broken reply.
//!
//! A session keeps its connections across frames in a [`BeamClientPool`]. A connection that broke
//! or was cancelled is dropped and reopened.
//!
//! # Limits
//!
//! A request names at most `MAX_BEAM_TILE_PIXELS` pixels; a bigger share is split into several
//! requests on the same connection, in tile order, and the replies are joined.
//!
//! # A failed request is not a failed render
//!
//! [`BeamTileFailure`] says only that the remote could not render these tiles; the caller hands
//! them to the next engine (see `worker::core::beam::remote`).

use super::{
    capability::RemoteCapability,
    dispatch::liveness_deadline,
    live_pass::{LiveStream, POLL, read_event, split_by_pixels},
};
use crate::bridge::remote::remote_render::{RemoteStream, connect_and_handshake};
use pleochroma::renderer::{
    accel_beam::BeamSettings,
    adaptive::{AdaptiveSettings, LiveTiles},
};
use pleochroma_net::{
    SceneState,
    messages::{
        BEAM_TILE_PIXEL_BYTES, Backend, BeamTilePixel, BeamTileRequest, BeamTileStats, Cancel,
        ClientMessage, LiveTileSet, MAX_BEAM_TILE_PIXELS, StreamEvent, ZoningPayload,
        decode_beam_tile_pixels, error_codes, write_message, write_zoning_payload,
    },
};
use std::{
    fmt,
    sync::atomic::{AtomicBool, Ordering},
    time::Instant,
};

/// What a beam-tile request asks, besides the tiles: the scene and the settings of the render.
#[derive(Clone, Copy, Debug)]
pub struct BeamTileOrder<'a> {
    /// The scene as the wire carries it.
    pub scene: &'a SceneState,
    /// The beam tracer's settings.
    pub beam: BeamSettings,
    /// The residual's adaptive settings; their `max_spp` is the render's samples per pixel and
    /// their `tile` is the edge of the tiles asked for.
    pub adaptive: AdaptiveSettings,
    /// The absolute index of the residual's first sample.
    pub sample_offset: u32,
}

/// The finished tiles a worker returned.
#[derive(Clone, Debug, PartialEq)]
pub struct BeamTiles {
    /// One pixel per pixel of the asked tiles: tiles ascending, row-major inside a tile.
    pub pixels: Vec<BeamTilePixel>,
    /// The counters of the tiles, summed over the requests that made them up.
    pub stats: BeamTileStats,
}

/// Why a beam-tile request produced nothing.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BeamTileFailure {
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
    /// `cancel` was raised: `CANCEL` was sent and the request was given up without waiting for
    /// `DONE`. The connection is out of step and is not reused.
    Cancelled,
}

impl fmt::Display for BeamTileFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Refused { code, message } => {
                write!(
                    f,
                    "the worker refused the beam tiles (code {code}): {message}"
                )
            }
            Self::Broken(why) => write!(f, "{why}"),
            Self::Cancelled => write!(f, "the beam tiles were cancelled"),
        }
    }
}

/// `a + b`, counter by counter, without overflow.
const fn add_stats(a: BeamTileStats, b: BeamTileStats) -> BeamTileStats {
    BeamTileStats {
        beams: a.beams.saturating_add(b.beams),
        leaves: a.leaves.saturating_add(b.leaves),
        fragments: a.fragments.saturating_add(b.fragments),
        drops: a.drops.saturating_add(b.drops),
        residual_samples: a.residual_samples.saturating_add(b.residual_samples),
    }
}

/// One connection to a remote worker for the beam tiles of a render -- see the module doc.
pub struct BeamTileClient<S: LiveStream = RemoteStream> {
    stream: S,
    /// Whether the worker's `WELCOME` advertised the zoning capability.
    takes_zoning: bool,
    /// The id of the next request; unique on this connection.
    next_request_id: u32,
    /// Whether the connection is still in step: `false` once a request broke it or was cancelled
    /// (a late `DONE` or reply may still arrive), so it is not used for another render.
    usable: bool,
    /// The workers a coordinator's `WELCOME` says it has joined (0 for a plain worker).
    joined_workers: u32,
}

impl BeamTileClient {
    /// Connects to the worker `capability` describes and checks it can render.
    ///
    /// # Errors
    ///
    /// Why the connection or handshake failed, or that the server renders nothing.
    pub fn connect(capability: &RemoteCapability) -> Result<Self, String> {
        let (stream, welcome) =
            connect_and_handshake(&capability.worker).map_err(|e| e.to_string())?;
        let Some(render) = welcome.render.as_ref() else {
            return Err(
                "the remote serves the design library only: it has no render capacity".to_owned(),
            );
        };
        let joined = match render.backend {
            Backend::Coordinator { workers, .. } => workers,
            _ => 0,
        };
        let mut client = Self::with_stream(stream, welcome.zoning);
        client.joined_workers = joined;
        Ok(client)
    }
}

impl<S: LiveStream> BeamTileClient<S> {
    /// A client over an already handshaken `stream`; `takes_zoning` is the `WELCOME`'s zoning bit.
    #[must_use]
    pub const fn with_stream(stream: S, takes_zoning: bool) -> Self {
        Self {
            stream,
            takes_zoning,
            next_request_id: 1,
            usable: true,
            joined_workers: 0,
        }
    }

    /// Whether the connection can carry another render (no request broke or cancelled on it).
    #[must_use]
    pub const fn is_usable(&self) -> bool {
        self.usable
    }

    /// The workers the coordinator on the other end has joined (0 for a plain worker).
    #[must_use]
    pub const fn joined_workers(&self) -> u32 {
        self.joined_workers
    }

    /// Renders the tiles `live` of `order`'s frame on the worker and returns their finished
    /// pixels, in the order of `live` (tiles ascending, row-major inside a tile).
    ///
    /// # Errors
    ///
    /// A [`BeamTileFailure`]; nothing is returned for a request that failed part way.
    pub fn render(
        &mut self,
        order: &BeamTileOrder<'_>,
        live: &LiveTiles,
        cancel: &AtomicBool,
    ) -> Result<BeamTiles, BeamTileFailure> {
        let mut pixels = Vec::with_capacity(usize::try_from(live.pixel_count()).unwrap_or(0));
        let mut stats = BeamTileStats::default();
        for chunk in split_by_pixels(live, MAX_BEAM_TILE_PIXELS) {
            let part = match self.request(order, &chunk, cancel) {
                Ok(part) => part,
                Err(failure) => {
                    // A refusal leaves the connection in step; anything else does not.
                    if !matches!(failure, BeamTileFailure::Refused { .. }) {
                        self.usable = false;
                    }
                    return Err(failure);
                }
            };
            pixels.extend(part.pixels);
            stats = add_stats(stats, part.stats);
        }
        Ok(BeamTiles { pixels, stats })
    }

    /// One request and its reply.
    fn request(
        &mut self,
        order: &BeamTileOrder<'_>,
        live: &LiveTiles,
        cancel: &AtomicBool,
    ) -> Result<BeamTiles, BeamTileFailure> {
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.wrapping_add(1).max(1);
        let zones = ZoningPayload::for_scene(request_id, order.scene);
        if zones.is_some() && !self.takes_zoning {
            return Err(BeamTileFailure::Refused {
                code: error_codes::UNSUPPORTED_REQUEST,
                message: "the worker does not take colour zones, so a zoned stone is not sent to \
                          it"
                .to_owned(),
            });
        }
        let broken = |e: pleochroma_net::messages::NetError| {
            BeamTileFailure::Broken(format!("sending the beam tiles failed: {e}"))
        };
        // The zones go first (they are `serde(skip)` in the scene), then the request.
        if let Some(zones) = &zones {
            write_zoning_payload(&mut self.stream, zones).map_err(broken)?;
        }
        write_message(
            &mut self.stream,
            &ClientMessage::BeamTileRequest(Box::new(BeamTileRequest {
                request_id,
                scene: order.scene.clone(),
                tiles: LiveTileSet::from_live(live),
                beam: order.beam,
                adaptive: order.adaptive,
                sample_offset: order.sample_offset,
            })),
        )
        .map_err(broken)?;

        let expected_pixels = live.pixel_count();
        let payload_bytes = expected_pixels.saturating_mul(BEAM_TILE_PIXEL_BYTES as u64);
        let (header, payload) = self.await_reply(request_id, payload_bytes, cancel)?;
        if header.sample_offset != order.sample_offset
            || u64::from(header.pixels) != expected_pixels
        {
            return Err(BeamTileFailure::Broken(format!(
                "the worker answered another request: offset {} for {} pixels, asked {} for \
                 {expected_pixels}",
                header.sample_offset, header.pixels, order.sample_offset
            )));
        }
        let pixels = decode_beam_tile_pixels(&payload, header.pixels as usize)
            .map_err(|e| BeamTileFailure::Broken(format!("the worker's payload is bad: {e}")))?;
        Ok(BeamTiles {
            pixels,
            stats: header.stats,
        })
    }

    /// Waits for the terminal reply of `request_id`. A raised `cancel` writes `CANCEL` and returns
    /// [`BeamTileFailure::Cancelled`] at once, without waiting for `DONE`. `payload_bytes` sizes
    /// the transfer allowance of the liveness deadline.
    fn await_reply(
        &mut self,
        request_id: u32,
        payload_bytes: u64,
        cancel: &AtomicBool,
    ) -> Result<(pleochroma_net::messages::BeamTilesHeader, Vec<u8>), BeamTileFailure> {
        let mut last_event = Instant::now();
        let mut seen_event = false;
        loop {
            if cancel.load(Ordering::Relaxed) {
                // Best effort: whether or not the write lands, the request is given up.
                let _sent = write_message(
                    &mut self.stream,
                    &ClientMessage::Cancel(Cancel { request_id }),
                );
                return Err(BeamTileFailure::Cancelled);
            }
            let read = read_event(&mut self.stream, POLL).map_err(BeamTileFailure::Broken)?;
            let Some((event, payload)) = read else {
                if last_event.elapsed() > liveness_deadline(seen_event, payload_bytes) {
                    return Err(BeamTileFailure::Broken(format!(
                        "the worker was silent for {:.0?}",
                        last_event.elapsed()
                    )));
                }
                continue;
            };
            last_event = Instant::now();
            seen_event = true;
            match event {
                StreamEvent::BeamTiles(header) if header.request_id == request_id => {
                    return Ok((header, payload.unwrap_or_default()));
                }
                StreamEvent::Done(done) if done.request_id == request_id => {
                    // A worker sends `DONE` for beam tiles only after a `CANCEL`. Any other
                    // `DONE` is a broken reply, and the tiles fail over.
                    return Err(if cancel.load(Ordering::Relaxed) {
                        BeamTileFailure::Cancelled
                    } else {
                        BeamTileFailure::Broken(
                            "the worker ended the request without tiles".to_owned(),
                        )
                    });
                }
                StreamEvent::Error(error) if error.request_id.is_none_or(|id| id == request_id) => {
                    return Err(BeamTileFailure::Refused {
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

/// The most connections a beam render opens to one coordinator, one per joined worker.
const MAX_COORDINATOR_CLIENTS: usize = 8;

/// How many connections a render uses against a remote: one per joined worker of a coordinator
/// (at least one, at most [`MAX_COORDINATOR_CLIENTS`]), one for a plain worker.
#[must_use]
pub fn wanted_clients(coordinator: bool, joined_workers: u32) -> usize {
    if coordinator {
        usize::try_from(joined_workers)
            .unwrap_or(MAX_COORDINATOR_CLIENTS)
            .clamp(1, MAX_COORDINATOR_CLIENTS)
    } else {
        1
    }
}

/// The beam-tile connections one render session keeps across frames, so a tilt video does not
/// reconnect for each frame.
///
/// A coordinator takes one request stream per connection, so it gets one client per joined
/// worker, each a slot in the tile assignment.
#[derive(Default)]
pub struct BeamClientPool {
    clients: Vec<BeamTileClient>,
}

impl BeamClientPool {
    /// The connections to `capability`'s remote for the next frame: the usable ones kept from
    /// earlier frames, reopened where one broke, topped up to [`wanted_clients`].
    ///
    /// # Errors
    ///
    /// Why the first connection could not be made. A failure to add a further one for a
    /// coordinator only leaves the render with fewer.
    pub fn open(&mut self, capability: &RemoteCapability) -> Result<&mut [BeamTileClient], String> {
        self.clients.retain(BeamTileClient::is_usable);
        if self.clients.is_empty() {
            self.clients.push(BeamTileClient::connect(capability)?);
        }
        let joined = self
            .clients
            .first()
            .map_or(0, BeamTileClient::joined_workers);
        let wanted = wanted_clients(capability.coordinator, joined);
        self.clients.truncate(wanted);
        while self.clients.len() < wanted {
            match BeamTileClient::connect(capability) {
                Ok(client) => self.clients.push(client),
                Err(_) => break,
            }
        }
        Ok(&mut self.clients)
    }

    /// The connections, to be owned by a lane that is not tied to this pool.
    #[must_use]
    pub fn into_clients(self) -> Vec<BeamTileClient> {
        self.clients
    }
}

#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests {
    use super::*;
    use crate::bridge::{
        export_thread::{remote::scene_state_from_snapshot, scene_snapshot::SceneSnapshot},
        render_thread::RenderContext,
    };
    use glam::Vec3;
    use pleochroma::optics::{
        absorption::AbsorptionTensor,
        zoning::{Zone, ZoneAbsorption, ZoneShape, ZonedAbsorption},
    };
    use pleochroma_net::messages::{
        Done, ErrorMsg, Progress, Stats, beam_tiles_reply, read_message, write_stream_event,
    };
    use std::{
        io::{Cursor, Read, Write},
        sync::Mutex,
        time::Duration,
    };

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

    fn scripted(events: &[(StreamEvent, Option<Vec<u8>>)]) -> BeamTileClient<Scripted> {
        let mut input = Vec::new();
        for (event, payload) in events {
            write_stream_event(&mut input, event, payload.as_deref()).unwrap();
        }
        BeamTileClient::with_stream(
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

    fn adaptive() -> AdaptiveSettings {
        AdaptiveSettings::provisional(64)
    }

    fn order(scene: &SceneState) -> BeamTileOrder<'_> {
        BeamTileOrder {
            scene,
            beam: BeamSettings::default(),
            adaptive: AdaptiveSettings {
                tile: 8,
                ..adaptive()
            },
            sample_offset: 33,
        }
    }

    /// Two 8 x 8 tiles of the 24 x 16 frame.
    fn live() -> LiveTiles {
        LiveTiles::from_indices(24, 16, 8, vec![1, 4])
    }

    fn pixels(count: u16) -> Vec<BeamTilePixel> {
        (0..count)
            .map(|i| {
                let f = f32::from(i);
                BeamTilePixel {
                    xyz: Vec3::new(f, f + 1.0, f + 2.0),
                    samples: u32::from(i) + 7,
                }
            })
            .collect()
    }

    fn stats() -> BeamTileStats {
        BeamTileStats {
            beams: 1,
            leaves: 2,
            fragments: 3,
            drops: 4,
            residual_samples: 5,
        }
    }

    fn reply(request_id: u32, sample_offset: u32, count: u16) -> (StreamEvent, Option<Vec<u8>>) {
        let (event, payload) =
            beam_tiles_reply(request_id, sample_offset, stats(), &pixels(count)).unwrap();
        (event, Some(payload))
    }

    fn never() -> AtomicBool {
        AtomicBool::new(false)
    }

    fn written(client: &BeamTileClient<Scripted>) -> Vec<ClientMessage> {
        let mut cursor = Cursor::new(client.stream.output.clone());
        let mut messages = Vec::new();
        while let Ok(message) = read_message::<_, ClientMessage>(&mut cursor) {
            messages.push(message);
        }
        messages
    }

    fn done(request_id: u32) -> (StreamEvent, Option<Vec<u8>>) {
        (
            StreamEvent::Done(Done {
                request_id,
                cancelled: true,
                stats: Stats {
                    samples_done: 0,
                    requested_cadence_ms: 0,
                    effective_cadence_ms: 0,
                    reclaimed_samples: 0,
                },
            }),
            None,
        )
    }

    #[test]
    fn a_request_sends_its_tiles_and_returns_the_replys_pixels_after_a_heartbeat() {
        let mut client = scripted(&[
            (
                StreamEvent::Progress(Progress {
                    request_id: 1,
                    samples_done: 0,
                }),
                None,
            ),
            reply(1, 33, 128),
        ]);
        let scene = scene();
        let got = client
            .render(&order(&scene), &live(), &never())
            .expect("the scripted worker answers");
        assert_eq!(got.pixels, pixels(128));
        assert_eq!(got.stats, stats());

        let sent = written(&client);
        let [ClientMessage::BeamTileRequest(request)] = sent.as_slice() else {
            panic!("exactly one request, got {sent:?}");
        };
        assert_eq!(request.request_id, 1);
        assert_eq!(request.sample_offset, 33);
        assert_eq!(request.tiles, LiveTileSet::from_live(&live()));
        assert_eq!(request.scene, scene);
        assert_eq!(request.beam, BeamSettings::default());
        assert_eq!(request.adaptive.tile, 8);
    }

    #[test]
    fn requests_on_one_connection_have_distinct_ids() {
        let mut client = scripted(&[reply(1, 33, 128), reply(2, 33, 128)]);
        let scene = scene();
        client.render(&order(&scene), &live(), &never()).unwrap();
        client.render(&order(&scene), &live(), &never()).unwrap();
        let ids: Vec<u32> = written(&client)
            .iter()
            .map(|m| match m {
                ClientMessage::BeamTileRequest(r) => r.request_id,
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
        let mut client = scripted(&[reply(1, 33, 128)]);
        let scene = zoned_scene();
        client.render(&order(&scene), &live(), &never()).unwrap();
        let sent = written(&client);
        assert!(
            matches!(sent.as_slice(), [
                ClientMessage::ZoningPayload(payload),
                ClientMessage::BeamTileRequest(request),
            ] if payload.request_id == 1 && request.request_id == 1),
            "{sent:?}"
        );
    }

    #[test]
    fn a_zoned_stone_is_not_sent_to_a_worker_without_the_capability() {
        let mut client = scripted(&[]);
        client.takes_zoning = false;
        let scene = zoned_scene();
        let failure = client
            .render(&order(&scene), &live(), &never())
            .unwrap_err();
        assert!(
            matches!(failure, BeamTileFailure::Refused { code, .. } if code == error_codes::UNSUPPORTED_REQUEST),
            "{failure:?}"
        );
        assert!(client.stream.output.is_empty(), "nothing was written");
    }

    #[test]
    fn a_refusal_a_cancel_and_a_closed_connection_are_told_apart() {
        let scene = scene();
        let refused = (
            StreamEvent::Error(ErrorMsg {
                code: error_codes::VALIDATION_FAILED,
                message: "no".to_owned(),
                request_id: Some(1),
            }),
            None,
        );
        let failure = scripted(&[refused])
            .render(&order(&scene), &live(), &never())
            .unwrap_err();
        assert_eq!(
            failure,
            BeamTileFailure::Refused {
                code: error_codes::VALIDATION_FAILED,
                message: "no".to_owned()
            }
        );
        // A `DONE` this client did not ask for is a broken reply (the tiles fail over), not the
        // user's cancel.
        let mut unasked = scripted(&[done(1)]);
        let failure = unasked
            .render(&order(&scene), &live(), &never())
            .unwrap_err();
        assert!(
            matches!(&failure, BeamTileFailure::Broken(why) if why.contains("without tiles")),
            "{failure:?}"
        );
        assert!(!unasked.is_usable(), "a broken connection is not reused");
        assert_eq!(
            scripted(&[done(1)])
                .render(&order(&scene), &live(), &AtomicBool::new(true))
                .unwrap_err(),
            BeamTileFailure::Cancelled
        );
        let failure = scripted(&[])
            .render(&order(&scene), &live(), &never())
            .unwrap_err();
        assert!(
            matches!(&failure, BeamTileFailure::Broken(why) if why.contains("closed")),
            "{failure:?}"
        );
    }

    #[test]
    fn a_reply_that_does_not_fit_the_request_breaks_the_connection() {
        let scene = scene();
        // Right id, wrong pixel count.
        let failure = scripted(&[reply(1, 33, 64)])
            .render(&order(&scene), &live(), &never())
            .unwrap_err();
        assert!(matches!(failure, BeamTileFailure::Broken(_)), "{failure:?}");
        // Another request's sample offset.
        let failure = scripted(&[reply(1, 34, 128)])
            .render(&order(&scene), &live(), &never())
            .unwrap_err();
        assert!(matches!(failure, BeamTileFailure::Broken(_)), "{failure:?}");
        // A late reply of an earlier request is ignored; the real one still lands.
        let got = scripted(&[reply(77, 33, 128), reply(1, 33, 128)])
            .render(&order(&scene), &live(), &never())
            .unwrap();
        assert_eq!(got.pixels.len(), 128);
    }

    #[test]
    fn a_raised_cancel_goes_out_as_a_cancel_before_the_reply_is_read() {
        let cancelled = AtomicBool::new(true);
        let scene = scene();
        let mut client = scripted(&[done(1)]);
        assert_eq!(
            client
                .render(&order(&scene), &live(), &cancelled)
                .unwrap_err(),
            BeamTileFailure::Cancelled
        );
        let sent = written(&client);
        assert!(
            matches!(sent.last(), Some(ClientMessage::Cancel(c)) if c.request_id == 1),
            "{sent:?}"
        );
    }

    #[test]
    fn a_raised_cancel_returns_within_one_poll_without_waiting_for_a_done() {
        // The script holds no `DONE`. A client that waited for one would read a closed stream
        // and report `Broken`.
        let cancelled = AtomicBool::new(true);
        let scene = scene();
        let mut client = scripted(&[]);
        let started = Instant::now();
        let failure = client
            .render(&order(&scene), &live(), &cancelled)
            .unwrap_err();
        assert_eq!(failure, BeamTileFailure::Cancelled);
        assert!(started.elapsed() < POLL, "{:?}", started.elapsed());
        let sent = written(&client);
        assert!(
            matches!(sent.last(), Some(ClientMessage::Cancel(c)) if c.request_id == 1),
            "{sent:?}"
        );
        assert!(!client.is_usable(), "a cancelled connection is out of step");
    }

    #[test]
    fn a_refusal_leaves_the_connection_usable() {
        let scene = scene();
        let refused = (
            StreamEvent::Error(ErrorMsg {
                code: error_codes::VALIDATION_FAILED,
                message: "no".to_owned(),
                request_id: Some(1),
            }),
            None,
        );
        let mut client = scripted(&[refused]);
        client
            .render(&order(&scene), &live(), &never())
            .unwrap_err();
        assert!(client.is_usable());
    }

    #[test]
    fn a_coordinator_gets_a_connection_per_joined_worker_within_bounds() {
        assert_eq!(wanted_clients(false, 5), 1, "a plain worker takes one");
        assert_eq!(wanted_clients(true, 0), 1, "its own lane at least");
        assert_eq!(wanted_clients(true, 3), 3);
        assert_eq!(wanted_clients(true, 500), MAX_COORDINATOR_CLIENTS);
    }

    #[test]
    fn the_counters_of_several_requests_add_up() {
        let sum = add_stats(stats(), stats());
        assert_eq!(
            sum,
            BeamTileStats {
                beams: 2,
                leaves: 4,
                fragments: 6,
                drops: 8,
                residual_samples: 10,
            }
        );
        let full = BeamTileStats {
            leaves: u64::MAX,
            ..BeamTileStats::default()
        };
        assert_eq!(add_stats(full, stats()).leaves, u64::MAX);
    }
}
