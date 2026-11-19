//! Tests of the material resolver and its label suffix rule: the job's new material equals what
//! the live capture gives the same stone, for every kind of material the job can change to.

use super::material::{
    MaterialChange, material_edit, material_names, resolve_job_material, with_material_suffix,
};
use crate::{
    bridge::{
        export_thread::{ExportParams, RemoteSelection, SceneSnapshot},
        render_thread::RenderContext,
    },
    gui::render_jobs::convert::{self, StillJobInputs},
};
use pleochroma::{
    color::ColorSpace,
    optics::{
        fluorescence::{EmissionBand, Fluorescence, FluorescentEmitter},
        materials::{AbsorptionUnit, GemMaterial},
    },
    renderer::engine::{EngineSpec, RayOptions, RenderEngine},
};
use pleochroma_net::scene::material_inclusions::MaterialInclusions;
use pleochroma_render_jobs::{RenderJobFile, job::DesignInfo};
use std::{
    path::Path,
    sync::{Arc, Mutex},
};

/// A context that shows `name`, with the default stone and no custom materials.
fn ctx_with(name: &str) -> RenderContext {
    RenderContext {
        material_name: name.to_string(),
        ..RenderContext::default()
    }
}

/// A queued still of the stone `ctx` shows, Ray-Traced or Path-Traced, tiny and valid.
pub(super) fn job_for(ctx: RenderContext, ray: bool) -> RenderJobFile {
    let engine = if ray {
        EngineSpec {
            engine: RenderEngine::RayTraced,
            ray: RayOptions {
                aa_taps: 8,
                fire: true,
            },
        }
    } else {
        EngineSpec::default()
    };
    let mut scene = SceneSnapshot::capture_finished_for_engine(&Mutex::new(ctx), None, engine)
        .expect("the material resolves");
    scene.max_bounces = 4;
    let remote = RemoteSelection::local_only();
    let output = Path::new("job-test.png");
    convert::build_still_job(
        &StillJobInputs {
            scene: &scene,
            params: ExportParams {
                engine,
                width: 16,
                height: 16,
                samples_per_pixel: if ray { 8 } else { 2 },
                max_bounces: 4,
            },
            color_space: ColorSpace::Srgb,
            remote: &remote,
            output,
            label: "RB 57".to_string(),
            design: DesignInfo::default(),
            preset_label: String::new(),
            adaptive: None,
            path_cache: None,
            beam: None,
        },
        1_791_302_580,
    )
    .expect("the job builds")
}

/// A queued still of the built-in `material` in the default context.
pub(super) fn still_job(material: &str, ray: bool) -> RenderJobFile {
    job_for(ctx_with(material), ray)
}

/// The material the live capture gives the stone `ctx` shows.
fn live_material(ctx: RenderContext) -> GemMaterial {
    SceneSnapshot::capture(&Mutex::new(ctx))
        .expect("the material resolves")
        .material
}

/// `got` equals `want` apart from the absorption scale, which agrees to a part in a thousand
/// (the job reads its stone width back from a scale, which rounds).
fn same_but_scale(got: &GemMaterial, want: &GemMaterial) {
    let (g, w) = (got.absorption_path_scale, want.absorption_path_scale);
    assert!((g - w).abs() <= w.abs() * 1e-3, "scale {g} vs {w}");
    let mut got = got.clone();
    got.absorption_path_scale = w;
    assert_eq!(&got, want);
}

/// `material` without its zones, as a job's scene holds it.
fn without_zones(mut material: GemMaterial) -> GemMaterial {
    material.zoning = None;
    material
}

fn banded(name: &str) -> GemMaterial {
    GemMaterial::new_custom(name, 1.6, 0.02, 0.0, [0.0; 3])
        .with_body_color_bands(&[[550.0, 60.0, 0.3]], 1.0)
}

fn context_with_custom(name: &str, custom: Vec<GemMaterial>) -> RenderContext {
    RenderContext {
        custom_materials: Arc::new(custom),
        ..ctx_with(name)
    }
}

/// Applies `change` to `job` through the edit crate, which validates the result.
fn applied(job: &RenderJobFile, change: MaterialChange) -> RenderJobFile {
    let Ok(done) = material_edit(change).apply(job) else {
        panic!("the material change must validate");
    };
    done
}

#[test]
fn a_built_in_material_equals_the_live_capture_of_the_same_stone() {
    let job = still_job("Quartz", false);
    let change =
        resolve_job_material(&RenderContext::default(), &job, "sapphire").expect("resolves");
    assert_eq!(change.material.name, "Sapphire");
    same_but_scale(&change.material, &live_material(ctx_with("Sapphire")));
    assert!(change.zoning.is_none());
    assert!(change.fluorescence.is_empty());
}

#[test]
fn edge_rounding_is_kept_and_haze_is_reset() {
    let old = RenderContext {
        edge_rounding_radius: 0.02,
        inclusion_sigma_s: 0.5,
        stone_width_mm: 9.0,
        ..ctx_with("Quartz")
    };
    let job = job_for(old, false);
    let change =
        resolve_job_material(&RenderContext::default(), &job, "Sapphire").expect("resolves");
    let expected = live_material(RenderContext {
        edge_rounding_radius: 0.02,
        stone_width_mm: 9.0,
        ..ctx_with("Sapphire")
    });
    same_but_scale(&change.material, &expected);
    assert!((change.material.edge_rounding_radius - 0.02).abs() < 1e-6);
    assert!(
        change.material.scattering_sigma_s.abs() < 1e-6,
        "the haze is the new material's own"
    );
}

#[test]
fn a_custom_material_resolves_from_the_context() {
    let glassy = GemMaterial::new_custom("Glassy", 1.6, 0.02, 0.0, [0.1, 0.2, 0.3]);
    let ctx = context_with_custom("Glassy", vec![glassy]);
    let job = still_job("Quartz", false);
    let change = resolve_job_material(&ctx, &job, "Glassy").expect("resolves");
    assert_eq!(change.material.name, "Glassy");
    same_but_scale(&change.material, &live_material(ctx));
}

#[test]
fn a_missing_custom_material_is_refused_with_a_sentence() {
    let job = still_job("Quartz", false);
    let refused = resolve_job_material(&RenderContext::default(), &job, "Nowhere")
        .expect_err("no such material");
    assert!(refused.contains("Nowhere"), "{refused}");
}

#[test]
fn a_physics_custom_material_brings_its_fluorescence_and_others_do_not() {
    let emitters = Fluorescence::new(vec![FluorescentEmitter {
        excitation: Vec::new(),
        emission: vec![EmissionBand::new(694.0, 5.0, 1.0)],
        quantum_yield: 0.5,
    }]);
    let glow = || RenderContext {
        custom_material_physics: Arc::new(vec!["Glow".to_string()]),
        custom_material_fluorescence: Arc::new(vec![(
            "Glow".to_string(),
            Arc::new(emitters.clone()),
        )]),
        ..context_with_custom("Glow", vec![banded("Glow")])
    };
    let ctx = glow();
    let job = still_job("Quartz", false);
    let change = resolve_job_material(&ctx, &job, "Glow").expect("resolves");
    assert_eq!(change.fluorescence, emitters);
    assert_eq!(change.material.absorption_unit, AbsorptionUnit::PerMm);
    same_but_scale(&change.material, &live_material(glow()));
    let plain = resolve_job_material(&ctx, &job, "Sapphire").expect("resolves");
    assert!(plain.fluorescence.is_empty());
}

#[test]
fn the_stone_width_survives_a_change_of_absorption_unit() {
    let banded_at_12 = || RenderContext {
        stone_width_mm: 12.0,
        ..context_with_custom("Banded", vec![banded("Banded")])
    };
    let quartz_at_12 = || RenderContext {
        stone_width_mm: 12.0,
        ..ctx_with("Quartz")
    };
    let ctx = banded_at_12();
    let from_model_unit = job_for(quartz_at_12(), false);
    let to_per_mm = resolve_job_material(&ctx, &from_model_unit, "Banded").expect("resolves");
    assert_eq!(to_per_mm.material.absorption_unit, AbsorptionUnit::PerMm);
    same_but_scale(&to_per_mm.material, &live_material(banded_at_12()));

    let from_per_mm = job_for(banded_at_12(), false);
    let to_model_unit = resolve_job_material(&ctx, &from_per_mm, "Quartz").expect("resolves");
    assert_eq!(
        to_model_unit.material.absorption_unit,
        AbsorptionUnit::ModelUnit
    );
    same_but_scale(&to_model_unit.material, &live_material(quartz_at_12()));
}

#[test]
fn a_zoned_material_moves_its_zones_to_the_job_and_back() {
    let job = still_job("Quartz", false);
    let change = resolve_job_material(&RenderContext::default(), &job, "Ametrine").expect("zoned");
    assert!(change.zoning.is_some());
    assert!(change.material.zoning.is_none(), "the scene holds none");
    assert_eq!(change.material.absorption_unit, AbsorptionUnit::PerMm);
    let zones = change.zoning.clone();
    let zoned = applied(&job, change);
    assert_eq!(zoned.zoning, zones);
    assert_eq!(zoned.format, 5);

    let back = resolve_job_material(&RenderContext::default(), &zoned, "Quartz").expect("plain");
    assert!(back.zoning.is_none());
    same_but_scale(
        &back.material,
        &without_zones(live_material(ctx_with("Quartz"))),
    );
    let plain = applied(&zoned, back);
    assert!(plain.zoning.is_none());
    assert_eq!(plain.format, 1);
}

#[test]
fn a_ray_traced_job_gets_no_zones_and_untouched_effects() {
    let job = still_job("Quartz", true);
    let change = resolve_job_material(&RenderContext::default(), &job, "Ametrine").expect("zoned");
    assert!(change.zoning.is_none());
    assert!(change.material.zoning.is_none());
    assert!(change.material_inclusions.is_none());
    let done = applied(&job, change);
    assert!(done.zoning.is_none());
    assert_eq!(done.scene.effects, job.scene.effects);
    assert!(done.engine.is_ray());
}

#[test]
fn the_inclusion_record_follows_the_material_only_when_it_has_to() {
    let job = still_job("Quartz", false);
    let none = resolve_job_material(&RenderContext::default(), &job, "Sapphire").expect("ok");
    assert!(
        none.material_inclusions.is_none(),
        "nothing before or after"
    );

    let mut with_record = job;
    with_record.scene.effects.material_inclusions = Some(MaterialInclusions {
        families: Vec::new(),
        structure: None,
    });
    let cleared =
        resolve_job_material(&RenderContext::default(), &with_record, "Sapphire").expect("ok");
    assert_eq!(
        cleared.material_inclusions,
        Some(MaterialInclusions {
            families: Vec::new(),
            structure: None,
        }),
        "a record the new material does not have is cleared"
    );
}

#[test]
fn a_material_change_sets_the_four_material_fields_and_nothing_else() {
    let job = still_job("Quartz", false);
    let change = resolve_job_material(&RenderContext::default(), &job, "Sapphire").expect("ok");
    let edit = material_edit(change.clone());
    assert_eq!(edit.material, Some(change.material));
    assert_eq!(edit.zoning, Some(None));
    assert_eq!(edit.fluorescence, Some(Fluorescence::default()));
    assert_eq!(edit.effects_material_inclusions, None);
    assert!(edit.target.is_none() && edit.output.is_none() && edit.label.is_none());
}

#[test]
fn the_material_list_has_the_built_ins_and_the_custom_ones() {
    let plain = material_names(&RenderContext::default());
    assert!(plain.iter().any(|name| name == "Sapphire"));
    let ctx = context_with_custom("Glassy", vec![banded("Glassy")]);
    let names = material_names(&ctx);
    assert!(names.iter().any(|name| name == "Glassy"));
    assert_eq!(names.len(), plain.len() + 1);
}

#[test]
fn the_label_suffix_is_replaced_not_stacked() {
    let known = vec!["Quartz".to_string(), "Sapphire".to_string()];
    let suffix = |text: &str| with_material_suffix(text, "Sapphire", &known);
    assert_eq!(suffix("RB 57"), "RB 57 \u{b7} Sapphire");
    assert_eq!(suffix("RB 57 \u{b7} Quartz"), "RB 57 \u{b7} Sapphire");
    assert_eq!(suffix("RB 57 \u{b7} sapphire"), "RB 57 \u{b7} Sapphire");
    assert_eq!(suffix("RB 57 \u{b7} Sapphire"), "RB 57 \u{b7} Sapphire");
    assert_eq!(
        suffix("RB 57 \u{b7} Pink"),
        "RB 57 \u{b7} Pink \u{b7} Sapphire",
        "an unknown suffix stays"
    );
    assert_eq!(suffix(""), "Sapphire");
    assert_eq!(suffix(" \u{b7} Quartz"), "Sapphire");
    assert_eq!(suffix("Rubin \u{e9}"), "Rubin \u{e9} \u{b7} Sapphire");
}
