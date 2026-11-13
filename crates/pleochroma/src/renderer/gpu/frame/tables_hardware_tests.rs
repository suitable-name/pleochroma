//! Hardware tests of the optics lookup table kernels in the real renderers.
//!
//! Each test skips with a printed note when no adapter exists (`PLEOCHROMA_REQUIRE_GPU=1`
//! turns that into a failure). The table variant of a kernel is selected by
//! [`material_class::OPTICS_TABLES`] in the pipeline class. These tests render the same small
//! frame through the plain and the table pipelines of [`GpuFrameRenderer`] and of the event
//! wavefront renderer, check that the renderer reports which variant ran, and check that the
//! two frames agree within the accuracy of the tables (not bit-identical: the tables
//! interpolate). `optics::tables::gpu_tests` holds the per-function twins of the lookups.

use glam::Vec3;

use super::{GpuFrameRenderer, GpuFrameScene, classify_material_with_tables, material_class};
use crate::{
    geometry::cuts::StandardGemCuts,
    optics::{
        materials::GemMaterial,
        raytracer::{Camera, LightingPreset},
        tables::OpticsTables,
    },
    renderer::gpu::{
        GpuContext,
        event_wavefront::{EventWavefrontRenderer, compare::compare_frames},
        merge_tests::renderer_or_skip,
    },
};

const FRAME: (u32, u32) = (32, 28);
const SPP: u32 = 8;

/// The relative difference of the summed luminance of two frames that the tables may cause.
///
/// The tables hold the Fresnel amplitudes to 2e-5 and the exponential to 2.5e-5. A path whose
/// reflect/transmit draw lies within that of the threshold takes the other branch, so single
/// pixels may move, but the frame total moves by a small fraction of a percent.
const FRAME_TOTAL_TOLERANCE: f32 = 0.02;

fn total_luminance(frame: &[Vec3]) -> f32 {
    frame.iter().map(|pixel| pixel.y).sum()
}

fn assert_frames_agree(plain: &[Vec3], tabled: &[Vec3]) {
    let diff = compare_frames(plain, tabled, SPP);
    assert_eq!(diff.non_finite_pixels, 0, "{diff:?}");
    let (a, b) = (total_luminance(plain), total_luminance(tabled));
    assert!(a > 0.0, "the probe must see light");
    assert!(
        (a - b).abs() <= FRAME_TOTAL_TOLERANCE * a,
        "frame totals {a} (plain) and {b} (tables) differ by more than {FRAME_TOTAL_TOLERANCE}: {diff:?}"
    );
}

fn render_megakernel(renderer: &mut GpuFrameRenderer, tables: OpticsTables) -> Vec<Vec3> {
    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let scene = GpuFrameScene {
        camera: &camera,
        width: FRAME.0,
        height: FRAME.1,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 6,
        environment: LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
    };
    let class = classify_material_with_tables(&material, tables);
    let mut accum = vec![Vec3::ZERO; (FRAME.0 * FRAME.1) as usize];
    renderer
        .accumulate_via_pipeline(&scene, class, 0, SPP, &mut accum, None)
        .expect("the frame renders");
    accum
}

#[test]
fn the_megakernel_reports_the_variant_it_ran_and_agrees_with_the_plain_one() {
    let Some(mut renderer) =
        renderer_or_skip("the_megakernel_reports_the_variant_it_ran_and_agrees_with_the_plain_one")
    else {
        return;
    };
    assert!(!renderer.optics_tables_in_use(), "nothing prepared yet");

    let plain = render_megakernel(&mut renderer, OpticsTables::Off);
    assert!(
        !renderer.optics_tables_in_use(),
        "the plain class ran plain"
    );

    let tabled = render_megakernel(&mut renderer, OpticsTables::On);
    assert!(
        renderer.optics_tables_in_use(),
        "the table class must run the table kernel"
    );
    let material = GemMaterial::diamond();
    let plain_class = classify_material_with_tables(&material, OpticsTables::Off);
    let table_class = classify_material_with_tables(&material, OpticsTables::On);
    assert_eq!(table_class, plain_class | material_class::OPTICS_TABLES);
    assert!(
        renderer.pipelines_specialised.contains_key(&plain_class)
            && renderer.pipelines_specialised.contains_key(&table_class),
        "both variants hold their own pipeline"
    );
    assert_frames_agree(&plain, &tabled);

    let again = render_megakernel(&mut renderer, OpticsTables::Off);
    assert!(
        !renderer.optics_tables_in_use(),
        "going back to the plain class runs plain again"
    );
    assert_eq!(
        compare_frames(&plain, &again, SPP).differing_pixels,
        0,
        "the plain class renders the same frame before and after a table frame"
    );
}

#[test]
fn the_event_wavefront_reports_the_variant_it_ran_and_agrees_with_the_plain_one() {
    let name = "the_event_wavefront_reports_the_variant_it_ran_and_agrees_with_the_plain_one";
    let ctx = match GpuContext::acquire() {
        Ok(ctx) => ctx,
        Err(error) => {
            assert!(
                !std::env::var("PLEOCHROMA_REQUIRE_GPU").is_ok_and(|value| value == "1"),
                "{name}: PLEOCHROMA_REQUIRE_GPU=1 was set, but GPU acquisition failed: {error}"
            );
            println!("skipping {name}: no GPU adapter ({error})");
            return;
        }
    };
    let mut renderer = EventWavefrontRenderer::new(ctx);
    let camera = Camera::new(0.35, 0.28, 5.0, 18.0);
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let scene = GpuFrameScene {
        camera: &camera,
        width: FRAME.0,
        height: FRAME.1,
        planes: &planes,
        facet_finishes: &[],
        material: &material,
        max_bounces: 6,
        environment: LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
    };
    let mut render = |tables: OpticsTables| -> Vec<Vec3> {
        let class = classify_material_with_tables(&material, tables);
        let mut accum = vec![Vec3::ZERO; (FRAME.0 * FRAME.1) as usize];
        pollster::block_on(renderer.accumulate_via_class(&scene, class, 0, SPP, &mut accum))
            .expect("the frame renders");
        accum
    };
    let plain = render(OpticsTables::Off);
    let tabled = render(OpticsTables::On);
    assert_frames_agree(&plain, &tabled);
    assert!(renderer.optics_tables_in_use());
    let compiled: Vec<_> = renderer.compiled_pipelines().collect();
    assert!(
        compiled
            .iter()
            .any(|(_, class)| material_class::uses_optics_tables(*class))
            && compiled
                .iter()
                .any(|(_, class)| !material_class::uses_optics_tables(*class)),
        "both variants hold their own pipelines: {compiled:?}"
    );
}
