//! Persist one compiled pipeline per shard.
//!
//! Independent renderer snapshots never
//! replace each other's material variants. Identical shards may race: either complete
//! blob contains the same pipeline, so last-writer-wins is harmless and needs no lock.
//! wgpu still validates shader modules, pipelines and driver cache compatibility.
use std::{
    cell::Cell,
    path::PathBuf,
    sync::{
        OnceLock,
        atomic::{AtomicU64, Ordering as AtomicOrdering},
    },
};

static CACHE_DIR: OnceLock<PathBuf> = OnceLock::new();
/// Shards the persistent cache did not have, or whose stored blob the driver outgrew: pipelines
/// this process compiled from source.
static FRESH_COMPILES: AtomicU64 = AtomicU64::new(0);

thread_local! {
    /// The part of [`FRESH_COMPILES`] counted on this thread.
    static THREAD_FRESH_COMPILES: Cell<u64> = const { Cell::new(0) };
}

/// How many pipelines this process compiled from source instead of loading them from the
/// persistent cache, since it started.
///
/// Only counts while a cache folder is set
/// ([`configured`]) and the device keeps driver data; without one every compile is fresh and
/// nothing is counted. A job that wants to know whether a stretch of work compiled anything
/// compares this number before and after.
#[must_use]
pub fn fresh_compiles() -> u64 {
    FRESH_COMPILES.load(AtomicOrdering::Relaxed)
}

/// The part of [`fresh_compiles`] counted on the calling thread.
///
/// These are pipelines this thread compiled from source. A step that runs on one thread compares the number before and after, which stays
/// exact while other threads compile at the same time (the process-wide count would include
/// theirs).
#[must_use]
pub fn fresh_compiles_on_this_thread() -> u64 {
    THREAD_FRESH_COMPILES.with(Cell::get)
}

/// Counts one fresh compile, process-wide and on this thread.
#[cfg(not(target_arch = "wasm32"))]
fn count_fresh_compile() {
    FRESH_COMPILES.fetch_add(1, AtomicOrdering::Relaxed);
    THREAD_FRESH_COMPILES.with(|count| count.set(count.get() + 1));
}

/// Whether a shard's driver data says the pipeline was compiled just now: no blob was stored
/// before (`stored` is `None`: a missing file, or a new adapter or driver folder), or the driver
/// data grew beyond it (the stored blob was rejected, or the pipeline was not in it).
#[cfg(not(target_arch = "wasm32"))]
const fn compiled_fresh(stored: Option<u64>, now: usize) -> bool {
    match stored {
        None => true,
        Some(bytes) => now as u64 > bytes,
    }
}

/// Enables best-effort native pipeline persistence. Call before acquiring devices.
/// Returns false if a directory was already configured (the first one wins).
pub fn set_pipeline_cache_dir(dir: PathBuf) -> bool {
    CACHE_DIR.set(dir).is_ok()
}

/// Whether [`set_pipeline_cache_dir`] has set a folder.
#[must_use]
pub fn configured() -> bool {
    CACHE_DIR.get().is_some()
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

/// Counts one pipeline about to compile from source because no stored blob exists for its shard.
/// Called before the compile, so a job watching [`fresh_compiles`] sees the rise while the
/// compile runs, not after it.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn note_missing_blob() {
    count_fresh_compile();
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
    // PID, timestamp and sequence keep parallel writers off the same temporary file.
    // Collisions are retried a bounded number of times.
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

/// Whether the driver's data must be written: unless it is byte for byte what `load_blob` handed
/// the driver (`loaded`), the stored shard is stale or missing.
#[cfg(not(target_arch = "wasm32"))]
fn needs_write(loaded: Option<&[u8]>, data: &[u8]) -> bool {
    loaded != Some(data)
}

/// Writes `data` atomically unless it equals `loaded`; returns whether a file was written.
/// An unchanged shard keeps its file, its modified time and its scan state untouched.
#[cfg(not(target_arch = "wasm32"))]
fn write_if_changed(
    path: &std::path::Path,
    loaded: Option<&[u8]>,
    data: &[u8],
) -> std::io::Result<bool> {
    if !needs_write(loaded, data) {
        return Ok(false);
    }
    write_atomic(path, data).map(|()| true)
}

/// Saves the driver's data for one shard. `loaded` is the blob [`load_blob`] returned for it
/// (`None` when there was none); an unchanged blob is not rewritten.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn persist(
    cache: &wgpu::PipelineCache,
    info: &wgpu::AdapterInfo,
    shard: &str,
    loaded: Option<&[u8]>,
) {
    let Some(path) = cache_file(info, shard) else {
        return;
    };
    let Some(data) = cache.get_data() else {
        return;
    };
    // The file on disk is what `load_blob` handed the driver; read its size before replacing it.
    let stored = std::fs::metadata(&path).ok().map(|meta| meta.len());
    // A shard with no stored blob was counted when it loaded (`note_missing_blob`), before the
    // compile; only a stored blob that the driver outgrew is counted here.
    if stored.is_some() && compiled_fresh(stored, data.len()) {
        count_fresh_compile();
    }
    match write_if_changed(&path, loaded, &data) {
        Ok(true) => {
            tracing::debug!(shard, bytes = data.len(), path = %path.display(), "pipeline cache saved");
        }
        Ok(false) => {
            tracing::debug!(shard, bytes = data.len(), "pipeline cache unchanged");
        }
        Err(error) => {
            // The first failure is a warning: a cache that cannot be written makes every start
            // compile the shaders again, which otherwise looks like a slow start and nothing more.
            static WARNED: std::sync::atomic::AtomicBool =
                std::sync::atomic::AtomicBool::new(false);
            if WARNED.swap(true, AtomicOrdering::Relaxed) {
                tracing::debug!(%error, shard, path = %path.display(), "pipeline cache save skipped");
            } else {
                tracing::warn!(
                    %error,
                    path = %path.display(),
                    "the GPU pipeline cache cannot be saved; every start compiles the shaders again"
                );
            }
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) const fn persist(
    _cache: &wgpu::PipelineCache,
    _info: &wgpu::AdapterInfo,
    _shard: &str,
    _loaded: Option<&[u8]>,
) {
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod fresh_tests {
    use super::{compiled_fresh, needs_write};

    #[test]
    fn a_missing_blob_is_a_fresh_compile() {
        assert!(compiled_fresh(None, 4096));
    }

    #[test]
    fn an_unchanged_blob_is_a_cache_hit() {
        assert!(!compiled_fresh(Some(4096), 4096));
        assert!(!compiled_fresh(Some(4096), 100));
    }

    #[test]
    fn a_grown_blob_is_a_fresh_compile() {
        assert!(compiled_fresh(Some(4096), 4097));
    }

    #[test]
    fn skipping_an_unchanged_write_does_not_change_the_fresh_count_decision() {
        let blob = vec![7u8; 4096];
        assert!(!needs_write(Some(blob.as_slice()), &blob));
        assert!(!compiled_fresh(Some(blob.len() as u64), blob.len()));
        assert!(needs_write(None, &blob));
        assert!(compiled_fresh(None, blob.len()));
        let grown = vec![7u8; 4097];
        assert!(needs_write(Some(blob.as_slice()), &grown));
        assert!(compiled_fresh(Some(blob.len() as u64), grown.len()));
        let same_size = vec![8u8; 4096];
        assert!(needs_write(Some(blob.as_slice()), &same_size));
        assert!(!compiled_fresh(Some(blob.len() as u64), same_size.len()));
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
#[path = "pipeline_cache_tests.rs"]
mod tests;
