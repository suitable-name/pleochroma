//! Lossless native inputs and the explicit, nonrecursive local corpus source.

use std::path::Path;

use pleochroma::{
    geometry::{
        cuts::StandardGemCuts,
        stone_metrics::{SolidStatus, build_solid_mesh},
    },
    optics::materials::GemMaterial,
};
use pleochroma_cut_core::{Design, MaterialLookup, PreformSpec, ResolvedMaterial, native};

use super::DbDesignRecord;

/// Whether a filename carries a self-contained native design rather than ASC instructions.
fn is_native(filename: &str) -> bool {
    Path::new(filename)
        .extension()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s.eq_ignore_ascii_case("psdd"))
}

pub(super) fn load_design_data(
    record: &DbDesignRecord,
    max_tiers: usize,
) -> Result<(Design, Option<ResolvedMaterial>), String> {
    if record.asc_bytes.is_empty() {
        return Err("empty design attachment".to_owned());
    }
    let (design, authored) = if is_native(&record.filename) {
        let text =
            std::str::from_utf8(&record.asc_bytes).map_err(|e| format!("native UTF-8: {e}"))?;
        let loaded = native::design_from_str(text).map_err(|e| format!("native parse: {e}"))?;
        if loaded.draft {
            return Err(
                "native file is marked draft; solve and save it before training".to_owned(),
            );
        }
        let source = native_source_material(&loaded)?;
        (loaded.design, Some(source))
    } else {
        let schedule = pleochroma_formats::asc::parse_asc_bytes(&record.asc_bytes)
            .map_err(|e| format!(".asc parse error: {e}"))?;
        let preform =
            inferred_rough(&schedule).unwrap_or_else(|| PreformSpec::block(2.0, 1.0, 2.0));
        let mut design = Design::from_asc_schedule(preform, &schedule);
        // ASC has no physical size. Sample one realistic width per record, explicitly labelled
        // as training augmentation. Native rough, physical dimensions and programs stay intact.
        let mut rng =
            pleochroma_ai::model::Prng::from_stream(record.id.unsigned_abs(), 0x5349_5A45);
        design.girdle_diameter_mm = Some([4.0, 5.5, 7.0, 10.0, 14.0][rng.next_index(5)]);
        (design, None)
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
    Ok((design, authored))
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
