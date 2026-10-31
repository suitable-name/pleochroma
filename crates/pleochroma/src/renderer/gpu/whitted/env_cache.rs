//! One-entry cache of the uploaded HDR environment storage buffer.
//!
//! A camera-only change gives the Ray-Traced scene a new generation, which used to repack
//! and re-upload the whole map (16 bytes per texel, 128 MiB at 4096x2048). The prepared
//! environment owns its map as an `Arc`, so that `Arc` is the cache identity: the cache
//! keeps a clone, hence the address cannot be reused by another map while the entry lives.
//!
//! The buffer is created with its contents and never written again, so scenes that still
//! bind a replaced entry keep it alive through wgpu's own reference count.

use std::sync::{Arc, Mutex, PoisonError};

use crate::renderer::env_map::EnvironmentMap;

/// The latest uploaded HDR map and what the scene needs to bind it again.
pub(super) struct CachedEnvironment {
    pub(super) owner: Arc<EnvironmentMap>,
    pub(super) buffer: wgpu::Buffer,
    /// `[width, height]` in texels; the raster size is per scene and not cached.
    pub(super) texel_dims: [u32; 2],
    /// Size of the uploaded buffer; never above the device storage limit.
    pub(super) bytes: u64,
}

/// Whether the cached map is the very map being prepared (pointer identity).
pub(super) fn cache_hit(owner: &Arc<EnvironmentMap>, cached: Option<&Arc<EnvironmentMap>>) -> bool {
    cached.is_some_and(|cached| Arc::ptr_eq(owner, cached))
}

/// Owned by one [`super::WhittedGpuRenderer`], so a lost device (which replaces the
/// renderer) takes its cache along.
#[derive(Default)]
pub(super) struct EnvironmentCache {
    entry: Mutex<Option<CachedEnvironment>>,
}

impl EnvironmentCache {
    /// The buffer and texel dimensions cached for `owner`, if it is the cached map.
    pub(super) fn lookup(&self, owner: &Arc<EnvironmentMap>) -> Option<(wgpu::Buffer, [u32; 2])> {
        let guard = self.entry.lock().unwrap_or_else(PoisonError::into_inner);
        guard
            .as_ref()
            .filter(|entry| cache_hit(owner, Some(&entry.owner)))
            .map(|entry| (entry.buffer.clone(), entry.texel_dims))
    }

    /// Replaces the entry with a fully uploaded buffer; the previous `Arc` and buffer
    /// handle are dropped here.
    pub(super) fn store(&self, entry: CachedEnvironment) {
        *self.entry.lock().unwrap_or_else(PoisonError::into_inner) = Some(entry);
    }

    pub(super) fn bytes(&self) -> u64 {
        self.entry
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .as_ref()
            .map_or(0, |entry| entry.bytes)
    }
}

#[cfg(test)]
thread_local! {
    static UPLOADS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

/// Counts one environment repack-and-upload on this thread (tests only; the thread-local
/// keeps parallel GPU tests from seeing each other's uploads).
#[cfg(test)]
pub(super) fn note_upload() {
    UPLOADS.with(|count| count.set(count.get() + 1));
}

#[cfg(test)]
pub(super) fn uploads_on_this_thread() -> usize {
    UPLOADS.with(std::cell::Cell::get)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hit_needs_the_same_allocation_not_an_equal_map() {
        let map = Arc::new(EnvironmentMap::uniform(2, 2, [0.5, 0.5, 0.5]));
        let twin = Arc::new(EnvironmentMap::uniform(2, 2, [0.5, 0.5, 0.5]));
        assert!(cache_hit(&map, Some(&Arc::clone(&map))));
        assert!(
            !cache_hit(&map, Some(&twin)),
            "equal content is not identity"
        );
        assert!(!cache_hit(&map, None));
    }
}
