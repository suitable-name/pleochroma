use serde::{Deserialize, Serialize};

/// The traced refractive-index range of one library design, as the `ri_ranges` side table
/// keeps it.
///
/// A row is current while `threshold_version` matches the version of the thresholds the
/// range was computed with. `planes_hash` fingerprints the design's facet planes when it was
/// computed, so a later edit of the design is noticed by whoever loads the entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StoredRiRange {
    /// The `diagram_entries.id` of the design.
    pub entry_id: i64,
    /// Hash of the design's facet planes at computation time.
    pub planes_hash: i64,
    /// The version of the thresholds the range was computed with.
    pub threshold_version: i64,
    /// The lowest refractive index the design works at, or `None` when it has no lower limit.
    pub n_low: Option<f64>,
    /// The highest refractive index the design works at, or `None` when the upper end is not
    /// bounded (or not calibrated).
    pub n_high: Option<f64>,
}
