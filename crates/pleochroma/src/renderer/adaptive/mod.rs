//! Adaptive sampling: per-tile convergence on two interleaved half-images.
//!
//! A render with adaptive sampling runs in fixed-size **passes**. Every pass adds the same
//! samples, with the same absolute sample indices `first_sample..first_sample + spp`, to every
//! pixel of every **live** tile. Between passes the convergence check (`map`, `metric`) compares
//! the two half-images (even and odd absolute sample indices) of each live tile and stops the
//! tiles that are clean enough. Because tiles stop as a unit and never reopen, all live pixels
//! always share one sample count, so a pass is just the existing `(sample_offset, spp)`
//! accumulation restricted to the live tiles, plus the odd-index half-buffer.
//!
//! The shared contract between the CPU and GPU sides is:
//!
//! - [`LiveTiles`]: the row-major ordered list of live tiles for one pass;
//! - the pass output: the existing summed accumulator (`&mut [Vec3]`, one entry per pixel of the
//!   full frame) plus a half-buffer of the same length that receives only the samples with an
//!   **odd** absolute sample index. Pixels outside the live tiles are left untouched in both.

use std::fmt;

pub mod map;
pub mod metric;
pub mod run;

pub use map::{ConvergenceMap, TileState};
pub use run::{AdaptiveImage, AdaptiveRun, Pass, PendingCheck};

/// Smallest tile edge in pixels.
pub const MIN_TILE: u32 = 8;
/// Largest tile edge in pixels.
pub const MAX_TILE: u32 = 64;
/// The tile edge of [`AdaptiveSettings::provisional`], in pixels.
pub const DEFAULT_TILE: u32 = 16;

/// How the tile error is compared with the target noise.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum NoiseMetric {
    /// Tile error divided by the tile's mean `L*` (floored), so `0.01` means 1 %.
    #[default]
    RelativeLstar,
    /// Tile error as an absolute Delta E (CIEDE2000).
    AbsoluteDeltaE,
}

impl NoiseMetric {
    /// The provisional target of this metric (guide section 14): 1 % relative, Delta E 1 absolute.
    #[must_use]
    pub const fn default_target(self) -> f32 {
        match self {
            Self::RelativeLstar => 0.01,
            Self::AbsoluteDeltaE => 1.0,
        }
    }
}

/// The "Target Noise" choices of the app's export dialogs, in combo order: Fine, Standard, Draft.
///
/// Each entry is the label and the relative `L*` target of [`NoiseMetric::RelativeLstar`]
/// (`0.01` is 1 %). Standard equals [`NoiseMetric::default_target`]. The one place these values
/// live, so they can change here without touching the app.
pub const TARGET_NOISE_PRESETS: [(&str, f32); 3] =
    [("Fine", 0.005), ("Standard", 0.01), ("Draft", 0.02)];

/// The index of "Standard" in [`TARGET_NOISE_PRESETS`], what the dialogs start with.
pub const DEFAULT_TARGET_NOISE_PRESET: usize = 1;

/// Settings for adaptive sampling; `None` in a request means off.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct AdaptiveSettings {
    /// Error at which a tile stops, in the unit of [`Self::metric`].
    pub target_noise: f32,
    /// How [`Self::target_noise`] is measured.
    pub metric: NoiseMetric,
    /// No tile stops before this many samples per pixel.
    pub min_spp: u32,
    /// Hard cap: the user's samples-per-pixel setting.
    pub max_spp: u32,
    /// Samples added to every live pixel per pass.
    pub pass_spp: u32,
    /// Tile edge in pixels (a power of two in `MIN_TILE..=MAX_TILE`).
    pub tile: u32,
}

/// Why an [`AdaptiveSettings`] value was rejected.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AdaptiveSettingsError {
    /// `target_noise` is not finite or not in `0 < target_noise < 1` (relative) or
    /// `0 < target_noise` (absolute).
    TargetNoise(f32),
    /// The sample counts do not satisfy `1 <= pass_spp <= min_spp <= max_spp`.
    SampleCounts {
        /// The rejected pass size.
        pass_spp: u32,
        /// The rejected minimum.
        min_spp: u32,
        /// The rejected maximum.
        max_spp: u32,
    },
    /// `min_spp` is not a multiple of `pass_spp`.
    MinNotMultipleOfPass {
        /// The rejected pass size.
        pass_spp: u32,
        /// The rejected minimum.
        min_spp: u32,
    },
    /// `tile` is not a power of two in `MIN_TILE..=MAX_TILE`.
    Tile(u32),
}

impl fmt::Display for AdaptiveSettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::TargetNoise(value) => write!(
                f,
                "target noise {value} is out of range (above 0, and below 1 for the relative metric)"
            ),
            Self::SampleCounts {
                pass_spp,
                min_spp,
                max_spp,
            } => write!(
                f,
                "sample counts must satisfy 1 <= pass ({pass_spp}) <= minimum ({min_spp}) <= maximum ({max_spp})"
            ),
            Self::MinNotMultipleOfPass { pass_spp, min_spp } => write!(
                f,
                "the minimum samples ({min_spp}) must be a multiple of the pass size ({pass_spp})"
            ),
            Self::Tile(tile) => write!(
                f,
                "tile size {tile} must be a power of two from {MIN_TILE} to {MAX_TILE}"
            ),
        }
    }
}

impl std::error::Error for AdaptiveSettingsError {}

impl AdaptiveSettings {
    /// The provisional defaults of guide section 14 for a render capped at `max_spp`.
    ///
    /// Relative `L*` metric at 1 %, 16-sample passes, 16-pixel tiles and at least 512 samples;
    /// for a small `max_spp` the pass and the minimum shrink so the result stays valid
    /// (a `max_spp` of 0 is still rejected by [`Self::validate`]).
    #[must_use]
    pub fn provisional(max_spp: u32) -> Self {
        let pass_spp = 16_u32.min(max_spp.max(1));
        let min_spp = ((512_u32.min(max_spp) / pass_spp) * pass_spp).max(pass_spp);
        Self {
            target_noise: NoiseMetric::RelativeLstar.default_target(),
            metric: NoiseMetric::RelativeLstar,
            min_spp,
            max_spp,
            pass_spp,
            tile: DEFAULT_TILE,
        }
    }

    /// Checks the documented ranges (guide section 6.2).
    ///
    /// # Errors
    ///
    /// Returns the first rule the settings break.
    pub fn validate(&self) -> Result<(), AdaptiveSettingsError> {
        let noise_ok = self.target_noise.is_finite()
            && self.target_noise > 0.0
            && (self.metric == NoiseMetric::AbsoluteDeltaE || self.target_noise < 1.0);
        if !noise_ok {
            return Err(AdaptiveSettingsError::TargetNoise(self.target_noise));
        }
        if !(1 <= self.pass_spp && self.pass_spp <= self.min_spp && self.min_spp <= self.max_spp) {
            return Err(AdaptiveSettingsError::SampleCounts {
                pass_spp: self.pass_spp,
                min_spp: self.min_spp,
                max_spp: self.max_spp,
            });
        }
        if !self.min_spp.is_multiple_of(self.pass_spp) {
            return Err(AdaptiveSettingsError::MinNotMultipleOfPass {
                pass_spp: self.pass_spp,
                min_spp: self.min_spp,
            });
        }
        if !self.tile.is_power_of_two() || !(MIN_TILE..=MAX_TILE).contains(&self.tile) {
            return Err(AdaptiveSettingsError::Tile(self.tile));
        }
        Ok(())
    }
}

/// The live tiles of one pass, in row-major tile order.
///
/// Tiles at the right and bottom edges may be partial; [`Self::tile_rect`] clips them to the
/// frame.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LiveTiles {
    width: u32,
    height: u32,
    tile: u32,
    indices: Vec<u32>,
}

impl LiveTiles {
    /// Every tile of a `width` x `height` frame, live.
    #[must_use]
    pub fn all(width: u32, height: u32, tile: u32) -> Self {
        let tile = tile.max(1);
        let count = width.div_ceil(tile) * height.div_ceil(tile);
        Self {
            width,
            height,
            tile,
            indices: (0..count).collect(),
        }
    }

    /// The given tiles, live; `indices` are sorted and deduplicated, out-of-frame ones dropped.
    #[must_use]
    pub fn from_indices(width: u32, height: u32, tile: u32, mut indices: Vec<u32>) -> Self {
        let tile = tile.max(1);
        let count = width.div_ceil(tile) * height.div_ceil(tile);
        indices.retain(|&index| index < count);
        indices.sort_unstable();
        indices.dedup();
        Self {
            width,
            height,
            tile,
            indices,
        }
    }

    /// Frame width in pixels.
    #[must_use]
    pub const fn width(&self) -> u32 {
        self.width
    }

    /// Frame height in pixels.
    #[must_use]
    pub const fn height(&self) -> u32 {
        self.height
    }

    /// Tile edge in pixels.
    #[must_use]
    pub const fn tile(&self) -> u32 {
        self.tile
    }

    /// Tiles per row.
    #[must_use]
    pub const fn tiles_x(&self) -> u32 {
        self.width.div_ceil(self.tile)
    }

    /// Tile rows.
    #[must_use]
    pub const fn tiles_y(&self) -> u32 {
        self.height.div_ceil(self.tile)
    }

    /// The live tile indices (row-major, ascending).
    #[must_use]
    pub fn indices(&self) -> &[u32] {
        &self.indices
    }

    /// `true` when no tile is live.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// `true` when every tile of the frame is live.
    #[must_use]
    pub const fn is_full(&self) -> bool {
        self.indices.len() == (self.tiles_x() * self.tiles_y()) as usize
    }

    /// Pixel rectangle `(x0, y0, x1, y1)` (end-exclusive, clipped to the frame) of tile `index`.
    #[must_use]
    pub fn tile_rect(&self, index: u32) -> (u32, u32, u32, u32) {
        let tiles_x = self.tiles_x().max(1);
        let x0 = (index % tiles_x) * self.tile;
        let y0 = (index / tiles_x) * self.tile;
        (
            x0,
            y0,
            (x0 + self.tile).min(self.width),
            (y0 + self.tile).min(self.height),
        )
    }

    /// Number of pixels covered by the live tiles.
    #[must_use]
    pub fn pixel_count(&self) -> u64 {
        self.indices
            .iter()
            .map(|&index| {
                let (x0, y0, x1, y1) = self.tile_rect(index);
                u64::from(x1 - x0) * u64::from(y1 - y0)
            })
            .sum()
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_map;
#[cfg(test)]
mod tests_metric;
#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests_offset;
#[cfg(test)]
mod tests_render;
#[cfg(test)]
mod tests_run;
#[cfg(test)]
mod tests_tile_errors;
