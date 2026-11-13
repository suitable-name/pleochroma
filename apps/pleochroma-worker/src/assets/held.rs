//! [`HeldAsset`]: the HDR map a coordinator holds for one job, so it can
//! answer a joined worker's `NEED_ASSET` without asking the viewer again.
//!
//! The bytes are pinned in memory for the job's lifetime, so an LRU eviction in the
//! on-disk [`AssetCache`] (another job's larger map, a write failure) cannot make the
//! coordinator lose the map mid-job and fail a joined worker's `NEED_ASSET`.

use super::AssetCache;
use pleochroma::renderer::env_map::EnvironmentMap;
use pleochroma_net::{messages::ContentHash, scene::HdrEnvironment};
use std::sync::{Arc, Mutex, PoisonError};

/// One job's HDR map as the coordinator holds it -- see the module doc comment.
pub struct HeldAsset {
    hdr: HdrEnvironment,
    cache: Arc<AssetCache>,
    bytes: Mutex<Option<Arc<Vec<u8>>>>,
    /// The decoded map, when the coordinator's own lane renders (pinned for the job).
    map: Option<Arc<EnvironmentMap>>,
}

impl HeldAsset {
    /// The map `hdr` names, backed by `cache`; `bytes` when they are already in memory
    /// (and the cache could not keep them), `map` when the own lane needs it decoded.
    #[must_use]
    pub const fn new(
        hdr: HdrEnvironment,
        cache: Arc<AssetCache>,
        bytes: Option<Arc<Vec<u8>>>,
        map: Option<Arc<EnvironmentMap>>,
    ) -> Self {
        Self {
            hdr,
            cache,
            bytes: Mutex::new(bytes),
            map,
        }
    }

    /// What the scene says about the map.
    #[must_use]
    pub const fn hdr(&self) -> &HdrEnvironment {
        &self.hdr
    }

    /// The map's SHA-256.
    #[must_use]
    pub const fn content_hash(&self) -> ContentHash {
        self.hdr.content_hash
    }

    /// The decoded map, if the coordinator decoded it (own lane).
    #[must_use]
    pub const fn map(&self) -> Option<&Arc<EnvironmentMap>> {
        self.map.as_ref()
    }

    /// The verified bytes, pinned in memory since the job resolved them (see the
    /// module doc comment) -- always `Some` in practice. The lazy re-read from the
    /// cache below is a defensive fallback only, in case a future caller ever
    /// constructs a [`Self`] without them up front.
    #[must_use]
    pub fn bytes(&self) -> Option<Arc<Vec<u8>>> {
        let mut held = self.bytes.lock().unwrap_or_else(PoisonError::into_inner);
        if held.is_none() {
            *held = self.cache.get(&self.hdr.content_hash).map(Arc::new);
        }
        let bytes = held.clone();
        drop(held);
        bytes
    }
}
