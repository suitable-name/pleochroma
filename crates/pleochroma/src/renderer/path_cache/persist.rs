//! The on-disk store of path caches (guide section 8.9).
//!
//! A cache lives in `<root>/<key hex>/`:
//!
//! ```text
//! meta.json           readable key inputs, format version, size, created and last-used times
//! tiles/00000.zst     one compressed chunk per tile (.lz4 or .bin for the other modes)
//! ```
//!
//! The root is a parameter; the desktop app passes `render_cache/` beside its settings file.
//! Disk is never read during a render: [`load_cache`] reads every tile into memory first. A chunk
//! that fails verification deletes the key folder and tells the caller to rebuild
//! ([`RebuildReason`]); that is logged, never an error dialog and never a panic.
//!
//! Metadata, folder handling and sizes need no extra crate. Writing and reading tiles needs the
//! `path-cache-disk` feature.
//!
//! The beam leaf cache's chunks (magic `PCBEAM1`) are also written and read here, with the `beam`
//! feature (and `path-cache-disk`); the bytes are the same in every build.

use std::{
    fmt::{self, Write as _},
    fs, io,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

#[cfg(all(feature = "path-cache-disk", feature = "beam"))]
use super::compress::{
    ZSTD_LEVEL, compression_code, compression_from_code, shuffle_words, unshuffle_words,
};
use super::{
    compress::file_extension,
    key::{CACHE_FORMAT_VERSION, CacheKey, CacheKeyInputs},
    layout::tile_count,
};
#[cfg(feature = "path-cache-disk")]
use super::{
    compress::{ChunkError, decode_tile, encode_tile},
    layout::TileCache,
};
use crate::renderer::accel::CacheCompression;

/// The first eight bytes of every beam chunk (the leaf cache, beam guide section 14.10).
#[cfg(all(feature = "path-cache-disk", feature = "beam"))]
pub const BEAM_CHUNK_MAGIC: [u8; 8] = *b"PCBEAM1\0";

/// Bytes of the beam chunk header.
#[cfg(all(feature = "path-cache-disk", feature = "beam"))]
pub const BEAM_CHUNK_HEADER_LEN: usize = 48;

/// Largest uncompressed beam chunk a header may claim (guards the decoder's allocation, which the
/// compression libraries make up front from the claimed size).
#[cfg(all(feature = "path-cache-disk", feature = "beam"))]
pub const MAX_BEAM_CHUNK_BYTES: u64 = 1 << 31;

/// One decoded beam chunk: the little endian words of one retained batch and where it belongs.
///
/// # Chunk layout (little endian)
///
/// ```text
/// offset  size  field
///      0     8  magic             b"PCBEAM1\0"
///      8     4  beam_version      BEAM_CACHE_FORMAT_VERSION
///     12     1  compression       0 none, 1 zstd, 2 lz4
///     13     3  reserved          zero
///     16     8  uncompressed_len  bytes of the word buffer (words x 4)
///     24     8  payload_len       bytes after the header
///     32     4  crc32             CRC32 (IEEE) of the uncompressed word buffer
///     36     4  batch_index       the batch this chunk holds
///     40     4  batch_count       the batches of the whole cache
///     44     4  reserved          zero
///     48        payload           the byte-shuffled words, compressed
/// ```
#[cfg(all(feature = "path-cache-disk", feature = "beam"))]
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BeamChunk {
    /// The batch this chunk holds.
    pub batch_index: u32,
    /// The number of batches of the whole cache.
    pub batch_count: u32,
    /// The words of the batch.
    pub words: Vec<u32>,
}

/// The little endian bytes of a word buffer (the bytes the CRC covers).
#[cfg(all(feature = "path-cache-disk", feature = "beam"))]
fn beam_words_le_bytes(words: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(size_of_val(words));
    for word in words {
        out.extend_from_slice(&word.to_le_bytes());
    }
    out
}

/// Encodes the words of batch `batch_index` of `batch_count` into a beam chunk: byte shuffle,
/// then the compression, with a CRC32 of the words in the header.
///
/// # Errors
///
/// Returns [`ChunkError::Codec`] when the compression library fails and [`ChunkError::BadLength`]
/// when the words exceed [`MAX_BEAM_CHUNK_BYTES`].
#[cfg(all(feature = "path-cache-disk", feature = "beam"))]
pub fn encode_beam_chunk(
    words: &[u32],
    batch_index: u32,
    batch_count: u32,
    beam_version: u32,
    compression: CacheCompression,
) -> Result<Vec<u8>, ChunkError> {
    if size_of_val(words) as u64 > MAX_BEAM_CHUNK_BYTES {
        return Err(ChunkError::BadLength);
    }
    let crc = crc32fast::hash(&beam_words_le_bytes(words));
    let shuffled = shuffle_words(words);
    let payload = match compression {
        CacheCompression::None => shuffled,
        CacheCompression::Zstd => zstd::bulk::compress(&shuffled, ZSTD_LEVEL)
            .map_err(|error| ChunkError::Codec(error.to_string()))?,
        CacheCompression::Lz4 => lz4_flex::block::compress(&shuffled),
    };
    let mut chunk = Vec::with_capacity(BEAM_CHUNK_HEADER_LEN + payload.len());
    chunk.extend_from_slice(&BEAM_CHUNK_MAGIC);
    chunk.extend_from_slice(&beam_version.to_le_bytes());
    chunk.push(compression_code(compression));
    chunk.extend_from_slice(&[0, 0, 0]);
    chunk.extend_from_slice(&(size_of_val(words) as u64).to_le_bytes());
    chunk.extend_from_slice(&(payload.len() as u64).to_le_bytes());
    chunk.extend_from_slice(&crc.to_le_bytes());
    chunk.extend_from_slice(&batch_index.to_le_bytes());
    chunk.extend_from_slice(&batch_count.to_le_bytes());
    chunk.extend_from_slice(&[0, 0, 0, 0]);
    chunk.extend_from_slice(&payload);
    Ok(chunk)
}

/// Decodes a beam chunk written for `beam_version` and verifies its CRC.
///
/// # Errors
///
/// Returns the first thing wrong with the chunk: truncation, a bad magic, another beam format
/// version, a nonzero reserved byte, a failed decompression or a CRC mismatch. Never panics.
#[cfg(all(feature = "path-cache-disk", feature = "beam"))]
pub fn decode_beam_chunk(bytes: &[u8], beam_version: u32) -> Result<BeamChunk, ChunkError> {
    let header = bytes
        .get(..BEAM_CHUNK_HEADER_LEN)
        .ok_or(ChunkError::Truncated)?;
    if header[..8] != BEAM_CHUNK_MAGIC {
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
    if version != beam_version {
        return Err(ChunkError::VersionMismatch { found: version });
    }
    let compression =
        compression_from_code(header[12]).ok_or(ChunkError::UnknownCompression(header[12]))?;
    if header[13..16] != [0, 0, 0] || u32_at(44) != 0 {
        return Err(ChunkError::BadLength);
    }
    let uncompressed_len = u64_at(16);
    if uncompressed_len > MAX_BEAM_CHUNK_BYTES || !uncompressed_len.is_multiple_of(4) {
        return Err(ChunkError::BadLength);
    }
    let to_usize = |value: u64| usize::try_from(value).map_err(|_| ChunkError::BadLength);
    let uncompressed_len = to_usize(uncompressed_len)?;
    let payload_len = to_usize(u64_at(24))?;
    let payload = &bytes[BEAM_CHUNK_HEADER_LEN..];
    if payload.len() < payload_len {
        return Err(ChunkError::Truncated);
    }
    if payload.len() > payload_len {
        return Err(ChunkError::BadLength);
    }
    let shuffled = match compression {
        CacheCompression::None => payload.to_vec(),
        CacheCompression::Zstd => zstd::bulk::decompress(payload, uncompressed_len)
            .map_err(|error| ChunkError::Codec(error.to_string()))?,
        CacheCompression::Lz4 => lz4_flex::block::decompress(payload, uncompressed_len)
            .map_err(|error| ChunkError::Codec(error.to_string()))?,
    };
    if shuffled.len() != uncompressed_len {
        return Err(ChunkError::BadLength);
    }
    let words = unshuffle_words(&shuffled);
    let actual = crc32fast::hash(&beam_words_le_bytes(&words));
    let expected = u32_at(32);
    if actual != expected {
        return Err(ChunkError::CrcMismatch { expected, actual });
    }
    Ok(BeamChunk {
        batch_index: u32_at(36),
        batch_count: u32_at(40),
        words,
    })
}

/// The name of the metadata file in a key folder.
pub const META_FILE_NAME: &str = "meta.json";

/// The name of the tile subfolder in a key folder.
pub const TILES_DIR_NAME: &str = "tiles";

/// Seconds since the Unix epoch (0 when the clock is before it).
#[must_use]
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// The readable description of a cached key, stored as `meta.json`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CacheMeta {
    /// The cache format version the tiles were written with.
    pub format_version: u32,
    /// The key as 32 lowercase hex digits (also the folder name).
    pub key: String,
    /// The material name.
    pub material: String,
    /// Facet planes of the stone.
    pub plane_count: u32,
    /// Frame width in pixels.
    pub width: u32,
    /// Frame height in pixels.
    pub height: u32,
    /// Internal bounce limit.
    pub max_bounces: u32,
    /// Tile edge in pixels.
    pub tile: u32,
    /// Positions every stone pixel starts with.
    pub k0: u32,
    /// Position cap of an edge pixel.
    pub k_max: u32,
    /// How the tiles are compressed.
    pub compression: CacheCompression,
    /// Tiles of the frame (chunks `00000` to `tile_count - 1`).
    pub tile_count: u32,
    /// Bytes of all chunks on disk; 0 until the cache is finished.
    pub size_bytes: u64,
    /// When the cache was created, in Unix seconds.
    pub created_unix: u64,
    /// When the cache was last read or written, in Unix seconds.
    pub last_used_unix: u64,
    /// `true` once every tile was written.
    pub complete: bool,
}

impl CacheMeta {
    /// The description of a cache about to be built for `inputs`, created at `now` (Unix seconds).
    #[must_use]
    pub fn describe(
        inputs: &CacheKeyInputs<'_>,
        key: &CacheKey,
        compression: CacheCompression,
        now: u64,
    ) -> Self {
        let settings = (*inputs.settings).sanitized();
        Self {
            format_version: CACHE_FORMAT_VERSION,
            key: key.to_string(),
            material: inputs.material.name.clone(),
            plane_count: u32::try_from(inputs.planes.len()).unwrap_or(u32::MAX),
            width: inputs.width,
            height: inputs.height,
            max_bounces: inputs.max_bounces,
            tile: settings.tile,
            k0: settings.k0,
            k_max: settings.k_max,
            compression,
            tile_count: tile_count(inputs.width, inputs.height, settings.tile),
            size_bytes: 0,
            created_unix: now,
            last_used_unix: now,
            complete: false,
        }
    }

    /// The `meta.json` text.
    #[must_use]
    pub fn to_json(&self) -> String {
        let compression = match self.compression {
            CacheCompression::None => "none",
            CacheCompression::Zstd => "zstd",
            CacheCompression::Lz4 => "lz4",
        };
        format!(
            "{{\n  \"format_version\": {},\n  \"key\": \"{}\",\n  \"material\": \"{}\",\n  \
             \"plane_count\": {},\n  \"width\": {},\n  \"height\": {},\n  \"max_bounces\": {},\n  \
             \"tile\": {},\n  \"k0\": {},\n  \"k_max\": {},\n  \"compression\": \"{}\",\n  \
             \"tile_count\": {},\n  \"size_bytes\": {},\n  \"created_unix\": {},\n  \
             \"last_used_unix\": {},\n  \"complete\": {}\n}}\n",
            self.format_version,
            json_escape(&self.key),
            json_escape(&self.material),
            self.plane_count,
            self.width,
            self.height,
            self.max_bounces,
            self.tile,
            self.k0,
            self.k_max,
            compression,
            self.tile_count,
            self.size_bytes,
            self.created_unix,
            self.last_used_unix,
            self.complete,
        )
    }

    /// Parses a `meta.json` text; `None` when a field is missing or malformed.
    #[must_use]
    pub fn from_json(text: &str) -> Option<Self> {
        let number = |name: &str| json_field(text, name)?.parse::<u64>().ok();
        let small = |name: &str| u32::try_from(number(name)?).ok();
        let compression = match json_field(text, "compression")?.as_str() {
            "none" => CacheCompression::None,
            "zstd" => CacheCompression::Zstd,
            "lz4" => CacheCompression::Lz4,
            _ => return None,
        };
        let complete = match json_field(text, "complete")?.as_str() {
            "true" => true,
            "false" => false,
            _ => return None,
        };
        Some(Self {
            format_version: small("format_version")?,
            key: json_field(text, "key")?,
            material: json_field(text, "material")?,
            plane_count: small("plane_count")?,
            width: small("width")?,
            height: small("height")?,
            max_bounces: small("max_bounces")?,
            tile: small("tile")?,
            k0: small("k0")?,
            k_max: small("k_max")?,
            compression,
            tile_count: small("tile_count")?,
            size_bytes: number("size_bytes")?,
            created_unix: number("created_unix")?,
            last_used_unix: number("last_used_unix")?,
            complete,
        })
    }
}

/// Escapes a string for a JSON string literal.
fn json_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            // Writing to a `String` cannot fail.
            c if u32::from(c) < 0x20 => {
                let _ = write!(out, "\\u{:04x}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out
}

/// The value of a top-level field of the flat `meta.json`, unescaped; strings lose their quotes.
fn json_field(text: &str, name: &str) -> Option<String> {
    let pattern = format!("\"{name}\":");
    let start = text.find(&pattern)? + pattern.len();
    let rest = text[start..].trim_start();
    if let Some(quoted) = rest.strip_prefix('"') {
        let mut out = String::new();
        let mut chars = quoted.chars();
        loop {
            match chars.next()? {
                '"' => return Some(out),
                '\\' => match chars.next()? {
                    'n' => out.push('\n'),
                    'r' => out.push('\r'),
                    't' => out.push('\t'),
                    'u' => {
                        let hex: String = chars.by_ref().take(4).collect();
                        let code = u32::from_str_radix(&hex, 16).ok()?;
                        out.push(char::from_u32(code)?);
                    }
                    other => out.push(other),
                },
                c => out.push(c),
            }
        }
    }
    let end = rest
        .find(|c: char| c == ',' || c == '}' || c.is_whitespace())
        .unwrap_or(rest.len());
    Some(rest[..end].to_string())
}

/// Reads and parses `<dir>/meta.json`; `None` when it is missing or malformed.
#[must_use]
pub fn read_meta(dir: &Path) -> Option<CacheMeta> {
    let text = fs::read_to_string(dir.join(META_FILE_NAME)).ok()?;
    CacheMeta::from_json(&text)
}

/// Writes a file through a temporary sibling and a rename, so a crash never leaves half a file.
fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    fs::write(&temporary, bytes)?;
    fs::rename(&temporary, path)
}

/// Bytes of the files of a key folder and its `tiles` subfolder (not a recursive walk).
#[must_use]
pub fn folder_size(dir: &Path) -> u64 {
    let sum = |folder: &Path| -> u64 {
        fs::read_dir(folder).map_or(0, |entries| {
            entries
                .flatten()
                .filter_map(|entry| entry.metadata().ok())
                .filter(fs::Metadata::is_file)
                .map(|metadata| metadata.len())
                .sum()
        })
    };
    sum(dir) + sum(&dir.join(TILES_DIR_NAME))
}

/// Why a cache has to be rebuilt instead of loaded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RebuildReason {
    /// There is no cache for the key.
    Missing,
    /// `meta.json` is missing, malformed, or describes another key.
    BadMeta,
    /// The cache was written by another format version.
    VersionChanged {
        /// The version found.
        found: u32,
    },
    /// The cache was never finished (a render was cancelled or crashed).
    Incomplete,
    /// A chunk is missing or failed verification.
    Corrupt {
        /// The tile whose chunk failed.
        tile: u32,
        /// What was wrong with it.
        detail: String,
    },
}

impl fmt::Display for RebuildReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => f.write_str("no cache for this key"),
            Self::BadMeta => f.write_str("the cache metadata is unreadable"),
            Self::VersionChanged { found } => {
                write!(f, "the cache has format version {found}")
            }
            Self::Incomplete => f.write_str("the cache was never finished"),
            Self::Corrupt { tile, detail } => write!(f, "tile {tile} is unusable: {detail}"),
        }
    }
}

/// Why a tile could not be written or read.
#[derive(Debug)]
pub enum PersistError {
    /// A file operation failed.
    Io(io::Error),
    /// A chunk failed to encode or failed verification.
    #[cfg(feature = "path-cache-disk")]
    Chunk(ChunkError),
    /// The chunk holds another tile than its file name says.
    #[cfg(feature = "path-cache-disk")]
    TileMismatch {
        /// The tile asked for.
        expected: u32,
        /// The tile index in the chunk.
        found: u32,
    },
}

impl fmt::Display for PersistError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(error) => write!(f, "file error: {error}"),
            #[cfg(feature = "path-cache-disk")]
            Self::Chunk(error) => write!(f, "{error}"),
            #[cfg(feature = "path-cache-disk")]
            Self::TileMismatch { expected, found } => {
                write!(f, "the chunk of tile {expected} holds tile {found}")
            }
        }
    }
}

impl std::error::Error for PersistError {}

impl From<io::Error> for PersistError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

#[cfg(feature = "path-cache-disk")]
impl From<ChunkError> for PersistError {
    fn from(error: ChunkError) -> Self {
        Self::Chunk(error)
    }
}

/// One cache folder: the writer and the reader of its tiles.
///
/// It holds only a path and the metadata, so it is `Send` and `Sync`: share it (for example in an
/// `Arc`) with the background thread that writes tiles as they complete. Chunks are named by tile
/// index, so the files on disk do not depend on the order tiles finish in.
#[derive(Clone, Debug)]
pub struct KeyFolder {
    dir: PathBuf,
    meta: CacheMeta,
}

impl KeyFolder {
    /// The folder of a key under `root`.
    #[must_use]
    pub fn folder_path(root: &Path, key: &CacheKey) -> PathBuf {
        root.join(key.to_string())
    }

    /// Creates an empty, unfinished cache folder for `meta`, replacing any folder of that key.
    ///
    /// # Errors
    ///
    /// Returns the I/O error when the folder or `meta.json` cannot be written.
    pub fn create(root: &Path, key: &CacheKey, mut meta: CacheMeta) -> io::Result<Self> {
        let dir = Self::folder_path(root, key);
        match fs::remove_dir_all(&dir) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        fs::create_dir_all(dir.join(TILES_DIR_NAME))?;
        meta.complete = false;
        meta.size_bytes = 0;
        let folder = Self { dir, meta };
        folder.write_meta()?;
        Ok(folder)
    }

    /// Opens a finished cache.
    ///
    /// A folder whose metadata is unusable, was written by another format version or was never
    /// finished is deleted (and logged), so the caller can simply build and write a new one.
    ///
    /// # Errors
    ///
    /// Returns why the cache has to be rebuilt.
    pub fn open(root: &Path, key: &CacheKey) -> Result<Self, RebuildReason> {
        let dir = Self::folder_path(root, key);
        if !dir.is_dir() {
            return Err(RebuildReason::Missing);
        }
        let Some(meta) = read_meta(&dir) else {
            return Err(discard(&dir, RebuildReason::BadMeta));
        };
        if meta.key != key.to_string() {
            return Err(discard(&dir, RebuildReason::BadMeta));
        }
        if meta.format_version != CACHE_FORMAT_VERSION {
            let found = meta.format_version;
            return Err(discard(&dir, RebuildReason::VersionChanged { found }));
        }
        if !meta.complete {
            return Err(discard(&dir, RebuildReason::Incomplete));
        }
        Ok(Self { dir, meta })
    }

    /// The folder.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The metadata as last written.
    #[must_use]
    pub const fn meta(&self) -> &CacheMeta {
        &self.meta
    }

    /// The file of a tile's chunk.
    #[must_use]
    pub fn tile_path(&self, index: u32) -> PathBuf {
        self.dir.join(TILES_DIR_NAME).join(format!(
            "{index:05}.{}",
            file_extension(self.meta.compression)
        ))
    }

    fn write_meta(&self) -> io::Result<()> {
        write_atomic(
            &self.dir.join(META_FILE_NAME),
            self.meta.to_json().as_bytes(),
        )
    }

    /// Marks the cache complete, with the total bytes of its chunks, and stamps the use time.
    ///
    /// # Errors
    ///
    /// Returns the I/O error when `meta.json` cannot be written.
    pub fn finish(&mut self, size_bytes: u64) -> io::Result<()> {
        self.meta.complete = true;
        self.meta.size_bytes = size_bytes;
        self.meta.last_used_unix = unix_now();
        self.write_meta()
    }

    /// Stamps the cache as used now.
    ///
    /// # Errors
    ///
    /// Returns the I/O error when `meta.json` cannot be written.
    pub fn mark_used(&mut self) -> io::Result<()> {
        self.mark_used_at(unix_now())
    }

    /// Stamps the cache as used at `unix` seconds.
    ///
    /// # Errors
    ///
    /// Returns the I/O error when `meta.json` cannot be written.
    pub fn mark_used_at(&mut self, unix: u64) -> io::Result<()> {
        self.meta.last_used_unix = unix;
        self.write_meta()
    }

    /// Deletes the folder (a failure is logged, not returned).
    pub fn delete(self) {
        remove_folder(&self.dir);
    }
}

#[cfg(feature = "path-cache-disk")]
impl KeyFolder {
    /// Encodes and writes one tile; returns the bytes written.
    ///
    /// Safe to call from a background thread as tiles complete, in any order.
    ///
    /// # Errors
    ///
    /// Returns the encoder or I/O error.
    pub fn write_tile(&self, tile: &TileCache) -> Result<u64, PersistError> {
        let chunk = encode_tile(tile, self.meta.compression)?;
        write_atomic(&self.tile_path(tile.header().tile_index), &chunk)?;
        Ok(chunk.len() as u64)
    }

    /// Reads and verifies one tile.
    ///
    /// # Errors
    ///
    /// Returns the I/O error, the verification failure, or a tile-index mismatch.
    pub fn read_tile(&self, index: u32) -> Result<TileCache, PersistError> {
        let bytes = fs::read(self.tile_path(index))?;
        let tile = decode_tile(&bytes)?;
        let found = tile.header().tile_index;
        if found != index {
            return Err(PersistError::TileMismatch {
                expected: index,
                found,
            });
        }
        Ok(tile)
    }

    /// Encodes and writes the chunk of retained batch `index` of `count` (the leaf cache, beam
    /// guide section 14.10); returns the bytes written. The chunk lives where a tile chunk of the
    /// same index would.
    ///
    /// # Errors
    ///
    /// Returns the encoder or I/O error.
    #[cfg(feature = "beam")]
    pub fn write_beam_chunk(
        &self,
        index: u32,
        count: u32,
        beam_version: u32,
        words: &[u32],
    ) -> Result<u64, PersistError> {
        let chunk = encode_beam_chunk(words, index, count, beam_version, self.meta.compression)?;
        write_atomic(&self.tile_path(index), &chunk)?;
        Ok(chunk.len() as u64)
    }

    /// Reads and verifies the chunk of retained batch `index`.
    ///
    /// # Errors
    ///
    /// Returns the I/O error (a missing chunk is [`io::ErrorKind::NotFound`]), the verification
    /// failure, or an index mismatch.
    #[cfg(feature = "beam")]
    pub fn read_beam_chunk(
        &self,
        index: u32,
        beam_version: u32,
    ) -> Result<BeamChunk, PersistError> {
        let bytes = fs::read(self.tile_path(index))?;
        let chunk = decode_beam_chunk(&bytes, beam_version)?;
        if chunk.batch_index != index {
            return Err(PersistError::TileMismatch {
                expected: index,
                found: chunk.batch_index,
            });
        }
        Ok(chunk)
    }

    /// Reads every tile in parallel, in tile order.
    ///
    /// The first chunk that fails (lowest tile index) deletes the whole folder and returns
    /// [`RebuildReason::Corrupt`].
    ///
    /// # Errors
    ///
    /// Returns why the cache has to be rebuilt.
    pub fn load_all(&self) -> Result<Vec<TileCache>, RebuildReason> {
        let count = self.meta.tile_count as usize;
        let threads = std::thread::available_parallelism()
            .map_or(1, std::num::NonZero::get)
            .min(count.max(1));
        let per_thread = count.div_ceil(threads).max(1);
        let mut results: Vec<Result<TileCache, PersistError>> = Vec::with_capacity(count);
        std::thread::scope(|scope| {
            let handles: Vec<_> = (0..count)
                .step_by(per_thread)
                .map(|start| {
                    let end = (start + per_thread).min(count);
                    scope.spawn(move || {
                        (start..end)
                            .map(|index| self.read_tile(u32::try_from(index).unwrap_or(u32::MAX)))
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            for handle in handles {
                match handle.join() {
                    Ok(part) => results.extend(part),
                    Err(_) => results.push(Err(PersistError::Io(io::Error::other(
                        "a tile reader thread panicked",
                    )))),
                }
            }
        });
        let mut tiles = Vec::with_capacity(count);
        for (index, result) in results.into_iter().enumerate() {
            match result {
                Ok(tile) => tiles.push(tile),
                Err(error) => {
                    let reason = RebuildReason::Corrupt {
                        tile: u32::try_from(index).unwrap_or(u32::MAX),
                        detail: error.to_string(),
                    };
                    return Err(discard(&self.dir, reason));
                }
            }
        }
        Ok(tiles)
    }
}

/// Opens the cache of `key`, loads every tile in parallel and stamps it as used.
///
/// # Errors
///
/// Returns why the cache has to be rebuilt; a corrupt cache is already deleted by then.
#[cfg(feature = "path-cache-disk")]
pub fn load_cache(
    root: &Path,
    key: &CacheKey,
) -> Result<(CacheMeta, Vec<TileCache>), RebuildReason> {
    let mut folder = KeyFolder::open(root, key)?;
    let tiles = folder.load_all()?;
    if let Err(error) = folder.mark_used() {
        tracing::warn!(%error, "path cache: could not stamp the use time");
    }
    Ok((folder.meta.clone(), tiles))
}

/// Deletes a key folder, logs why, and returns the reason for the caller.
fn discard(dir: &Path, reason: RebuildReason) -> RebuildReason {
    tracing::warn!(folder = %dir.display(), %reason, "path cache: discarding the cache, it will be rebuilt");
    remove_folder(dir);
    reason
}

/// Deletes a folder tree; a failure other than "not found" is logged.
fn remove_folder(dir: &Path) {
    match fs::remove_dir_all(dir) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => {
            tracing::warn!(folder = %dir.display(), %error, "path cache: could not delete a folder");
        }
    }
}
