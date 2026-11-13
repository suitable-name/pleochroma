//! Tests of the disk limit, the eviction order and clearing the cache folder.

use std::{fs, path::Path};

use super::{
    key::CacheKey,
    lru::{
        CacheEntry, DEFAULT_DISK_LIMIT_BYTES, clear, enforce_limit, plan_eviction, scan, total_size,
    },
    persist::KeyFolder,
    tests_persist::{TempDir, sample_meta},
};
use crate::renderer::accel::CacheCompression;

/// A key whose hex starts with the digit pattern of `n` (so the sort order is easy to read).
fn key(n: u8) -> CacheKey {
    CacheKey::from_bytes([n; 16])
}

/// Creates a finished cache of `size` bytes last used at `used` Unix seconds.
fn add(root: &Path, n: u8, size: u64, used: u64) -> CacheKey {
    let key = key(n);
    let (_, mut meta) = sample_meta(CacheCompression::Zstd);
    meta.key = key.to_string();
    let mut folder = KeyFolder::create(root, &key, meta).expect("creates");
    folder.finish(size).expect("finishes");
    folder.mark_used_at(used).expect("marks");
    key
}

fn entry(n: u8, size: u64, used: u64) -> CacheEntry {
    CacheEntry {
        key: key(n).to_string(),
        size_bytes: size,
        last_used_unix: used,
    }
}

fn names(keys: &[CacheKey]) -> Vec<String> {
    keys.iter().map(ToString::to_string).collect()
}

#[test]
fn the_default_limit_is_fifty_gigabytes() {
    assert_eq!(DEFAULT_DISK_LIMIT_BYTES, 50_000_000_000);
}

#[test]
fn the_plan_evicts_the_oldest_first_until_the_total_fits() {
    let entries = [entry(1, 100, 30), entry(2, 100, 10), entry(3, 100, 20)];
    assert_eq!(
        plan_eviction(&entries, 300, None),
        Vec::<String>::new(),
        "exactly at the limit is fine"
    );
    assert_eq!(plan_eviction(&entries, 250, None), names(&[key(2)]));
    assert_eq!(plan_eviction(&entries, 150, None), names(&[key(2), key(3)]));
    assert_eq!(
        plan_eviction(&entries, 0, None),
        names(&[key(2), key(3), key(1)])
    );
}

#[test]
fn equal_last_used_times_are_broken_by_the_key_hex() {
    let entries = [entry(7, 100, 5), entry(3, 100, 5), entry(5, 100, 5)];
    assert_eq!(
        plan_eviction(&entries, 100, None),
        names(&[key(3), key(5)]),
        "the smaller hex goes first"
    );
    let reversed = [entries[2].clone(), entries[1].clone(), entries[0].clone()];
    assert_eq!(
        plan_eviction(&reversed, 100, None),
        plan_eviction(&entries, 100, None),
        "the input order does not matter"
    );
}

#[test]
fn the_plan_skips_a_big_entry_only_when_it_is_protected() {
    let entries = [entry(1, 1000, 1), entry(2, 10, 2)];
    assert_eq!(plan_eviction(&entries, 500, None), names(&[key(1)]));
    let protected = key(1).to_string();
    assert_eq!(
        plan_eviction(&entries, 500, Some(&protected)),
        names(&[key(2)]),
        "the protected cache stays even when it alone is over the limit"
    );
}

#[test]
fn a_root_that_does_not_exist_is_empty() {
    let root = TempDir::new("lru_missing");
    let nowhere = root.path().join("nowhere");
    assert_eq!(scan(&nowhere).expect("scans"), Vec::new());
    assert_eq!(total_size(&nowhere).expect("sums"), 0);
    assert_eq!(
        enforce_limit(&nowhere, 0, None).expect("enforces"),
        Vec::<String>::new()
    );
    assert_eq!(clear(&nowhere).expect("clears"), 0);
}

#[test]
fn scan_reads_sizes_and_times_from_meta_in_key_order() {
    let root = TempDir::new("lru_scan");
    add(root.path(), 5, 500, 50);
    add(root.path(), 2, 200, 20);
    add(root.path(), 9, 900, 90);
    let entries = scan(root.path()).expect("scans");
    assert_eq!(
        entries,
        vec![entry(2, 200, 20), entry(5, 500, 50), entry(9, 900, 90)]
    );
    assert_eq!(total_size(root.path()).expect("sums"), 1600);
}

#[test]
fn scan_ignores_everything_that_is_not_a_cache_folder() {
    let root = TempDir::new("lru_ignore");
    add(root.path(), 1, 100, 10);
    fs::create_dir_all(root.path().join("my_photos")).expect("creates");
    fs::write(root.path().join("my_photos").join("a.png"), "x").expect("writes");
    fs::write(root.path().join("notes.txt"), "x").expect("writes");
    // A file named like a key is not a cache folder either.
    fs::write(root.path().join(key(4).to_string()), "x").expect("writes");
    // An upper case name is not a key.
    fs::create_dir_all(root.path().join(key(0xab).to_string().to_uppercase())).expect("creates");

    let entries = scan(root.path()).expect("scans");
    assert_eq!(entries, vec![entry(1, 100, 10)]);

    enforce_limit(root.path(), 0, None).expect("enforces");
    assert!(root.path().join("my_photos").join("a.png").is_file());
    assert!(root.path().join("notes.txt").is_file());
}

#[test]
fn a_folder_without_meta_counts_by_its_files_and_goes_first() {
    let root = TempDir::new("lru_nometa");
    add(root.path(), 1, 100, 10);
    let orphan = root.path().join(key(2).to_string());
    fs::create_dir_all(orphan.join("tiles")).expect("creates");
    fs::write(orphan.join("tiles").join("00000.zst"), vec![0_u8; 40]).expect("writes");
    fs::write(orphan.join("stray.bin"), vec![0_u8; 2]).expect("writes");

    let entries = scan(root.path()).expect("scans");
    assert_eq!(entries, vec![entry(1, 100, 10), entry(2, 42, 0)]);
    assert_eq!(
        plan_eviction(&entries, 100, None),
        names(&[key(2)]),
        "no last-used time sorts before every real one"
    );
}

#[test]
fn enforce_limit_deletes_in_last_used_order_and_respects_the_limit() {
    let root = TempDir::new("lru_enforce");
    add(root.path(), 1, 400, 300);
    add(root.path(), 2, 400, 100);
    add(root.path(), 3, 400, 200);
    add(root.path(), 4, 400, 400);

    let evicted = enforce_limit(root.path(), 900, None).expect("enforces");
    assert_eq!(evicted, names(&[key(2), key(3)]));
    assert!(total_size(root.path()).expect("sums") <= 900);
    let left: Vec<String> = scan(root.path())
        .expect("scans")
        .into_iter()
        .map(|e| e.key)
        .collect();
    assert_eq!(left, names(&[key(1), key(4)]));

    assert_eq!(
        enforce_limit(root.path(), 900, None).expect("enforces"),
        Vec::<String>::new(),
        "a second pass has nothing to do"
    );
}

#[test]
fn enforce_limit_never_deletes_the_cache_just_written() {
    let root = TempDir::new("lru_protect");
    add(root.path(), 1, 400, 900);
    let fresh = add(root.path(), 2, 700, 1);
    let evicted = enforce_limit(root.path(), 500, Some(&fresh)).expect("enforces");
    assert_eq!(evicted, names(&[key(1)]));
    assert!(KeyFolder::open(root.path(), &fresh).is_ok());
}

#[test]
fn using_a_cache_protects_it_from_eviction() {
    let root = TempDir::new("lru_touch");
    add(root.path(), 1, 100, 10);
    add(root.path(), 2, 100, 20);
    let mut oldest = KeyFolder::open(root.path(), &key(1)).expect("opens");
    oldest.mark_used_at(30).expect("marks");
    let evicted = enforce_limit(root.path(), 100, None).expect("enforces");
    assert_eq!(evicted, names(&[key(2)]), "cache 1 was used last");
}

#[test]
fn clear_deletes_the_contents_and_keeps_the_root() {
    let root = TempDir::new("lru_clear");
    add(root.path(), 1, 100, 10);
    add(root.path(), 2, 100, 20);
    fs::write(root.path().join("loose.txt"), "x").expect("writes");
    assert_eq!(clear(root.path()).expect("clears"), 3);
    assert!(root.path().is_dir());
    assert_eq!(fs::read_dir(root.path()).expect("lists").count(), 0);
    assert_eq!(clear(root.path()).expect("clears"), 0, "already clear");
}
