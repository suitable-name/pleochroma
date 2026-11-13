//! `-> LIVE_PASS_REQUEST`: one chunk of an adaptive-sampling pass.
//!
//! An adaptive render runs in passes; every pass adds the same absolute samples to every pixel of
//! the tiles that are still live (`pleochroma::renderer::adaptive`). A coordinator-side export
//! splits the live tiles between its engines by tile, and a remote engine gets its share as one
//! (or a few) of these requests:
//!
//! ```text
//! -> LivePassRequest { request_id, scene, first_sample, samples, live: LiveTileSet }
//! <- PROGRESS        { request_id, samples_done }                       -- heartbeat, ~every 2 s
//! <- LIVE_PASS       { request_id, first_sample, samples, live_pixels, payload_len } + payload
//!    or ERROR        { code, message, request_id }
//!    or DONE         { request_id, cancelled: true, .. }                -- after a CANCEL
//! ```
//!
//! # A separate request, not a field of `RenderRequest`
//!
//! A `RenderRequest` is today's chunk: a sample range over the whole frame, answered by summed
//! `FRAME`s. Appending a field to it would change every request's bytes (postcard is positional),
//! live tiles or not. A live pass is its own message instead (`ClientMessage::LivePassRequest`,
//! index 11) with its own reply, so a `RenderRequest` encodes to exactly the bytes it always did.
//!
//! # The live-tile set: a bitmask
//!
//! [`LiveTileSet`] is the tile edge and a bitmask over the frame's tiles in row-major tile order
//! (tile `i` is bit `i % 8` of byte `i / 8`, least significant bit first). The frame size is the
//! scene's, so the mask needs no dimensions of its own. A bitmask is the smallest encoding that
//! does not depend on how many tiles are live: a 4K frame of 16-pixel tiles has 32 400 tiles and
//! a 4 050-byte mask whether one tile or all of them are live, where a list of `u32` indices is
//! 130 kB when everything is live. The encoding is canonical (no bits past the last tile), so one
//! set has exactly one byte string.
//!
//! # The reply payload
//!
//! `payload_len = live_pixels * 24` raw bytes (the radiance is not compressed in this version):
//! for each live pixel, tiles ascending and pixels row-major inside a tile, six little-endian
//! `f32`: the pass total X, Y, Z and then the total of the odd absolute sample indices (half
//! buffer B) X, Y, Z. The totals are the PASS's own sums from zero, in ascending sample order, the
//! very values a local pass adds to its buffers, so adding them is bit for bit what a local pass
//! does.
//!
//! Only compiled under this crate's `render` feature, like `RenderRequest`.

use crate::scene::SceneState;
use glam::Vec3;
use pleochroma::renderer::adaptive::{LiveTiles, MAX_TILE, MIN_TILE};
use serde::{Deserialize, Serialize};

/// Bytes per live pixel in a reply payload: six little-endian `f32`.
pub const LIVE_PIXEL_BYTES: usize = 24;

/// The most live pixels one [`LivePassRequest`] may name (4 Mi pixels: a 100 MB reply payload).
///
/// A sender with more live pixels splits them over several requests.
pub const MAX_LIVE_PASS_PIXELS: u64 = 4 * 1024 * 1024;

/// The bit of a [`LivePassRequest::request_id`] that asks the worker to shade the stone from a
/// path cache of its own.
///
/// The id is only a label the worker echoes on every reply, so a client that sets this bit on the
/// ids it chooses reads the replies by the same ids. The bit adds no field and no message. The
/// worker builds and keeps its own cache for the tiles it is given; no cache bytes travel.
pub const PATH_CACHE_FLAG: u32 = 1 << 31;

/// `id` as a request id, with [`PATH_CACHE_FLAG`] set when `path_cache` is `true`.
#[must_use]
pub const fn request_id_with_path_cache(id: u32, path_cache: bool) -> u32 {
    if path_cache {
        id | PATH_CACHE_FLAG
    } else {
        id & !PATH_CACHE_FLAG
    }
}

/// Why a [`LivePassRequest`] or its reply cannot be used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LivePassError {
    /// The tile edge is not a power of two from `MIN_TILE` to `MAX_TILE`.
    Tile(u32),
    /// The mask is not exactly one bit per tile of the scene's frame, rounded up to bytes.
    MaskLength {
        /// The byte count the scene's frame needs.
        expected: u64,
        /// The byte count the mask has.
        actual: usize,
    },
    /// A bit past the last tile is set.
    StrayBit,
    /// No tile is live.
    Empty,
    /// More live pixels than [`MAX_LIVE_PASS_PIXELS`].
    TooManyPixels {
        /// The live pixels named.
        pixels: u64,
    },
    /// The request asks for no samples.
    NoSamples,
    /// A reply payload is not `live_pixels * 24` bytes.
    PayloadLength {
        /// The byte count the live pixels need.
        expected: usize,
        /// The byte count received.
        actual: usize,
    },
    /// A reply holds a non-finite or negative radiance component.
    BadValue,
}

impl std::fmt::Display for LivePassError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tile(tile) => write!(
                f,
                "live-pass tile size {tile} must be a power of two from {MIN_TILE} to {MAX_TILE}"
            ),
            Self::MaskLength { expected, actual } => write!(
                f,
                "the live-tile mask has {actual} bytes, the frame's tiles need {expected}"
            ),
            Self::StrayBit => write!(f, "the live-tile mask sets a bit past the last tile"),
            Self::Empty => write!(f, "the live-tile mask names no tile"),
            Self::TooManyPixels { pixels } => write!(
                f,
                "a live pass may name at most {MAX_LIVE_PASS_PIXELS} pixels (got {pixels})"
            ),
            Self::NoSamples => write!(f, "a live pass needs at least one sample"),
            Self::PayloadLength { expected, actual } => write!(
                f,
                "the live-pass payload has {actual} bytes, its pixels need {expected}"
            ),
            Self::BadValue => write!(
                f,
                "the live-pass payload holds a non-finite or negative radiance value"
            ),
        }
    }
}

impl std::error::Error for LivePassError {}

/// The tiles of a frame that receive samples in a pass, as a bitmask -- see the module doc.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LiveTileSet {
    /// Tile edge in pixels.
    pub tile: u32,
    /// One bit per tile in row-major tile order, least significant bit first.
    pub mask: Vec<u8>,
}

/// The number of tiles of a `width` x `height` frame cut into `tile`-pixel tiles.
fn tile_count(width: u32, height: u32, tile: u32) -> u64 {
    let tile = tile.max(1);
    u64::from(width.div_ceil(tile)) * u64::from(height.div_ceil(tile))
}

impl LiveTileSet {
    /// The set naming exactly the tiles of `live`.
    #[must_use]
    pub fn from_live(live: &LiveTiles) -> Self {
        let tiles = tile_count(live.width(), live.height(), live.tile());
        let len = usize::try_from(tiles.div_ceil(8)).unwrap_or(0);
        let mut mask = vec![0_u8; len];
        for &index in live.indices() {
            if let Some(byte) = mask.get_mut((index / 8) as usize) {
                *byte |= 1 << (index % 8);
            }
        }
        Self {
            tile: live.tile(),
            mask,
        }
    }

    /// The live tiles of a `width` x `height` frame this set names.
    ///
    /// # Errors
    ///
    /// A [`LivePassError`] for a tile edge out of range, a mask of the wrong length or with a bit
    /// past the last tile, or a set that names no tile.
    pub fn to_live(&self, width: u32, height: u32) -> Result<LiveTiles, LivePassError> {
        if !self.tile.is_power_of_two() || !(MIN_TILE..=MAX_TILE).contains(&self.tile) {
            return Err(LivePassError::Tile(self.tile));
        }
        let tiles = tile_count(width, height, self.tile);
        let expected = tiles.div_ceil(8);
        if u64::try_from(self.mask.len()).ok() != Some(expected) {
            return Err(LivePassError::MaskLength {
                expected,
                actual: self.mask.len(),
            });
        }
        let mut indices = Vec::new();
        for (byte_at, &byte) in self.mask.iter().enumerate() {
            if byte == 0 {
                continue;
            }
            for bit in 0..8_u32 {
                if (byte >> bit) & 1 == 0 {
                    continue;
                }
                let index = byte_at as u64 * 8 + u64::from(bit);
                if index >= tiles {
                    return Err(LivePassError::StrayBit);
                }
                indices.push(u32::try_from(index).map_err(|_| LivePassError::StrayBit)?);
            }
        }
        if indices.is_empty() {
            return Err(LivePassError::Empty);
        }
        Ok(LiveTiles::from_indices(width, height, self.tile, indices))
    }
}

/// `-> LIVE_PASS_REQUEST`: trace `samples` absolute samples from `first_sample` for the pixels of
/// the `live` tiles of `scene`'s frame -- see the module doc for the reply.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LivePassRequest {
    /// Client-chosen id, echoed on every reply.
    pub request_id: u32,
    /// The fully resolved scene to trace (zones travel ahead as a `ZoningPayload`).
    pub scene: SceneState,
    /// The absolute index of the pass's first sample.
    pub first_sample: u32,
    /// Samples per pixel in the pass.
    pub samples: u32,
    /// The tiles that receive them.
    pub live: LiveTileSet,
}

impl LivePassRequest {
    /// Whether the client asks for a path cache ([`PATH_CACHE_FLAG`] in the request id).
    #[must_use]
    pub const fn path_cache(&self) -> bool {
        self.request_id & PATH_CACHE_FLAG != 0
    }

    /// The live tiles this request names on its scene's frame, checked against every limit.
    ///
    /// # Errors
    ///
    /// A [`LivePassError`] for no samples, a bad set, or more than [`MAX_LIVE_PASS_PIXELS`]
    /// live pixels.
    pub fn live_tiles(&self) -> Result<LiveTiles, LivePassError> {
        if self.samples == 0 {
            return Err(LivePassError::NoSamples);
        }
        let live = self.live.to_live(self.scene.width, self.scene.height)?;
        let pixels = live.pixel_count();
        if pixels > MAX_LIVE_PASS_PIXELS {
            return Err(LivePassError::TooManyPixels { pixels });
        }
        Ok(live)
    }
}

/// The `(pass total, odd-sample total)` of every live pixel of `live`, read from full-frame
/// buffers, in the payload's pixel order (tiles ascending, row-major inside a tile).
///
/// A pixel outside the buffers reads as zero.
#[must_use]
pub fn gather_live(live: &LiveTiles, sum: &[Vec3], half_b: &[Vec3]) -> Vec<(Vec3, Vec3)> {
    let width = live.width() as usize;
    let mut out = Vec::with_capacity(usize::try_from(live.pixel_count()).unwrap_or(0));
    for &tile in live.indices() {
        let (x0, y0, x1, y1) = live.tile_rect(tile);
        for y in y0..y1 {
            for x in x0..x1 {
                let pixel = y as usize * width + x as usize;
                out.push((
                    sum.get(pixel).copied().unwrap_or(Vec3::ZERO),
                    half_b.get(pixel).copied().unwrap_or(Vec3::ZERO),
                ));
            }
        }
    }
    out
}

/// Adds `pixels` (in the payload's pixel order) into the full-frame buffers `sum` and `half_b`.
///
/// Returns `false`, touching nothing, when `pixels` is not one entry per live pixel of `live` or
/// a buffer is not `width * height` long.
pub fn scatter_live(
    live: &LiveTiles,
    pixels: &[(Vec3, Vec3)],
    sum: &mut [Vec3],
    half_b: &mut [Vec3],
) -> bool {
    let width = live.width() as usize;
    let frame = width * live.height() as usize;
    if usize::try_from(live.pixel_count()).ok() != Some(pixels.len())
        || sum.len() != frame
        || half_b.len() != frame
    {
        return false;
    }
    let mut pixels = pixels.iter();
    for &tile in live.indices() {
        let (x0, y0, x1, y1) = live.tile_rect(tile);
        for y in y0..y1 {
            for x in x0..x1 {
                let Some(&(total, odd)) = pixels.next() else {
                    return true;
                };
                let pixel = y as usize * width + x as usize;
                sum[pixel] += total;
                half_b[pixel] += odd;
            }
        }
    }
    true
}

/// The reply payload of `pixels`: [`LIVE_PIXEL_BYTES`] bytes per pixel -- see the module doc.
#[must_use]
pub fn encode_live_sums(pixels: &[(Vec3, Vec3)]) -> Vec<u8> {
    let mut out = Vec::with_capacity(pixels.len() * LIVE_PIXEL_BYTES);
    for (total, odd) in pixels {
        for value in [total.x, total.y, total.z, odd.x, odd.y, odd.z] {
            out.extend_from_slice(&value.to_le_bytes());
        }
    }
    out
}

/// Decodes a reply payload of `pixels` live pixels.
///
/// # Errors
///
/// [`LivePassError::PayloadLength`] if `bytes` is not `pixels * 24` long, or
/// [`LivePassError::BadValue`] for a non-finite or negative component.
pub fn decode_live_sums(bytes: &[u8], pixels: usize) -> Result<Vec<(Vec3, Vec3)>, LivePassError> {
    let expected = pixels.saturating_mul(LIVE_PIXEL_BYTES);
    if bytes.len() != expected {
        return Err(LivePassError::PayloadLength {
            expected,
            actual: bytes.len(),
        });
    }
    let mut out = Vec::with_capacity(pixels);
    for chunk in bytes.as_chunks::<LIVE_PIXEL_BYTES>().0 {
        let mut values = [0.0_f32; 6];
        for (value, raw) in values.iter_mut().zip(chunk.as_chunks::<4>().0) {
            *value = f32::from_le_bytes(*raw);
        }
        if values.iter().any(|v| !v.is_finite() || *v < 0.0) {
            return Err(LivePassError::BadValue);
        }
        out.push((
            Vec3::new(values[0], values[1], values[2]),
            Vec3::new(values[3], values[4], values[5]),
        ));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::{
        super::{
            ClientMessage, RenderRequest, RequestIntent, StreamConfig, StreamEvent, TransferMode,
            codec::{read_message, write_message},
        },
        *,
    };

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
        }
    }

    fn request(width: u32, height: u32, tiles: Vec<u32>) -> LivePassRequest {
        LivePassRequest {
            request_id: 7,
            scene: scene(width, height),
            first_sample: 33,
            samples: 16,
            live: LiveTileSet::from_live(&LiveTiles::from_indices(width, height, 8, tiles)),
        }
    }

    #[test]
    fn a_request_round_trips_through_the_framed_codec() {
        // 48 x 40 with 8-pixel tiles: 6 x 5 = 30 tiles, so tile 29 exists.
        let request = request(48, 40, vec![0, 3, 7, 29]);
        let mut buf = Vec::new();
        write_message(&mut buf, &request).unwrap();
        let decoded: LivePassRequest = read_message(&mut std::io::Cursor::new(buf)).unwrap();
        assert_eq!(decoded, request);
        let live = decoded.live_tiles().unwrap();
        assert_eq!(live.indices(), [0, 3, 7, 29]);
    }

    #[test]
    fn the_mask_is_one_bit_per_tile_least_significant_bit_first() {
        // 40 x 36 with 8-pixel tiles: 5 x 5 = 25 tiles, 4 mask bytes.
        let set = request(40, 36, vec![0, 1, 9, 24]).live;
        assert_eq!(set.tile, 8);
        assert_eq!(set.mask, vec![0b0000_0011, 0b0000_0010, 0, 0b0000_0001]);
    }

    #[test]
    fn the_mask_size_depends_on_the_frame_and_not_on_how_many_tiles_are_live() {
        let one = LiveTileSet::from_live(&LiveTiles::from_indices(3840, 2160, 16, vec![5]));
        let all = LiveTileSet::from_live(&LiveTiles::all(3840, 2160, 16));
        assert_eq!(one.mask.len(), 4050);
        assert_eq!(all.mask.len(), 4050);
    }

    #[test]
    fn every_subset_of_a_small_frame_survives_the_mask() {
        let (width, height) = (20, 12); // 3 x 2 = 6 tiles at 8 pixels
        for bits in 1_u32..(1 << 6) {
            let tiles: Vec<u32> = (0..6).filter(|i| ((bits >> i) & 1) == 1).collect();
            let live = LiveTiles::from_indices(width, height, 8, tiles);
            let set = LiveTileSet::from_live(&live);
            assert_eq!(set.to_live(width, height).unwrap(), live, "bits {bits:#b}");
        }
    }

    #[test]
    fn a_bad_set_is_refused_with_its_reason() {
        let ok = request(40, 36, vec![2]).live;
        let mut bad_tile = ok.clone();
        bad_tile.tile = 12;
        assert_eq!(bad_tile.to_live(40, 36), Err(LivePassError::Tile(12)));
        bad_tile.tile = 128;
        assert_eq!(bad_tile.to_live(40, 36), Err(LivePassError::Tile(128)));

        let mut short = ok.clone();
        short.mask.pop();
        assert_eq!(
            short.to_live(40, 36),
            Err(LivePassError::MaskLength {
                expected: 4,
                actual: 3
            })
        );
        // The same mask on a frame with more tiles is the wrong length too.
        assert!(matches!(
            ok.to_live(80, 36),
            Err(LivePassError::MaskLength { .. })
        ));

        // 25 tiles use bits 0..=24 of 32: bit 25 is stray.
        let mut stray = ok.clone();
        stray.mask[3] |= 0b0000_0010;
        assert_eq!(stray.to_live(40, 36), Err(LivePassError::StrayBit));

        let mut empty = ok;
        empty.mask = vec![0; 4];
        assert_eq!(empty.to_live(40, 36), Err(LivePassError::Empty));
    }

    #[test]
    fn a_request_without_samples_or_with_too_many_pixels_is_refused() {
        let mut none = request(40, 36, vec![0]);
        none.samples = 0;
        assert_eq!(none.live_tiles(), Err(LivePassError::NoSamples));

        // 4097 x 1024 pixels live is more than 4 Mi.
        let mut big = request(8, 8, vec![0]);
        big.scene.width = 4100;
        big.scene.height = 1024;
        big.live = LiveTileSet::from_live(&LiveTiles::all(4100, 1024, 8));
        assert!(matches!(
            big.live_tiles(),
            Err(LivePassError::TooManyPixels { .. })
        ));
    }

    #[test]
    fn the_new_message_is_appended_and_a_render_request_keeps_its_bytes() {
        let live = ClientMessage::LivePassRequest(Box::new(request(40, 36, vec![0])));
        let tagged = postcard::to_allocvec(&live).unwrap();
        assert_eq!(tagged[0], 11, "appended after ClientMessage::Job (10)");
        let decoded: ClientMessage = postcard::from_bytes(&tagged).unwrap();
        assert_eq!(decoded, live);
        // Older indices did not move.
        let render = RenderRequest {
            request_id: 5,
            scene: scene(4, 4),
            first_sample: 3,
            samples: 9,
            stream: StreamConfig {
                transfer_mode: TransferMode::FinalOnly,
                cadence_ms: 1000,
                preview: None,
            },
            intent: RequestIntent::Batch,
        };
        let tagged =
            postcard::to_allocvec(&ClientMessage::RenderRequest(Box::new(render.clone()))).unwrap();
        assert_eq!(tagged[0], 2);
        // A RenderRequest: its fields in order and nothing else (no live-tile field).
        let fields = postcard::to_allocvec(&(
            render.request_id,
            &render.scene,
            render.first_sample,
            render.samples,
            render.stream,
            render.intent,
        ))
        .unwrap();
        assert_eq!(postcard::to_allocvec(&render).unwrap(), fields);
        assert_eq!(&tagged[1..], fields.as_slice());
    }

    #[test]
    fn the_reply_event_is_appended_after_the_batch_events() {
        let event = StreamEvent::LivePass(super::super::LivePassHeader {
            request_id: 4,
            first_sample: 16,
            samples: 16,
            live_pixels: 64,
            payload_len: 64 * 24,
        });
        let tagged = postcard::to_allocvec(&event).unwrap();
        assert_eq!(tagged[0], 14, "appended after BatchDone (13)");
        assert_eq!(event.payload_len(), Some(64 * 24));
        assert_eq!(event.request_id(), Some(4));
        let mut buf = Vec::new();
        let payload = vec![0_u8; 64 * 24];
        crate::messages::write_stream_event(&mut buf, &event, Some(&payload)).unwrap();
        let (decoded, body) =
            crate::messages::read_stream_event(&mut std::io::Cursor::new(buf)).unwrap();
        assert_eq!(decoded, event);
        assert_eq!(body.as_deref(), Some(payload.as_slice()));
    }

    fn sample_pixels() -> Vec<(Vec3, Vec3)> {
        (0_u8..10)
            .map(|i| {
                let f = f32::from(i);
                (
                    Vec3::new(f, f + 0.5, f * 2.0),
                    Vec3::new(f * 0.25, 0.0, 1.0),
                )
            })
            .collect()
    }

    /// A pixel index as a float (frames in these tests are far below 2^16 pixels).
    fn index_f32(index: usize) -> f32 {
        f32::from(u16::try_from(index).unwrap())
    }

    #[test]
    fn the_payload_round_trips_bit_for_bit() {
        let pixels = sample_pixels();
        let bytes = encode_live_sums(&pixels);
        assert_eq!(bytes.len(), pixels.len() * LIVE_PIXEL_BYTES);
        assert_eq!(decode_live_sums(&bytes, pixels.len()).unwrap(), pixels);
        // The first pixel's first float is the pass total X, little endian.
        assert_eq!(bytes[..4], pixels[0].0.x.to_le_bytes());
    }

    #[test]
    fn a_bad_payload_is_refused() {
        let pixels = sample_pixels();
        let bytes = encode_live_sums(&pixels);
        assert_eq!(
            decode_live_sums(&bytes[..bytes.len() - 1], pixels.len()),
            Err(LivePassError::PayloadLength {
                expected: 240,
                actual: 239
            })
        );
        for bad in [f32::NAN, f32::INFINITY, -1.0] {
            let mut broken = bytes.clone();
            broken[8..12].copy_from_slice(&bad.to_le_bytes());
            assert_eq!(
                decode_live_sums(&broken, pixels.len()),
                Err(LivePassError::BadValue)
            );
        }
    }

    #[test]
    fn gather_and_scatter_walk_the_tiles_in_payload_order() {
        // 12 x 10 with 8-pixel tiles: tiles 0, 1 (clipped to 4 wide), 2 (2 high), 3 (4 x 2).
        let (width, height) = (12_u32, 10_u32);
        let frame = (width * height) as usize;
        let sum: Vec<Vec3> = (0..frame).map(|i| Vec3::splat(index_f32(i))).collect();
        let odd: Vec<Vec3> = (0..frame)
            .map(|i| Vec3::splat(index_f32(i) * 0.5))
            .collect();
        let live = LiveTiles::from_indices(width, height, 8, vec![1, 2]);
        let gathered = gather_live(&live, &sum, &odd);
        assert_eq!(gathered.len(), 4 * 8 + 8 * 2);
        // Tile 1 is x 8..12, y 0..8: its first pixel is (8, 0), its second row starts (8, 1).
        assert_eq!(gathered[0].0, Vec3::splat(8.0));
        assert_eq!(gathered[4].0, Vec3::splat(20.0));
        // Tile 2 is x 0..8, y 8..10: after the 32 pixels of tile 1.
        assert_eq!(gathered[32].0, Vec3::splat(96.0));

        let mut got_sum = vec![Vec3::ZERO; frame];
        let mut got_odd = vec![Vec3::ZERO; frame];
        assert!(scatter_live(&live, &gathered, &mut got_sum, &mut got_odd));
        for (pixel, (s, o)) in got_sum.iter().zip(&got_odd).enumerate() {
            let pixel_at = u32::try_from(pixel).unwrap();
            let (x, y) = (pixel_at % width, pixel_at / width);
            let live_pixel = (x >= 8 && y < 8) || (x < 8 && y >= 8);
            assert_eq!(*s, if live_pixel { sum[pixel] } else { Vec3::ZERO });
            assert_eq!(*o, if live_pixel { odd[pixel] } else { Vec3::ZERO });
        }
        // Scattering adds: twice gives twice.
        assert!(scatter_live(&live, &gathered, &mut got_sum, &mut got_odd));
        assert_eq!(got_sum[8], sum[8] * 2.0);
    }

    #[test]
    fn scatter_refuses_a_payload_that_does_not_fit_and_touches_nothing() {
        let live = LiveTiles::from_indices(16, 16, 8, vec![0]);
        let mut sum = vec![Vec3::ZERO; 256];
        let mut odd = vec![Vec3::ZERO; 256];
        assert!(!scatter_live(&live, &sample_pixels(), &mut sum, &mut odd));
        let right = vec![(Vec3::ONE, Vec3::ONE); 64];
        assert!(!scatter_live(&live, &right, &mut sum[..255], &mut odd));
        assert!(sum.iter().chain(&odd).all(|v| *v == Vec3::ZERO));
    }

    #[test]
    fn the_path_cache_flag_rides_in_the_request_id_and_changes_no_other_byte() {
        let plain = request(40, 36, vec![0, 5]);
        assert!(!plain.path_cache());
        assert_eq!(request_id_with_path_cache(7, false), 7);
        assert_eq!(request_id_with_path_cache(7, true), 7 | PATH_CACHE_FLAG);
        assert_eq!(
            request_id_with_path_cache(7 | PATH_CACHE_FLAG, false),
            7,
            "the flag can be taken off again"
        );

        let flagged = LivePassRequest {
            request_id: request_id_with_path_cache(plain.request_id, true),
            ..plain.clone()
        };
        assert!(flagged.path_cache());
        // It round-trips whole, flag included.
        let mut buf = Vec::new();
        write_message(&mut buf, &flagged).unwrap();
        let decoded: LivePassRequest = read_message(&mut std::io::Cursor::new(buf)).unwrap();
        assert_eq!(decoded, flagged);
        assert!(decoded.path_cache());
        assert_eq!(decoded.live_tiles().unwrap().indices(), [0, 5]);

        // Same message, same layout: only the id differs, and a request without the flag is
        // exactly today's bytes (its fields in order and nothing else).
        let fields = postcard::to_allocvec(&(
            plain.request_id,
            &plain.scene,
            plain.first_sample,
            plain.samples,
            &plain.live,
        ))
        .unwrap();
        assert_eq!(postcard::to_allocvec(&plain).unwrap(), fields);
        let tagged =
            postcard::to_allocvec(&ClientMessage::LivePassRequest(Box::new(flagged))).unwrap();
        assert_eq!(tagged[0], 11, "still the one live-pass message");
        let untagged =
            postcard::to_allocvec(&ClientMessage::LivePassRequest(Box::new(plain))).unwrap();
        assert_eq!(untagged[0], 11);
        assert_eq!(
            &tagged[6..],
            &untagged[2..],
            "everything after the id (a five-byte and a one-byte varint) is the same bytes"
        );
    }
}
