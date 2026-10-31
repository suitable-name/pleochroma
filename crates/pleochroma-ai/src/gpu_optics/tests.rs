//! Native CUDA source-parity fixtures. Defined for the user's later GPU verification;
//! these are deliberately ignored by default and have not been executed in this change.
use super::{CudaOpticalScorer, OpticalError};
use glam::Vec3;
use pleochroma::{
    color::metrics::{FaceUpTone, FinishedStone},
    geometry::{
        cuts::StandardGemCuts,
        tool::{StoneGeometry, ToolPrimitive, ToolSweep},
    },
    optics::{
        materials::GemMaterial,
        raytracer::{FacetFinish, LightingPreset},
    },
};
use pleochroma_cut_core::optimize::{
    ObjectiveFidelity, evaluate_objective_with_tone_finished_under_threaded,
};
use std::sync::atomic::AtomicBool;

fn compare(
    scorer: &CudaOpticalScorer,
    stone: FinishedStone<'_>,
    material: &GemMaterial,
    fidelity: ObjectiveFidelity,
    preset: LightingPreset,
) {
    let cuda = scorer
        .evaluate(stone, material, fidelity, preset, &AtomicBool::new(false))
        .unwrap_or_else(|e| panic!("{e}"));
    let (cpu, tone) = evaluate_objective_with_tone_finished_under_threaded(
        stone,
        material,
        fidelity,
        preset,
        Some(1),
    );
    // Counts are discrete decisions: no percentage tolerance can certify their parity.
    assert_eq!(
        cuda.components.windowing_pct.to_bits(),
        cpu.windowing_pct.to_bits(),
        "windowing: {} {preset:?}",
        material.name
    );
    assert_eq!(
        cuda.components.extinction_pct.to_bits(),
        cpu.extinction_pct.to_bits(),
        "extinction: {} {preset:?}",
        material.name
    );
    assert_eq!(
        cuda.components.tilt_brilliance_pct.to_bits(),
        cpu.tilt_brilliance_pct.to_bits(),
        "brilliance: {} {preset:?}",
        material.name
    );
    compare_tone(cuda.tone, tone);
}

fn compare_tone(cuda: FaceUpTone, cpu: FaceUpTone) {
    // Regression budgets, not universal numerical bounds or label certification:
    // 0.01 Lab units is 1/100 of a typical just-noticeable colour difference;
    // a one-code sRGB allowance covers only final 8-bit quantization. Geometry paths
    // retain a 1e-5 relative/absolute budget. Ray-count fields above and below stay exact.
    assert!(
        (cuda.l_star - cpu.l_star).abs() <= 0.01,
        "L*: {cuda:?} vs {cpu:?}"
    );
    assert!(
        (cuda.chroma - cpu.chroma).abs() <= 0.01,
        "chroma: {cuda:?} vs {cpu:?}"
    );
    let hue = (cuda.hue_deg - cpu.hue_deg).abs();
    let hue = hue.min(360.0 - hue);
    assert!(hue <= 0.05, "circular hue: {cuda:?} vs {cpu:?}");
    for (gpu, reference) in cuda.srgb.into_iter().zip(cpu.srgb) {
        assert!(gpu.abs_diff(reference) <= 1, "sRGB: {cuda:?} vs {cpu:?}");
    }
    let path_budget = 1e-5 * cpu.mean_path_units.abs().max(1.0);
    assert!(
        (cuda.mean_path_units - cpu.mean_path_units).abs() <= path_budget,
        "path: {cuda:?} vs {cpu:?}"
    );
    assert_eq!(
        cuda.returned_fraction.to_bits(),
        cpu.returned_fraction.to_bits()
    );
}

#[test]
#[ignore = "requires native CUDA/NVRTC; discrete source parity and continuous regression budgets, not executed during implementation"]
fn native_cuda_all_materials_lights_and_supported_shapes_match_cpu_bits() {
    let scorer = CudaOpticalScorer::new(0, 128).unwrap_or_else(|e| panic!("{e}"));
    let materials = [
        GemMaterial::diamond(),
        GemMaterial::sapphire(),
        GemMaterial::by_name("Quartz").unwrap_or_else(|| panic!("Quartz fixture")),
        GemMaterial::by_name("Topaz").unwrap_or_else(|| panic!("Topaz fixture")),
        GemMaterial::by_name("Tanzanite").unwrap_or_else(|| panic!("Tanzanite fixture")),
    ];
    for planes in [
        StandardGemCuts::standard_round_brilliant(),
        StandardGemCuts::emerald_cut(),
    ] {
        for material in &materials {
            for preset in LightingPreset::ALL {
                compare(
                    &scorer,
                    FinishedStone::polished(StoneGeometry::planes_only(&planes)),
                    material,
                    ObjectiveFidelity::Fast,
                    preset,
                );
            }
        }
    }
}

#[test]
#[ignore = "requires native CUDA/NVRTC; strict CSG/sweep/frost/principal path source-parity audit"]
fn native_cuda_tools_sweeps_frost_and_principal_absorption_match_cpu_bits() {
    let scorer = CudaOpticalScorer::new(0, 32).unwrap_or_else(|e| panic!("{e}"));
    let planes = StandardGemCuts::standard_round_brilliant();
    let materials = [
        GemMaterial::diamond().with_body_color([0.2, 0.1, 0.05]),
        GemMaterial::sapphire(),
        GemMaterial::by_name("Topaz").unwrap_or_else(|| panic!("Topaz fixture")),
    ];
    for base in [
        ToolPrimitive::ball(Vec3::new(0.2, 0.3, 0.0), 0.2),
        ToolPrimitive::cylinder(Vec3::new(0.0, 0.3, 0.0), Vec3::Z, 0.15, 0.4),
        ToolPrimitive::frustum(Vec3::new(0.1, 0.3, 0.0), Vec3::Z, 0.1, 0.2, 0.3),
        ToolPrimitive::bicone(Vec3::new(0.0, 0.3, 0.1), Vec3::Z, 0.2, 0.4),
    ] {
        for sweep in [ToolSweep::None, ToolSweep::AlongAxis, ToolSweep::AcrossAxis] {
            let tools = [base.with_sweep(sweep, 0.1, Vec3::X)];
            for frost in [false, true] {
                let finishes = vec![
                    if frost {
                        FacetFinish::Frosted
                    } else {
                        FacetFinish::Polished
                    };
                    planes.len() + 1
                ];
                for material in &materials {
                    let stone = FinishedStone::new(
                        StoneGeometry {
                            planes: &planes,
                            tools: &tools,
                        },
                        &finishes,
                    );
                    compare(
                        &scorer,
                        stone,
                        material,
                        ObjectiveFidelity::Fast,
                        LightingPreset::RingLights,
                    );
                }
            }
        }
    }
}

#[test]
#[ignore = "requires native CUDA/NVRTC; 724 pose ordering and bounded tile parity"]
fn native_cuda_full_sweep_matches_cpu_with_different_tile_sizes() {
    let planes = StandardGemCuts::emerald_cut();
    let material = GemMaterial::sapphire();
    for tile in [1, 17, 128] {
        let scorer = CudaOpticalScorer::new(0, tile).unwrap_or_else(|e| panic!("{e}"));
        compare(
            &scorer,
            FinishedStone::polished(StoneGeometry::planes_only(&planes)),
            &material,
            ObjectiveFidelity::Full,
            LightingPreset::LightTent,
        );
    }
}

#[test]
#[ignore = "requires native CUDA/NVRTC; request isolation and cancellation audit"]
fn native_cuda_concurrent_candidates_keep_order_and_cancellation() {
    let scorer =
        std::sync::Arc::new(CudaOpticalScorer::new(0, 128).unwrap_or_else(|e| panic!("{e}")));
    std::thread::scope(|scope| {
        for index in 0..32 {
            let scorer = std::sync::Arc::clone(&scorer);
            scope.spawn(move || {
                let planes = if index % 2 == 0 {
                    StandardGemCuts::emerald_cut()
                } else {
                    StandardGemCuts::standard_round_brilliant()
                };
                let material = if index % 3 == 0 {
                    GemMaterial::sapphire()
                } else {
                    GemMaterial::diamond()
                };
                compare(
                    &scorer,
                    FinishedStone::polished(StoneGeometry::planes_only(&planes)),
                    &material,
                    ObjectiveFidelity::Fast,
                    LightingPreset::RingLights,
                );
            });
        }
    });
    let cancelled = AtomicBool::new(true);
    let planes = StandardGemCuts::emerald_cut();
    let result = scorer.evaluate(
        FinishedStone::polished(StoneGeometry::planes_only(&planes)),
        &GemMaterial::diamond(),
        ObjectiveFidelity::Fast,
        LightingPreset::RingLights,
        &cancelled,
    );
    assert!(matches!(result, Err(OpticalError::Cancelled)));
}
