//! Changing a queued job's material: the base material re-resolved field by field.
//!
//! Every step goes through the live functions (`resolve_material_with_override`,
//! `material_for_stone`, `MaterialInclusions::of_material`), so a job whose material changed
//! shows what the same stone shows in the viewport with that material chosen.

use crate::bridge::render_thread::RenderContext;
use pleochroma::{
    optics::{fluorescence::Fluorescence, materials::GemMaterial, zoning::ZonedAbsorption},
    render_setup::{
        material_for_stone, measure_model_width, resolve_material_with_override,
        stone_width_for_scale,
    },
};
use pleochroma_net::scene::material_inclusions::MaterialInclusions;
use pleochroma_render_jobs::{RenderJobFile, edit::JobEdit};

/// The separator between a job label and its material: " · Sapphire".
const SUFFIX_SEPARATOR: &str = " \u{b7} ";

/// What a job takes from a new material.
#[derive(Debug, Clone, PartialEq)]
pub(in crate::gui) struct MaterialChange {
    /// The new base material, sized for the job's stone, without its zones.
    pub material: GemMaterial,
    /// The new material's own zones; `None` for no zones, always `None` for a Ray-Traced job.
    pub zoning: Option<ZonedAbsorption>,
    /// The new value of `scene.effects.material_inclusions`; `None` leaves the record as it is
    /// (a Ray-Traced job, or no inclusions before and after). A material without inclusions on a
    /// job that had some is an empty record, which the edit reads as "none".
    pub material_inclusions: Option<MaterialInclusions>,
    /// The emitters of a physics custom material; empty for every other material.
    pub fluorescence: Fluorescence,
}

/// Re-resolves the material `name` for `job`, field by field.
///
/// - The base material is looked up by name among the built-in presets and the context's custom
///   materials; a missing one is refused.
/// - The stone width is read back from the job's old material and sized again with the new
///   material's unit.
/// - The edge rounding is kept; haze and crystal axes are the new material's own.
/// - Zones are the new material's own, none for a Ray-Traced job.
/// - Fluorescence comes from a physics custom material only.
///
/// # Errors
///
/// A sentence when `name` is neither a built-in preset nor a saved custom material.
pub(in crate::gui) fn resolve_job_material(
    ctx: &RenderContext,
    job: &RenderJobFile,
    name: &str,
) -> Result<MaterialChange, String> {
    let base = resolve_material_with_override(
        &GemMaterial::all_materials(),
        &ctx.custom_materials,
        None,
        name,
    )
    .ok_or_else(|| {
        format!(
            "'{}' is not a built-in preset or a saved custom material, so the job cannot use it.",
            name.trim()
        )
    })?;
    let mut material = sized_for_job(base, job);
    let fluorescence = physics_fluorescence(ctx, &material.name);
    let material_inclusions = inclusions_change(job, &material);
    let zones = material.zoning.take();
    let zoning = if job.engine.is_ray() { None } else { zones };
    Ok(MaterialChange {
        material,
        zoning,
        material_inclusions,
        fluorescence,
    })
}

/// `base` sized for the job's stone: the old material's width read back, then applied with the
/// new material's own unit, and the old edge rounding kept.
fn sized_for_job(base: GemMaterial, job: &RenderJobFile) -> GemMaterial {
    let old = &job.scene.material;
    let planes = &job.scene.planes;
    let width_mm = stone_width_for_scale(
        old.absorption_unit,
        old.absorption_path_scale,
        measure_model_width(planes),
    )
    .unwrap_or(0.0);
    let sized = material_for_stone(base, width_mm, planes);
    if old.edge_rounding_radius > 0.0 {
        sized.with_edge_rounding(old.edge_rounding_radius)
    } else {
        sized
    }
}

/// The emitters of the custom physics material `name`; empty for any other material.
fn physics_fluorescence(ctx: &RenderContext, name: &str) -> Fluorescence {
    let physics = ctx
        .custom_material_physics
        .iter()
        .any(|listed| listed.eq_ignore_ascii_case(name));
    if !physics {
        return Fluorescence::default();
    }
    ctx.custom_material_fluorescence
        .iter()
        .find(|(listed, _)| listed.eq_ignore_ascii_case(name))
        .map_or_else(Fluorescence::default, |(_, emitters)| {
            Fluorescence::clone(emitters)
        })
}

/// The `effects.material_inclusions` the job takes with `material`; see
/// [`MaterialChange::material_inclusions`].
fn inclusions_change(job: &RenderJobFile, material: &GemMaterial) -> Option<MaterialInclusions> {
    if job.engine.is_ray() {
        return None;
    }
    MaterialInclusions::of_material(material).or_else(|| {
        job.scene
            .effects
            .material_inclusions
            .is_some()
            .then(empty_inclusions)
    })
}

/// A record with no families and no structure: the edit reads it as "no inclusions".
const fn empty_inclusions() -> MaterialInclusions {
    MaterialInclusions {
        families: Vec::new(),
        structure: None,
    }
}

/// The [`JobEdit`] fields a material change sets: material, zoning, inclusions, fluorescence.
pub(in crate::gui) fn material_edit(change: MaterialChange) -> JobEdit {
    JobEdit {
        material: Some(change.material),
        zoning: Some(change.zoning),
        effects_material_inclusions: change.material_inclusions,
        fluorescence: Some(change.fluorescence),
        ..JobEdit::default()
    }
}

/// Built-in names plus the context's custom materials, in the Stone tab's order.
#[must_use]
pub(in crate::gui) fn material_names(ctx: &RenderContext) -> Vec<String> {
    pleochroma_editor::material::render_material_options(&ctx.custom_materials)
}

/// `text` with " · `material`" at its end: replaced when `text` already ends in " · " and a name
/// of `known`, appended otherwise.
///
/// The label rule of Copy to Materials and of a material change; the video name follows it too.
#[must_use]
pub(in crate::gui) fn with_material_suffix(text: &str, material: &str, known: &[String]) -> String {
    let base = known
        .iter()
        .filter_map(|name| stem_before_suffix(text, name))
        .min_by_key(|stem| stem.len())
        .unwrap_or(text)
        .trim_end();
    if base.is_empty() {
        material.to_string()
    } else {
        format!("{base}{SUFFIX_SEPARATOR}{material}")
    }
}

/// `text` without a trailing " · `name`" (ASCII case ignored), or `None` when it has none.
fn stem_before_suffix<'a>(text: &'a str, name: &str) -> Option<&'a str> {
    let suffix_len = SUFFIX_SEPARATOR.len() + name.len();
    let cut = text.len().checked_sub(suffix_len)?;
    if !text.is_char_boundary(cut) {
        return None;
    }
    let (stem, tail) = text.split_at(cut);
    let separator = tail.get(..SUFFIX_SEPARATOR.len())?;
    let tail_name = tail.get(SUFFIX_SEPARATOR.len()..)?;
    (separator == SUFFIX_SEPARATOR && tail_name.eq_ignore_ascii_case(name)).then_some(stem)
}
