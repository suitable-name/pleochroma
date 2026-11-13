//! The on-disk codec of one cached tile (guide section 8.9).
//!
//! A tile goes through four steps: the `dir_oct` of every record is delta-encoded against the
//! previous record of the same position (wrapping `u16`), the words and records are byte-shuffled
//! (all byte 0s, then all byte 1s, and so on, like Blosc), the result is compressed with zstd or
//! lz4 (or stored), and a chunk header with a CRC32 of the original tile is put in front. Decoding
//! is the exact inverse and verifies the CRC, so a round trip is bit exact.
//!
//! The delta and shuffle steps need no extra crate and are always compiled. The chunk codec
//! ([`encode_tile`], [`decode_tile`]) needs the `path-cache-disk` feature.
//!
//! # Chunk layout (little endian)
//!
//! ```text
//! offset  size  field
//!      0     8  magic            b"PCTILE1\0"
//!      8     4  format_version   CACHE_FORMAT_VERSION
//!     12     1  compression      0 none, 1 zstd, 2 lz4
//!     13     3  reserved         zero
//!     16     8  uncompressed_len bytes of the tile buffer (words x 4)
//!     24     8  payload_len      bytes after the header
//!     32     4  crc32            CRC32 (IEEE) of the uncompressed tile buffer
//!     36     4  prefix_words     words before the records (header, offsets, residuals)
//!     40        payload          the compressed, shuffled, delta-encoded tile
//! ```
//!
//! The payload is, before compression, the byte-shuffled prefix words followed by the
//! byte-shuffled records (44 byte streams of one byte per record).

use std::{fmt, ops::Range};

#[cfg(feature = "path-cache-disk")]
use super::{key::CACHE_FORMAT_VERSION, layout::TileCache};
use super::{
    layout::RECORD_WORDS,
    record::{COMB_LEN, DEPTH_LEN, ExitRecord},
};
use crate::renderer::accel::CacheCompression;

/// The first eight bytes of every chunk.
pub const CHUNK_MAGIC: [u8; 8] = *b"PCTILE1\0";

/// Bytes of the chunk header.
pub const CHUNK_HEADER_LEN: usize = 40;

/// The zstd level of [`CacheCompression::Zstd`] chunks.
pub const ZSTD_LEVEL: i32 = 2;

/// Largest uncompressed tile a chunk may claim (guards the decoder's allocation).
pub const MAX_TILE_BYTES: u64 = 1 << 31;

/// Bytes of one record.
const RECORD_BYTES: usize = RECORD_WORDS * size_of::<u32>();

/// Why a chunk could not be written or read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ChunkError {
    /// The chunk is shorter than its header or than the payload the header announces.
    Truncated,
    /// The chunk does not start with [`CHUNK_MAGIC`].
    BadMagic,
    /// The chunk was written by another format version.
    VersionMismatch {
        /// The version in the chunk.
        found: u32,
    },
    /// The compression code is not known.
    UnknownCompression(u8),
    /// A length in the header is not consistent with the rest of the chunk.
    BadLength,
    /// The tile does not match the CRC stored in the header.
    CrcMismatch {
        /// CRC from the header.
        expected: u32,
        /// CRC of the decoded tile.
        actual: u32,
    },
    /// The compression library failed.
    Codec(String),
    /// The decoded words are not a valid tile.
    Layout(String),
}

impl fmt::Display for ChunkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Truncated => f.write_str("the chunk is truncated"),
            Self::BadMagic => f.write_str("the chunk does not start with the tile magic"),
            Self::VersionMismatch { found } => {
                write!(f, "the chunk has format version {found}")
            }
            Self::UnknownCompression(code) => write!(f, "unknown compression code {code}"),
            Self::BadLength => f.write_str("a length in the chunk header is inconsistent"),
            Self::CrcMismatch { expected, actual } => {
                write!(
                    f,
                    "CRC mismatch: header {expected:#010x}, tile {actual:#010x}"
                )
            }
            Self::Codec(message) => write!(f, "decompression failed: {message}"),
            Self::Layout(message) => write!(f, "the decoded tile is invalid: {message}"),
        }
    }
}

impl std::error::Error for ChunkError {}

/// The code of a compression mode in the chunk header.
#[must_use]
pub const fn compression_code(compression: CacheCompression) -> u8 {
    match compression {
        CacheCompression::None => 0,
        CacheCompression::Zstd => 1,
        CacheCompression::Lz4 => 2,
    }
}

/// The compression mode of a header code.
#[must_use]
pub const fn compression_from_code(code: u8) -> Option<CacheCompression> {
    match code {
        0 => Some(CacheCompression::None),
        1 => Some(CacheCompression::Zstd),
        2 => Some(CacheCompression::Lz4),
        _ => None,
    }
}

/// The file extension of a compression mode's chunks (without the dot).
#[must_use]
pub const fn file_extension(compression: CacheCompression) -> &'static str {
    match compression {
        CacheCompression::None => "bin",
        CacheCompression::Zstd => "zst",
        CacheCompression::Lz4 => "lz4",
    }
}

/// The record ranges of the positions; offsets that do not fit the record count are skipped.
fn position_spans(
    position_offsets: &[u32],
    record_count: usize,
) -> impl Iterator<Item = Range<usize>> + '_ {
    position_offsets.windows(2).filter_map(move |pair| {
        let start = pair[0] as usize;
        let end = pair[1] as usize;
        (start <= end && end <= record_count).then_some(start..end)
    })
}

/// Replaces the `dir_oct` of every record by its wrapping difference to the previous record of
/// the same position; the first record of a position stays as it is.
#[must_use]
pub fn delta_encode_dir(records: &[ExitRecord], position_offsets: &[u32]) -> Vec<ExitRecord> {
    let mut out = records.to_vec();
    for span in position_spans(position_offsets, records.len()) {
        for i in span.start.saturating_add(1)..span.end {
            let [p0, p1] = records[i - 1].dir_oct;
            let [c0, c1] = records[i].dir_oct;
            out[i].dir_oct = [c0.wrapping_sub(p0), c1.wrapping_sub(p1)];
        }
    }
    out
}

/// Undoes [`delta_encode_dir`] in place.
pub fn delta_decode_dir(records: &mut [ExitRecord], position_offsets: &[u32]) {
    let spans: Vec<Range<usize>> = position_spans(position_offsets, records.len()).collect();
    for span in spans {
        for i in span.start.saturating_add(1)..span.end {
            let [p0, p1] = records[i - 1].dir_oct;
            let [c0, c1] = records[i].dir_oct;
            records[i].dir_oct = [c0.wrapping_add(p0), c1.wrapping_add(p1)];
        }
    }
}

/// Byte-shuffles words: all low bytes, then all second bytes, then all third, then all high bytes.
#[must_use]
pub fn shuffle_words(words: &[u32]) -> Vec<u8> {
    let n = words.len();
    let mut out = vec![0_u8; size_of_val(words)];
    for (i, word) in words.iter().enumerate() {
        for (k, byte) in word.to_le_bytes().iter().enumerate() {
            out[k * n + i] = *byte;
        }
    }
    out
}

/// Undoes [`shuffle_words`]; trailing bytes that do not make a whole word are ignored.
#[must_use]
pub fn unshuffle_words(bytes: &[u8]) -> Vec<u32> {
    let n = bytes.len() / size_of::<u32>();
    let (b0, rest) = bytes.split_at(n);
    let (b1, rest) = rest.split_at(n);
    let (b2, b3) = rest.split_at(n);
    b0.iter()
        .zip(b1)
        .zip(b2)
        .zip(b3)
        .map(|(((&a, &b), &c), &d)| u32::from_le_bytes([a, b, c, d]))
        .collect()
}

/// The 44 little endian bytes of a record, field by field.
fn record_bytes(record: &ExitRecord) -> [u8; RECORD_BYTES] {
    let mut out = [0_u8; RECORD_BYTES];
    let mut at = 0;
    let mut put = |bytes: &[u8]| {
        out[at..at + bytes.len()].copy_from_slice(bytes);
        at += bytes.len();
    };
    for value in record.dir_oct {
        put(&value.to_le_bytes());
    }
    for value in record.ddir_dlambda {
        put(&value.to_le_bytes());
    }
    for value in record.weight {
        put(&value.to_le_bytes());
    }
    put(&record.flags.to_le_bytes());
    put(&record.signature.to_le_bytes());
    for value in record.depth_bits {
        put(&value.to_le_bytes());
    }
    out
}

/// The record of 44 little endian bytes (the inverse of [`record_bytes`]).
fn record_from_bytes(bytes: &[u8; RECORD_BYTES]) -> ExitRecord {
    let u16_at = |offset: usize| u16::from_le_bytes([bytes[offset], bytes[offset + 1]]);
    let u32_at = |offset: usize| {
        u32::from_le_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ])
    };
    let mut weight = [0_u16; COMB_LEN];
    for (slot, value) in weight.iter_mut().enumerate() {
        *value = u16_at(8 + 2 * slot);
    }
    let mut depth_bits = [0_u32; DEPTH_LEN];
    for (slot, value) in depth_bits.iter_mut().enumerate() {
        *value = u32_at(32 + 4 * slot);
    }
    ExitRecord {
        dir_oct: [u16_at(0), u16_at(2)],
        ddir_dlambda: [u16_at(4), u16_at(6)],
        weight,
        flags: u32_at(24),
        signature: u32_at(28),
        depth_bits,
    }
}

/// Byte-shuffles records: stream `k` holds byte `k` of every record.
#[must_use]
pub fn shuffle_records(records: &[ExitRecord]) -> Vec<u8> {
    let n = records.len();
    let mut out = vec![0_u8; n * RECORD_BYTES];
    for (i, record) in records.iter().enumerate() {
        for (k, byte) in record_bytes(record).iter().enumerate() {
            out[k * n + i] = *byte;
        }
    }
    out
}

/// Undoes [`shuffle_records`]; `None` when the length is not a whole number of records.
#[must_use]
pub fn unshuffle_records(bytes: &[u8]) -> Option<Vec<ExitRecord>> {
    if !bytes.len().is_multiple_of(RECORD_BYTES) {
        return None;
    }
    let n = bytes.len() / RECORD_BYTES;
    let mut records = Vec::with_capacity(n);
    for i in 0..n {
        let mut raw = [0_u8; RECORD_BYTES];
        for (k, byte) in raw.iter_mut().enumerate() {
            *byte = bytes[k * n + i];
        }
        records.push(record_from_bytes(&raw));
    }
    Some(records)
}

/// The little endian bytes of a word buffer (the bytes the CRC covers).
#[cfg(feature = "path-cache-disk")]
fn words_le_bytes(words: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(size_of_val(words));
    for word in words {
        out.extend_from_slice(&word.to_le_bytes());
    }
    out
}

/// The parsed, checked header of a chunk.
#[cfg(feature = "path-cache-disk")]
struct ChunkHeader {
    compression: CacheCompression,
    uncompressed_len: usize,
    payload_len: usize,
    crc: u32,
    prefix_words: usize,
}

#[cfg(feature = "path-cache-disk")]
impl ChunkHeader {
    fn parse(bytes: &[u8]) -> Result<Self, ChunkError> {
        let header = bytes.get(..CHUNK_HEADER_LEN).ok_or(ChunkError::Truncated)?;
        if header[..8] != CHUNK_MAGIC {
            return Err(ChunkError::BadMagic);
        }
        let u32_at = |at: usize| {
            u32::from_le_bytes([header[at], header[at + 1], header[at + 2], header[at + 3]])
        };
        let u64_at = |at: usize| {
            let mut raw = [0_u8; 8];
            raw.copy_from_slice(&header[at..at + 8]);
            u64::from_le_bytes(raw)
        };
        let version = u32_at(8);
        if version != CACHE_FORMAT_VERSION {
            return Err(ChunkError::VersionMismatch { found: version });
        }
        let compression =
            compression_from_code(header[12]).ok_or(ChunkError::UnknownCompression(header[12]))?;
        let uncompressed_len = u64_at(16);
        let payload_len = u64_at(24);
        if uncompressed_len > MAX_TILE_BYTES || !uncompressed_len.is_multiple_of(4) {
            return Err(ChunkError::BadLength);
        }
        let to_usize = |value: u64| usize::try_from(value).map_err(|_| ChunkError::BadLength);
        let uncompressed_len = to_usize(uncompressed_len)?;
        let payload_len = to_usize(payload_len)?;
        let prefix_words = u32_at(36) as usize;
        if prefix_words.saturating_mul(4) > uncompressed_len
            || !(uncompressed_len - prefix_words * 4).is_multiple_of(RECORD_BYTES)
        {
            return Err(ChunkError::BadLength);
        }
        Ok(Self {
            compression,
            uncompressed_len,
            payload_len,
            crc: u32_at(32),
            prefix_words,
        })
    }
}

/// Encodes one tile into a chunk.
///
/// # Errors
///
/// Returns [`ChunkError::Codec`] when the compression library fails.
#[cfg(feature = "path-cache-disk")]
pub fn encode_tile(tile: &TileCache, compression: CacheCompression) -> Result<Vec<u8>, ChunkError> {
    let words = tile.as_words();
    let records = tile.records();
    let prefix_words = words.len().saturating_sub(records.len() * RECORD_WORDS);
    let crc = crc32fast::hash(&words_le_bytes(words));

    let delta = delta_encode_dir(records, tile.position_offsets());
    let mut transformed = shuffle_words(&words[..prefix_words]);
    transformed.extend_from_slice(&shuffle_records(&delta));

    let payload = match compression {
        CacheCompression::None => transformed,
        CacheCompression::Zstd => zstd::bulk::compress(&transformed, ZSTD_LEVEL)
            .map_err(|error| ChunkError::Codec(error.to_string()))?,
        CacheCompression::Lz4 => lz4_flex::block::compress(&transformed),
    };

    let mut chunk = Vec::with_capacity(CHUNK_HEADER_LEN + payload.len());
    chunk.extend_from_slice(&CHUNK_MAGIC);
    chunk.extend_from_slice(&CACHE_FORMAT_VERSION.to_le_bytes());
    chunk.push(compression_code(compression));
    chunk.extend_from_slice(&[0, 0, 0]);
    chunk.extend_from_slice(&(size_of_val(words) as u64).to_le_bytes());
    chunk.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    chunk.extend_from_slice(&crc.to_le_bytes());
    chunk.extend_from_slice(
        &u32::try_from(prefix_words)
            .unwrap_or(u32::MAX)
            .to_le_bytes(),
    );
    chunk.extend_from_slice(&payload);
    Ok(chunk)
}

/// Decodes a chunk into its tile and verifies the CRC.
///
/// # Errors
///
/// Returns the first thing wrong with the chunk: truncation, a bad magic, another format version,
/// a failed decompression, an invalid layout or a CRC mismatch. Never panics.
#[cfg(feature = "path-cache-disk")]
pub fn decode_tile(bytes: &[u8]) -> Result<TileCache, ChunkError> {
    let header = ChunkHeader::parse(bytes)?;
    let payload = bytes.get(CHUNK_HEADER_LEN..).ok_or(ChunkError::Truncated)?;
    if payload.len() < header.payload_len {
        return Err(ChunkError::Truncated);
    }
    if payload.len() > header.payload_len {
        return Err(ChunkError::BadLength);
    }
    let transformed = match header.compression {
        CacheCompression::None => payload.to_vec(),
        CacheCompression::Zstd => zstd::bulk::decompress(payload, header.uncompressed_len)
            .map_err(|error| ChunkError::Codec(error.to_string()))?,
        CacheCompression::Lz4 => lz4_flex::block::decompress(payload, header.uncompressed_len)
            .map_err(|error| ChunkError::Codec(error.to_string()))?,
    };
    if transformed.len() != header.uncompressed_len {
        return Err(ChunkError::BadLength);
    }

    let (prefix, record_bytes) = transformed.split_at(header.prefix_words * size_of::<u32>());
    let mut words = unshuffle_words(prefix);
    let records = unshuffle_records(record_bytes).ok_or(ChunkError::BadLength)?;
    let record_count = records.len();
    words.extend_from_slice(bytemuck::cast_slice(&records));

    // The records still hold deltas here; the check only needs the sizes and the offsets.
    let tile =
        TileCache::from_words(words).map_err(|error| ChunkError::Layout(error.to_string()))?;
    let offsets = tile.position_offsets().to_vec();
    let mut words = tile.into_words();
    let first_record_word = words.len() - record_count * RECORD_WORDS;
    let records: &mut [ExitRecord] = bytemuck::try_cast_slice_mut(&mut words[first_record_word..])
        .map_err(|error| ChunkError::Layout(error.to_string()))?;
    delta_decode_dir(records, &offsets);

    let actual = crc32fast::hash(&words_le_bytes(&words));
    if actual != header.crc {
        return Err(ChunkError::CrcMismatch {
            expected: header.crc,
            actual,
        });
    }
    TileCache::from_words(words).map_err(|error| ChunkError::Layout(error.to_string()))
}
