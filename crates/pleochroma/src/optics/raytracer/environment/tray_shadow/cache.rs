//! A small process-local most-recently-used cache of completed irradiance maps.
use super::{
    TrayShadowInput, TrayShadowKey, TrayShadowMap, TrayShadowOff, TrayShadowQuality,
    build_tray_shadow_with_threads, tray_shadow_key,
};
use std::{
    num::NonZeroUsize,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
    },
    thread,
};

/// Completed maps, most recently used first; builds never hold this lock.
static CACHE: Mutex<Vec<(TrayShadowKey, Arc<TrayShadowMap>)>> = Mutex::new(Vec::new());

/// Test-only: places a prebuilt map in the cache under its own key, so a test can observe a
/// key match without running a precompute.
#[cfg(test)]
pub(super) fn seed_for_test(map: &Arc<TrayShadowMap>) {
    let mut cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    cache.insert(0, (map.key(), Arc::clone(map)));
    cache.truncate(3);
}

/// Reuses a completed map or computes it outside the cache lock, on every available core.
///
/// # Errors
/// Returns the same geometry, lighting and cancellation declines as the builder.
pub fn shared_tray_shadow(
    input: &TrayShadowInput<'_>,
    quality: TrayShadowQuality,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
) -> Result<Arc<TrayShadowMap>, TrayShadowOff> {
    let threads = thread::available_parallelism().unwrap_or(NonZeroUsize::MIN);
    shared_tray_shadow_with_threads(input, quality, cancel, progress, threads)
}

/// [`shared_tray_shadow`] with a cap on the build's worker threads. The cap never changes the
/// map's bits, so it is not part of the cache key: a hit built with another cap is reused.
///
/// # Errors
/// Returns the same geometry, lighting and cancellation declines as the builder.
pub fn shared_tray_shadow_with_threads(
    input: &TrayShadowInput<'_>,
    quality: TrayShadowQuality,
    cancel: &AtomicBool,
    progress: &mut dyn FnMut(f32),
    threads: NonZeroUsize,
) -> Result<Arc<TrayShadowMap>, TrayShadowOff> {
    if cancel.load(Ordering::Relaxed) {
        return Err(TrayShadowOff::Cancelled);
    }
    let key = tray_shadow_key(input, quality);
    {
        let mut cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(index) = cache.iter().position(|(candidate, _)| *candidate == key) {
            let entry = cache.remove(index);
            let map = Arc::clone(&entry.1);
            cache.insert(0, entry);
            drop(cache);
            progress(1.0);
            if cancel.load(Ordering::Relaxed) {
                return Err(TrayShadowOff::Cancelled);
            }
            return Ok(map);
        }
    }
    let built = Arc::new(build_tray_shadow_with_threads(
        input, quality, cancel, progress, threads,
    )?);
    let mut cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    // A finished map is valid for any later request with this key, so it is cached even when the
    // cancel token was set in the last moments of the build.
    let map = if let Some(index) = cache.iter().position(|(candidate, _)| *candidate == key) {
        let existing = cache.remove(index);
        let map = Arc::clone(&existing.1);
        cache.insert(0, existing);
        map
    } else {
        cache.insert(0, (key, Arc::clone(&built)));
        cache.truncate(3);
        built
    };
    drop(cache);
    if cancel.load(Ordering::Relaxed) {
        return Err(TrayShadowOff::Cancelled);
    }
    Ok(map)
}
