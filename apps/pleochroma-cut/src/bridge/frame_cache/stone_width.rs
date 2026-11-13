//! Cached model-space girdle width for the "Stone size" absorption-scale control.
//!
//! `apply_material_overrides` turns the `stone_width_mm` setting into
//! `GemMaterial::absorption_path_scale`, which needs the design's girdle width in model units.
//! The measurement is `pleochroma::render_setup::measure_model_width`, shared with the browser
//! app. It is not free: it enumerates every feasible plane-triple intersection of the design.
//! The render loop calls `apply_material_overrides` every frame, so [`StoneWidthCache`]
//! recomputes only when `render_thread::hash_planes`'s key differs from the last call. The cache
//! lives here, not in the renderer, because it is per-call state rather than render physics.

use crate::bridge::render_thread::hash_planes;
use pleochroma::{geometry::plane::GpuFacetPlane, render_setup::measure_model_width};

/// `key` starts `None`, so the first [`Self::ensure`] call measures before anything reads `width`.
#[derive(Debug, Default)]
pub struct StoneWidthCache {
    key: Option<u64>,
    width: Option<f64>,
}

impl StoneWidthCache {
    /// Creates an empty instance.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            key: None,
            width: None,
        }
    }

    /// Returns the design's girdle width in model units (`SolidMetrics::width_axis`
    /// of the solid bounded by `planes`), or `None` when the plane arrangement doesn't
    /// bound a measurable solid (e.g. an in-progress custom design missing its closing
    /// planes) -- `apply_material_overrides` treats `None` the same as the control
    /// being off, leaving the material's `absorption_path_scale` untouched. Measures again
    /// only when `planes`'s hash differs from the last call.
    pub fn ensure(&mut self, planes: &[GpuFacetPlane]) -> Option<f64> {
        let key = hash_planes(planes);
        if self.key != Some(key) {
            self.width = measure_model_width(planes);
            self.key = Some(key);
        }
        self.width
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma::geometry::cuts::StandardGemCuts;

    #[test]
    fn ensure_measures_the_standard_round_brilliant_girdle_width() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let mut cache = StoneWidthCache::new();
        let width = cache.ensure(&planes).expect("SRB must measure");
        assert!(width > 0.0, "width {width}");
    }

    /// A repeat call with the same planes returns the same width.
    #[test]
    fn ensure_is_stable_across_repeated_calls_with_unchanged_planes() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let mut cache = StoneWidthCache::new();
        let first = cache.ensure(&planes);
        let second = cache.ensure(&planes);
        assert_eq!(first, second);
    }

    /// A design change (different plane set) must remeasure, not keep serving the
    /// previous design's stale width.
    #[test]
    fn ensure_remeasures_when_the_design_changes() {
        let srb = StandardGemCuts::standard_round_brilliant();
        let emerald = StandardGemCuts::emerald_cut();
        let mut cache = StoneWidthCache::new();
        let srb_width = cache.ensure(&srb);
        let emerald_width = cache.ensure(&emerald);
        assert_eq!(emerald_width, measure_model_width(&emerald));
        assert_ne!(
            srb_width, emerald_width,
            "SRB and emerald_cut have different girdle widths, so a stale \
             (un-remeasured) result would show up as an equal width"
        );
    }
}
