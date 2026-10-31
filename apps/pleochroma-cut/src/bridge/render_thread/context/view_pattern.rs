//! Color patterns scoped to the stone currently shown in the library preview.
//!
//! The material preset is never mutated. A different stone or material drops the preview
//! override, whereas camera, lighting, size and crystal-axis changes keep it.

use super::{ColorTarget, PlanesOwner, RenderContext};
use pleochroma::optics::materials::GemMaterial;
use pleochroma_cut_core::material::pattern::{ColorPattern, apply_color_pattern};

/// A preview pattern and the stone it belongs to.
///
/// Only [`RenderContext::set_view_color_pattern`] constructs this record. Keeping its source
/// material makes a same-named edited custom material a new source too.
pub struct ViewColorPattern {
    owner: PlanesOwner,
    source: GemMaterial,
    pattern: ColorPattern,
}

/// Existing zones are replaced by the preview pattern. Their frame/size may also be rewritten
/// by the ordinary stone-axis and stone-width controls, so those changes are not a new source.
fn same_source(a: &GemMaterial, b: &GemMaterial) -> bool {
    if a == b {
        return true;
    }
    let mut a = a.clone();
    let mut b = b.clone();
    a.zoning = None;
    b.zoning = None;
    a.absorption_path_scale = 1.0;
    b.absorption_path_scale = 1.0;
    a == b
}

impl RenderContext {
    /// The underlying material, before a view-only color or pattern override.
    #[must_use]
    pub fn base_pattern_material(&self) -> Option<GemMaterial> {
        self.base_pattern_material_ref().cloned()
    }

    /// Whether `material` still names the same pattern source after size/axis updates.
    #[must_use]
    pub fn matches_pattern_material(&self, material: &GemMaterial) -> bool {
        self.base_pattern_material_ref()
            .is_some_and(|current| same_source(current, material))
    }

    fn base_pattern_material_ref(&self) -> Option<&GemMaterial> {
        self.material_override.as_ref().or_else(|| {
            self.custom_materials
                .iter()
                .chain(GemMaterial::built_in())
                .find(|material| material.name.eq_ignore_ascii_case(&self.material_name))
        })
    }

    /// The pattern that applies to the current preview stone, if one was set.
    #[must_use]
    pub fn current_view_color_pattern(&self) -> Option<&ColorPattern> {
        let stored = self.view_color_pattern.as_ref()?;
        (self.color_target() == ColorTarget::View
            && !self.physics_color()
            && self.material_unresolved.is_none()
            && stored.owner == self.planes_owner
            && self.matches_pattern_material(&stored.source))
        .then_some(&stored.pattern)
    }

    /// Changes only this preview stone's pattern; `None` restores its material's pattern.
    ///
    /// # Errors
    ///
    /// Refuses linked design targets, unresolved or physics materials, and invalid patterns.
    pub fn set_view_color_pattern(&mut self, pattern: Option<ColorPattern>) -> Result<(), String> {
        if self.color_target() != ColorTarget::View {
            return Err("The linked design's pattern must be applied as a design edit.".into());
        }
        let next = if let Some(pattern) = pattern {
            if self.physics_color() || self.material_unresolved.is_some() {
                return Err("This material cannot have a preview color pattern.".into());
            }
            let source = self
                .base_pattern_material()
                .ok_or_else(|| "Select a material before editing its color pattern.".to_owned())?;
            apply_color_pattern(source.clone(), &pattern, f64::from(self.stone_width_mm))
                .map_err(|error| error.to_string())?;
            Some(ViewColorPattern {
                owner: self.planes_owner,
                source,
                pattern,
            })
        } else {
            None
        };
        let changed =
            self.current_view_color_pattern() != next.as_ref().map(|value| &value.pattern);
        self.view_color_pattern = next;
        if changed {
            self.dirty = true;
        }
        Ok(())
    }

    /// Removes stale state so returning to an earlier selection cannot resurrect a pattern.
    pub(super) fn discard_stale_view_color_pattern(&mut self) {
        if self.view_color_pattern.is_some() && self.current_view_color_pattern().is_none() {
            self.view_color_pattern = None;
            self.dirty = true;
        } else if let Some(stored) = self.view_color_pattern.as_ref()
            && self.base_pattern_material_ref() != Some(&stored.source)
        {
            // Keep the fast equality path after the source's own zones were reframed/resized.
            let source = self.base_pattern_material();
            if let (Some(stored), Some(source)) = (self.view_color_pattern.as_mut(), source) {
                stored.source = source;
            }
        }
    }

    /// The preview pattern in the current physical size and crystal orientation.
    pub(super) fn patterned_view_material(&self) -> Option<GemMaterial> {
        let pattern = self.current_view_color_pattern()?;
        let source = self.base_pattern_material()?;
        let source = pleochroma_editor::stone_orientation::with_orientation(
            source,
            self.c_axis_override
                .map(|gamma| (gamma, self.alpha_axis_override)),
        );
        // Validation was performed before storing. Size and orientation keep valid geometry.
        apply_color_pattern(source, pattern, f64::from(self.stone_width_mm)).ok()
    }
}

#[cfg(test)]
#[path = "view_pattern_tests.rs"]
mod tests;
