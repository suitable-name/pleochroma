//! Focused transport regressions for the Windows AMD and Linux RADV failures.
//! Every group runs even if an earlier group fails; no tolerance is relaxed.

use pleochroma::renderer::gpu::{GpuContext, estimator_check};

use crate::{
    common::{report_furnace_components, report_image_comparison_material},
    event_wavefront_checks::{run_event_wavefront_cpu_checks, run_event_wavefront_pipeline_checks},
    lighting_and_pipeline_checks::run_wavefront_pipeline_checks,
    nee_and_g7_checks::run_finding_g7_nee_checks,
    summary,
};

pub fn run(ctx: &GpuContext) -> bool {
    let groups = [
        ("hdr_nee", run_finding_g7_nee_checks(ctx)),
        ("scattering_without_nee", run_scattering_control(ctx)),
        ("extraordinary_dispersion_cpu", run_dispersion_controls(ctx)),
        (
            "zoned_oriented_rim",
            report_image_comparison_material(
                "zoned dichroic rim, Star Silk",
                &estimator_check::run_image_comparison_star_zoned_rim(ctx),
            ),
        ),
        ("event_wavefront", run_event_wavefront_pipeline_checks(ctx)),
        (
            "event_wavefront_cpu_reference",
            run_event_wavefront_cpu_checks(ctx),
        ),
        ("wavefront_pipeline", run_wavefront_pipeline_checks()),
    ];
    for (name, passed) in groups {
        summary::record_group(name, passed);
    }
    groups.iter().all(|&(_, passed)| passed)
}

fn run_scattering_control(ctx: &GpuContext) -> bool {
    let result = estimator_check::run_furnace_scattering(ctx);
    let passed = result.passed_scattering();
    println!(
        "[Furnace, scattering without NEE] {}: target={:?}, CPU={:?}, GPU={:?}, z={:?}",
        if passed { "PASS" } else { "FAIL" },
        result.analytic_target.to_array(),
        result.cpu_mean.to_array(),
        result.gpu_mean.to_array(),
        result.cpu_gpu_z,
    );
    report_furnace_components("scattering without NEE", &result, passed);
    passed
}

fn run_dispersion_controls(ctx: &GpuContext) -> bool {
    let quartz = report_image_comparison_material(
        "Quartz, genuine extraordinary curve",
        &estimator_check::run_image_comparison_quartz(ctx),
    );
    let rutile = report_image_comparison_material(
        "Rutile, genuine extraordinary curve",
        &estimator_check::run_image_comparison_rutile(ctx),
    );
    quartz && rutile
}
