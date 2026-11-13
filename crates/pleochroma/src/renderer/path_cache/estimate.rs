//! The size estimate of a path cache, from a cheap pre-pass.
//!
//! Before building a cache the renderer wants to know how large it will be, to choose where it can
//! live ([`super::placement`]) and to tell the user. The pre-pass builds the first level of
//! sub-pixel positions (`K0`) of a lattice of at most [`PROBE_PIXELS`] pixels of the frame itself,
//! which is deterministic and cheap, and measures three things on them: the share of stone pixels,
//! the mean number of exit records per position, and the share of edge pixels (those whose
//! positions disagree on the facet sequence and would refine). Edge pixels are assumed to refine to
//! [`K_EDGE_MEAN`] positions on average, interior pixels stay at `K0`. See guide section 8.7.

use std::sync::atomic::{AtomicU32, Ordering};

use super::{
    build::TraceKit,
    layout::{HEADER_WORDS, RECORD_WORDS, tile_count},
    positions::{BuildSettings, PositionData, build_pixel},
    record::{f16_bits_to_f32, f32_to_f16_bits},
};
use crate::{
    optics::raytracer::transport::PreparedMaterial, renderer::frame_scene::FrameScene,
    simd::PlanesSoA32,
};

/// Mean positions an edge pixel ends with after refinement (`K0` doubling up to `K_MAX`).
///
/// Provisional: not yet calibrated against measured refinement (guide section 14).
pub const K_EDGE_MEAN: f64 = 24.0;

/// Assumed compression ratio of a persisted cache; not yet measured (guide section 8.7).
pub const COMPRESSION_RATIO_DEFAULT: f64 = 2.0;

/// The most pixels the pre-pass probes: a 32 x 32 lattice on a square frame.
///
/// Bounds the pre-pass cost; a 512 x 512 frame is probed at one pixel in 256.
pub const PROBE_PIXELS: u32 = 1024;

/// Bytes of one exit record in the tile buffer.
const RECORD_BYTES: u64 = (RECORD_WORDS * size_of::<u32>()) as u64;

/// Bytes per position besides its records: one offset word and one `f16` residual.
const POSITION_BYTES: u64 = 6;

/// Bytes per pixel: one offset word.
const PIXEL_BYTES: u64 = 4;

/// Bytes per tile besides pixels and positions: the header and the two trailing offset words.
const TILE_BYTES: u64 = (HEADER_WORDS as u64 + 2) * 4;

/// What the pre-pass counted on the probed pixels.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Measurements {
    /// Probed pixels.
    pub pixels: u64,
    /// Pixels with at least one position (the stone and its silhouette).
    pub stone_pixels: u64,
    /// Stone pixels whose positions disagree on the dominant facet sequence.
    pub edge_pixels: u64,
    /// Positions over all stone pixels.
    pub positions: u64,
    /// Exit records over all positions.
    pub records: u64,
    /// Σ over positions of the residual completion's trace probability
    /// `min(1, RESIDUAL_FIRE_GAIN · residual)` (zero below `RESIDUAL_FLOOR`), in thousandths.
    pub fire_milli: u64,
}

impl Measurements {
    /// Adds the six counters of `other` (integer sums, so the order does not matter).
    const fn merge(&mut self, other: &Self) {
        self.pixels += other.pixels;
        self.stone_pixels += other.stone_pixels;
        self.edge_pixels += other.edge_pixels;
        self.positions += other.positions;
        self.records += other.records;
        self.fire_milli += other.fire_milli;
    }

    /// Adds one probed pixel: its first-level positions (empty for a pixel off the stone).
    fn add_pixel(&mut self, positions: &[PositionData]) {
        self.pixels += 1;
        let Some(first) = positions.first() else {
            return;
        };
        self.stone_pixels += 1;
        self.positions += positions.len() as u64;
        let mut edge = false;
        for position in positions {
            self.records += position.records.len() as u64;
            // The residual as the tile stores it (an f16).
            self.fire_milli += fire_milli(f16_bits_to_f32(f32_to_f16_bits(position.residual)));
            edge |= position.signature != first.signature;
        }
        self.edge_pixels += u64::from(edge);
    }
}

/// The residual completion's trace probability of a position with `residual`, in thousandths
/// (the rule of `shade::Shader`: `min(1, RESIDUAL_FIRE_GAIN · residual)`, zero below the floor).
fn fire_milli(residual: f32) -> u64 {
    if residual.is_nan() || residual < super::shade::RESIDUAL_FLOOR {
        return 0;
    }
    let probability = (super::shade::RESIDUAL_FIRE_GAIN * residual).min(1.0);
    to_u64(f64::from(probability) * 1000.0)
}

/// A `u64` count as a float; counts here stay far below the 2^53 where the conversion is exact.
#[expect(
    clippy::cast_precision_loss,
    reason = "byte and pixel counts are far below 2^53"
)]
const fn to_f64(value: u64) -> f64 {
    value as f64
}

/// A non-negative float, rounded, as a count; saturates instead of wrapping.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is floored at zero first; `as` saturates at the top and maps NaN to 0"
)]
const fn to_u64(value: f64) -> u64 {
    value.round().max(0.0) as u64
}

/// The estimated size of the cache of a frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SizeEstimate {
    /// Pixels of the full frame.
    pub frame_pixels: u64,
    /// Stone pixels of the full frame, scaled up from the pre-pass.
    pub stone_pixels: u64,
    /// Share of stone pixels that are edge pixels, 0 to 1.
    pub edge_share: f64,
    /// Mean positions per stone pixel after refinement.
    pub positions_per_stone_pixel: f64,
    /// Mean exit records per position, as measured.
    pub records_per_position: f64,
    /// Positions of the whole cache.
    pub positions: u64,
    /// Exit records of the whole cache.
    pub records: u64,
    /// Size of the cache in memory, in bytes; 0 when there is no stone to cache.
    pub raw_bytes: u64,
    /// Size of the cache on disk after compression, in bytes.
    pub compressed_bytes: u64,
    /// The share of cached samples, 0 to 1, whose residual completion traces a full path. The GPU
    /// traces them itself (`shaders/path_cache/complete.wgsl`); the status line reports the share.
    pub fire_share: f64,
}

impl SizeEstimate {
    /// The estimate for a `width` by `height` frame from the pre-pass measurements.
    ///
    /// `compression_ratio` below 1 is treated as 1. A frame with no stone pixel gives all zeros.
    #[must_use]
    pub fn from_measurements(
        measured: &Measurements,
        frame: (u32, u32),
        settings: &BuildSettings,
        compression_ratio: f64,
    ) -> Self {
        let settings = settings.sanitized();
        let frame_pixels = u64::from(frame.0) * u64::from(frame.1);
        if measured.stone_pixels == 0 || measured.pixels == 0 || frame_pixels == 0 {
            return Self {
                frame_pixels,
                stone_pixels: 0,
                edge_share: 0.0,
                positions_per_stone_pixel: 0.0,
                records_per_position: 0.0,
                positions: 0,
                records: 0,
                raw_bytes: 0,
                compressed_bytes: 0,
                fire_share: 0.0,
            };
        }
        let stone_share = to_f64(measured.stone_pixels) / to_f64(measured.pixels);
        let stone_pixels = to_u64(stone_share * to_f64(frame_pixels));
        let edge_share = to_f64(measured.edge_pixels) / to_f64(measured.stone_pixels);
        let k0 = f64::from(settings.k0);
        let k_edge = K_EDGE_MEAN.clamp(k0, f64::from(settings.k_max));
        let positions_per_stone_pixel = k0.mul_add(1.0 - edge_share, k_edge * edge_share);
        let records_per_position = if measured.positions == 0 {
            0.0
        } else {
            to_f64(measured.records) / to_f64(measured.positions)
        };
        let positions = to_u64(to_f64(stone_pixels) * positions_per_stone_pixel);
        let records = to_u64(to_f64(positions) * records_per_position);
        let tiles = u64::from(tile_count(frame.0, frame.1, settings.tile));
        let raw_bytes = records * RECORD_BYTES
            + positions * POSITION_BYTES
            + frame_pixels * PIXEL_BYTES
            + tiles * TILE_BYTES;
        let compressed_bytes = to_u64(to_f64(raw_bytes) / compression_ratio.max(1.0));
        let fire_share = if measured.positions == 0 {
            0.0
        } else {
            to_f64(measured.fire_milli) / (1000.0 * to_f64(measured.positions))
        };
        Self {
            frame_pixels,
            stone_pixels,
            edge_share,
            positions_per_stone_pixel,
            records_per_position,
            positions,
            records,
            raw_bytes,
            compressed_bytes,
            fire_share,
        }
    }

    /// Whether the frame has any stone to cache.
    #[must_use]
    pub const fn has_stone(&self) -> bool {
        self.raw_bytes > 0
    }

    /// The size for people, such as "about 11 GB".
    #[must_use]
    pub fn size_text(&self) -> String {
        format!("about {}", format_bytes(self.raw_bytes))
    }
}

/// Decimal bytes for people: "850 MB", "4.2 GB", "11 GB"; below one megabyte "under 1 MB".
#[must_use]
pub fn format_bytes(bytes: u64) -> String {
    const MB: f64 = 1.0e6;
    const GB: f64 = 1.0e9;
    let value = to_f64(bytes);
    if value < MB {
        "under 1 MB".to_owned()
    } else if value < GB {
        format!("{:.0} MB", value / MB)
    } else if value < 10.0 * GB {
        format!("{:.1} GB", value / GB)
    } else {
        format!("{:.0} GB", value / GB)
    }
}

/// The lattice step of the pre-pass: the smallest step whose lattice has at most
/// [`PROBE_PIXELS`] pixels (1 for a frame that small).
#[must_use]
pub fn probe_step(width: u32, height: u32) -> u32 {
    let mut step = 1_u32;
    while u64::from(width.div_ceil(step)) * u64::from(height.div_ceil(step))
        > u64::from(PROBE_PIXELS)
    {
        step += 1;
    }
    step
}

/// Counts the pre-pass: phase A with the first level of positions only, on a lattice of at most
/// [`PROBE_PIXELS`] pixels of the frame ([`probe_step`] apart, the middle pixel of each cell).
///
/// `plane_soa` must be the intersection arena of `scene.planes` and `prepared` the prepared
/// form of `scene.material`. Deterministic: the same inputs give the same counts.
#[must_use]
pub fn measure_prepass<'a>(
    scene: &FrameScene<'a>,
    prepared: &PreparedMaterial<'a>,
    plane_soa: &PlanesSoA32,
    settings: &BuildSettings,
) -> Measurements {
    let settings = settings.sanitized();
    let mut measured = Measurements::default();
    if scene.width == 0 || scene.height == 0 {
        return measured;
    }
    let first_level = BuildSettings {
        k_max: settings.k0,
        ..settings
    };
    let kit = TraceKit::new(
        scene.planes,
        plane_soa,
        prepared,
        (scene.max_bounces, first_level.cutoff),
        first_level.dispersion_gate,
    );
    let step = probe_step(scene.width, scene.height);
    let columns = scene.width.div_ceil(step);
    let total = columns * scene.height.div_ceil(step);
    let workers = std::thread::available_parallelism()
        .map_or(1, std::num::NonZero::get)
        .min(usize::try_from(total.max(1)).unwrap_or(1));
    let next = AtomicU32::new(0);
    let parts: Vec<Measurements> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..workers)
            .map(|_| {
                scope.spawn(|| {
                    let mut part = Measurements::default();
                    loop {
                        let index = next.fetch_add(1, Ordering::Relaxed);
                        if index >= total {
                            break part;
                        }
                        // The middle pixel of the lattice cell, clipped to the frame.
                        let x = ((index % columns) * step + step / 2).min(scene.width - 1);
                        let y = ((index / columns) * step + step / 2).min(scene.height - 1);
                        part.add_pixel(&build_pixel(&kit, scene, (x, y), &first_level));
                    }
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| {
                handle
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            })
            .collect()
    });
    for part in &parts {
        measured.merge(part);
    }
    measured
}

/// The size estimate of a frame: the pre-pass of [`measure_prepass`] scaled to the full frame with
/// the default compression ratio.
#[must_use]
pub fn estimate_size<'a>(
    scene: &FrameScene<'a>,
    prepared: &PreparedMaterial<'a>,
    plane_soa: &PlanesSoA32,
    settings: &BuildSettings,
) -> SizeEstimate {
    let measured = measure_prepass(scene, prepared, plane_soa, settings);
    SizeEstimate::from_measurements(
        &measured,
        (scene.width, scene.height),
        settings,
        COMPRESSION_RATIO_DEFAULT,
    )
}
