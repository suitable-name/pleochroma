use pleochroma_cut_core::{MaterialSelection, material::VariantChoice};
use serde::{Deserialize, Serialize};

/// The linked design's per-design material overrides (`MaterialSelection`'s RI
/// override and body color).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct DesignMaterialOverrides {
    /// `MaterialSelection::refractive_index_override`: a flat dispersion at this `n_d`.
    pub refractive_index_override: Option<f64>,
    /// `MaterialSelection::body_color_override`: an isotropic body color.
    pub body_color_override: Option<[f32; 3]>,
    /// `MaterialSelection::body_color_bands_override`: the seven-band body colour of the
    /// path-aware L*C*h editor, rows `[centre_nm, width_nm, amplitude_per_mm]`. Empty is
    /// "no bands" (the triple above colours the stone); non-empty bands win over the triple.
    /// Wire version 12.
    #[serde(default)]
    pub body_color_bands: Vec<[f32; 3]>,
    /// `MaterialSelection::absorption_path_scale_override`: mm per model unit for the bands.
    /// Wire version 12.
    #[serde(default)]
    pub absorption_path_scale_override: Option<f32>,
    /// Optional per-stone pattern, applied before render-time size and axis settings.
    #[serde(default, deserialize_with = "crate::pattern_wire::deserialize")]
    pub color_pattern_override: Option<pleochroma_cut_core::material::pattern::ColorPattern>,
    /// `MaterialSelection::color_variant_override`: the design's color variant, a snapshot of
    /// the whole per-axis tensor (so a Worker needs no variant library and keeps the stone's
    /// pleochroism). Appended last, after the pattern. Wire version 31.
    #[serde(default)]
    pub color_variant: Option<VariantChoice>,
}

impl DesignMaterialOverrides {
    /// Copies the render-relevant overrides from a design's selection.
    #[must_use]
    pub fn from_selection(selection: &MaterialSelection) -> Self {
        Self {
            refractive_index_override: selection.refractive_index_override,
            body_color_override: selection.body_color_override,
            body_color_bands: selection
                .body_color_bands_override
                .clone()
                .unwrap_or_default(),
            absorption_path_scale_override: selection.absorption_path_scale_override,
            color_pattern_override: selection.color_pattern_override.clone(),
            color_variant: selection.color_variant_override.clone(),
        }
    }

    pub(super) fn to_selection(&self) -> MaterialSelection {
        let bands = (!self.body_color_bands.is_empty()).then(|| self.body_color_bands.clone());
        MaterialSelection {
            refractive_index_override: self.refractive_index_override,
            body_color_override: self.body_color_override,
            absorption_path_scale_override: bands.as_ref().and(self.absorption_path_scale_override),
            color_pattern_override: self.color_pattern_override.clone(),
            body_color_bands_override: bands,
            color_variant_override: self.color_variant.clone(),
            ..MaterialSelection::default()
        }
    }
}
