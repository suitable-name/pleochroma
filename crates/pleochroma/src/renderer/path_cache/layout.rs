//! The memory layout of one cached tile (guide section 8.5).
//!
//! A tile is one contiguous `u32` buffer the GPU reads as a single storage buffer:
//!
//! ```text
//! words [0, 4)            TileCacheHeader { tile_index, pixel_count, position_count, record_count }
//! pixel_offsets           pixel_count + 1 words, into the positions
//! position_offsets        position_count + 1 words, into the records
//! position_residual       position_count f16, two per word (even index in the low half)
//! records                 record_count x 11 words (ExitRecord)
//! ```
//!
//! Pixels are the tile's clipped rectangle in row-major order. A pixel that is not a stone pixel
//! owns zero positions.

use std::ops::Range;

use super::record::{ExitRecord, f16_bits_to_f32};

/// Words in the header.
pub const HEADER_WORDS: usize = 4;

/// Words in one record.
pub const RECORD_WORDS: usize = size_of::<ExitRecord>() / size_of::<u32>();

/// The first four words of a tile buffer.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, bytemuck::Pod, bytemuck::Zeroable)]
pub struct TileCacheHeader {
    /// Row-major tile index in the frame.
    pub tile_index: u32,
    /// Pixels in the tile (the clipped rectangle).
    pub pixel_count: u32,
    /// Cached sub-pixel positions in the tile.
    pub position_count: u32,
    /// Exit records in the tile.
    pub record_count: u32,
}

/// Why a buffer is not a valid tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LayoutError {
    /// Fewer words than a header.
    TooShort {
        /// Words present.
        words: usize,
    },
    /// The buffer length differs from the length the header implies.
    SizeMismatch {
        /// Words the header implies.
        expected: usize,
        /// Words present.
        actual: usize,
    },
    /// `pixel_offsets` is not a non-decreasing prefix sum from 0 to the position count.
    PixelOffsets,
    /// `position_offsets` is not a non-decreasing prefix sum from 0 to the record count.
    PositionOffsets,
}

impl std::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match *self {
            Self::TooShort { words } => {
                write!(f, "a tile buffer needs a header, got {words} words")
            }
            Self::SizeMismatch { expected, actual } => {
                write!(
                    f,
                    "the header implies {expected} words, the buffer has {actual}"
                )
            }
            Self::PixelOffsets => f.write_str("the pixel offsets are not a valid prefix sum"),
            Self::PositionOffsets => f.write_str("the position offsets are not a valid prefix sum"),
        }
    }
}

impl std::error::Error for LayoutError {}

/// Word ranges of the sections of a tile buffer.
struct Sections {
    pixel_offsets: Range<usize>,
    position_offsets: Range<usize>,
    residual: Range<usize>,
    records: Range<usize>,
}

impl Sections {
    const fn of(header: &TileCacheHeader) -> Self {
        let pixels = header.pixel_count as usize;
        let positions = header.position_count as usize;
        let records = header.record_count as usize;
        let pixel_offsets = HEADER_WORDS..HEADER_WORDS.saturating_add(pixels).saturating_add(1);
        let position_offsets = pixel_offsets.end
            ..pixel_offsets
                .end
                .saturating_add(positions)
                .saturating_add(1);
        let residual =
            position_offsets.end..position_offsets.end.saturating_add(positions.div_ceil(2));
        let record_words = residual.end
            ..residual
                .end
                .saturating_add(records.saturating_mul(RECORD_WORDS));
        Self {
            pixel_offsets,
            position_offsets,
            residual,
            records: record_words,
        }
    }
}

/// One tile's cache: a validated contiguous word buffer with typed views.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TileCache {
    words: Vec<u32>,
}

impl TileCache {
    /// Packs the sections into one buffer.
    ///
    /// `pixel_offsets` has one entry more than the tile has pixels, `position_offsets` one more
    /// than `residual_bits` (the f16 bits of each position's residual), and the offsets are prefix
    /// sums starting at 0.
    #[must_use]
    pub fn from_parts(
        tile_index: u32,
        pixel_offsets: &[u32],
        position_offsets: &[u32],
        residual_bits: &[u16],
        records: &[ExitRecord],
    ) -> Self {
        let header = TileCacheHeader {
            tile_index,
            pixel_count: u32::try_from(pixel_offsets.len().saturating_sub(1)).unwrap_or(u32::MAX),
            position_count: u32::try_from(residual_bits.len()).unwrap_or(u32::MAX),
            record_count: u32::try_from(records.len()).unwrap_or(u32::MAX),
        };
        let sections = Sections::of(&header);
        let mut words = Vec::with_capacity(sections.records.end);
        words.extend_from_slice(&[
            header.tile_index,
            header.pixel_count,
            header.position_count,
            header.record_count,
        ]);
        words.extend_from_slice(pixel_offsets);
        if pixel_offsets.is_empty() {
            words.push(0);
        }
        words.extend_from_slice(position_offsets);
        if position_offsets.is_empty() {
            words.push(0);
        }
        for pair in residual_bits.chunks(2) {
            let low = u32::from(pair[0]);
            let high = pair.get(1).map_or(0, |&bits| u32::from(bits));
            words.push(low | (high << 16));
        }
        // An `ExitRecord` is 44 bytes at 4-byte alignment, so the cast to words cannot fail.
        words.extend_from_slice(bytemuck::cast_slice(records));
        Self { words }
    }

    /// Validates a word buffer (for example one read back from disk) as a tile.
    ///
    /// # Errors
    ///
    /// Returns the first way the buffer is not a tile.
    pub fn from_words(words: Vec<u32>) -> Result<Self, LayoutError> {
        if words.len() < HEADER_WORDS {
            return Err(LayoutError::TooShort { words: words.len() });
        }
        let header = TileCacheHeader {
            tile_index: words[0],
            pixel_count: words[1],
            position_count: words[2],
            record_count: words[3],
        };
        let sections = Sections::of(&header);
        if sections.records.end != words.len() {
            return Err(LayoutError::SizeMismatch {
                expected: sections.records.end,
                actual: words.len(),
            });
        }
        if !is_prefix_sum(&words[sections.pixel_offsets], header.position_count) {
            return Err(LayoutError::PixelOffsets);
        }
        if !is_prefix_sum(&words[sections.position_offsets], header.record_count) {
            return Err(LayoutError::PositionOffsets);
        }
        Ok(Self { words })
    }

    /// The tile's words, ready to upload.
    #[must_use]
    pub fn as_words(&self) -> &[u32] {
        &self.words
    }

    /// Consumes the tile into its words.
    #[must_use]
    pub fn into_words(self) -> Vec<u32> {
        self.words
    }

    /// Size of the buffer in bytes.
    #[must_use]
    pub const fn byte_len(&self) -> usize {
        self.words.len() * size_of::<u32>()
    }

    /// The header.
    #[must_use]
    pub fn header(&self) -> TileCacheHeader {
        TileCacheHeader {
            tile_index: self.words[0],
            pixel_count: self.words[1],
            position_count: self.words[2],
            record_count: self.words[3],
        }
    }

    fn sections(&self) -> Sections {
        Sections::of(&self.header())
    }

    /// `pixel_count + 1` offsets into the positions.
    #[must_use]
    pub fn pixel_offsets(&self) -> &[u32] {
        &self.words[self.sections().pixel_offsets]
    }

    /// `position_count + 1` offsets into the records.
    #[must_use]
    pub fn position_offsets(&self) -> &[u32] {
        &self.words[self.sections().position_offsets]
    }

    /// The f16 bits of a position's residual.
    #[must_use]
    pub fn position_residual_bits(&self, position: usize) -> u16 {
        let residual = self.sections().residual;
        let Some(&word) = self.words[residual].get(position / 2) else {
            return 0;
        };
        let half = if position.is_multiple_of(2) {
            word
        } else {
            word >> 16
        };
        u16::try_from(half & 0xffff).unwrap_or(0)
    }

    /// A position's residual: the summed spectral-mean weight of its dropped branches.
    #[must_use]
    pub fn position_residual(&self, position: usize) -> f32 {
        f16_bits_to_f32(self.position_residual_bits(position))
    }

    /// Every record of the tile.
    #[must_use]
    pub fn records(&self) -> &[ExitRecord] {
        let records = self.sections().records;
        bytemuck::try_cast_slice(&self.words[records]).unwrap_or(&[])
    }

    /// The positions of a pixel (indices into the position arrays); empty out of range.
    #[must_use]
    pub fn pixel_positions(&self, pixel: usize) -> Range<usize> {
        let offsets = self.pixel_offsets();
        match (offsets.get(pixel), offsets.get(pixel + 1)) {
            (Some(&start), Some(&end)) => start as usize..end as usize,
            _ => 0..0,
        }
    }

    /// The records of a position; empty out of range.
    #[must_use]
    pub fn position_records(&self, position: usize) -> &[ExitRecord] {
        let offsets = self.position_offsets();
        match (offsets.get(position), offsets.get(position + 1)) {
            (Some(&start), Some(&end)) => self
                .records()
                .get(start as usize..end as usize)
                .unwrap_or(&[]),
            _ => &[],
        }
    }
}

/// `true` when `offsets` starts at 0, never decreases and ends at `total`.
fn is_prefix_sum(offsets: &[u32], total: u32) -> bool {
    offsets.first() == Some(&0)
        && offsets.last() == Some(&total)
        && offsets.windows(2).all(|pair| pair[0] <= pair[1])
}

/// Pixel rectangle `(x0, y0, x1, y1)` (end-exclusive, clipped to the frame) of a row-major tile.
#[must_use]
pub fn tile_rect(width: u32, height: u32, tile: u32, index: u32) -> (u32, u32, u32, u32) {
    let tile = tile.max(1);
    let tiles_x = width.div_ceil(tile).max(1);
    let x0 = (index % tiles_x) * tile;
    let y0 = (index / tiles_x) * tile;
    (
        x0.min(width),
        y0.min(height),
        (x0 + tile).min(width),
        (y0 + tile).min(height),
    )
}

/// Number of tiles of a frame.
#[must_use]
pub const fn tile_count(width: u32, height: u32, tile: u32) -> u32 {
    let tile = if tile == 0 { 1 } else { tile };
    width.div_ceil(tile) * height.div_ceil(tile)
}
