//! Tests of the persisted leaf cache: its chunks, its key and the shared LRU.
#![cfg(feature = "path-cache-disk")]

use std::{
    fs,
    path::{Path, PathBuf},
    sync::atomic::{AtomicU32, Ordering},
};

use glam::{DVec2, Mat4, Vec3};

use super::{
    BeamSettings, RegionMode,
    beam::{BranchPrefix, Leaf, SpectralInterval},
    geom::polygon::ConvexPolygon,
    leaf_cache::{BEAM_CACHE_FORMAT_VERSION, LeafCache, RetainedBatch},
    primary::TileRect,
    residual::KeptIndex,
    tests_support::{BeamRig, glass, sapphire},
};
use crate::{
    optics::raytracer::{Camera, LightingPreset},
    renderer::{
        accel::CacheCompression,
        path_cache::{
            CacheKey, CacheKeyInputs, CacheMeta, KeyFolder,
            key::{beam_cache_key, beam_canonical_bytes},
            lru,
            positions::BuildSettings,
        },
    },
};

/// The frame edge of the key scenes.
const SIZE: u32 = 12;

/// The adaptive tile edge of the key scenes.
const TILE: u32 = 8;

/// The Unix second the path-cache entry of the LRU test was last used.
const OLDEST_USED: u64 = 100;

/// The Unix second the older beam entry of the LRU test was last used.
const MIDDLE_USED: u64 = 200;

/// Bytes the path-cache entry of the LRU test claims.
const PATH_ENTRY_BYTES: u64 = 1000;

/// The compression modes of the store.
const MODES: [CacheCompression; 3] = [
    CacheCompression::None,
    CacheCompression::Zstd,
    CacheCompression::Lz4,
];

/// A uniquely named folder under the system temp folder, removed when dropped.
struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "pleochroma_beam_cache_{label}_{}_{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("temp folder");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A leaf with irregular but deterministic content; `seed` varies every field.
fn leaf(tile: u32, seed: u8) -> Leaf {
    let x = f64::from(seed % 5).mul_add(0.5, f64::from(tile % 2) * 4.0) + 0.1;
    let y = f64::from(seed % 3).mul_add(0.7, 0.2);
    let w = f64::from(seed).mul_add(0.01, 1.0);
    let image = ConvexPolygon::from_image_points(&[
        DVec2::new(x, y),
        DVec2::new(x + w, y),
        DVec2::new(x + w + 0.1, y + w),
        DVec2::new(x, y + w),
    ])
    .expect("a trapezoid");
    let nodes = u8::try_from(image.len()).expect("small") * (seed % 4 + 1);
    let first = u16::from(seed % 3);
    let last = first + u16::from(seed % 4);
    let dirs = (0..nodes)
        .map(|i| {
            let f = f32::from(i);
            Vec3::new(
                f.mul_add(0.37, f32::from(seed) / 3.0),
                f.mul_add(-0.11, 1.0 / 7.0),
                f32::from(seed).mul_add(1.0e-3, f),
            )
        })
        .collect();
    let weights = (0..nodes)
        .map(|i| f32::from(i).mul_add(1.0e-3, 1.0 / 9.0))
        .collect();
    let prefix = BranchPrefix::EMPTY
        .push(0x0102)
        .and_then(|p| p.push((u32::from(seed) << 8) | 1))
        .expect("room for two steps");
    Leaf {
        tile,
        image,
        dirs,
        weights,
        spectrum: SpectralInterval { first, last },
        sig: [0x9e37_79b9 ^ u32::from(seed), u32::from(seed) * 0x0101_0101],
        prefix,
        glare: seed.is_multiple_of(2),
        reflection: seed.is_multiple_of(3),
        tau_omega: f32::from(seed).mul_add(1.0e-3, 1.0e-3),
        pre_clip: None,
    }
}

/// A batch of two tiles with three leaves each, and the kept index of each tile.
fn batch(seed: u8) -> RetainedBatch {
    let rects = [
        TileRect {
            x0: 0,
            y0: 0,
            x1: 4,
            y1: 4,
        },
        TileRect {
            x0: 4,
            y0: 0,
            x1: 8,
            y1: 4,
        },
    ];
    let mut leaves = Vec::new();
    let mut kept = Vec::new();
    for (tile, rect) in (0_u32..).zip(rects) {
        let group: Vec<Leaf> = (0..3_u8)
            .map(|k| leaf(tile, seed.wrapping_mul(7).wrapping_add(k)))
            .collect();
        kept.push((rect, KeptIndex::from_leaves(rect, &group)));
        leaves.push((tile, group));
    }
    RetainedBatch {
        tiles: rects.to_vec(),
        leaves,
        kept,
    }
}

/// Every float of a leaf as bits, to compare without `==` semantics.
fn bits(leaf: &Leaf) -> Vec<u64> {
    let mut out = Vec::new();
    for vertex in leaf.image.vertices() {
        out.extend([vertex.x, vertex.y, vertex.z].map(f64::to_bits));
    }
    for dir in &leaf.dirs {
        out.extend([dir.x, dir.y, dir.z].map(|v| u64::from(v.to_bits())));
    }
    out.extend(leaf.weights.iter().map(|w| u64::from(w.to_bits())));
    out.push(u64::from(leaf.tau_omega.to_bits()));
    out
}

/// Asserts two batches are the same in every field and every float bit.
fn assert_same(a: &RetainedBatch, b: &RetainedBatch) {
    assert_eq!(a.tiles, b.tiles);
    assert_eq!(a.leaves, b.leaves);
    assert_eq!(a.kept, b.kept);
    for ((_, left), (_, right)) in a.leaves.iter().zip(&b.leaves) {
        for (x, y) in left.iter().zip(right) {
            assert_eq!(bits(x), bits(y));
        }
    }
}

/// The key and the metadata of a beam cache for `rig`, created at Unix second 1000.
fn beam_meta(
    rig: &BeamRig,
    beam: &BeamSettings,
    compression: CacheCompression,
) -> (CacheKey, CacheMeta) {
    let scene = rig.scene(SIZE, 8);
    let settings = BuildSettings::default();
    let inputs = CacheKeyInputs::from_scene(&scene, Mat4::IDENTITY, &settings);
    let key = beam_cache_key(&inputs, beam, TILE);
    let meta = CacheMeta::describe(&inputs, &key, compression, 1000);
    (key, meta)
}

/// A root holding one stored cache of `batches` under a fresh key folder.
struct Stored {
    root: TempDir,
    key: CacheKey,
    cache: LeafCache,
}

impl Stored {
    fn new(label: &str, compression: CacheCompression, batches: Vec<RetainedBatch>) -> Self {
        let root = TempDir::new(label);
        let rig = BeamRig::cube(glass());
        let (key, meta) = beam_meta(&rig, &BeamSettings::default(), compression);
        let folder = KeyFolder::create(root.path(), &key, meta).expect("create the folder");
        let cache = LeafCache::persist_test_cache(batches);
        cache.store(&folder).expect("store");
        Self { root, key, cache }
    }

    fn open(&self) -> KeyFolder {
        KeyFolder::open(self.root.path(), &self.key).expect("a finished cache")
    }

    fn dir(&self) -> PathBuf {
        KeyFolder::folder_path(self.root.path(), &self.key)
    }
}

#[test]
fn store_then_load_gives_bit_identical_batches() {
    for compression in MODES {
        let stored = Stored::new("roundtrip", compression, vec![batch(1), batch(2), batch(5)]);
        let loaded = LeafCache::load(&stored.open(), u64::MAX)
            .expect("no error")
            .expect("a cache");
        let (before, after) = (
            stored.cache.persist_test_batches(),
            loaded.persist_test_batches(),
        );
        assert_eq!(before.len(), after.len());
        for (a, b) in before.iter().zip(after) {
            assert_same(a, b);
        }
        assert_eq!(loaded.bytes(), stored.cache.bytes());
        assert!(stored.dir().is_dir());
    }
}

#[test]
fn the_cap_keeps_only_the_batches_that_fit() {
    let one = LeafCache::persist_test_cache(vec![batch(1)]).bytes();
    let stored = Stored::new("cap", CacheCompression::Zstd, vec![batch(1), batch(1)]);
    let loaded = LeafCache::load(&stored.open(), one)
        .expect("no error")
        .expect("a cache");
    assert_eq!(loaded.persist_test_batches().len(), 1);
    assert_eq!(loaded.bytes(), one);
    assert_same(&loaded.persist_test_batches()[0], &batch(1));
}

#[test]
fn nothing_stored_loads_as_none_and_keeps_the_folder() {
    let root = TempDir::new("empty");
    let rig = BeamRig::cube(glass());
    let (key, meta) = beam_meta(&rig, &BeamSettings::default(), CacheCompression::Zstd);
    let folder = KeyFolder::create(root.path(), &key, meta).expect("create");
    assert!(
        LeafCache::load(&folder, u64::MAX)
            .expect("no error")
            .is_none()
    );
    LeafCache::new(1).store(&folder).expect("an empty store");
    assert!(
        LeafCache::load(&folder, u64::MAX)
            .expect("no error")
            .is_none()
    );
    assert!(folder.dir().is_dir());
}

#[test]
fn a_flipped_byte_in_any_chunk_rebuilds_silently() {
    let probe = Stored::new("probe", CacheCompression::None, vec![batch(1), batch(2)]);
    let lengths: Vec<usize> = (0..2)
        .map(|index| {
            fs::read(probe.open().tile_path(index))
                .expect("chunk")
                .len()
        })
        .collect();
    drop(probe);
    for index in 0..2_u32 {
        let length = lengths[index as usize];
        // Every header byte, then every seventh payload byte and the last one.
        let positions = (0..48).chain((48..length).step_by(7)).chain([length - 1]);
        for at in positions {
            let stored = Stored::new("flip", CacheCompression::None, vec![batch(1), batch(2)]);
            let folder = stored.open();
            let path = folder.tile_path(index);
            let mut bytes = fs::read(&path).expect("chunk");
            bytes[at] ^= 0x01;
            fs::write(&path, bytes).expect("write back");
            let loaded = LeafCache::load(&folder, u64::MAX).expect("no error");
            assert!(loaded.is_none(), "chunk {index} byte {at} went unnoticed");
            assert!(
                !stored.dir().exists(),
                "chunk {index} byte {at}: folder kept"
            );
        }
    }
}

#[test]
fn a_flipped_header_byte_rebuilds_for_every_compression() {
    for compression in MODES {
        for at in 0..48 {
            let stored = Stored::new("header", compression, vec![batch(3), batch(4)]);
            let folder = stored.open();
            let path = folder.tile_path(0);
            let mut bytes = fs::read(&path).expect("chunk");
            bytes[at] ^= 0x01;
            fs::write(&path, bytes).expect("write back");
            assert!(
                LeafCache::load(&folder, u64::MAX)
                    .expect("no error")
                    .is_none()
            );
            assert!(!stored.dir().exists());
        }
    }
}

#[test]
fn a_missing_or_stray_chunk_rebuilds_silently() {
    let missing = Stored::new("missing", CacheCompression::Lz4, vec![batch(1), batch(2)]);
    let folder = missing.open();
    fs::remove_file(folder.tile_path(1)).expect("remove");
    assert!(
        LeafCache::load(&folder, u64::MAX)
            .expect("no error")
            .is_none()
    );
    assert!(!missing.dir().exists());

    let stray = Stored::new("stray", CacheCompression::Lz4, vec![batch(1), batch(2)]);
    let folder = stray.open();
    fs::copy(folder.tile_path(0), folder.tile_path(2)).expect("copy");
    assert!(
        LeafCache::load(&folder, u64::MAX)
            .expect("no error")
            .is_none()
    );
    assert!(!stray.dir().exists());
}

#[test]
fn another_beam_format_version_rebuilds_silently() {
    let stored = Stored::new("version", CacheCompression::Zstd, vec![batch(1), batch(2)]);
    let folder = stored.open();
    folder
        .write_beam_chunk(0, 2, BEAM_CACHE_FORMAT_VERSION + 1, &[0, 0, 0, 0])
        .expect("write");
    assert!(
        LeafCache::load(&folder, u64::MAX)
            .expect("no error")
            .is_none()
    );
    assert!(!stored.dir().exists());
}

#[test]
fn a_kept_index_that_does_not_derive_from_its_leaves_is_refused() {
    let mut odd = batch(1);
    let other = batch(2);
    odd.kept[0] = other.kept[1].clone();
    let root = TempDir::new("odd");
    let rig = BeamRig::cube(glass());
    let (key, meta) = beam_meta(&rig, &BeamSettings::default(), CacheCompression::Zstd);
    let folder = KeyFolder::create(root.path(), &key, meta).expect("create");
    let cache = LeafCache::persist_test_cache(vec![odd]);
    assert!(cache.store(&folder).is_err());
}

/// The key of `rig` with `beam` and `tile`.
fn key_of(rig: &BeamRig, beam: &BeamSettings, tile: u32) -> CacheKey {
    let scene = rig.scene(SIZE, 8);
    let settings = BuildSettings::default();
    beam_cache_key(
        &CacheKeyInputs::from_scene(&scene, Mat4::IDENTITY, &settings),
        beam,
        tile,
    )
}

#[test]
fn the_key_ignores_the_lighting_and_the_path_cache_build_settings() {
    let rig = BeamRig::cube(glass());
    let beam = BeamSettings::default();
    let key = key_of(&rig, &beam, TILE);

    let mut scene = rig.scene(SIZE, 8);
    scene.environment = LightingPreset::LightTent.studio(3.5, 0.9, 0.1);
    let settings = BuildSettings {
        k0: 3,
        k_max: 11,
        ..BuildSettings::default()
    };
    let relit = beam_cache_key(
        &CacheKeyInputs::from_scene(&scene, Mat4::IDENTITY, &settings),
        &beam,
        TILE,
    );
    assert_eq!(key, relit);

    // Settings that do not change a tile's leaves are not in the key. A memory cap below one
    // tile's estimate makes that tile residual-only and unretained: the folder then lacks the
    // tile, but every leaf it holds is the same, so the cap stays out of the key. The lighting
    // tolerance is in the key: the export's re-render does not refine, so a finer tolerance must
    // trace afresh (`path_cache::tests_key` pins that).
    for same in [
        BeamSettings {
            residual_min_spp: 7,
            ..beam
        },
        BeamSettings {
            batch_tiles: 3,
            ..beam
        },
        BeamSettings {
            max_fragment_levels: 2,
            ..beam
        },
        BeamSettings {
            leaf_cache_bytes: 1,
            ..beam
        },
        BeamSettings {
            memory_cap_bytes: 1,
            ..beam
        },
        BeamSettings {
            keep_leaves: true,
            ..beam
        },
    ] {
        assert_eq!(key, key_of(&rig, &same, TILE));
    }
}

#[test]
fn the_key_changes_with_the_pose_the_material_and_the_design() {
    let base = BeamRig::cube(glass());
    let beam = BeamSettings::default();
    let key = key_of(&base, &beam, TILE);

    let pose = |yaw: f32, distance: f32, fov: f32| {
        BeamRig::new(
            base.planes.clone(),
            glass(),
            Camera::new(yaw, std::f32::consts::FRAC_PI_2, distance, fov),
        )
    };
    assert_eq!(key, key_of(&pose(0.0, 8.0, 24.0), &beam, TILE));
    assert_ne!(key, key_of(&pose(0.02, 8.0, 24.0), &beam, TILE));
    assert_ne!(key, key_of(&pose(0.0, 7.0, 24.0), &beam, TILE));
    assert_ne!(key, key_of(&pose(0.0, 8.0, 30.0), &beam, TILE));

    assert_ne!(key, key_of(&BeamRig::cube(sapphire()), &beam, TILE));
    assert_ne!(key, key_of(&BeamRig::brilliant(glass()), &beam, TILE));

    let scene = base.scene(SIZE, 8);
    let settings = BuildSettings::default();
    let rotated = beam_cache_key(
        &CacheKeyInputs::from_scene(&scene, Mat4::from_rotation_y(0.01), &settings),
        &beam,
        TILE,
    );
    assert_ne!(key, rotated);
    let bigger = base.scene(SIZE + 1, 8);
    let resized = beam_cache_key(
        &CacheKeyInputs::from_scene(&bigger, Mat4::IDENTITY, &settings),
        &beam,
        TILE,
    );
    assert_ne!(key, resized);
}

#[test]
fn the_key_changes_with_each_beam_setting_that_changes_leaves() {
    let rig = BeamRig::cube(glass());
    let beam = BeamSettings::default();
    let key = key_of(&rig, &beam, TILE);
    let changed = [
        BeamSettings {
            dir_tolerance_rad: 2.0e-3,
            ..beam
        },
        BeamSettings {
            weight_tolerance: 2.0e-3,
            ..beam
        },
        BeamSettings {
            chord_tolerance_px: 2.0e-3,
            ..beam
        },
        BeamSettings {
            cutoff: 2.0e-3,
            ..beam
        },
        BeamSettings {
            spectral_nodes: 24,
            ..beam
        },
        BeamSettings {
            absorption_subnodes: 2,
            ..beam
        },
        BeamSettings {
            region_mode: RegionMode::PerNode,
            ..beam
        },
        BeamSettings {
            max_split_depth: 12,
            ..beam
        },
    ];
    let mut keys = vec![key, key_of(&rig, &beam, TILE * 2)];
    keys.extend(changed.iter().map(|setting| key_of(&rig, setting, TILE)));
    let mut sorted = keys.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), keys.len(), "two settings share a key");
}

#[test]
fn the_key_holds_the_beam_format_version_and_is_not_a_path_cache_key() {
    let rig = BeamRig::cube(glass());
    let scene = rig.scene(SIZE, 8);
    let settings = BuildSettings::default();
    let inputs = CacheKeyInputs::from_scene(&scene, Mat4::IDENTITY, &settings);
    let bytes = beam_canonical_bytes(&inputs, &BeamSettings::default(), TILE);
    assert_eq!(&bytes[0..7], b"PCBKEY1");
    assert_eq!(bytes[7..11], BEAM_CACHE_FORMAT_VERSION.to_le_bytes());
    assert_ne!(
        beam_cache_key(&inputs, &BeamSettings::default(), TILE),
        CacheKey::new(&inputs)
    );
}

#[test]
fn the_lru_evicts_the_oldest_key_across_path_cache_and_beam_entries() {
    let root = TempDir::new("lru");
    let rig = BeamRig::cube(glass());
    let beam = BeamSettings::default();

    // A path-cache entry, last used first.
    let scene = rig.scene(SIZE, 8);
    let settings = BuildSettings::default();
    let inputs = CacheKeyInputs::from_scene(&scene, Mat4::IDENTITY, &settings);
    let path_key = CacheKey::new(&inputs);
    let meta = CacheMeta::describe(&inputs, &path_key, CacheCompression::Zstd, OLDEST_USED);
    let mut path_entry = KeyFolder::create(root.path(), &path_key, meta).expect("create");
    path_entry.finish(PATH_ENTRY_BYTES).expect("finish");
    path_entry.mark_used_at(OLDEST_USED).expect("stamp");

    // A beam entry used a little later.
    let cache = LeafCache::persist_test_cache(vec![batch(1), batch(2)]);
    let (older_key, meta) = beam_meta(&rig, &beam, CacheCompression::Zstd);
    let older = KeyFolder::create(root.path(), &older_key, meta).expect("create");
    cache.store(&older).expect("store");
    KeyFolder::open(root.path(), &older_key)
        .expect("finished")
        .mark_used_at(MIDDLE_USED)
        .expect("stamp");

    // The beam entry just written.
    let newer_beam = BeamSettings {
        spectral_nodes: 24,
        ..beam
    };
    let (newer_key, meta) = beam_meta(&rig, &newer_beam, CacheCompression::Zstd);
    let newer = KeyFolder::create(root.path(), &newer_key, meta).expect("create");
    cache.store(&newer).expect("store");

    let size_of_key = |key: &CacheKey| {
        lru::scan(root.path())
            .expect("scan")
            .into_iter()
            .find(|entry| entry.key == key.to_string())
            .map(|entry| entry.size_bytes)
            .expect("an entry")
    };
    let (older_bytes, newer_bytes) = (size_of_key(&older_key), size_of_key(&newer_key));

    // Room for both beam entries: only the oldest, the path-cache entry, goes.
    let limit = older_bytes + newer_bytes + PATH_ENTRY_BYTES / 2;
    let evicted = cache.store_with_limit(&newer, limit).expect("store");
    assert_eq!(evicted, vec![path_key.to_string()]);
    assert!(!KeyFolder::folder_path(root.path(), &path_key).exists());
    assert!(KeyFolder::folder_path(root.path(), &older_key).is_dir());

    // Room for one: the older beam entry goes, the one just written stays.
    let evicted = cache
        .store_with_limit(&newer, newer_bytes + older_bytes / 2)
        .expect("store");
    assert_eq!(evicted, vec![older_key.to_string()]);
    assert!(!KeyFolder::folder_path(root.path(), &older_key).exists());
    assert!(KeyFolder::folder_path(root.path(), &newer_key).is_dir());
}
