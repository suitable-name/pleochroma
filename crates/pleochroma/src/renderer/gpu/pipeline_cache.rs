//! Persist one compiled pipeline per shard.
//!
//! Independent renderer snapshots never
//! replace each other's material variants. Identical shards may race: either complete
//! blob contains the same pipeline, so last-writer-wins is harmless and needs no lock.
//! wgpu still validates shader modules, pipelines and driver cache compatibility.
use std::{path::PathBuf, sync::OnceLock};

static CACHE_DIR: OnceLock<PathBuf> = OnceLock::new();

/// Enables best-effort native pipeline persistence. Call before acquiring devices.
/// Returns false if a directory was already configured (the first one wins).
pub fn set_pipeline_cache_dir(dir: PathBuf) -> bool {
    CACHE_DIR.set(dir).is_ok()
}

/// Stable shader and override identity, independent of labels or override ordering.
///
/// The version includes the pinned wgpu cache format and our
/// compilation policy (automatic layouts, zeroed workgroup memory).
pub(crate) fn shard_key(shader: &str, entry: &str, constants: &[(&str, f64)]) -> String {
    let mut hashes = [0xcbf2_9ce4_8422_2325u64, 0x8422_2325_cbf2_9ce4u64];
    let mut feed = |bytes: &[u8]| {
        for byte in (bytes.len() as u64).to_le_bytes().iter().chain(bytes) {
            for hash in &mut hashes {
                *hash = (*hash ^ u64::from(*byte)).wrapping_mul(0x0000_0100_0000_01b3);
            }
        }
    };
    feed(b"pleochroma-wgpu30.0.1-pipeline-v2-auto-layout-zero-workgroup");
    feed(shader.as_bytes());
    feed(entry.as_bytes());
    let mut sorted = constants.to_vec();
    sorted.sort_unstable_by(|left, right| left.0.cmp(right.0));
    for (name, value) in sorted {
        feed(name.as_bytes());
        feed(&value.to_bits().to_le_bytes());
    }
    format!("{:016x}{:016x}", hashes[0], hashes[1])
}

#[cfg(not(target_arch = "wasm32"))]
fn cache_file(info: &wgpu::AdapterInfo, shard: &str) -> Option<PathBuf> {
    let dir = CACHE_DIR.get()?;
    let adapter = wgpu::util::pipeline_cache_key(info)?;
    Some(
        dir.join("wgpu30-v2")
            .join(adapter)
            .join(format!("{shard}.bin")),
    )
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn load_blob(info: &wgpu::AdapterInfo, shard: &str) -> Option<Vec<u8>> {
    use std::io::Read;
    // Bound allocations even if an interrupted external writer changes the file
    // between metadata and read. Cache misses always fall back to compilation.
    const MAX_BLOB_BYTES: u64 = 64 * 1024 * 1024;
    let path = cache_file(info, shard)?;
    let read = || -> std::io::Result<Vec<u8>> {
        let file = std::fs::File::open(&path)?;
        let mut bytes = Vec::new();
        file.take(MAX_BLOB_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.is_empty() || bytes.len() as u64 > MAX_BLOB_BYTES {
            return Err(std::io::Error::other("empty or oversized pipeline cache"));
        }
        Ok(bytes)
    };
    match read() {
        Ok(bytes) => {
            tracing::debug!(shard, bytes = bytes.len(), path = %path.display(), "pipeline cache loaded");
            Some(bytes)
        }
        Err(error) => {
            tracing::debug!(%error, shard, path = %path.display(), "pipeline cache miss");
            None
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) const fn load_blob(_info: &wgpu::AdapterInfo, _shard: &str) -> Option<Vec<u8>> {
    None
}

/// Unique create-new temporary files and an atomic replacement retain the previous
/// complete shard on failure. There is no cross-process lock or unbounded retry.
#[cfg(not(target_arch = "wasm32"))]
fn write_atomic(path: &std::path::Path, data: &[u8]) -> std::io::Result<()> {
    use std::{
        io::Write,
        sync::atomic::{AtomicU64, Ordering},
    };
    static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::other("cache needs parent"))?;
    std::fs::create_dir_all(parent)?;
    let mut temporary = None;
    // PID, timestamp and sequence prevent stale files or parallel writers from
    // claiming the same temporary file. Retry collisions only a bounded number.
    for _ in 0..8 {
        let sequence = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let candidate =
            path.with_extension(format!("{}.{nanos}.{sequence}.tmp", std::process::id()));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(file) => {
                temporary = Some((candidate, file));
                break;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    let (temporary_path, mut file) =
        temporary.ok_or_else(|| std::io::Error::other("cache temporary names busy"))?;
    let result = (|| {
        file.write_all(data)?;
        file.sync_all()?;
        drop(file);
        // Same-directory rename atomically replaces on Unix and Windows. Readers
        // therefore see a complete old or new blob, never a partial write.
        std::fs::rename(&temporary_path, path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary_path);
    }
    result
}

#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn persist(cache: &wgpu::PipelineCache, info: &wgpu::AdapterInfo, shard: &str) {
    let Some(path) = cache_file(info, shard) else {
        return;
    };
    let Some(data) = cache.get_data() else {
        return;
    };
    match write_atomic(&path, &data) {
        Ok(()) => {
            tracing::debug!(shard, bytes = data.len(), path = %path.display(), "pipeline cache saved");
        }
        Err(error) => {
            tracing::debug!(%error, shard, path = %path.display(), "pipeline cache save skipped");
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) const fn persist(_cache: &wgpu::PipelineCache, _info: &wgpu::AdapterInfo, _shard: &str) {
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "pipeline_cache_tests.rs"]
mod tests;
