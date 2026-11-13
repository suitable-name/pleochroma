//! The gemological-metrics cache -- see [`compute_or_reuse_metrics`].
//!
//! Recomputing `evaluate_gem_optical_metrics`/`evaluate_angular_profile` is expensive
//! (single-threaded analytical raytracing), and its result depends only on a handful of inputs
//! that don't change between progressive-accumulation samples.
//!
//! The cache itself lives in `pleochroma::color::metrics`, shared with the browser app's
//! solve worker so both recompute on the same changes; this module only names it for the
//! render loop.

/// Identity of a facet-plane set, shared by every per-design cache in this crate and the
/// browser app. `bridge::frame_cache::girdle_finish`, `stone_width` and
/// `bridge::frame_cache::guide_pass::GuideCache` reuse this hash in their invalidation keys
/// instead of a second implementation.
pub use pleochroma::render_setup::hash_planes;

pub(super) use pleochroma::color::metrics::MetricsCache;
