//! Path cache, phase A: exit records, adaptive sub-pixel positions and the tile layout.
//!
//! For each stone pixel and each sub-pixel position, a deterministic branch tree follows every
//! way light can reflect and refract inside the stone and records where each branch leaves it
//! ([`record::ExitRecord`]). Lighting is not part of the cache: phase B evaluates the environment
//! along the stored directions.
//!
//! # How the records reproduce the path tracer
//!
//! The path tracer chooses reflect or transmit (and, in a birefringent crystal, the eigenmode)
//! at random and divides by the probability it chose with, so its expected value is the sum over
//! outcomes of the unweighted Stokes-Mueller result. [`bounce::expand`] returns exactly that sum's
//! terms, using the path tracer's own refraction geometry, Fresnel and Mueller matrices, closed
//! form uniaxial solves, mode-selection probabilities, plane-of-incidence rotation and
//! Beer-Lambert absorption. A record's weights are the absorption-free Stokes intensity of each
//! comb channel for unpolarized camera light, and its three optical-depth coefficients give the
//! absorption at any wavelength (`record`). A zoned stone is not cached
//! (`PathCacheOff::ZonedColor`). See [`bounce`] for the per-bounce correspondence
//! and [`build`] for the tree, the dispersion handling and the residual.
//!
//! The entry point is [`tile::build_tile`] (or [`tile::TileBuilder`] for many tiles).

pub mod bounce;
pub mod build;
pub mod compress;
pub mod eligibility;
pub mod estimate;
pub mod frame;
pub mod key;
pub mod layout;
pub mod lru;
pub mod persist;
pub mod placement;
pub mod positions;
pub mod record;
pub mod reshade;
pub mod shade;
// The 32-bit-lane branch signature shared by the beam tracer, the reference tracer and WGSL.
pub mod signature;
pub mod tile;

pub use build::{DISPERSION_GATE_BG, TraceKit};
pub use compress::{CHUNK_MAGIC, ChunkError};
#[cfg(feature = "path-cache-disk")]
pub use compress::{decode_tile, encode_tile};
pub use eligibility::{PathCacheOff, StoneTraits, check_eligibility};
pub use estimate::{Measurements, SizeEstimate, estimate_size, measure_prepass};
pub use frame::{CacheOptions, PathCacheFrame};
pub use key::{CACHE_FORMAT_VERSION, CacheKey, CacheKeyInputs};
pub use layout::{LayoutError, TileCache, TileCacheHeader};
pub use lru::{DEFAULT_DISK_LIMIT_BYTES, enforce_limit};
#[cfg(feature = "path-cache-disk")]
pub use persist::load_cache;
pub use persist::{CacheMeta, KeyFolder, PersistError, RebuildReason};
pub use placement::{MemoryBudget, OffReason, Placement, choose_placement, status_text};
pub use positions::{BuildSettings, K_MAX, K0, PositionData};
pub use record::ExitRecord;
pub use reshade::{COMBS, ReshadeError, Reshaded, comb_hero_rand, reshade};
pub use shade::{PositionRef, SampleParts, Shader};
pub use tile::{TileBuilder, build_tile};

#[cfg(test)]
mod tests;
#[cfg(test)]
mod tests_bounce;
#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests_bounce_beam;
#[cfg(test)]
mod tests_build;
#[cfg(test)]
mod tests_compress;
#[cfg(test)]
mod tests_eligibility;
#[cfg(test)]
mod tests_estimate;
#[cfg(test)]
mod tests_key;
#[cfg(test)]
mod tests_lru;
#[cfg(test)]
mod tests_persist;
#[cfg(test)]
mod tests_placement;
#[cfg(test)]
mod tests_reshade;
#[cfg(test)]
mod tests_shade;
#[cfg(test)]
mod tests_shade_residual;
#[cfg(test)]
mod tests_signature;
#[cfg(test)]
mod tests_support;
#[cfg(test)]
mod tests_tile;
