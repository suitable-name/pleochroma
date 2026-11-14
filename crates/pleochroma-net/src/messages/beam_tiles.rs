//! `-> BEAM_TILE_REQUEST`: whole beam-traced tiles from a worker.
//!
//! A beam-traced frame is cut into tiles; the coordinator gives a worker a share of them and the
//! worker renders each whole tile, beam part and residual, with its own engines from the scene in
//! the request. No beam tree and no kept index crosses the network.
//!
//! ```text
//! -> BeamTileRequest { request_id, scene, tiles: LiveTileSet, beam, adaptive, sample_offset }
//! <- PROGRESS        { request_id, samples_done }                       -- heartbeat, ~every 2 s
//! <- BEAM_TILES      { request_id, sample_offset, pixels, stats, payload_len } + payload
//!    or ERROR        { code, message, request_id }
//!    or DONE         { request_id, cancelled: true, .. }                -- after a CANCEL
//! ```
//!
//! # A separate request
//!
//! Like the live pass (`messages::live_pass`), a beam-tile request is its own message
//! (`ClientMessage::BeamTileRequest`, index 12) with its own reply (`StreamEvent::BeamTiles`,
//! index 15), so older messages keep their encoding. It carries a `request_id`
//! (the reply and any `PROGRESS`, `ERROR` or `CANCEL` are matched by it) and the scene (the
//! worker needs the frame size to read the tile set, and keeps no scene between requests).
//!
//! # The tile set
//!
//! `tiles` is the live-pass [`LiveTileSet`]: a tile edge and a bitmask over the frame's tiles in
//! row-major tile order. A set that names no tile, a mask of the wrong length for the scene's
//! frame, or a stray bit past the last tile is refused.
//!
//! # The reply payload
//!
//! `payload_len = pixels * 16` raw bytes ([`BEAM_TILE_PIXEL_BYTES`] each): for each pixel of the
//! requested tiles, tiles ascending and pixels row-major inside a tile (clipped at the frame's
//! right and bottom edges), four little-endian 32-bit values: the finished X, Y, Z (the beam part
//! plus the mean of the residual samples, as the worker's own frame holds them), then the
//! residual samples the pixel received as a `u32`.
//!
//! The header ([`BeamTilesHeader`](crate::messages::BeamTilesHeader)) echoes `request_id` and
//! `sample_offset`, and carries the counters of the tiles
//! ([`BeamTileStats`](crate::messages::BeamTileStats)).
//!
//! # Determinism
//!
//! A worker's beam part is bit-identical to a local one on the same platform and toolchain; across
//! platforms it is within the twin tolerance, and the residual is within it whenever the engines
//! differ. The wire carries `f32` bits unchanged.
//!
//! Only compiled under this crate's `render` feature, like `LivePassRequest`.

use super::{
    ClientMessage, StreamEvent,
    codec::{NetError, decode_control_frame},
    live_pass::{LivePassError, LiveTileSet, MAX_LIVE_PASS_PIXELS},
    stream::{BeamTileStats, BeamTilesHeader},
};
use crate::scene::SceneState;
use glam::Vec3;
use pleochroma::renderer::{
    accel_beam::{BeamSettings, BeamSettingsError},
    adaptive::{AdaptiveSettings, AdaptiveSettingsError, LiveTiles},
};
use serde::{Deserialize, Serialize};

/// Bytes per pixel in a reply payload: three little-endian `f32` (X, Y, Z) and one little-endian
/// `u32` (the pixel's residual sample count).
pub const BEAM_TILE_PIXEL_BYTES: usize = 16;

/// The most pixels one [`BeamTileRequest`] may name: the live-pass cap (4 Mi pixels, a 64 MB
/// reply payload).
///
/// A sender with more pixels splits the tiles over several requests.
pub const MAX_BEAM_TILE_PIXELS: u64 = MAX_LIVE_PASS_PIXELS;

/// The most residual samples per pixel one [`BeamTileRequest`] may ask for (`adaptive.max_spp`).
///
/// It equals the worker's cap on samples per request (65,536), so a viewer refuses before sending
/// what the worker would refuse.
pub const MAX_BEAM_TILE_SPP: u32 = 65_536;

/// Why a [`BeamTileRequest`] or its reply cannot be used.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BeamTileError {
    /// The tile set is unusable for the scene's frame, or names more than
    /// [`MAX_BEAM_TILE_PIXELS`] pixels.
    Tiles(LivePassError),
    /// `spectral_nodes` is not a multiple of 8 within the engines' range (8 to 1024).
    SpectralNodes(u32),
    /// `batch_tiles` is zero.
    BatchTiles,
    /// Another beam setting is outside [`BeamSettings::validate`]'s range.
    Settings(BeamSettingsError),
    /// The adaptive settings break a documented range.
    Adaptive(AdaptiveSettingsError),
    /// `adaptive.max_spp` is more than [`MAX_BEAM_TILE_SPP`].
    MaxSpp(u32),
    /// A reply payload is not `pixels * 16` bytes.
    PayloadLength {
        /// The byte count the pixels need.
        expected: usize,
        /// The byte count received.
        actual: usize,
    },
    /// A reply holds a non-finite X, Y or Z.
    BadValue,
    /// The bytes are not a decodable client message.
    Undecodable,
    /// The client message decoded, but it is not a [`BeamTileRequest`].
    NotABeamTileRequest,
    /// The stream event is not a [`StreamEvent::BeamTiles`].
    NotABeamTilesReply,
}

impl std::fmt::Display for BeamTileError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tiles(error) => write!(f, "beam tiles: {error}"),
            Self::SpectralNodes(nodes) => write!(
                f,
                "beam tiles need a multiple of 8 spectral nodes from 8 to 1024 (got {nodes})"
            ),
            Self::BatchTiles => write!(f, "beam tiles need at least one tile per batch"),
            Self::Settings(error) => write!(f, "beam tiles: {error}"),
            Self::Adaptive(error) => write!(f, "beam tiles: {error}"),
            Self::MaxSpp(max_spp) => write!(
                f,
                "beam tiles take at most {MAX_BEAM_TILE_SPP} samples per pixel (got {max_spp})"
            ),
            Self::PayloadLength { expected, actual } => write!(
                f,
                "the beam-tile payload has {actual} bytes, its pixels need {expected}"
            ),
            Self::BadValue => write!(f, "the beam-tile payload holds a non-finite color value"),
            Self::Undecodable => write!(f, "the bytes are not a decodable client message"),
            Self::NotABeamTileRequest => write!(f, "the client message is not a beam-tile request"),
            Self::NotABeamTilesReply => write!(f, "the stream event is not a beam-tiles reply"),
        }
    }
}

impl std::error::Error for BeamTileError {}

/// `-> BEAM_TILE_REQUEST`: render the `tiles` of `scene`'s frame whole, beams and residual -- see
/// the module doc for the reply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BeamTileRequest {
    /// Client-chosen id, echoed on every reply.
    pub request_id: u32,
    /// The fully resolved scene to render (zones travel ahead as a `ZoningPayload`).
    pub scene: SceneState,
    /// The tiles to render.
    pub tiles: LiveTileSet,
    /// The beam tracer's settings.
    pub beam: BeamSettings,
    /// The adaptive sampling settings the residual follows.
    pub adaptive: AdaptiveSettings,
    /// The absolute index of the residual's first sample, so a tile gets the same samples
    /// whichever engine renders it.
    pub sample_offset: u32,
}

impl BeamTileRequest {
    /// The tiles this request names on its scene's frame, with every limit checked: the beam
    /// settings, the adaptive settings, the tile set, and [`MAX_BEAM_TILE_PIXELS`].
    ///
    /// # Errors
    ///
    /// A [`BeamTileError`] for beam settings outside [`BeamSettings::validate`]'s range
    /// (`spectral_nodes` a multiple of 8 within 8..=1024, a zero `batch_tiles`, a non-positive
    /// tolerance, ...), impossible adaptive settings, `max_spp` over [`MAX_BEAM_TILE_SPP`], a
    /// tile set that is empty, outside the frame or of a bad tile edge, or more than
    /// [`MAX_BEAM_TILE_PIXELS`] pixels.
    pub fn live_tiles(&self) -> Result<LiveTiles, BeamTileError> {
        self.beam.validate().map_err(|error| match error {
            BeamSettingsError::SpectralNodes(nodes) => BeamTileError::SpectralNodes(nodes),
            BeamSettingsError::Zero("batch_tiles") => BeamTileError::BatchTiles,
            other => BeamTileError::Settings(other),
        })?;
        self.adaptive.validate().map_err(BeamTileError::Adaptive)?;
        if self.adaptive.max_spp > MAX_BEAM_TILE_SPP {
            return Err(BeamTileError::MaxSpp(self.adaptive.max_spp));
        }
        let live = self
            .tiles
            .to_live(self.scene.width, self.scene.height)
            .map_err(BeamTileError::Tiles)?;
        let pixels = live.pixel_count();
        if pixels > MAX_BEAM_TILE_PIXELS {
            return Err(BeamTileError::Tiles(LivePassError::TooManyPixels {
                pixels,
            }));
        }
        Ok(live)
    }
}

/// One pixel of a reply: the finished color and the residual samples it received.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BeamTilePixel {
    /// The finished X, Y, Z: the beam part plus the mean of the residual samples.
    pub xyz: Vec3,
    /// The residual samples the pixel received.
    pub samples: u32,
}

/// The bytes of `request` as a tagged `ClientMessage::BeamTileRequest`, the content of one frame
/// (a writer such as `framing::write_frame` adds the length prefix).
///
/// # Errors
///
/// [`NetError::Postcard`] if the request fails to serialize.
pub fn encode_beam_tile_request(request: BeamTileRequest) -> Result<Vec<u8>, NetError> {
    Ok(postcard::to_allocvec(&ClientMessage::BeamTileRequest(
        Box::new(request),
    ))?)
}

/// The request in the bytes of one frame written by [`encode_beam_tile_request`].
///
/// The request is decoded, not validated; call [`BeamTileRequest::live_tiles`].
///
/// # Errors
///
/// [`BeamTileError::Undecodable`] if `bytes` are not a client message, or
/// [`BeamTileError::NotABeamTileRequest`] if they are another one.
pub fn decode_beam_tile_request(bytes: &[u8]) -> Result<BeamTileRequest, BeamTileError> {
    match decode_control_frame::<ClientMessage>(bytes) {
        Ok(ClientMessage::BeamTileRequest(request)) => Ok(*request),
        Ok(_) => Err(BeamTileError::NotABeamTileRequest),
        Err(_) => Err(BeamTileError::Undecodable),
    }
}

/// The reply payload of `pixels`: [`BEAM_TILE_PIXEL_BYTES`] bytes per pixel -- see the module
/// doc.
#[must_use]
pub fn encode_beam_tile_pixels(pixels: &[BeamTilePixel]) -> Vec<u8> {
    let mut out = Vec::with_capacity(pixels.len() * BEAM_TILE_PIXEL_BYTES);
    for pixel in pixels {
        for value in pixel.xyz.to_array() {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out.extend_from_slice(&pixel.samples.to_le_bytes());
    }
    out
}

/// Decodes a reply payload of `pixels` pixels.
///
/// # Errors
///
/// [`BeamTileError::PayloadLength`] if `bytes` is not `pixels * 16` long, or
/// [`BeamTileError::BadValue`] for a non-finite X, Y or Z.
pub fn decode_beam_tile_pixels(
    bytes: &[u8],
    pixels: usize,
) -> Result<Vec<BeamTilePixel>, BeamTileError> {
    let expected = pixels.saturating_mul(BEAM_TILE_PIXEL_BYTES);
    if bytes.len() != expected {
        return Err(BeamTileError::PayloadLength {
            expected,
            actual: bytes.len(),
        });
    }
    let mut out = Vec::with_capacity(pixels);
    for chunk in bytes.as_chunks::<BEAM_TILE_PIXEL_BYTES>().0 {
        let [x, y, z, samples] = chunk.as_chunks::<4>().0 else {
            return Err(BeamTileError::PayloadLength {
                expected,
                actual: bytes.len(),
            });
        };
        let xyz = Vec3::new(
            f32::from_le_bytes(*x),
            f32::from_le_bytes(*y),
            f32::from_le_bytes(*z),
        );
        if !xyz.is_finite() {
            return Err(BeamTileError::BadValue);
        }
        out.push(BeamTilePixel {
            xyz,
            samples: u32::from_le_bytes(*samples),
        });
    }
    Ok(out)
}

/// The reply to a request: the `BeamTiles` event and the payload frame that goes with it.
///
/// `pixels` are in the payload's order (tiles ascending, row-major inside a tile). Write the pair
/// with `write_stream_event`.
///
/// # Errors
///
/// [`BeamTileError::Tiles`] with [`LivePassError::TooManyPixels`] for more than
/// [`MAX_BEAM_TILE_PIXELS`] pixels.
pub fn beam_tiles_reply(
    request_id: u32,
    sample_offset: u32,
    stats: BeamTileStats,
    pixels: &[BeamTilePixel],
) -> Result<(StreamEvent, Vec<u8>), BeamTileError> {
    let count = u64::try_from(pixels.len()).unwrap_or(u64::MAX);
    let too_many = || BeamTileError::Tiles(LivePassError::TooManyPixels { pixels: count });
    if count > MAX_BEAM_TILE_PIXELS {
        return Err(too_many());
    }
    let payload = encode_beam_tile_pixels(pixels);
    let header = BeamTilesHeader {
        request_id,
        sample_offset,
        pixels: u32::try_from(pixels.len()).map_err(|_| too_many())?,
        stats,
        payload_len: u32::try_from(payload.len()).map_err(|_| too_many())?,
    };
    Ok((StreamEvent::BeamTiles(header), payload))
}

/// The header and the pixels of a `BeamTiles` event and its payload frame.
///
/// # Errors
///
/// [`BeamTileError::NotABeamTilesReply`] for another event, [`BeamTileError::PayloadLength`] if
/// the payload is not the size the header declares or `pixels * 16`, or
/// [`BeamTileError::BadValue`] for a non-finite color.
pub fn decode_beam_tiles_reply(
    event: &StreamEvent,
    payload: &[u8],
) -> Result<(BeamTilesHeader, Vec<BeamTilePixel>), BeamTileError> {
    let StreamEvent::BeamTiles(header) = event else {
        return Err(BeamTileError::NotABeamTilesReply);
    };
    let pixels = usize::try_from(header.pixels).unwrap_or(usize::MAX);
    let declared = usize::try_from(header.payload_len).unwrap_or(usize::MAX);
    if declared != payload.len() {
        return Err(BeamTileError::PayloadLength {
            expected: declared,
            actual: payload.len(),
        });
    }
    Ok((*header, decode_beam_tile_pixels(payload, pixels)?))
}

#[cfg(test)]
#[cfg(beam_tests)]
mod tests {
    use super::{
        super::{
            codec::{read_message, write_message},
            read_stream_event, write_stream_event,
        },
        *,
    };
    use pleochroma::renderer::accel_beam::{RegionMode, ResidualMode};

    fn scene(width: u32, height: u32) -> SceneState {
        use pleochroma::{
            geometry::cuts::StandardGemCuts,
            optics::{materials::GemMaterial, raytracer::LightingPreset},
        };
        SceneState {
            engine: pleochroma::renderer::engine::EngineSpec::default(),
            width,
            height,
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
            environment: crate::scene::SceneEnvironment::Studio,
            surface_glare: 1.0,
            tools: Vec::new(),
            fluorescence: pleochroma::optics::fluorescence::Fluorescence::default(),
            head_shadow_deg: 16.0,
            frosted_planes: Vec::new(),
            effects: crate::scene::effects::SceneEffects::default(),
            camera_pan: [0.0, 0.0],
        }
    }

    /// Every field of the settings off its default.
    fn odd_beam() -> BeamSettings {
        BeamSettings {
            dir_tolerance_rad: 2.5e-3,
            chord_tolerance_px: 5e-4,
            weight_tolerance: 2e-3,
            lighting_tolerance: 4e-3,
            cutoff: 5e-3,
            spectral_nodes: 24,
            absorption_subnodes: 2,
            max_split_depth: 18,
            max_fragment_levels: 3,
            residual: ResidualMode::Off,
            residual_min_spp: 16,
            region_mode: RegionMode::PerNode,
            batch_tiles: 7,
            leaf_cache_bytes: 123_456_789,
            memory_cap_bytes: 987_654_321,
            keep_leaves: true,
        }
    }

    fn request(width: u32, height: u32, tiles: Vec<u32>) -> BeamTileRequest {
        BeamTileRequest {
            request_id: 7,
            scene: scene(width, height),
            tiles: LiveTileSet::from_live(&LiveTiles::from_indices(width, height, 8, tiles)),
            beam: odd_beam(),
            adaptive: AdaptiveSettings::provisional(64),
            sample_offset: 33,
        }
    }

    #[test]
    fn a_request_with_every_beam_setting_off_its_default_round_trips() {
        assert_ne!(odd_beam(), BeamSettings::default());
        // 48 x 40 with 8-pixel tiles: 6 x 5 = 30 tiles, so tile 29 exists.
        let request = request(48, 40, vec![0, 3, 7, 29]);
        let mut buf = Vec::new();
        write_message(&mut buf, &request).unwrap();
        let decoded: BeamTileRequest = read_message(&mut std::io::Cursor::new(buf)).unwrap();
        assert_eq!(decoded, request);
        assert_eq!(decoded.beam, odd_beam());
        assert_eq!(decoded.live_tiles().unwrap().indices(), [0, 3, 7, 29]);
    }

    #[test]
    fn the_codec_functions_round_trip_the_request() {
        let request = request(48, 40, vec![1, 2]);
        let bytes = encode_beam_tile_request(request.clone()).unwrap();
        assert_eq!(decode_beam_tile_request(&bytes).unwrap(), request);
        assert_eq!(
            decode_beam_tile_request(&bytes[..bytes.len() / 2]),
            Err(BeamTileError::Undecodable)
        );
        let cancel = postcard::to_allocvec(&ClientMessage::Cancel(super::super::Cancel {
            request_id: 3,
        }))
        .unwrap();
        assert_eq!(
            decode_beam_tile_request(&cancel),
            Err(BeamTileError::NotABeamTileRequest)
        );
    }

    #[test]
    fn the_request_is_appended_at_index_12_and_its_fields_are_in_declaration_order() {
        let plain = request(40, 36, vec![0]);
        let tagged =
            postcard::to_allocvec(&ClientMessage::BeamTileRequest(Box::new(plain.clone())))
                .unwrap();
        assert_eq!(tagged[0], 12, "appended after LivePassRequest (11)");
        let decoded: ClientMessage = postcard::from_bytes(&tagged).unwrap();
        assert_eq!(
            decoded,
            ClientMessage::BeamTileRequest(Box::new(plain.clone()))
        );
        let fields = postcard::to_allocvec(&(
            plain.request_id,
            &plain.scene,
            &plain.tiles,
            plain.beam,
            plain.adaptive,
            plain.sample_offset,
        ))
        .unwrap();
        assert_eq!(&tagged[1..], fields.as_slice());
        // The older indices did not move.
        let live = ClientMessage::Cancel(super::super::Cancel { request_id: 1 });
        assert_eq!(postcard::to_allocvec(&live).unwrap()[0], 0);
    }

    #[test]
    fn the_reply_event_is_appended_after_the_live_pass_event_with_a_pinned_header() {
        let event = StreamEvent::BeamTiles(BeamTilesHeader {
            request_id: 5,
            sample_offset: 3,
            pixels: 4,
            stats: BeamTileStats {
                beams: 1,
                leaves: 2,
                fragments: 3,
                drops: 4,
                residual_samples: 5,
            },
            payload_len: 64,
        });
        let tagged = postcard::to_allocvec(&event).unwrap();
        // Tag 15 (after LivePass, 14), then the header fields as one-byte varints.
        assert_eq!(tagged, [15, 5, 3, 4, 1, 2, 3, 4, 5, 64]);
        assert_eq!(event.payload_len(), Some(64));
        assert_eq!(event.request_id(), Some(5));
        let live = StreamEvent::LivePass(super::super::LivePassHeader {
            request_id: 4,
            first_sample: 16,
            samples: 16,
            live_pixels: 64,
            payload_len: 64 * 24,
        });
        assert_eq!(postcard::to_allocvec(&live).unwrap()[0], 14);
    }

    fn pixels() -> Vec<BeamTilePixel> {
        (0_u8..10)
            .map(|i| {
                let f = f32::from(i);
                BeamTilePixel {
                    xyz: Vec3::new(f, f + 0.5, f * 2.0),
                    samples: u32::from(i) * 100 + 7,
                }
            })
            .collect()
    }

    #[test]
    fn the_payload_layout_is_x_y_z_then_the_count_little_endian() {
        assert_eq!(BEAM_TILE_PIXEL_BYTES, 16);
        let pixels = pixels();
        let bytes = encode_beam_tile_pixels(&pixels);
        assert_eq!(bytes.len(), pixels.len() * BEAM_TILE_PIXEL_BYTES);
        // Pixel 3 starts at byte 48.
        let second = &bytes[3 * BEAM_TILE_PIXEL_BYTES..4 * BEAM_TILE_PIXEL_BYTES];
        assert_eq!(second[0..4], 3.0_f32.to_le_bytes());
        assert_eq!(second[4..8], 3.5_f32.to_le_bytes());
        assert_eq!(second[8..12], 6.0_f32.to_le_bytes());
        assert_eq!(second[12..16], 307_u32.to_le_bytes());
        assert_eq!(
            decode_beam_tile_pixels(&bytes, pixels.len()).unwrap(),
            pixels
        );
    }

    #[test]
    fn a_reply_round_trips_through_the_stream_event_codec() {
        let stats = BeamTileStats {
            beams: 11,
            leaves: 22,
            fragments: 33,
            drops: 44,
            residual_samples: 55_000_000_000,
        };
        let pixels = pixels();
        let (event, payload) = beam_tiles_reply(9, 128, stats, &pixels).unwrap();
        let mut buf = Vec::new();
        write_stream_event(&mut buf, &event, Some(&payload)).unwrap();
        let (decoded, body) = read_stream_event(&mut std::io::Cursor::new(buf)).unwrap();
        assert_eq!(decoded, event);
        let (header, got) = decode_beam_tiles_reply(&decoded, &body.unwrap()).unwrap();
        assert_eq!(header.request_id, 9);
        assert_eq!(header.sample_offset, 128);
        assert_eq!(header.pixels, 10);
        assert_eq!(header.payload_len, 160);
        assert_eq!(header.stats, stats);
        assert_eq!(got, pixels);
    }

    #[test]
    fn a_bad_reply_is_refused() {
        let pixels = pixels();
        let (event, payload) = beam_tiles_reply(1, 0, BeamTileStats::default(), &pixels).unwrap();
        assert_eq!(
            decode_beam_tile_pixels(&payload[..payload.len() - 1], pixels.len()),
            Err(BeamTileError::PayloadLength {
                expected: 160,
                actual: 159
            })
        );
        assert_eq!(
            decode_beam_tiles_reply(&event, &payload[..payload.len() - 4]),
            Err(BeamTileError::PayloadLength {
                expected: 160,
                actual: 156
            })
        );
        for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let mut broken = payload.clone();
            broken[8..12].copy_from_slice(&bad.to_le_bytes());
            assert_eq!(
                decode_beam_tiles_reply(&event, &broken),
                Err(BeamTileError::BadValue)
            );
        }
        let mut lying = event;
        if let StreamEvent::BeamTiles(header) = &mut lying {
            header.pixels = 11;
        }
        assert_eq!(
            decode_beam_tiles_reply(&lying, &payload),
            Err(BeamTileError::PayloadLength {
                expected: 176,
                actual: 160
            })
        );
        let other = StreamEvent::Pong { nonce: 1 };
        assert_eq!(
            decode_beam_tiles_reply(&other, &payload),
            Err(BeamTileError::NotABeamTilesReply)
        );
    }

    #[test]
    fn an_impossible_request_is_refused_with_its_reason() {
        let ok = request(48, 40, vec![0, 3]);
        assert!(ok.live_tiles().is_ok());

        for nodes in [0, 4, 12, 52, 1032] {
            let mut bad = ok.clone();
            bad.beam.spectral_nodes = nodes;
            assert_eq!(bad.live_tiles(), Err(BeamTileError::SpectralNodes(nodes)));
        }
        let mut good_nodes = ok.clone();
        good_nodes.beam.spectral_nodes = 56;
        assert!(good_nodes.live_tiles().is_ok());

        // A negative lighting tolerance and a deep fragment recursion.
        let mut negative = ok.clone();
        negative.beam.lighting_tolerance = -1e-3;
        assert_eq!(
            negative.live_tiles(),
            Err(BeamTileError::Settings(BeamSettingsError::NotPositive(
                "lighting_tolerance"
            )))
        );
        let mut deep = ok.clone();
        deep.beam.max_fragment_levels = 40;
        assert!(matches!(
            deep.live_tiles(),
            Err(BeamTileError::Settings(BeamSettingsError::TooLarge { .. }))
        ));

        let mut no_batch = ok.clone();
        no_batch.beam.batch_tiles = 0;
        assert_eq!(no_batch.live_tiles(), Err(BeamTileError::BatchTiles));

        let mut too_many_spp = ok.clone();
        too_many_spp.adaptive.max_spp = MAX_BEAM_TILE_SPP + 1;
        assert_eq!(
            too_many_spp.live_tiles(),
            Err(BeamTileError::MaxSpp(MAX_BEAM_TILE_SPP + 1))
        );

        let mut adaptive = ok.clone();
        adaptive.adaptive = AdaptiveSettings::provisional(0);
        assert!(matches!(
            adaptive.live_tiles(),
            Err(BeamTileError::Adaptive(_))
        ));

        let mut empty = ok.clone();
        empty.tiles.mask = vec![0; empty.tiles.mask.len()];
        assert_eq!(
            empty.live_tiles(),
            Err(BeamTileError::Tiles(LivePassError::Empty))
        );

        // A set built for a larger frame does not fit this one.
        let mut outside = ok.clone();
        outside.scene.width = 96;
        assert!(matches!(
            outside.live_tiles(),
            Err(BeamTileError::Tiles(LivePassError::MaskLength { .. }))
        ));

        // 30 tiles use bits 0..=29 of 32: bit 31 is outside the frame.
        let mut stray = ok.clone();
        stray.tiles.mask[3] |= 0b1000_0000;
        assert_eq!(
            stray.live_tiles(),
            Err(BeamTileError::Tiles(LivePassError::StrayBit))
        );

        let mut bad_tile = ok;
        bad_tile.tiles.tile = 12;
        assert_eq!(
            bad_tile.live_tiles(),
            Err(BeamTileError::Tiles(LivePassError::Tile(12)))
        );
    }

    #[test]
    fn a_request_above_the_pixel_cap_is_refused_and_one_at_the_cap_is_not() {
        // 4100 x 1024 pixels live is more than 4 Mi.
        let mut big = request(8, 8, vec![0]);
        big.scene.width = 4100;
        big.scene.height = 1024;
        big.tiles = LiveTileSet::from_live(&LiveTiles::all(4100, 1024, 8));
        assert!(matches!(
            big.live_tiles(),
            Err(BeamTileError::Tiles(LivePassError::TooManyPixels { .. }))
        ));
        // 4096 x 1024 is exactly 4 Mi pixels.
        let mut exact = big;
        exact.scene.width = 4096;
        exact.tiles = LiveTileSet::from_live(&LiveTiles::all(4096, 1024, 8));
        assert_eq!(
            exact.live_tiles().unwrap().pixel_count(),
            MAX_BEAM_TILE_PIXELS
        );
    }

    #[test]
    fn the_message_text_names_the_reason() {
        assert!(
            BeamTileError::SpectralNodes(12)
                .to_string()
                .contains("multiple of 8")
        );
        assert!(
            BeamTileError::Tiles(LivePassError::Empty)
                .to_string()
                .contains("names no tile")
        );
    }
}
