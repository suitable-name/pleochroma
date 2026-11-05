//! Lossless native inputs and the explicit, nonrecursive local corpus source.

use std::path::Path;

use pleochroma::{
    geometry::{
        cuts::StandardGemCuts,
        stone_metrics::{SolidStatus, build_solid_mesh},
    },
    optics::materials::GemMaterial,
};
use pleochroma_cut_core::{
    Design, MaterialLookup, MaterialSelection, PreformSpec, ResolvedMaterial, native,
};

use super::DbDesignRecord;

/// Whether a filename carries a self-contained native design rather than ASC instructions.
pub(super) fn is_native(filename: &str) -> bool {
    Path::new(filename)
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("psdd"))
}

#[derive(Debug)]
pub(super) struct LoadedDesignData {
    pub design: Design,
    pub authored_material: Option<ResolvedMaterial>,
    pub physical_size_augmented: bool,
}

pub(super) fn load_design_data(
    record: &DbDesignRecord,
    max_tiers: usize,
) -> Result<LoadedDesignData, String> {
    if record.asc_bytes.is_empty() {
        return Err("empty design attachment".to_owned());
    }
    let (mut design, authored_material) = if is_native(&record.filename) {
        load_native(&record.asc_bytes)?
    } else {
        let schedule = pleochroma_formats::asc::parse_asc_bytes(&record.asc_bytes)
            .map_err(|e| format!(".asc parse error: {e}"))?;
        let preform =
            inferred_rough(&schedule).unwrap_or_else(|| PreformSpec::block(2.0, 1.0, 2.0));
        (Design::from_asc_schedule(preform, &schedule), None)
    };
    let tiers = design.tiers.len();
    if tiers == 0 {
        return Err("the design has no tiers".to_owned());
    }
    if tiers > max_tiers {
        return Err(format!("{tiers} tiers exceed the limit of {max_tiers}"));
    }
    pleochroma_ai::env::design_features::validate_design_representation(&design)
        .map_err(|e| format!("unsupported design representation: {e}"))?;
    design
        .evaluate_relations()
        .map_err(|e| format!("invalid angle relation: {e}"))?;
    let physical_size_augmented = augment_missing_size(&mut design, record.id)?;
    Ok(LoadedDesignData {
        design,
        authored_material,
        physical_size_augmented,
    })
}

fn load_native(bytes: &[u8]) -> Result<(Design, Option<ResolvedMaterial>), String> {
    let text = std::str::from_utf8(bytes).map_err(|e| format!("native UTF-8: {e}"))?;
    let file = pleochroma_formats::native::design::parse(text)
        .map_err(|e| format!("native parse: {e}"))?;
    if file.material.custom.is_some() && file.material.name.is_none() {
        return Err("native custom snapshot has no material name".to_owned());
    }
    let context = NativeSourceContext::capture(&file);
    let loaded = native::design_from_file(file).map_err(|e| format!("native parse: {e}"))?;
    if loaded.draft {
        return Err("native file is marked draft; solve and save it before training".to_owned());
    }
    let design = &loaded.design;
    context.validate(design)?;
    let source_authored = context.authored
        || loaded.restorable_custom_material.is_some()
        || design.material != MaterialSelection::none()
        || !design.render_effects.is_default()
        || design.crystal_axis_plan.is_some()
        || design
            .file_extras
            .as_ref()
            .is_some_and(|extras| !extras.material.is_empty());
    let authored_material = source_authored
        .then(|| native_source_material(&loaded))
        .transpose()?;
    Ok((loaded.design, authored_material))
}

/// Source intent before the application's tolerant native reader applies compatibility defaults.
struct NativeSourceContext {
    authored: bool,
    inclusion: Option<String>,
    structure: Option<String>,
    axis_rule: Option<String>,
    color_variant: Option<String>,
}

impl NativeSourceContext {
    fn capture(file: &native::DesignFile) -> Self {
        let effects = file.render_effects.as_ref();
        Self {
            // Include every raw material field, even one the older core reader drops.
            // Unknown metadata stays opaque; it must not make the source sampleable.
            authored: file.material != native::MaterialTable::new(None, None, None)
                || effects.is_some_and(|effects| !effects.is_empty())
                || file.crystal_axis_plan.is_some(),
            inclusion: effects
                .and_then(|effects| effects.inclusions.as_ref())
                .map(|inclusions| inclusions.preset.clone()),
            structure: effects
                .and_then(|effects| effects.structure.as_ref())
                .map(|structure| structure.pattern.clone()),
            axis_rule: file
                .crystal_axis_plan
                .as_ref()
                .filter(|plan| !matches!(plan.rule.as_str(), "" | "none"))
                .map(|plan| plan.rule.clone()),
            color_variant: file
                .material
                .color_variant
                .as_ref()
                .filter(|variant| {
                    // Removing a recorded fallback RGB intentionally resets an older
                    // editor to Material Default. A changed RGB survives conversion
                    // and is accepted by validate as the supported replacement.
                    variant.fallback_rgb.is_none() || file.material.body_color_override.is_some()
                })
                .map(|variant| variant.label.clone()),
        }
    }

    fn validate(&self, design: &Design) -> Result<(), String> {
        if let Some(preset) = &self.inclusion
            && design.render_effects.inclusions.is_none()
        {
            return Err(format!(
                "native inclusion preset {preset:?} cannot be represented by this trainer"
            ));
        }
        if let Some(pattern) = &self.structure
            && design.render_effects.structure.is_none()
        {
            return Err(format!(
                "native structure pattern {pattern:?} cannot be represented by this trainer"
            ));
        }
        if let Some(rule) = &self.axis_rule
            && !design
                .crystal_axis_plan
                .as_ref()
                .is_some_and(|plan| plan.rule.is_some())
        {
            return Err(format!(
                "native crystal-axis rule {rule:?} cannot be represented by this trainer"
            ));
        }
        let material = &design.material;
        if let Some(label) = &self.color_variant
            && material.color_variant_override.is_none()
            && material.body_color_override.is_none()
            && material.body_color_bands_override.is_none()
            && material.color_pattern_override.is_none()
        {
            return Err(format!(
                "native color variant {label:?} cannot be represented and has no supported color fallback"
            ));
        }
        Ok(())
    }
}

/// Fill only an unanchored, dimensionless design; targets must keep their author's scale.
fn augment_missing_size(design: &mut Design, record_id: i64) -> Result<bool, String> {
    if let Some(width) = design.girdle_diameter_mm {
        if !width.is_finite() || width <= 0.0 {
            return Err("authored girdle_diameter_mm must be finite and positive".to_owned());
        }
        return Ok(false);
    }
    if !design.tier_targets.is_empty() {
        return Err(
            "dimensional tier targets require an authored girdle_diameter_mm; training cannot invent their physical scale"
                .to_owned(),
        );
    }
    // Match the existing ASC distribution and stream. This is in-memory training
    // augmentation, independent of material sampling, not a recovered measurement.
    let mut rng = pleochroma_ai::model::Prng::from_stream(record_id.unsigned_abs(), 0x5349_5A45);
    design.girdle_diameter_mm = Some([4.0, 5.5, 7.0, 10.0, 14.0][rng.next_index(5)]);
    Ok(true)
}

fn native_source_material(loaded: &native::LoadedDesign) -> Result<ResolvedMaterial, String> {
    let selection = &loaded.design.material;
    if let Some(snapshot) = &loaded.restorable_custom_material {
        let name = selection
            .name
            .as_deref()
            .ok_or("custom snapshot has no material name")?;
        let lookup = SnapshotLookup(native::gem_material_from_custom_snapshot(name, snapshot));
        let mut material = selection.resolve(&lookup);
        material.gem = selection.apply_overrides(material.gem);
        return Ok(material);
    }
    if let Some(name) = &selection.name {
        // Never silently let the normal application's tolerant lookup choose diamond or a
        // substring match for an un-restorable custom training material.
        let Some(gem) = GemMaterial::built_in()
            .iter()
            .find(|gem| gem.name.eq_ignore_ascii_case(name))
        else {
            return Err(format!(
                "native material {name:?} has no embedded snapshot or exact built-in"
            ));
        };
        let mut gem = gem.clone();
        gem.name.clone_from(name);
        let mut material = selection.resolve(&SnapshotLookup(gem));
        material.gem = selection.apply_overrides(material.gem);
        return Ok(material);
    }
    Ok(pleochroma_ai::env::design_source_material(&loaded.design))
}

/// Fit a centred bounding block around a closed imported cut, with a 5% cutting margin.
/// ASC's finite explicit masts make this independent of meet solving or native constraints.
fn inferred_rough(schedule: &pleochroma_formats::asc::AscSchedule) -> Option<PreformSpec> {
    let planes: Vec<_> = StandardGemCuts::from_asc_schedule(schedule)
        .into_iter()
        .map(pleochroma::geometry::GpuFacetPlane::to_halfspace_f64)
        .collect();
    let SolidStatus::Closed(mesh) = build_solid_mesh(&planes) else {
        return None;
    };
    let mut extent = [0.0f64; 3];
    for p in mesh.positions {
        for (limit, value) in extent.iter_mut().zip([p.x, p.y, p.z]) {
            *limit = limit.max(value.abs());
        }
    }
    if extent
        .iter()
        .any(|value| !value.is_finite() || *value <= 0.0)
    {
        return None;
    }
    Some(PreformSpec::block(
        extent[0] * 1.05,
        extent[2] / extent[0],
        extent[1] * 2.1,
    ))
}

struct SnapshotLookup(GemMaterial);

impl MaterialLookup for SnapshotLookup {
    fn lookup(&self, name: &str) -> Option<GemMaterial> {
        (self.0.name == name).then(|| self.0.clone())
    }
}

#[cfg(test)]
#[path = "dataset_inputs_tests.rs"]
mod tests;
