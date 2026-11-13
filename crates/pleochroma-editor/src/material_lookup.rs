//! The editor-side `pleochroma_cut_core::MaterialLookup`.
//!
//! Built-ins plus whatever custom catalogue materials the render context currently has
//! loaded -- and the small pure helpers built on top of it: a `GemMaterial` that actually
//! reflects an RI override (for the optimizer and, later, the tilt curve), and the
//! "nearest built-in to this refractive index" search the catalogue-load suggestion and
//! the linked-viewport-material fallback both need.
//!
//! # Per-axis dispersion is not authored yet
//!
//! `pleochroma_vault::model::material::CustomMaterialRow::per_axis_dispersion_json` is
//! carried through the library rows, but the Material Editor does not author it yet, and a
//! save keeps the stored value. The core applies a stored curve to the material's extraordinary
//! ray (`pleochroma_cut_core::material`); without one the ray follows the constant
//! `birefringence_delta` approximation.
//!
//! # A custom material's ordinary-ray curve
//!
//! A custom material is either the Cauchy fit of a refractive index and an `n_F - n_C`
//! figure (`GemMaterial::new_custom`) or, when its author typed coefficients in the
//! Material Editor, a Sellmeier or Cauchy model of their own
//! (`GemMaterial::new_custom_with_dispersion`). Either way the curve is the material's
//! `dispersion` field, and every function here returns it unchanged. The one thing that
//! replaces it is a design's refractive-index override (`MaterialSelection::
//! apply_overrides`), which flattens any material to a non-dispersive index.

use crate::stone_orientation::{orientation_override, oriented_for_effects};
use pleochroma::{geometry::GpuFacetPlane, optics::materials::GemMaterial, render_setup};
use pleochroma_cut_core::{
    Design, StoneRenderEffects,
    material::{
        BuiltinMaterials, MaterialLookup, MaterialSelection, pattern::follow_noted_pattern,
    },
};

/// Built-ins plus whatever custom catalogue materials the caller hands in.
///
/// Custom materials take precedence over built-ins, as in
/// `pleochroma::render_setup::resolve_material`, so a design resolves the same way in the
/// editor as in the live render.
pub struct EditorMaterialLookup<'a> {
    custom: &'a [GemMaterial],
    /// Opt-in source for a CUSTOM material's specific gravity. `None` unless a caller opts in
    /// via [`Self::with_specific_gravity`]. Built-in names resolve via `BuiltinMaterials`
    /// regardless. Shaped like `RenderContext::custom_material_specific_gravity`, so a live
    /// context can pass it directly.
    specific_gravity_source: Option<&'a [(String, f64)]>,
}

impl<'a> EditorMaterialLookup<'a> {
    /// Builds a lookup with `custom` and no specific-gravity source -- opt in via
    /// [`Self::with_specific_gravity`].
    #[must_use]
    pub const fn new(custom: &'a [GemMaterial]) -> Self {
        Self {
            custom,
            specific_gravity_source: None,
        }
    }

    /// Opts this lookup into resolving a CUSTOM material's specific gravity from `entries`, a
    /// `(name, sg)` list shaped like `RenderContext::custom_material_specific_gravity`.
    #[must_use]
    pub const fn with_specific_gravity(mut self, entries: &'a [(String, f64)]) -> Self {
        self.specific_gravity_source = Some(entries);
        self
    }
}

impl MaterialLookup for EditorMaterialLookup<'_> {
    fn lookup(&self, name: &str) -> Option<GemMaterial> {
        self.custom
            .iter()
            .find(|m| m.name.eq_ignore_ascii_case(name))
            .cloned()
            .or_else(|| BuiltinMaterials.lookup(name))
    }

    /// Custom-over-built-in, matching [`Self::lookup`]'s own precedence: a custom
    /// material's own recorded SG (when [`Self::with_specific_gravity`] opted this
    /// lookup into reading it) wins, falling back to the built-in table for a name
    /// that names a built-in preset instead. The case-insensitive scan matches
    /// `RenderContext::custom_specific_gravity`'s own matching convention, so the
    /// two never disagree about what counts as the same name.
    fn specific_gravity(&self, name: &str) -> Option<f64> {
        self.specific_gravity_source
            .and_then(|entries| {
                entries
                    .iter()
                    .find(|(n, _)| n.eq_ignore_ascii_case(name))
                    .map(|(_, sg)| *sg)
            })
            .or_else(|| BuiltinMaterials.specific_gravity(name))
    }
}

/// Resolves `selection` to the real [`GemMaterial`] a consumer that needs actual optics
/// (the optimizer's objective, the tilt curve) should score against.
///
/// Unlike [`MaterialSelection::resolve`] alone, this makes `refractive_index_override`
/// actually show up in the returned material's own dispersion, not just in the
/// caller-visible `n_d` scalar, and applies `body_color_override` to its absorption.
///
/// When no override is set, this is exactly `selection.resolve(lookup).gem` -- the
/// resolved material's own real dispersion curve and color, unchanged. Otherwise the
/// overrides are applied by [`MaterialSelection::apply_overrides`], the one shared
/// resolution point every "selection -> `GemMaterial`" path in this crate goes
/// through: an RI override replaces the dispersion with a flat (non-dispersive)
/// `DispersionModel::Cauchy { a: n_d, b: 0.0, c: 0.0 }` fit at exactly the override
/// value, and a body color replaces the absorption with the isotropic band set of
/// that color (`GemMaterial::with_body_color`) -- everything else about the
/// resolved material (name, crystal system, birefringence, c-axis) is left untouched.
#[must_use]
pub fn resolved_gem_material(
    selection: &MaterialSelection,
    lookup: &dyn MaterialLookup,
) -> GemMaterial {
    selection.apply_overrides(selection.resolve(lookup).gem)
}

/// [`resolved_gem_material`] with the design's own Crystal Axis applied.
///
/// See [`crate::stone_orientation::oriented_for_effects`]: the material a scoring scene (a sweep,
/// Tune, Baseline, Trials, Retarget, the Verdict) traces for the stone the design describes.
/// `effects` is the design's `render_effects`; one that sets no axis gives exactly
/// [`resolved_gem_material`]'s material.
#[must_use]
pub fn resolved_gem_material_for_design(
    selection: &MaterialSelection,
    lookup: &dyn MaterialLookup,
    effects: &StoneRenderEffects,
) -> GemMaterial {
    oriented_for_effects(resolved_gem_material(selection, lookup), effects)
}

/// `material` sized for the design's real stone, for a run that works out the face-up
/// colour (the Optimize tone objective).
///
/// The renderer's own size rule, `render_setup::material_for_stone_oriented` (the stone-size
/// step of `render_setup::apply_material_overrides`), with [`Design::girdle_diameter_mm`] as the
/// width: a per-model-unit material (the built-ins, legacy colour triples) gets `mm / 7`, a
/// per-mm one (band colours, physics recipes) gets `mm / w` for the planes' model width `w`
/// ([`pleochroma::render_setup::measure_model_width`]), and 7 mm when the design has no girdle
/// diameter. A per-model-unit material with no girdle diameter gets the face-up calibration
/// `1 / K`, rescaled by the planes' model width, so the objective sees the colour the render
/// shows. The stone is oriented as the design orients it
/// (`Design::render_effects`, see [`crate::stone_orientation`]).
#[must_use]
pub fn sized_material_for_optimize(
    material: GemMaterial,
    design: &Design,
    planes: &[GpuFacetPlane],
) -> GemMaterial {
    let mm = design_girdle_mm(design);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "f32 is the GPU/render precision; a girdle diameter in mm is a small value"
    )]
    let mm_f32 = mm as f32;
    let material = with_design_pattern(material, design);
    let orientation = orientation_override(&design.render_effects, &material);
    let mut sized =
        render_setup::material_for_stone_oriented(material, mm_f32, planes, orientation);
    // A pattern that follows the Crystal Axis takes the stone's axis here too (the design's, or
    // the material's own); the mode is read off the zones, so any other material is untouched.
    follow_noted_pattern(&mut sized);
    sized
}

/// The design's girdle diameter in millimetres, `0.0` when it has none (or a useless one).
fn design_girdle_mm(design: &Design) -> f64 {
    design
        .girdle_diameter_mm
        .filter(|mm| *mm > 0.0 && mm.is_finite())
        .unwrap_or(0.0)
}

/// `material` with the stone's own color pattern applied, when the design carries one that
/// applies; otherwise `material` unchanged.
///
/// A pattern replaces the absorption with its per-mm zone colors, so it also changes which size
/// rule the material gets: see [`optimize_needs_model_width`].
fn with_design_pattern(material: GemMaterial, design: &Design) -> GemMaterial {
    let Some(pattern) = design.material.color_pattern_override.as_ref() else {
        return material;
    };
    pleochroma_cut_core::material::pattern::apply_color_pattern(
        material.clone(),
        pattern,
        design_girdle_mm(design),
    )
    .unwrap_or(material)
}

/// Whether [`sized_material_for_optimize`] needs the planes' model width for `material`.
///
/// Always, since every material reads it (a per-model-unit one for the reference-width
/// rescale, a per-mm one for millimetres per model unit), see
/// [`render_setup::needs_model_width`].
///
/// Decided on the material after the stone's color pattern is applied, not on the base: a
/// pattern can turn a per-model-unit base into a per-mm one, and deciding on the base would
/// size it with no model width.
#[must_use]
pub fn optimize_needs_model_width(material: &GemMaterial, design: &Design) -> bool {
    render_setup::needs_model_width(&with_design_pattern(material.clone(), design))
}

/// How close a design's own refractive index must sit to a built-in preset's for that
/// preset to stand in for an unnamed material.
///
/// 0.02 is roughly the gap between neighbouring species in the built-in table, so a match
/// this close is the same stone by any practical reading. A design between two presets is
/// refused rather than rounded to the nearer one. Shared by [`traced_material_for`] and
/// [`material_for_refractive_index`] (a catalogue row), so the two agree on what matches.
pub const MATERIAL_MATCH_TOLERANCE: f64 = 0.02;

/// The built-in material a design carrying `n_d` and no species name should be rendered as.
///
/// The nearest preset within [`MATERIAL_MATCH_TOLERANCE`], resolved to a real [`GemMaterial`],
/// or `None` when nothing built in is that close. A library row records a refractive index but
/// never a species, so this is how it is named. It refuses rather than substituting, since a
/// design rendered as another species gives wrong optics with no sign on screen.
#[must_use]
pub fn material_for_refractive_index(n_d: f64) -> Option<(String, GemMaterial)> {
    let (name, _) = nearest_built_in_material(n_d, MATERIAL_MATCH_TOLERANCE)?;
    let gem = BuiltinMaterials.lookup(&name)?;
    Some((name, gem))
}

/// The [`GemMaterial`] a design is rendered as, given the traced material name (see
/// [`traced_material_for`]). `None` when nothing in this catalogue answers to that name.
///
/// Unlike [`resolved_gem_material`], this keys off the traced name rather than
/// `MaterialSelection::name`, and refuses instead of substituting. `MaterialSelection::resolve`
/// falls back to diamond for an unnamed selection, whose empty absorption would render the stone
/// colorless. `RenderContext::material_override` takes precedence over the by-name lookup and
/// must follow the same rule.
///
/// `selection` supplies its overrides, applied as in [`resolved_gem_material`] (through
/// [`MaterialSelection::apply_overrides`]), so a design's color reaches the render, the tilt
/// sweep, the hover preview and the export.
#[must_use]
pub fn traced_gem_material(
    traced_name: &str,
    selection: &MaterialSelection,
    lookup: &dyn MaterialLookup,
) -> Option<GemMaterial> {
    traced_gem_material_at_width(traced_name, selection, lookup, 7.0)
}

/// Resolves the traced material and this stone's pattern at its current width in mm.
///
/// Relative pattern geometry is rebuilt from the saved pattern at this width, before
/// render setup computes absorption scale from the actual model geometry.
#[must_use]
pub fn traced_gem_material_at_width(
    traced_name: &str,
    selection: &MaterialSelection,
    lookup: &dyn MaterialLookup,
    stone_width_mm: f64,
) -> Option<GemMaterial> {
    lookup
        .lookup(traced_name)
        .map(|gem| selection.apply_overrides_at_width(gem, stone_width_mm))
}

/// The built-in species [`nearest_built_in_material`] guesses for a nameless design, from its
/// refractive index alone.
///
/// Guessing is limited to these common species. An exhaustive search names some stones
/// wrongly: at `n_d` 1.76, Benitoite lies closer than Sapphire. Only species a cutter is likely
/// to hold are listed.
const RI_INFERENCE_CANDIDATES: [&str; 13] = [
    "Diamond",
    "Sapphire",
    "Ruby",
    "Emerald",
    "Zircon",
    "Alexandrite",
    "Topaz",
    "Spinel",
    "Quartz",
    "Tourmaline",
    "Tanzanite",
    "Synthetic Moissanite",
    "Cubic Zirconia",
];

/// The built-in preset (from [`RI_INFERENCE_CANDIDATES`]) whose `n_D` is closest to `n_d`, when
/// that distance is within `tolerance`, else `None`.
///
/// Ties keep the first match in list order, so the result is deterministic.
#[must_use]
pub fn nearest_built_in_material(n_d: f64, tolerance: f64) -> Option<(String, f64)> {
    RI_INFERENCE_CANDIDATES
        .iter()
        .filter_map(|&name| {
            pleochroma_cut_core::built_in_refractive_index(name).map(|ri| (name, ri))
        })
        .map(|(name, ri)| {
            let diff = (ri - n_d).abs();
            (name, ri, diff)
        })
        .filter(|&(_, _, diff)| diff <= tolerance)
        .min_by(|a, b| a.2.total_cmp(&b.2))
        .map(|(name, ri, _)| (name.to_string(), ri))
}

/// Every [`RI_INFERENCE_CANDIDATES`] preset within `tolerance` of `n_d`, nearest first. The guess
/// badge's tooltip lists these, so a cutter can see what else was close before trusting the
/// nearest name.
#[must_use]
pub fn material_guess_candidates(n_d: f64, tolerance: f64) -> Vec<(String, f64)> {
    let mut matches: Vec<(&'static str, f64, f64)> = RI_INFERENCE_CANDIDATES
        .iter()
        .filter_map(|&name| {
            pleochroma_cut_core::built_in_refractive_index(name).map(|ri| (name, ri))
        })
        .map(|(name, ri)| (name, ri, (ri - n_d).abs()))
        .filter(|&(_, _, diff)| diff <= tolerance)
        .collect();
    matches.sort_by(|a, b| a.2.total_cmp(&b.2));
    matches
        .into_iter()
        .map(|(name, ri, _)| (name.to_string(), ri))
        .collect()
}

/// The material name the tracer should use for `design`, or the reason it must not trace.
///
/// A design from an `.asc`, or a new one, may carry no material name, since the schedule
/// records only a refractive index. Substituting `"Diamond"` would trace a quartz design at
/// n=2.417 while the critical angle and MARGIN beside it use its real n=1.5442.
///
/// The design's own material is used when the catalogue (`custom` plus the built-ins, the lookup
/// [`EditorMaterialLookup`] resolves through) has it. Otherwise the nearest built-in within
/// [`MATERIAL_MATCH_TOLERANCE`] of the design's refractive index is used. When nothing is that
/// close, a `Some(reason)` is returned instead, and the reason is shown in place of a trace.
#[must_use]
pub fn traced_material_for(design: &Design, custom: &[GemMaterial]) -> (String, Option<String>) {
    if let Some(name) = &design.material.name {
        // Checked against the SAME catalogue `EditorMaterialLookup` resolves
        // through, so this can never disagree with what actually gets traced: a
        // design naming a since-deleted custom material (or an old `.asc`'s typo)
        // must not read as "resolved".
        return if EditorMaterialLookup::new(custom).lookup(name).is_some() {
            (name.clone(), None)
        } else {
            (
                String::new(),
                Some(format!(
                    "This design's material '{name}' is not a built-in preset or a \
                     saved custom material, so there is nothing to trace -- pick a \
                     material in Design Settings, or re-save the missing custom \
                     material, rather than rendering it as something else."
                )),
            )
        };
    }
    let n_d = design.effective_refractive_index();
    if let Some((name, _)) = nearest_built_in_material(n_d, MATERIAL_MATCH_TOLERANCE) {
        return (name, None);
    }
    // The name is left empty deliberately: nothing should trace, so there is no
    // material to name, and a plausible-looking placeholder here is exactly the bug.
    (
        String::new(),
        Some(format!(
            "This design names no material, and its refractive index ({n_d:.4}) matches \
             no built-in preset within {MATERIAL_MATCH_TOLERANCE:.2}. Pick a material in \
             Design Settings -- rendering it as something else would give you the optics \
             of a different stone."
        )),
    )
}

/// The inferred-material badge for a design that names no material.
///
/// Returns the guess label (`"Sapphire? (from RI 1.76)"`), the guessed name, and a tooltip
/// listing the other close candidates (`""` when there are none). `None` when a material name
/// is set, or when nothing built in is close enough to `n_d`.
#[must_use]
pub fn material_guess(design: &Design, n_d: f64) -> Option<(String, String, String)> {
    if design.material.name.is_some() {
        return None;
    }
    let (name, _) = material_for_refractive_index(n_d)?;
    let text = format!("{name}? (from RI {n_d:.2})");
    let others: Vec<String> = material_guess_candidates(n_d, MATERIAL_MATCH_TOLERANCE)
        .into_iter()
        .filter(|(candidate, _)| candidate != &name)
        .map(|(candidate, ri)| format!("{candidate} ({ri:.3})"))
        .collect();
    let others_text = if others.is_empty() {
        String::new()
    } else {
        format!("Also within tolerance: {}", others.join(", "))
    };
    Some((text, name, others_text))
}

/// The inferred-material badge for a material the editor applied when a library design opened
/// (`crate::loading::apply_library_material`).
///
/// Returns the same three strings as [`material_guess`]. `applied` is the name the load applied.
/// The badge shows only while the design still has that material, so a later pick in the Stone
/// tab is never labelled a guess. `None` when nothing was applied.
#[must_use]
pub fn applied_material_guess(
    design: &Design,
    n_d: f64,
    applied: Option<&str>,
) -> Option<(String, String, String)> {
    let applied = applied?;
    let current = design.material.name.as_deref()?;
    if !current.eq_ignore_ascii_case(applied) {
        return None;
    }
    let text = format!("{current}? (from library, RI {n_d:.2})");
    let note = "Applied from the library entry when this design was opened. Pick another \
                material in the Stone tab to change it."
        .to_owned();
    Some((text, current.to_owned(), note))
}

#[cfg(test)]
#[path = "material_lookup/tests.rs"]
mod tests;
