//! Rendering a job file without a window: the entry point `pleochroma-cli render` and
//! `pleochroma-cli tilt-video` call.
//!
//! It runs the very same executor the desktop render queue runs, so a job rendered from a
//! script produces the same picture as one rendered in the app, and reuses the app's
//! remote-worker client (probe, full-data lanes, final picture, HDR map upload). Nothing
//! here creates a window or touches Slint.

use super::{
    convert::local_target_of,
    execute::{ExecContext, execute_job_catching},
};
use crate::{
    gui::color_pattern::{own_color, solve_pattern_colors, zone_variants},
    settings::WorkerSettings,
};
use pleochroma::{
    geometry::GpuFacetPlane,
    optics::materials::{GemMaterial, variants::host_rank},
    render_setup::{
        MaterialOverrides, apply_material_overrides, measure_model_width, stone_width_for_scale,
    },
};
use pleochroma_cut_core::material::pattern::{
    apply_color_pattern, typical_patterns, wants_axis_through_table,
};
use pleochroma_editor::lch_color::{axis_lch_from_tensor, reference_path_mm};
use pleochroma_render_jobs::{
    ComputeChoice, JobOutcome, JobSink, LocalEngines, RenderJobFile, TransferChoice,
};
use std::{
    path::{Path, PathBuf},
    sync::atomic::AtomicBool,
};

/// A remote render worker: its address and the folder holding its certificate bundle
/// (`ca.pem`, `client.pem` and `client.key`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEndpoint {
    /// `host:port` the worker listens on.
    pub address: String,
    /// The folder with the mutual-TLS certificate bundle.
    pub cert_dir: PathBuf,
}

/// How this computer renders a job: the machine's own choices and the command line's
/// overrides of what the job froze.
#[derive(Debug, Clone, Default)]
pub struct HeadlessOptions {
    /// Which engines of this computer render. Without the `gpu` feature every choice
    /// renders on the processor ([`gpu_compiled_in`]).
    pub local: LocalEngines,
    /// The remote worker, when one is to be used.
    pub remote: Option<RemoteEndpoint>,
    /// Replaces the job's compute target.
    pub compute: Option<ComputeChoice>,
    /// Replaces the job's transfer choice.
    pub transfer: Option<TransferChoice>,
    /// Replaces the job's "this computer renders a share too" choice.
    pub contribute_local: Option<bool>,
    /// A still: the PNG to write. A video: the frame folder. Replaces the job's own.
    pub output: Option<PathBuf>,
    /// A tilt video: delete its frames first and start at frame one.
    pub restart_frames: bool,
}

/// Renders `job` and reports how it ended.
///
/// `job_dir` is the folder of the job file: relative paths in the job (the output, the HDR
/// map) resolve against it. Setting `cancel` stops the render at the next sample batch (a
/// picture) or the next frame (a video); a video keeps its finished frames and resumes
/// from them the next time. `sink` receives the progress. A panic inside the render is
/// reported as a failed outcome, never unwound. The remote connections the job parked for
/// reuse are closed cleanly before this returns: the caller is usually a process about to
/// exit.
pub fn run_job(
    job: &RenderJobFile,
    job_dir: &Path,
    options: &HeadlessOptions,
    cancel: &AtomicBool,
    sink: &mut dyn JobSink,
) -> JobOutcome {
    let worker = options.remote.as_ref().map(|remote| WorkerSettings {
        address: remote.address.clone(),
        cert_dir: remote.cert_dir.display().to_string(),
        ..WorkerSettings::default()
    });
    let ctx = ExecContext {
        base_dir: job_dir.to_path_buf(),
        local_compute: local_target_of(options.local),
        worker,
        compute_override: options.compute,
        transfer_override: options.transfer,
        contribute_local_override: options.contribute_local,
        output_override: options.output.clone(),
        restart_frames: options.restart_frames,
    };
    let outcome = execute_job_catching(job, &ctx, cancel, sink);
    crate::bridge::remote::remote_render::close_idle_connections();
    outcome
}

/// Whether this build can render on the graphics card (the `gpu` feature). Without it,
/// every `--local` choice renders on the processor.
#[must_use]
pub const fn gpu_compiled_in() -> bool {
    cfg!(feature = "gpu")
}

/// The labels of the typical color patterns of the material called `material_name`, in menu
/// order; empty for a material without any.
#[must_use]
pub fn typical_pattern_labels(material_name: &str) -> Vec<&'static str> {
    typical_patterns(material_name)
        .iter()
        .map(|entry| entry.label)
        .collect()
}

/// A job's stone (`material`, already sized for the stone; `planes`, its facets) in one of its
/// material's typical color patterns, as the Color Pattern window builds and solves it.
///
/// The pattern is built on the built-in material's own color and its built-in variants at the
/// stone's width, its colors are solved at the window's light path, and it is applied to the
/// bare built-in material, which is then sized for the stone as the app sizes a design's
/// material. A Watermelon or Reverse Watermelon also turns the Crystal Axis through the table, as
/// the app does when it applies one. `label` names the pattern in any letter case, with or without
/// its spaces (`Hexagonal Banding`, `hexagonal-banding`).
///
/// # Errors
///
/// A message when the material is not a built-in one, has no pattern called `label` (naming
/// its patterns), the stone's width cannot be measured, or a color cannot be solved.
pub fn material_with_typical_pattern(
    material: &GemMaterial,
    planes: &[GpuFacetPlane],
    label: &str,
) -> Result<GemMaterial, String> {
    let squeeze = |text: &str| {
        text.chars()
            .filter(char::is_ascii_alphanumeric)
            .collect::<String>()
            .to_ascii_lowercase()
    };
    let name = &material.name;
    let labels = typical_pattern_labels(name);
    if labels.is_empty() {
        return Err(format!("{name} has no typical color patterns"));
    }
    let wanted = squeeze(label);
    let entry = typical_patterns(name)
        .iter()
        .find(|entry| squeeze(entry.label) == wanted)
        .ok_or_else(|| {
            format!(
                "{name} has no color pattern {label:?}; its patterns are {}",
                labels.join(", ")
            )
        })?;
    let bare = GemMaterial::built_in()
        .iter()
        .find(|built_in| built_in.name.eq_ignore_ascii_case(name))
        .cloned()
        .ok_or_else(|| format!("{name} is not a built-in material"))?;
    let model_width = measure_model_width(planes);
    let width = stone_width_for_scale(
        material.absorption_unit,
        material.absorption_path_scale,
        model_width,
    )
    .ok_or_else(|| format!("cannot size {name} for the stone: its width cannot be measured"))?;
    let width_mm = f64::from(width);
    let path_mm = reference_path_mm(Some(width_mm));
    let variants = zone_variants(&bare, None);
    let seed = axis_lch_from_tensor(
        &bare.absorption,
        bare.absorption_unit,
        host_rank(&bare),
        width_mm,
        path_mm,
    )
    .first()
    .copied()
    .unwrap_or([90.0, 0.0, 0.0]);
    let own = own_color(&bare, None, &variants, seed, width_mm);
    let pattern = solve_pattern_colors(
        entry.for_stone_on(&own, &variants),
        path_mm,
        &AtomicBool::new(false),
    )
    .ok_or_else(|| format!("a color of {name}'s {} could not be solved", entry.label))?;
    let patterned =
        apply_color_pattern(bare, &pattern, width_mm).map_err(|error| error.to_string())?;
    // A watermelon is cut as a slice across c: its rings show face up only with the Crystal Axis
    // through the table, which the app sets when it applies one (see `wants_axis_through_table`).
    let overrides = MaterialOverrides {
        inclusion_sigma_s: 0.0,
        c_axis_override: wants_axis_through_table(&pattern).then_some(glam::Vec3::Y),
        alpha_axis_override: None,
        edge_rounding_radius: 0.0,
        stone_width_mm: width,
    };
    Ok(apply_material_overrides(patterned, &overrides, model_width))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma::{
        geometry::cuts::StandardGemCuts, optics::materials::AbsorptionUnit,
        render_setup::material_for_stone,
    };

    #[test]
    fn a_typical_pattern_zones_the_stone_at_its_width_and_an_unknown_one_names_the_others() {
        let planes = StandardGemCuts::standard_round_brilliant();
        let sapphire = material_for_stone(GemMaterial::sapphire(), 9.0, &planes);
        let banded = material_with_typical_pattern(&sapphire, &planes, "hexagonal-banding")
            .expect("Sapphire has Hexagonal Banding");
        assert!(banded.zoning.is_some());
        assert_eq!(banded.absorption_unit, AbsorptionUnit::PerMm);
        // A per-millimetre material is sized by the stone width over the design's own width.
        let expected = 9.0 / measure_model_width(&planes).expect("a measurable width");
        let scale = f64::from(banded.absorption_path_scale);
        assert!(
            (scale - expected).abs() < 1e-4 * expected,
            "{scale} against {expected}"
        );

        let error = material_with_typical_pattern(&sapphire, &planes, "Zebra").expect_err("none");
        assert!(error.contains("Parti Color"), "{error}");
        let diamond = material_for_stone(GemMaterial::diamond(), 9.0, &planes);
        let error = material_with_typical_pattern(&diamond, &planes, "Split").expect_err("none");
        assert!(error.contains("no typical color patterns"), "{error}");
        assert!(typical_pattern_labels("Diamond").is_empty());
    }
}
