use super::{shard_key, write_atomic};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

fn scratch_dir() -> PathBuf {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let path = std::env::temp_dir().join(format!(
        "pleochroma-cache-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path).expect("test directory");
    path
}

#[test]
fn shard_identity_includes_shader_entry_and_exact_overrides() {
    let base = shard_key("shader", "main", &[("B", 2.0), ("A", 1.0)]);
    assert_eq!(base, shard_key("shader", "main", &[("A", 1.0), ("B", 2.0)]));
    assert_ne!(
        base,
        shard_key("changed", "main", &[("B", 2.0), ("A", 1.0)])
    );
    assert_ne!(
        base,
        shard_key("shader", "other", &[("B", 2.0), ("A", 1.0)])
    );
    assert_ne!(base, shard_key("shader", "main", &[("B", 3.0), ("A", 1.0)]));
    assert_ne!(shard_key("ab", "c", &[]), shard_key("a", "bc", &[]));
    assert_ne!(
        shard_key("s", "m", &[("A", 0.0)]),
        shard_key("s", "m", &[("A", -0.0)])
    );
}

#[test]
fn independent_variants_and_simultaneous_same_variant_writes_keep_complete_blobs() {
    let dir = scratch_dir();
    let path = dir.join("path-material.bin");
    let ray = dir.join("ray.bin");
    write_atomic(&path, b"original").expect("first write");
    write_atomic(&ray, b"ray pipeline").expect("different variant");
    let barrier = std::sync::Barrier::new(8);
    std::thread::scope(|scope| {
        for value in 0u8..8 {
            let (path, barrier) = (&path, &barrier);
            scope.spawn(move || {
                barrier.wait();
                write_atomic(path, &vec![value; 4096]).expect("concurrent atomic replacement");
            });
        }
    });
    let bytes = std::fs::read(&path).expect("complete cache");
    assert_eq!(bytes.len(), 4096);
    assert!(bytes.iter().all(|value| *value == bytes[0]));
    assert_eq!(std::fs::read(&ray).unwrap(), b"ray pipeline");
    assert_eq!(
        std::fs::read_dir(&dir).unwrap().count(),
        2,
        "no temporary files retained"
    );
    std::fs::remove_file(path).unwrap();
    std::fs::remove_file(ray).unwrap();
    std::fs::remove_dir(dir).unwrap();
}

#[test]
fn failed_replacement_cleans_up_temporary_file() {
    let dir = scratch_dir();
    let destination = dir.join("directory-not-file.bin");
    std::fs::create_dir(&destination).unwrap();
    assert!(write_atomic(&destination, b"cache").is_err());
    assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
    std::fs::remove_dir(destination).unwrap();
    std::fs::remove_dir(dir).unwrap();
}
