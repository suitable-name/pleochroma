//! Measures a tilt video's metric curves from the job file alone, off the UI thread.
//!
//! The curves come from the job's own planes, tools, frosting and material, never from the stone
//! on screen. They run through the same sweep as the Tilt Performance chart
//! (`tilt_profile::sweep_all_axes`), under the same fixed tilt standard, so a measured curve
//! equals the chart's for the same stone whatever lighting the job renders with.

use crate::{
    MainWindow,
    gui::{
        render_jobs::convert,
        tilt::tilt_profile::{AxisCurveRows, sweep_all_axes},
    },
};
use pleochroma::{
    color::metrics::{FinishedStone, PROFILE_AZIMUTHS_DEG},
    geometry::tool::StoneGeometry,
};
use pleochroma_render_jobs::{RenderJobFile, job::MetricCurvesData};
use slint::ComponentHandle as _;

/// Picks the curves of one axis out of the three row sets of `sweep_all_axes`.
///
/// The sweep's tuple is `(brilliance, extinction, windowing)`; `MetricCurvesData` names its
/// fields `{brilliance, windowing, extinction}`, so each row is taken by name.
fn curves_of_axis(
    brilliance: &AxisCurveRows,
    extinction: &AxisCurveRows,
    windowing: &AxisCurveRows,
    axis: usize,
) -> MetricCurvesData {
    MetricCurvesData {
        brilliance: brilliance[axis].to_vec(),
        windowing: windowing[axis].to_vec(),
        extinction: extinction[axis].to_vec(),
    }
}

/// Measures the three curves of every axis in `axes` for the stone and material frozen in `job`.
///
/// Runs on a worker thread and takes about a second and a half (all four axes are swept, as the
/// chart does). The result has one entry per axis, in the order of `axes`: one value per degree
/// from -90 to 90. An axis number of 4 or more is an `Err` with a plain sentence.
pub(in crate::gui) fn measure_job_curves(
    job: &RenderJobFile,
    axes: &[u32],
) -> Result<Vec<MetricCurvesData>, String> {
    let axis_count = PROFILE_AZIMUTHS_DEG.len();
    let mut picked = Vec::with_capacity(axes.len());
    for &axis in axes {
        let index = usize::try_from(axis)
            .ok()
            .filter(|index| *index < axis_count)
            .ok_or_else(|| {
                format!(
                    "The tilt video has no axis {axis}; the axes are 0 to {}.",
                    axis_count - 1
                )
            })?;
        picked.push(index);
    }
    // The curves are scored under the tilt standard, so the job's lighting and HDR map are not
    // read: a job whose map is missing still measures.
    let snapshot = convert::snapshot_from_job(job, None);
    let stone = FinishedStone::new(
        StoneGeometry {
            planes: &snapshot.active_planes,
            tools: &snapshot.tools,
        },
        &snapshot.facet_finishes,
    );
    let (brilliance, extinction, windowing) = sweep_all_axes(stone, &snapshot.material);
    Ok(picked
        .into_iter()
        .map(|axis| curves_of_axis(&brilliance, &extinction, &windowing, axis))
        .collect())
}

/// Measures the curves of `job` on a worker thread and hands the result to `done` on the UI thread.
///
/// `done` runs once. It is dropped without a call when the window is gone before the sweep ends.
pub(in crate::gui) fn spawn_measure(
    ui: &MainWindow,
    job: RenderJobFile,
    axes: Vec<u32>,
    done: impl FnOnce(&MainWindow, Result<Vec<MetricCurvesData>, String>) + Send + 'static,
) {
    let weak = ui.as_weak();
    std::thread::spawn(move || {
        let result = measure_job_curves(&job, &axes);
        let _ = weak.upgrade_in_event_loop(move |ui| done(&ui, result));
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        bridge::{
            export_thread::{ExportParams, RemoteSelection, SceneSnapshot},
            render_thread::RenderContext,
        },
        gui::render_jobs::convert::StillJobInputs,
    };
    use pleochroma::color::ColorSpace;
    use pleochroma_render_jobs::job::DesignInfo;
    use std::{path::Path, sync::Mutex};

    /// A small still job of the default stone; the curves ignore the job's kind and size.
    fn small_job() -> RenderJobFile {
        let scene = SceneSnapshot::capture(&Mutex::new(RenderContext::default()))
            .expect("the material resolves");
        let remote = RemoteSelection::local_only();
        let inputs = StillJobInputs {
            scene: &scene,
            params: ExportParams {
                engine: pleochroma::renderer::engine::EngineSpec::default(),
                width: 16,
                height: 16,
                samples_per_pixel: 1,
                max_bounces: 4,
            },
            color_space: ColorSpace::Srgb,
            remote: &remote,
            output: Path::new("curves-job.png"),
            label: "Curves job".to_string(),
            design: DesignInfo::default(),
            preset_label: String::new(),
            adaptive: None,
            path_cache: None,
            beam: None,
        };
        convert::build_still_job(&inputs, 1_791_302_580).expect("the job builds")
    }

    #[test]
    fn curves_follow_the_axes_and_the_chart_sweep() {
        let job = small_job();
        let curves = measure_job_curves(&job, &[2, 0]).expect("the curves measure");
        assert_eq!(curves.len(), 2);
        for curve in &curves {
            assert_eq!(curve.brilliance.len(), 181);
            assert_eq!(curve.windowing.len(), 181);
            assert_eq!(curve.extinction.len(), 181);
        }

        let snapshot = convert::snapshot_from_job(&job, None);
        let stone = FinishedStone::new(
            StoneGeometry {
                planes: &snapshot.active_planes,
                tools: &snapshot.tools,
            },
            &snapshot.facet_finishes,
        );
        let (brilliance, extinction, windowing) = sweep_all_axes(stone, &snapshot.material);
        for (curve, axis) in curves.iter().zip([2_usize, 0]) {
            assert_eq!(curve.brilliance, brilliance[axis].to_vec());
            assert_eq!(curve.windowing, windowing[axis].to_vec());
            assert_eq!(curve.extinction, extinction[axis].to_vec());
        }
    }

    #[test]
    fn an_axis_past_the_fourth_is_an_error() {
        let job = small_job();
        let message = measure_job_curves(&job, &[0, 4]).expect_err("axis 4 does not exist");
        assert!(message.contains("axis 4"), "{message}");
    }
}
