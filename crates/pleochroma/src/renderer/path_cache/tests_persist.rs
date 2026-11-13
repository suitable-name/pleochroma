//! Tests of the on-disk store: metadata, folder handling and (with `path-cache-disk`) tiles.

use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU32, Ordering},
};

use glam::Mat4;

use super::{
    key::{CACHE_FORMAT_VERSION, CacheKey, CacheKeyInputs},
    persist::{CacheMeta, KeyFolder, RebuildReason, read_meta},
    positions::BuildSettings,
    tests_support::{Rig, glass},
};
#[cfg(feature = "path-cache-disk")]
use super::{
    persist::load_cache,
    tests_compress::{MODES, real_tiles},
};
use crate::renderer::accel::CacheCompression;

/// A uniquely named folder under the system temp folder, removed when dropped.
pub(super) struct TempDir(PathBuf);

impl TempDir {
    pub(super) fn new(label: &str) -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "pleochroma_path_cache_{label}_{}_{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("temp folder");
        Self(path)
    }

    pub(super) fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// The key and the metadata of a 12 x 12 cube frame with 8 pixel tiles (four tiles), created at
/// Unix second 1000.
pub(super) fn sample_meta(compression: CacheCompression) -> (CacheKey, CacheMeta) {
    let rig = Rig::cube_from_above(glass());
    let scene = rig.scene(12, 8);
    let settings = BuildSettings {
        tile: 8,
        ..BuildSettings::default()
    };
    let inputs = CacheKeyInputs::from_scene(&scene, Mat4::IDENTITY, &settings);
    let key = CacheKey::new(&inputs);
    let meta = CacheMeta::describe(&inputs, &key, compression, 1000);
    (key, meta)
}

#[test]
fn the_description_names_the_inputs() {
    let (key, meta) = sample_meta(CacheCompression::Zstd);
    assert_eq!(meta.format_version, CACHE_FORMAT_VERSION);
    assert_eq!(meta.key, key.to_string());
    assert_eq!(meta.material, "cache test glass");
    assert_eq!(meta.plane_count, 6);
    assert_eq!((meta.width, meta.height), (12, 12));
    assert_eq!(meta.max_bounces, 8);
    assert_eq!(meta.tile, 8);
    assert_eq!(meta.tile_count, 4);
    assert_eq!(meta.compression, CacheCompression::Zstd);
    assert_eq!((meta.created_unix, meta.last_used_unix), (1000, 1000));
    assert_eq!(meta.size_bytes, 0);
    assert!(!meta.complete);
}

#[test]
fn meta_json_round_trips_and_stays_readable() {
    for mode in [
        CacheCompression::None,
        CacheCompression::Zstd,
        CacheCompression::Lz4,
    ] {
        let (_, mut meta) = sample_meta(mode);
        meta.size_bytes = u64::MAX;
        meta.complete = true;
        let text = meta.to_json();
        assert_eq!(CacheMeta::from_json(&text), Some(meta.clone()));
        assert!(text.contains("\"material\": \"cache test glass\""));
        assert!(text.contains("\"width\": 12"));
        assert!(text.contains("\"tile_count\": 4"));
        assert!(text.contains("\"complete\": true"));
        assert!(text.contains(&format!("\"format_version\": {CACHE_FORMAT_VERSION}")));
    }
}

#[test]
fn meta_strings_with_quotes_slashes_and_control_characters_survive() {
    let (_, mut meta) = sample_meta(CacheCompression::None);
    meta.material =
        "a \"quoted\" \\ name\nwith\ttab, {braces}, \"width\": 99 and \u{1} \u{e9}\u{4e2d}"
            .to_string();
    let parsed = CacheMeta::from_json(&meta.to_json()).expect("parses");
    assert_eq!(parsed, meta);
}

#[test]
fn a_malformed_meta_is_none() {
    let (_, meta) = sample_meta(CacheCompression::Zstd);
    let good = meta.to_json();
    assert_eq!(CacheMeta::from_json(""), None);
    assert_eq!(CacheMeta::from_json("{}"), None);
    assert_eq!(CacheMeta::from_json("not json"), None);
    assert_eq!(CacheMeta::from_json(&good[..good.len() / 2]), None);
    assert_eq!(
        CacheMeta::from_json(&good.replace("\"zstd\"", "\"gzip\"")),
        None
    );
    assert_eq!(
        CacheMeta::from_json(&good.replace("\"complete\": false", "\"complete\": maybe")),
        None
    );
    assert_eq!(
        CacheMeta::from_json(&good.replace("\"tile\": 8", "\"tile\": -1")),
        None
    );
}

#[test]
fn the_folder_types_can_cross_threads() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<KeyFolder>();
    assert_send_sync::<CacheMeta>();
}

#[test]
fn tile_files_are_numbered_and_carry_the_mode_extension() {
    let root = TempDir::new("paths");
    for (mode, expected) in [
        (CacheCompression::Zstd, "00007.zst"),
        (CacheCompression::Lz4, "00007.lz4"),
        (CacheCompression::None, "00007.bin"),
    ] {
        let (key, meta) = sample_meta(mode);
        let folder = KeyFolder::create(root.path(), &key, meta).expect("creates");
        let path = folder.tile_path(7);
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some(expected)
        );
        assert_eq!(path.parent(), Some(folder.dir().join("tiles").as_path()));
        assert_eq!(folder.dir(), KeyFolder::folder_path(root.path(), &key));
    }
}

#[test]
fn a_key_folder_is_named_by_the_key_hex() {
    let (key, _) = sample_meta(CacheCompression::Zstd);
    let path = KeyFolder::folder_path(Path::new("root"), &key);
    assert_eq!(
        path.file_name().and_then(|name| name.to_str()),
        Some(key.to_string().as_str())
    );
    assert_eq!(key.to_string().len(), 32);
}

#[test]
fn create_makes_an_unfinished_folder_that_open_refuses_and_deletes() {
    let root = TempDir::new("unfinished");
    let (key, meta) = sample_meta(CacheCompression::Zstd);
    let folder = KeyFolder::create(root.path(), &key, meta.clone()).expect("creates");
    assert!(folder.dir().join("tiles").is_dir());
    assert_eq!(read_meta(folder.dir()), Some(meta));
    assert!(!folder.meta().complete);

    assert_eq!(
        KeyFolder::open(root.path(), &key).err(),
        Some(RebuildReason::Incomplete)
    );
    assert!(!folder.dir().exists(), "an unfinished cache is deleted");
}

#[test]
fn a_finished_folder_opens_with_its_meta() {
    let root = TempDir::new("finished");
    let (key, meta) = sample_meta(CacheCompression::Lz4);
    let mut folder = KeyFolder::create(root.path(), &key, meta).expect("creates");
    folder.finish(1234).expect("finishes");
    assert!(folder.meta().complete);

    let opened = KeyFolder::open(root.path(), &key).expect("opens");
    assert_eq!(opened.meta().size_bytes, 1234);
    assert!(opened.meta().complete);
    assert_eq!(opened.meta().compression, CacheCompression::Lz4);
    assert_eq!(opened.meta().created_unix, 1000);
    assert!(opened.meta().last_used_unix >= 1000);
}

#[test]
fn mark_used_stores_the_last_used_time() {
    let root = TempDir::new("used");
    let (key, meta) = sample_meta(CacheCompression::Zstd);
    let mut folder = KeyFolder::create(root.path(), &key, meta).expect("creates");
    folder.finish(10).expect("finishes");
    folder.mark_used_at(5000).expect("marks");
    assert_eq!(
        read_meta(folder.dir()).map(|m| m.last_used_unix),
        Some(5000)
    );
    let opened = KeyFolder::open(root.path(), &key).expect("opens");
    assert_eq!(opened.meta().last_used_unix, 5000);
    assert_eq!(opened.meta().created_unix, 1000, "creation time is kept");
}

#[test]
fn a_missing_key_is_missing_and_nothing_is_created() {
    let root = TempDir::new("missing");
    let (key, _) = sample_meta(CacheCompression::Zstd);
    assert_eq!(
        KeyFolder::open(root.path(), &key).err(),
        Some(RebuildReason::Missing)
    );
    assert_eq!(fs::read_dir(root.path()).expect("lists").count(), 0);
}

#[test]
fn an_unreadable_meta_deletes_the_folder() {
    let root = TempDir::new("badmeta");
    let (key, meta) = sample_meta(CacheCompression::Zstd);
    let folder = KeyFolder::create(root.path(), &key, meta).expect("creates");
    fs::write(folder.dir().join("meta.json"), "garbage").expect("writes");
    assert_eq!(
        KeyFolder::open(root.path(), &key).err(),
        Some(RebuildReason::BadMeta)
    );
    assert!(!folder.dir().exists());
}

#[test]
fn a_meta_of_another_format_version_deletes_the_folder() {
    let root = TempDir::new("version");
    let (key, mut meta) = sample_meta(CacheCompression::Zstd);
    meta.format_version = CACHE_FORMAT_VERSION + 1;
    meta.complete = true;
    let folder = KeyFolder::create(root.path(), &key, meta.clone()).expect("creates");
    // `create` leaves the cache unfinished; rewrite the meta as a finished old-format one.
    fs::write(folder.dir().join("meta.json"), meta.to_json()).expect("writes");
    assert_eq!(
        KeyFolder::open(root.path(), &key).err(),
        Some(RebuildReason::VersionChanged {
            found: CACHE_FORMAT_VERSION + 1
        })
    );
    assert!(!folder.dir().exists());
}

#[test]
fn a_meta_that_names_another_key_deletes_the_folder() {
    let root = TempDir::new("otherkey");
    let (key, mut meta) = sample_meta(CacheCompression::Zstd);
    meta.key = CacheKey::from_bytes([9; 16]).to_string();
    let mut folder = KeyFolder::create(root.path(), &key, meta).expect("creates");
    folder.finish(1).expect("finishes");
    assert_eq!(
        KeyFolder::open(root.path(), &key).err(),
        Some(RebuildReason::BadMeta)
    );
    assert!(!folder.dir().exists());
}

#[test]
fn create_replaces_what_was_there() {
    let root = TempDir::new("replace");
    let (key, meta) = sample_meta(CacheCompression::Zstd);
    let first = KeyFolder::create(root.path(), &key, meta.clone()).expect("creates");
    let stray = first.dir().join("tiles").join("00000.zst");
    fs::write(&stray, "old").expect("writes");
    let second = KeyFolder::create(root.path(), &key, meta).expect("creates again");
    assert!(!stray.exists());
    assert!(second.dir().join("tiles").is_dir());
}

#[test]
fn delete_removes_the_folder() {
    let root = TempDir::new("delete");
    let (key, meta) = sample_meta(CacheCompression::Zstd);
    let folder = KeyFolder::create(root.path(), &key, meta).expect("creates");
    let dir = folder.dir().to_path_buf();
    folder.delete();
    assert!(!dir.exists());
}

#[cfg(feature = "path-cache-disk")]
mod tiles {
    use super::{
        CacheCompression, KeyFolder, MODES, RebuildReason, TempDir, fs, load_cache, real_tiles,
        sample_meta,
    };
    use crate::renderer::path_cache::layout::TileCache;

    /// Writes all tiles of a frame, one thread per tile, and finishes the folder.
    fn write_all(root: &super::Path, mode: CacheCompression, order: &[usize]) -> KeyFolder {
        let tiles = real_tiles(8);
        let (key, meta) = sample_meta(mode);
        let mut folder = KeyFolder::create(root, &key, meta).expect("creates");
        let total: u64 = std::thread::scope(|scope| {
            let handles: Vec<_> = order
                .iter()
                .map(|&index| {
                    let tile: &TileCache = &tiles[index];
                    let folder = &folder;
                    scope.spawn(move || folder.write_tile(tile).expect("writes"))
                })
                .collect();
            // Every writer is spawned above before the first join, so the tiles finish in any
            // order.
            let mut total = 0_u64;
            for handle in handles {
                total += handle.join().expect("joins");
            }
            total
        });
        folder.finish(total).expect("finishes");
        folder
    }

    #[test]
    fn written_tiles_load_back_identical_in_every_mode() {
        for mode in MODES {
            let root = TempDir::new("roundtrip");
            let folder = write_all(root.path(), mode, &[0, 1, 2, 3]);
            let (key, _) = sample_meta(mode);
            let (meta, tiles) = load_cache(root.path(), &key).expect("loads");
            assert_eq!(tiles, real_tiles(8), "{mode:?}");
            assert_eq!(meta.tile_count, 4);
            assert_eq!(meta.compression, mode);
            assert!(meta.complete);
            assert!(meta.size_bytes > 0);
            assert!(meta.last_used_unix > 1000, "loading stamps the use time");
            assert!(folder.dir().is_dir());
        }
    }

    #[test]
    fn the_files_do_not_depend_on_the_order_tiles_finish_in() {
        let forward = TempDir::new("forward");
        let reverse = TempDir::new("reverse");
        let a = write_all(forward.path(), CacheCompression::Zstd, &[0, 1, 2, 3]);
        let b = write_all(reverse.path(), CacheCompression::Zstd, &[3, 2, 1, 0]);
        for index in 0..4 {
            assert_eq!(
                fs::read(a.tile_path(index)).expect("reads"),
                fs::read(b.tile_path(index)).expect("reads"),
                "tile {index}"
            );
        }
        assert_eq!(a.meta().size_bytes, b.meta().size_bytes);
    }

    #[test]
    fn one_tile_can_be_read_alone() {
        let root = TempDir::new("single");
        let folder = write_all(root.path(), CacheCompression::Lz4, &[0, 1, 2, 3]);
        let tiles = real_tiles(8);
        assert_eq!(folder.read_tile(2).expect("reads"), tiles[2]);
        assert!(folder.read_tile(9).is_err(), "there is no tile 9");
    }

    #[test]
    fn a_corrupt_chunk_deletes_the_folder_and_asks_for_a_rebuild() {
        let root = TempDir::new("corrupt");
        let folder = write_all(root.path(), CacheCompression::None, &[0, 1, 2, 3]);
        let path = folder.tile_path(2);
        let mut bytes = fs::read(&path).expect("reads");
        let last = bytes.len() - 1;
        bytes[last] ^= 0x10;
        fs::write(&path, bytes).expect("writes");

        let (key, _) = sample_meta(CacheCompression::None);
        let reason = load_cache(root.path(), &key).expect_err("must not load");
        assert!(
            matches!(reason, RebuildReason::Corrupt { tile: 2, .. }),
            "{reason:?}"
        );
        assert!(!folder.dir().exists(), "the key folder is deleted");
        assert_eq!(
            KeyFolder::open(root.path(), &key).err(),
            Some(RebuildReason::Missing)
        );
    }

    #[test]
    fn a_truncated_missing_or_misplaced_chunk_also_rebuilds() {
        // Truncated.
        let root = TempDir::new("truncated");
        let folder = write_all(root.path(), CacheCompression::Zstd, &[0, 1, 2, 3]);
        let bytes = fs::read(folder.tile_path(1)).expect("reads");
        fs::write(folder.tile_path(1), &bytes[..20]).expect("writes");
        let (key, _) = sample_meta(CacheCompression::Zstd);
        assert!(matches!(
            load_cache(root.path(), &key).err(),
            Some(RebuildReason::Corrupt { tile: 1, .. })
        ));
        assert!(!folder.dir().exists());

        // Missing.
        let root = TempDir::new("missingchunk");
        let folder = write_all(root.path(), CacheCompression::Zstd, &[0, 1, 2, 3]);
        fs::remove_file(folder.tile_path(3)).expect("removes");
        assert!(matches!(
            load_cache(root.path(), &key).err(),
            Some(RebuildReason::Corrupt { tile: 3, .. })
        ));
        assert!(!folder.dir().exists());

        // A valid chunk in the wrong slot.
        let root = TempDir::new("misplaced");
        let folder = write_all(root.path(), CacheCompression::Zstd, &[0, 1, 2, 3]);
        fs::copy(folder.tile_path(0), folder.tile_path(1)).expect("copies");
        assert!(matches!(
            load_cache(root.path(), &key).err(),
            Some(RebuildReason::Corrupt { tile: 1, .. })
        ));
        assert!(!folder.dir().exists());
    }

    #[test]
    fn the_lowest_failing_tile_is_the_one_reported() {
        let root = TempDir::new("lowest");
        let folder = write_all(root.path(), CacheCompression::None, &[0, 1, 2, 3]);
        for index in [3, 1] {
            fs::write(folder.tile_path(index), b"junk").expect("writes");
        }
        let (key, _) = sample_meta(CacheCompression::None);
        assert!(matches!(
            load_cache(root.path(), &key).err(),
            Some(RebuildReason::Corrupt { tile: 1, .. })
        ));
    }

    #[test]
    fn a_folder_whose_writer_never_finished_is_rebuilt() {
        let root = TempDir::new("cancelled");
        let tiles = real_tiles(8);
        let (key, meta) = sample_meta(CacheCompression::Zstd);
        let folder = KeyFolder::create(root.path(), &key, meta).expect("creates");
        folder.write_tile(&tiles[0]).expect("writes");
        assert_eq!(
            load_cache(root.path(), &key).err(),
            Some(RebuildReason::Incomplete)
        );
        assert!(!folder.dir().exists());
    }
}
