//! Optional render accelerations, shared by jobs, the CLI, the app and remote rendering.
//!
//! The default is everything off, so the render matches one without them.

use super::{accel_beam::BeamSettings, adaptive::AdaptiveSettings};

/// Optional render accelerations; the default is everything off.
#[derive(Clone, Debug, Default, PartialEq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(default)
)]
pub struct RenderAccel {
    /// Adaptive sampling; `None` renders every pixel to the full sample count.
    pub adaptive: Option<AdaptiveSettings>,
    /// Path cache; `None` traces every stone sample in full.
    pub path_cache: Option<PathCacheSettings>,
    /// Beam tracing of faceted stones; `None` renders as before (beam guide §14.2).
    pub beam: Option<BeamSettings>,
}

impl RenderAccel {
    /// `true` when no acceleration is on, so the render is exactly the render without them.
    #[must_use]
    pub const fn is_off(&self) -> bool {
        self.adaptive.is_none() && self.path_cache.is_none() && self.beam.is_none()
    }
}

/// Path-cache options (guide section 10.1).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(default)
)]
pub struct PathCacheSettings {
    /// Write the cache to disk after rendering, for later re-renders of the same pose.
    pub persist: bool,
    /// Compression of persisted tiles.
    pub compression: CacheCompression,
}

/// Compression of persisted path-cache tiles.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "snake_case")
)]
pub enum CacheCompression {
    /// zstd at level 2 (the default).
    #[default]
    Zstd,
    /// lz4: faster, larger.
    Lz4,
    /// Stored as is.
    None,
}
