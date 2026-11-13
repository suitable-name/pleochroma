//! Tests of the tile codec: delta, shuffle and (with `path-cache-disk`) the chunk format.

use super::{
    compress::{
        CHUNK_HEADER_LEN, CHUNK_MAGIC, compression_code, compression_from_code, delta_decode_dir,
        delta_encode_dir, file_extension, shuffle_records, shuffle_words, unshuffle_records,
        unshuffle_words,
    },
    record::ExitRecord,
};
#[cfg(feature = "path-cache-disk")]
use super::{
    layout::TileCache,
    positions::BuildSettings,
    tests_support::{Rig, glass},
    tile::TileBuilder,
};
#[cfg(feature = "path-cache-disk")]
use crate::optics::raytracer::transport::PreparedMaterial;
use crate::renderer::accel::CacheCompression;

/// Every compression mode.
pub(super) const MODES: [CacheCompression; 3] = [
    CacheCompression::None,
    CacheCompression::Zstd,
    CacheCompression::Lz4,
];

/// A small deterministic generator (xorshift32).
pub(super) struct Lcg(pub(super) u32);

impl Lcg {
    pub(super) fn next(&mut self) -> u32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        x
    }

    fn short(&mut self) -> u16 {
        u16::try_from(self.next() >> 16).unwrap_or(0)
    }
}

/// A record full of noise, with extreme `dir_oct` values mixed in to exercise the wrapping.
pub(super) fn noisy_record(rng: &mut Lcg, extreme: bool) -> ExitRecord {
    let mut weight = [0_u16; 8];
    for value in &mut weight {
        *value = rng.short();
    }
    let dir_oct = if extreme {
        [0, u16::MAX]
    } else {
        [rng.short(), rng.short()]
    };
    ExitRecord {
        dir_oct,
        ddir_dlambda: [rng.short(), rng.short()],
        weight,
        flags: rng.next(),
        signature: rng.next(),
        depth_bits: [rng.next(), rng.next(), rng.next()],
    }
}

/// A synthetic tile: 3 pixels, 3 positions (one pixel without any), 9 records.
#[cfg(feature = "path-cache-disk")]
pub(super) fn synthetic_tile() -> TileCache {
    let mut rng = Lcg(0x1234_5678);
    let records: Vec<ExitRecord> = (0..9).map(|i| noisy_record(&mut rng, i % 4 == 0)).collect();
    TileCache::from_parts(
        7,
        &[0, 2, 2, 3],
        &[0, 3, 5, 9],
        &[0x3c00, 0x0001, 0x7bff],
        &records,
    )
}

/// A synthetic tile with no positions at all (a tile off the stone).
#[cfg(feature = "path-cache-disk")]
pub(super) fn empty_tile() -> TileCache {
    TileCache::from_parts(3, &[0, 0, 0], &[0], &[], &[])
}

/// The tiles of a real 12 x 12 frame of a cube, built with `tile` pixels per tile edge.
#[cfg(feature = "path-cache-disk")]
pub(super) fn real_tiles(tile: u32) -> Vec<TileCache> {
    let rig = Rig::cube_from_above(glass());
    let scene = rig.scene(12, 8);
    let prepared = PreparedMaterial::new(&rig.material);
    let settings = BuildSettings {
        tile,
        ..BuildSettings::default()
    };
    let builder = TileBuilder::new(&scene, &prepared, &rig.soa, settings);
    (0..builder.tile_count())
        .map(|index| builder.build_tile(index))
        .collect()
}

#[test]
fn words_shuffle_and_unshuffle_exactly() {
    let words = [0x0102_0304_u32, 0xffee_ddcc, 0, u32::MAX, 0x8000_0001];
    let shuffled = shuffle_words(&words);
    assert_eq!(shuffled.len(), 20);
    assert_eq!(
        &shuffled[..5],
        &[0x04, 0xcc, 0x00, 0xff, 0x01],
        "the first stream is the low byte of every word"
    );
    assert_eq!(unshuffle_words(&shuffled), words);
    assert_eq!(unshuffle_words(&[]), Vec::<u32>::new());
}

#[test]
fn records_shuffle_into_one_stream_per_byte() {
    let mut rng = Lcg(99);
    let records: Vec<ExitRecord> = (0..5).map(|_| noisy_record(&mut rng, false)).collect();
    let shuffled = shuffle_records(&records);
    assert_eq!(shuffled.len(), 5 * 44);
    let low_bytes: Vec<u8> = records
        .iter()
        .map(|r| r.dir_oct[0].to_le_bytes()[0])
        .collect();
    assert_eq!(
        &shuffled[..5],
        low_bytes.as_slice(),
        "stream 0 is the low byte of dir_oct[0]"
    );
    assert_eq!(unshuffle_records(&shuffled), Some(records));
    assert_eq!(unshuffle_records(&[]), Some(Vec::new()));
}

#[test]
fn a_ragged_record_stream_is_rejected() {
    assert_eq!(unshuffle_records(&[0; 31]), None);
    assert_eq!(unshuffle_records(&[0; 65]), None);
}

#[test]
fn delta_round_trips_with_wrapping_and_leaves_each_first_record_alone() {
    let mut rng = Lcg(5);
    let records: Vec<ExitRecord> = (0..9).map(|i| noisy_record(&mut rng, i % 3 == 0)).collect();
    let offsets = [0_u32, 3, 3, 9];
    let encoded = delta_encode_dir(&records, &offsets);
    assert_eq!(encoded[0].dir_oct, records[0].dir_oct);
    assert_eq!(encoded[3].dir_oct, records[3].dir_oct);
    assert_eq!(
        encoded[1].dir_oct[0],
        records[1].dir_oct[0].wrapping_sub(records[0].dir_oct[0])
    );
    assert_eq!(
        encoded[4].dir_oct[1],
        records[4].dir_oct[1].wrapping_sub(records[3].dir_oct[1])
    );
    for (before, after) in records.iter().zip(&encoded) {
        assert_eq!(before.weight, after.weight, "only dir_oct changes");
        assert_eq!(before.flags, after.flags);
    }
    let mut decoded = encoded;
    delta_decode_dir(&mut decoded, &offsets);
    assert_eq!(decoded, records);
}

#[test]
fn delta_ignores_offsets_that_do_not_fit_the_records() {
    let mut rng = Lcg(8);
    let records: Vec<ExitRecord> = (0..4).map(|_| noisy_record(&mut rng, false)).collect();
    assert_eq!(delta_encode_dir(&records, &[0, 99]), records);
    assert_eq!(delta_encode_dir(&records, &[3, 1]), records);
    let mut same = records.clone();
    delta_decode_dir(&mut same, &[2, 1, 99]);
    assert_eq!(same, records);
}

#[test]
fn compression_codes_and_extensions_are_distinct_and_invertible() {
    let mut codes = Vec::new();
    let mut extensions = Vec::new();
    for mode in MODES {
        let code = compression_code(mode);
        assert_eq!(compression_from_code(code), Some(mode));
        codes.push(code);
        extensions.push(file_extension(mode));
    }
    assert_eq!(codes, [0, 1, 2]);
    assert_eq!(extensions, ["bin", "zst", "lz4"]);
    assert_eq!(compression_from_code(3), None);
    assert_eq!(CacheCompression::default(), CacheCompression::Zstd);
}

#[test]
fn the_header_is_forty_bytes_and_the_magic_names_the_format() {
    assert_eq!(CHUNK_HEADER_LEN, 40);
    assert_eq!(&CHUNK_MAGIC, b"PCTILE1\0");
}

#[cfg(feature = "path-cache-disk")]
mod chunks {
    use super::{
        CHUNK_HEADER_LEN, CHUNK_MAGIC, MODES, TileCache, empty_tile, real_tiles, synthetic_tile,
    };
    use crate::renderer::{
        accel::CacheCompression,
        path_cache::{
            compress::{ChunkError, decode_tile, encode_tile},
            key::CACHE_FORMAT_VERSION,
        },
    };

    fn round_trip(tile: &TileCache, mode: CacheCompression) {
        let chunk = encode_tile(tile, mode).expect("encodes");
        let back = decode_tile(&chunk).expect("decodes");
        assert_eq!(&back, tile, "{mode:?} must round-trip bit-exactly");
        assert_eq!(back.as_words(), tile.as_words());
    }

    #[test]
    fn every_mode_round_trips_a_synthetic_tile_bit_exactly() {
        for mode in MODES {
            round_trip(&synthetic_tile(), mode);
            round_trip(&empty_tile(), mode);
        }
    }

    #[test]
    fn every_mode_round_trips_real_tiles_bit_exactly() {
        let tiles = real_tiles(8);
        assert_eq!(tiles.len(), 4, "a 12 x 12 frame at tile 8 has 2 x 2 tiles");
        assert!(
            tiles.iter().any(|tile| !tile.records().is_empty()),
            "the cube covers part of the frame"
        );
        for mode in MODES {
            for tile in &tiles {
                round_trip(tile, mode);
            }
        }
    }

    #[test]
    fn the_chunk_header_has_the_documented_layout() {
        let tile = synthetic_tile();
        let chunk = encode_tile(&tile, CacheCompression::Lz4).expect("encodes");
        assert_eq!(&chunk[..8], &CHUNK_MAGIC);
        assert_eq!(chunk[8..12], CACHE_FORMAT_VERSION.to_le_bytes());
        assert_eq!(chunk[12], 2, "lz4");
        assert_eq!(chunk[13..16], [0, 0, 0]);
        assert_eq!(
            u64::from_le_bytes(chunk[16..24].try_into().expect("8 bytes")),
            tile.byte_len() as u64
        );
        assert_eq!(
            u64::from_le_bytes(chunk[24..32].try_into().expect("8 bytes")),
            (chunk.len() - CHUNK_HEADER_LEN) as u64
        );
        // 4 header words, 4 pixel offsets, 4 position offsets and 2 residual words precede the
        // records.
        assert_eq!(
            u32::from_le_bytes(chunk[36..40].try_into().expect("4 bytes")),
            14
        );
    }

    #[test]
    fn encoding_is_deterministic() {
        let tile = synthetic_tile();
        for mode in MODES {
            assert_eq!(
                encode_tile(&tile, mode).expect("encodes"),
                encode_tile(&tile, mode).expect("encodes")
            );
        }
    }

    #[test]
    fn a_flipped_payload_byte_is_a_crc_failure_when_stored_plain() {
        let tile = synthetic_tile();
        let mut chunk = encode_tile(&tile, CacheCompression::None).expect("encodes");
        let last = chunk.len() - 1;
        chunk[last] ^= 0x40;
        assert!(matches!(
            decode_tile(&chunk),
            Err(ChunkError::CrcMismatch { .. })
        ));
    }

    #[test]
    fn a_flipped_byte_anywhere_is_rejected_in_every_mode() {
        let tile = synthetic_tile();
        for mode in MODES {
            let chunk = encode_tile(&tile, mode).expect("encodes");
            for at in 0..chunk.len() {
                let mut broken = chunk.clone();
                broken[at] ^= 0x01;
                // A flipped bit inside the 3 reserved bytes is the one change nothing reads.
                if (13..16).contains(&at) {
                    continue;
                }
                // The CRC covers the decoded tile, not the compressed bytes: a flipped lz4 match
                // offset inside a run of equal bytes can decode to the same tile, which is not
                // corruption. A chunk that decodes must decode to the tile that was written.
                if let Ok(decoded) = decode_tile(&broken) {
                    assert_eq!(
                        decoded, tile,
                        "{mode:?}: byte {at} flipped and the chunk decoded to another tile"
                    );
                }
            }
        }
    }

    #[test]
    fn a_truncated_chunk_is_rejected_not_a_panic() {
        let tile = synthetic_tile();
        for mode in MODES {
            let chunk = encode_tile(&tile, mode).expect("encodes");
            for len in 0..chunk.len() {
                assert!(
                    decode_tile(&chunk[..len]).is_err(),
                    "{mode:?}: a chunk cut to {len} bytes decoded"
                );
            }
            assert_eq!(decode_tile(&chunk[..10]), Err(ChunkError::Truncated));
            assert_eq!(
                decode_tile(&chunk[..chunk.len() - 1]),
                Err(ChunkError::Truncated)
            );
        }
        assert_eq!(decode_tile(&[]), Err(ChunkError::Truncated));
    }

    #[test]
    fn trailing_bytes_are_rejected() {
        let mut chunk = encode_tile(&synthetic_tile(), CacheCompression::Zstd).expect("encodes");
        chunk.push(0);
        assert_eq!(decode_tile(&chunk), Err(ChunkError::BadLength));
    }

    #[test]
    fn a_foreign_magic_version_or_compression_is_named() {
        let chunk = encode_tile(&synthetic_tile(), CacheCompression::None).expect("encodes");

        let mut bad_magic = chunk.clone();
        bad_magic[0] = b'X';
        assert_eq!(decode_tile(&bad_magic), Err(ChunkError::BadMagic));

        let mut bad_version = chunk.clone();
        bad_version[8..12].copy_from_slice(&(CACHE_FORMAT_VERSION + 1).to_le_bytes());
        assert_eq!(
            decode_tile(&bad_version),
            Err(ChunkError::VersionMismatch {
                found: CACHE_FORMAT_VERSION + 1
            })
        );

        let mut bad_mode = chunk;
        bad_mode[12] = 9;
        assert_eq!(
            decode_tile(&bad_mode),
            Err(ChunkError::UnknownCompression(9))
        );
    }

    #[test]
    fn an_absurd_length_is_rejected_before_it_allocates() {
        let mut chunk = encode_tile(&synthetic_tile(), CacheCompression::Zstd).expect("encodes");
        chunk[16..24].copy_from_slice(&u64::MAX.to_le_bytes());
        assert_eq!(decode_tile(&chunk), Err(ChunkError::BadLength));
    }
}
