//! The output defaults of the Render & Output tab: seeding the Export and Tilt Video dialogs
//! from `AppSettings::output` and writing the values a run used back ("last used").
//!
//! The dialogs keep their own controls (`ExportModel`, `TiltVideoExportModel`); those are
//! per-run overrides seeded from the persisted values. Seeding happens when the still export
//! dialog opens and at start-up for the tilt-video section (its model persists for the
//! session); remembering happens when a run starts or is added to the queue, and when the
//! dialog closes, so values typed without exporting are kept too. Every value read from the settings file
//! goes through `OutputSettings::sanitised`, so a hand-edited file cannot seed an impossible
//! slider position.

use crate::{
    ExportModel, MainWindow, SettingsModel, TiltVideoExportModel,
    gui::tilt::video_export::params,
    settings::{
        ExportTransfer, SettingsPersister, TiltVideoCompute,
        model::{
            AppSettings, EXPORT_SAMPLES_EXPONENT_RANGE, OutputSettings, StillOutput,
            TILT_STEP_HUNDREDTHS_RANGE, TiltVideoOutput, bounce_index, clamp_exponent,
            nearest_bounce_cap,
        },
    },
};
use pleochroma::color::metrics::PROFILE_AZIMUTHS_DEG;
use slint::{ComponentHandle, Model as _, ModelRc, VecModel};

/// The remote endpoint's own default transfer (what an export follows until a choice was made).
fn endpoint_transfer(settings: &AppSettings) -> ExportTransfer {
    settings
        .remote
        .as_ref()
        .map_or(ExportTransfer::FullData, |remote| remote.export_transfer)
}

fn as_i32(value: u32) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

fn as_u32(value: i32) -> u32 {
    u32::try_from(value).unwrap_or(0)
}

/// The rounded slider exponent as a settings value, inside the export range.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "value is rounded and floored at 0 first; a sample exponent is a small slider value; try_from is not const"
)]
const fn exponent_of(raw: f32) -> u32 {
    clamp_exponent(raw.round().max(0.0) as u32, EXPORT_SAMPLES_EXPONENT_RANGE)
}

/// The step field's degrees as whole hundredths, inside [`TILT_STEP_HUNDREDTHS_RANGE`]; a value
/// that is not a number reads as 1 degree.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "rounded and clamped to the small positive step range first"
)]
fn step_hundredths_of(step_deg: f32) -> u32 {
    let hundredths = (step_deg * 100.0).round();
    if hundredths.is_finite() {
        hundredths.clamp(
            TILT_STEP_HUNDREDTHS_RANGE.0 as f32,
            TILT_STEP_HUNDREDTHS_RANGE.1 as f32,
        ) as u32
    } else {
        100
    }
}

/// The section's `video_axes` for the remembered `axes`: the chart-axis indices in the order the
/// video plays them, each once; empty when no axis was chosen, which the section reads as "the
/// axis selected in the chart".
fn axis_list(axes: &[u8]) -> Vec<i32> {
    let mut list: Vec<i32> = Vec::new();
    for &axis in axes {
        if usize::from(axis) < PROFILE_AZIMUTHS_DEG.len() && !list.contains(&i32::from(axis)) {
            list.push(i32::from(axis));
        }
    }
    list
}

/// The chosen axes of the section's chips, in the order the video plays them.
fn axes_of(list: &[i32]) -> Vec<u8> {
    let mut axes: Vec<u8> = Vec::new();
    for &axis in list {
        if let Ok(axis) = u8::try_from(axis)
            && usize::from(axis) < PROFILE_AZIMUTHS_DEG.len()
            && !axes.contains(&axis)
        {
            axes.push(axis);
        }
    }
    axes
}

const COLOR_SPACES: [&str; 3] = ["sRGB", "Display P3", "Rec.2020"];

fn color_space_name(index: i32) -> &'static str {
    usize::try_from(index)
        .ok()
        .and_then(|i| COLOR_SPACES.get(i))
        .copied()
        .unwrap_or(COLOR_SPACES[0])
}

const fn compute_name(compute: TiltVideoCompute) -> &'static str {
    match compute {
        TiltVideoCompute::Local => "local only",
        TiltVideoCompute::Remote => "remote only",
        TiltVideoCompute::Both => "local + remote",
    }
}

/// One line describing the still export's remembered values.
#[must_use]
pub(in crate::gui) fn still_summary(still: &StillOutput) -> String {
    let size = match still.size_preset {
        0 => "1920x1080".to_string(),
        1 => "3840x2160".to_string(),
        _ => format!("{}x{}", still.custom_width, still.custom_height),
    };
    if still.engine.is_ray() {
        return format!(
            "{size}, Ray Traced, {} AA taps, Fire {}, {}, {}",
            still.engine.ray.sanitized().aa_taps,
            if still.engine.ray.fire { "on" } else { "off" },
            color_space_name(still.color_space_index),
            compute_name(still.compute)
        );
    }
    format!(
        "{size}, {} samples, {} bounces, {}, {}",
        1u64 << still.sample_exponent.min(40),
        still.bounces,
        color_space_name(still.color_space_index),
        compute_name(still.compute),
    )
}

/// One line describing the tilt video's remembered values.
#[must_use]
pub(in crate::gui) fn tilt_summary(video: &TiltVideoOutput, compute: TiltVideoCompute) -> String {
    let (width, height) = params::resolve_resolution(
        video.resolution_preset,
        as_i32(video.custom_width),
        as_i32(video.custom_height),
    );
    if video.engine.is_ray() {
        return format!(
            "{width}x{height}, Ray Traced, {} AA taps, Fire {}, {}, {}",
            video.engine.ray.sanitized().aa_taps,
            if video.engine.ray.fire { "on" } else { "off" },
            color_space_name(video.color_space_index),
            compute_name(compute)
        );
    }
    format!(
        "{width}x{height}, {} samples, {} bounces, {}, {}",
        1u64 << video.sample_exponent.min(40),
        video.bounces,
        color_space_name(video.color_space_index),
        compute_name(compute),
    )
}

/// Shows the remembered values as the two summary lines of the Render & Output tab.
pub(in crate::gui) fn publish_summaries(
    ui: &MainWindow,
    output: &OutputSettings,
    tilt_compute: TiltVideoCompute,
) {
    let settings = ui.global::<SettingsModel>();
    settings.set_still_output_summary(still_summary(&output.still).into());
    settings.set_tilt_output_summary(tilt_summary(&output.tilt_video, tilt_compute).into());
}

/// The still export dialog's controls as plain values: what seeding sets and what a run reads
/// back. Kept apart from `ExportModel` so the mapping can be tested without a window.
#[derive(Debug, Clone, PartialEq)]
struct StillControls {
    engine: pleochroma::renderer::engine::EngineSpec,
    size_preset: i32,
    custom_width: i32,
    custom_height: i32,
    sample_exponent_raw: f32,
    color_space_index: i32,
    compute_target_index: i32,
    bounce_index: i32,
    transfer_index: i32,
}

/// The tilt-video section's controls as plain values (see [`StillControls`]).
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent switches of the section, not a state machine"
)]
#[derive(Debug, Clone, PartialEq)]
struct TiltControls {
    engine: pleochroma::renderer::engine::EngineSpec,
    resolution_preset: i32,
    custom_width: i32,
    custom_height: i32,
    sample_exponent_raw: f32,
    bounce_index: i32,
    color_space_index: i32,
    compute_target_index: i32,
    transfer_index: i32,
    filename_template: String,
    start_angle_deg: i32,
    end_angle_deg: i32,
    step_deg: f32,
    fps_index: i32,
    /// The axis chips in play order (`TiltVideoExportModel.video_axes`).
    video_axes: Vec<i32>,
    quarter_turn: bool,
    show_values: bool,
    metric_brilliance: bool,
    metric_windowing: bool,
    metric_extinction: bool,
    metric_tilt_brilliance: bool,
    metric_angle: bool,
    keep_frames: bool,
}

/// What the still export dialog shows when it opens: the remembered values, limited to what
/// the controls allow. A transfer nobody chose follows the remote endpoint's own default.
fn still_controls(settings: &AppSettings) -> StillControls {
    let still = settings.output.clone().sanitised().still;
    StillControls {
        engine: still.engine,
        size_preset: still.size_preset,
        custom_width: as_i32(still.custom_width),
        custom_height: as_i32(still.custom_height),
        sample_exponent_raw: still.sample_exponent as f32,
        color_space_index: still.color_space_index,
        compute_target_index: still.compute.index(),
        bounce_index: i32::try_from(bounce_index(still.bounces)).unwrap_or(i32::MAX),
        transfer_index: still
            .transfer
            .unwrap_or_else(|| endpoint_transfer(settings))
            .index(),
    }
}

/// The values a still export run remembers. The transfer counts only when the run sends work
/// to the remote (`uses_remote`); otherwise the earlier choice, or none, stays.
fn still_from_controls(
    controls: &StillControls,
    previous_transfer: Option<ExportTransfer>,
    uses_remote: bool,
) -> StillOutput {
    StillOutput {
        engine: controls.engine,
        size_preset: controls.size_preset,
        custom_width: as_u32(controls.custom_width),
        custom_height: as_u32(controls.custom_height),
        sample_exponent: exponent_of(controls.sample_exponent_raw),
        bounces: nearest_bounce_cap(controls.bounce_index),
        color_space_index: controls.color_space_index,
        compute: TiltVideoCompute::from_index(controls.compute_target_index),
        transfer: if uses_remote {
            Some(ExportTransfer::from_index(controls.transfer_index))
        } else {
            previous_transfer
        },
    }
}

/// What the tilt-video section shows at start-up and after a tab reset. The compute pill is
/// the standing `tilt_video_compute_target`, not part of [`TiltVideoOutput`].
fn tilt_controls(settings: &AppSettings) -> TiltControls {
    let video = settings.output.clone().sanitised().tilt_video;
    TiltControls {
        engine: video.engine,
        resolution_preset: video.resolution_preset.min(params::CUSTOM_RESOLUTION_INDEX),
        custom_width: as_i32(video.custom_width),
        custom_height: as_i32(video.custom_height),
        sample_exponent_raw: video.sample_exponent as f32,
        bounce_index: i32::try_from(bounce_index(video.bounces)).unwrap_or(i32::MAX),
        color_space_index: video.color_space_index,
        compute_target_index: settings.tilt_video_compute_target.index(),
        transfer_index: video
            .transfer
            .unwrap_or_else(|| endpoint_transfer(settings))
            .index(),
        filename_template: video.filename_template,
        start_angle_deg: video.start_angle_deg,
        end_angle_deg: video.end_angle_deg,
        step_deg: video.step_hundredths_deg as f32 / 100.0,
        fps_index: video.fps_index,
        video_axes: axis_list(&video.axes),
        quarter_turn: video.quarter_turn,
        show_values: video.show_values,
        metric_brilliance: video.metric_brilliance,
        metric_windowing: video.metric_windowing,
        metric_extinction: video.metric_extinction,
        metric_tilt_brilliance: video.metric_tilt_brilliance,
        metric_angle: video.metric_angle,
        keep_frames: video.keep_frames,
    }
}

/// The values a tilt video run remembers (see [`still_from_controls`] for `uses_remote`).
fn tilt_from_controls(
    controls: &TiltControls,
    previous_transfer: Option<ExportTransfer>,
    uses_remote: bool,
) -> TiltVideoOutput {
    TiltVideoOutput {
        engine: controls.engine,
        resolution_preset: controls.resolution_preset,
        custom_width: as_u32(controls.custom_width),
        custom_height: as_u32(controls.custom_height),
        sample_exponent: exponent_of(controls.sample_exponent_raw),
        bounces: nearest_bounce_cap(controls.bounce_index),
        color_space_index: controls.color_space_index,
        transfer: if uses_remote {
            Some(ExportTransfer::from_index(controls.transfer_index))
        } else {
            previous_transfer
        },
        filename_template: controls.filename_template.clone(),
        start_angle_deg: controls.start_angle_deg,
        end_angle_deg: controls.end_angle_deg,
        step_hundredths_deg: step_hundredths_of(controls.step_deg),
        fps_index: controls.fps_index,
        axes: axes_of(&controls.video_axes),
        quarter_turn: controls.quarter_turn,
        show_values: controls.show_values,
        metric_brilliance: controls.metric_brilliance,
        metric_windowing: controls.metric_windowing,
        metric_extinction: controls.metric_extinction,
        metric_tilt_brilliance: controls.metric_tilt_brilliance,
        metric_angle: controls.metric_angle,
        keep_frames: controls.keep_frames,
    }
}

/// Seeds the still export dialog's controls from the remembered values. Called every time the
/// dialog opens (`populate_fanout_presets`).
pub(in crate::gui) fn seed_still_dialog(ui: &MainWindow, settings: &AppSettings) {
    let controls = still_controls(settings);
    let model = ui.global::<ExportModel>();
    model.set_engine_index(i32::from(controls.engine.engine.index()));
    model.set_ray_taps(i32::from(controls.engine.ray.sanitized().aa_taps));
    model.set_ray_fire(controls.engine.ray.fire);
    model.set_selected_size_preset(controls.size_preset);
    model.set_custom_width(controls.custom_width);
    model.set_custom_height(controls.custom_height);
    model.set_sample_exponent_raw(controls.sample_exponent_raw);
    model.set_selected_color_space_index(controls.color_space_index);
    model.set_compute_target_index(controls.compute_target_index);
    model.set_bounce_index(controls.bounce_index);
    model.set_transfer_index(controls.transfer_index);
}

/// Seeds the tilt-video section's controls from the remembered values, and the export folder
/// it shows (shared with the still export). Called at start-up and after a tab reset.
pub(in crate::gui) fn seed_tilt_dialog(ui: &MainWindow, settings: &AppSettings) {
    let controls = tilt_controls(settings);
    let model = ui.global::<TiltVideoExportModel>();
    model.set_engine_index(i32::from(controls.engine.engine.index()));
    model.set_ray_taps(i32::from(controls.engine.ray.sanitized().aa_taps));
    model.set_ray_fire(controls.engine.ray.fire);
    model.set_resolution_preset(controls.resolution_preset);
    model.set_custom_width(controls.custom_width);
    model.set_custom_height(controls.custom_height);
    model.set_sample_exponent_raw(controls.sample_exponent_raw);
    model.set_selected_bounce_index(controls.bounce_index);
    model.set_selected_color_space_index(controls.color_space_index);
    model.set_compute_target_index(controls.compute_target_index);
    model.set_transfer_index(controls.transfer_index);
    model.set_filename_template(controls.filename_template.into());
    model.set_start_angle_deg(controls.start_angle_deg);
    model.set_end_angle_deg(controls.end_angle_deg);
    model.set_step_deg(controls.step_deg);
    model.set_fps_index(controls.fps_index);
    model.set_video_axes(ModelRc::new(VecModel::from(controls.video_axes)));
    model.set_quarter_turn(controls.quarter_turn);
    model.set_show_values(controls.show_values);
    model.set_metric_brilliance(controls.metric_brilliance);
    model.set_metric_windowing(controls.metric_windowing);
    model.set_metric_extinction(controls.metric_extinction);
    model.set_metric_tilt_brilliance(controls.metric_tilt_brilliance);
    model.set_metric_angle(controls.metric_angle);
    model.set_keep_frames(controls.keep_frames);
    ui.global::<ExportModel>()
        .set_directory(settings.export_directory.clone().into());
}

/// When the remote endpoint changes, a tilt video that has never made its own transfer choice
/// follows the endpoint's default again. Called after the remote is saved or removed.
pub(in crate::gui) fn follow_remote_default_for_tilt(ui: &MainWindow, settings: &AppSettings) {
    if settings.output.tilt_video.transfer.is_none() {
        ui.global::<TiltVideoExportModel>()
            .set_transfer_index(endpoint_transfer(settings).index());
    }
}

/// Remembers the values of the still export dialog: an export starting, or the dialog closing.
/// `uses_remote` says whether the run sends work to the remote: only then does the transfer
/// choice count, so a run without a remote (or a dialog closed without a run) does not pin
/// the transfer to "full data" for later.
pub(in crate::gui) fn remember_still(
    ui: &MainWindow,
    settings_store: &SettingsPersister,
    uses_remote: bool,
) {
    let model = ui.global::<ExportModel>();
    let mut output = settings_store.snapshot().settings.output;
    let controls = StillControls {
        engine: still_engine(ui),
        size_preset: model.get_selected_size_preset(),
        custom_width: model.get_custom_width(),
        custom_height: model.get_custom_height(),
        sample_exponent_raw: model.get_sample_exponent_raw(),
        color_space_index: model.get_selected_color_space_index(),
        compute_target_index: model.get_compute_target_index(),
        bounce_index: model.get_bounce_index(),
        transfer_index: model.get_transfer_index(),
    };
    output.still = still_from_controls(&controls, output.still.transfer, uses_remote);
    store_and_publish(ui, settings_store, output);
}

/// Remembers the values of the tilt-video section: a run starting or being queued, or the
/// dialog closing (see [`remember_still`] for `uses_remote`; closing passes `false`).
pub(in crate::gui) fn remember_tilt(
    ui: &MainWindow,
    settings_store: &SettingsPersister,
    uses_remote: bool,
) {
    let model = ui.global::<TiltVideoExportModel>();
    let mut output = settings_store.snapshot().settings.output;
    let controls = TiltControls {
        engine: tilt_engine(ui),
        resolution_preset: model.get_resolution_preset(),
        custom_width: model.get_custom_width(),
        custom_height: model.get_custom_height(),
        sample_exponent_raw: model.get_sample_exponent_raw(),
        bounce_index: model.get_selected_bounce_index(),
        color_space_index: model.get_selected_color_space_index(),
        compute_target_index: model.get_compute_target_index(),
        transfer_index: model.get_transfer_index(),
        filename_template: model.get_filename_template().to_string(),
        start_angle_deg: model.get_start_angle_deg(),
        end_angle_deg: model.get_end_angle_deg(),
        step_deg: model.get_step_deg(),
        fps_index: model.get_fps_index(),
        video_axes: model.get_video_axes().iter().collect(),
        quarter_turn: model.get_quarter_turn(),
        show_values: model.get_show_values(),
        metric_brilliance: model.get_metric_brilliance(),
        metric_windowing: model.get_metric_windowing(),
        metric_extinction: model.get_metric_extinction(),
        metric_tilt_brilliance: model.get_metric_tilt_brilliance(),
        metric_angle: model.get_metric_angle(),
        keep_frames: model.get_keep_frames(),
    };
    output.tilt_video = tilt_from_controls(&controls, output.tilt_video.transfer, uses_remote);
    store_and_publish(ui, settings_store, output);
}

fn store_and_publish(ui: &MainWindow, settings_store: &SettingsPersister, output: OutputSettings) {
    let output = output.sanitised();
    let compute = settings_store.snapshot().settings.tilt_video_compute_target;
    settings_store.update(|s| s.settings.output = output.clone());
    publish_summaries(ui, &output, compute);
}

/// The still dialog's renderer, read without changing its callback signature.
pub(in crate::gui) fn still_engine(ui: &MainWindow) -> pleochroma::renderer::engine::EngineSpec {
    let model = ui.global::<ExportModel>();
    output_engine(
        model.get_engine_index(),
        model.get_ray_taps(),
        model.get_ray_fire(),
    )
}

/// The tilt dialog's renderer, frozen when its run is captured.
pub(in crate::gui) fn tilt_engine(ui: &MainWindow) -> pleochroma::renderer::engine::EngineSpec {
    let model = ui.global::<TiltVideoExportModel>();
    output_engine(
        model.get_engine_index(),
        model.get_ray_taps(),
        model.get_ray_fire(),
    )
}

fn output_engine(index: i32, taps: i32, fire: bool) -> pleochroma::renderer::engine::EngineSpec {
    use pleochroma::renderer::engine::{EngineSpec, RayOptions, RenderEngine};
    EngineSpec {
        engine: RenderEngine::from_index(u8::try_from(index).unwrap_or_default()),
        ray: RayOptions {
            aa_taps: u8::try_from(taps).unwrap_or(16),
            fire,
        }
        .sanitized(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::model::RemoteEndpoint;

    #[test]
    fn the_summaries_name_the_remembered_values() {
        let still = StillOutput::default();
        let text = still_summary(&still);
        assert!(
            text.starts_with("1920x1080, 1024 samples, 12 bounces"),
            "{text}"
        );
        let video = TiltVideoOutput::default();
        let text = tilt_summary(&video, TiltVideoCompute::Both);
        assert!(text.starts_with("1920x1080, 64 samples"), "{text}");
        assert!(text.ends_with("local + remote"), "{text}");
    }

    /// A settings value with every remembered output default off its built-in value.
    fn chosen() -> AppSettings {
        let mut settings = AppSettings::default();
        settings.output.still = StillOutput {
            engine: pleochroma::renderer::engine::EngineSpec::default(),
            size_preset: 2,
            custom_width: 2400,
            custom_height: 1600,
            sample_exponent: 12,
            bounces: 64,
            color_space_index: 1,
            compute: TiltVideoCompute::Local,
            transfer: Some(ExportTransfer::FinalPicture),
        };
        settings.output.tilt_video = TiltVideoOutput {
            engine: pleochroma::renderer::engine::EngineSpec::default(),
            resolution_preset: 3,
            custom_width: 640,
            custom_height: 360,
            sample_exponent: 8,
            bounces: 24,
            color_space_index: 2,
            transfer: Some(ExportTransfer::FinalPicture),
            filename_template: "video_{design}".to_string(),
            start_angle_deg: -30,
            end_angle_deg: 45,
            step_hundredths_deg: 25,
            fps_index: 2,
            axes: vec![1, 3],
            quarter_turn: true,
            show_values: true,
            metric_brilliance: false,
            metric_windowing: true,
            metric_extinction: false,
            metric_tilt_brilliance: true,
            metric_angle: false,
            keep_frames: true,
        };
        settings.tilt_video_compute_target = TiltVideoCompute::Remote;
        settings
    }

    #[test]
    fn a_fresh_install_seeds_the_built_in_still_defaults() {
        let controls = still_controls(&AppSettings::default());
        assert_eq!(
            controls,
            StillControls {
                engine: StillOutput::default().engine,
                size_preset: 0,
                custom_width: 1920,
                custom_height: 1080,
                sample_exponent_raw: 10.0,
                color_space_index: 0,
                compute_target_index: TiltVideoCompute::Both.index(),
                bounce_index: 2,
                transfer_index: ExportTransfer::FullData.index(),
            }
        );
    }

    #[test]
    fn the_dialogs_open_with_the_last_used_values() {
        let settings = chosen();
        let still = still_controls(&settings);
        assert_eq!(
            (
                still.size_preset,
                still.custom_width,
                still.custom_height,
                still.sample_exponent_raw.to_bits(),
                still.color_space_index,
                still.compute_target_index,
                still.bounce_index,
                still.transfer_index,
            ),
            (
                2,
                2400,
                1600,
                12.0f32.to_bits(),
                1,
                TiltVideoCompute::Local.index(),
                4,
                ExportTransfer::FinalPicture.index(),
            )
        );
        let tilt = tilt_controls(&settings);
        assert_eq!(tilt.resolution_preset, 3);
        assert_eq!((tilt.custom_width, tilt.custom_height), (640, 360));
        assert_eq!(tilt.sample_exponent_raw.to_bits(), 8.0f32.to_bits());
        assert_eq!(tilt.bounce_index, 3);
        assert_eq!(tilt.color_space_index, 2);
        // The compute pill is the standing tilt-video preference.
        assert_eq!(tilt.compute_target_index, TiltVideoCompute::Remote.index());
        assert_eq!(tilt.transfer_index, ExportTransfer::FinalPicture.index());
        assert_eq!(tilt.filename_template, "video_{design}");
        // The sweep, the axes, the 90° turn and the frame toggles.
        assert_eq!((tilt.start_angle_deg, tilt.end_angle_deg), (-30, 45));
        assert_eq!(tilt.step_deg.to_bits(), 0.25f32.to_bits());
        assert_eq!(tilt.fps_index, 2);
        assert_eq!(tilt.video_axes, vec![1, 3]);
        assert!(tilt.quarter_turn && tilt.show_values && tilt.keep_frames);
        assert!(!tilt.metric_brilliance && tilt.metric_tilt_brilliance);
    }

    /// A typed step is kept to the hundredth inside the field's range, and no chosen axis stays
    /// "the chart's axis" instead of becoming four unticked chips.
    #[test]
    fn the_step_is_kept_in_hundredths_and_no_chosen_axis_stays_the_charts_axis() {
        assert_eq!(step_hundredths_of(0.333), 33);
        assert_eq!(step_hundredths_of(0.001), TILT_STEP_HUNDREDTHS_RANGE.0);
        assert_eq!(step_hundredths_of(9.0), TILT_STEP_HUNDREDTHS_RANGE.1);
        assert_eq!(step_hundredths_of(f32::NAN), 100);
        assert!(axis_list(&[]).is_empty());
        assert!(axes_of(&[]).is_empty());
        // The play order survives the round trip; repeats and indices off the chart do not.
        assert_eq!(axes_of(&axis_list(&[2, 0])), vec![2, 0]);
        assert_eq!(axis_list(&[3, 3, 9, 1]), vec![3, 1]);
        assert_eq!(axes_of(&[-1, 2, 7, 2, 0]), vec![2, 0]);
        let fresh = tilt_controls(&AppSettings::default());
        assert!(fresh.video_axes.is_empty());
        assert_eq!(
            (fresh.start_angle_deg, fresh.end_angle_deg, fresh.fps_index),
            (-90, 90, 1)
        );
        assert_eq!(fresh.step_deg.to_bits(), 1.0f32.to_bits());
    }

    #[test]
    fn what_a_run_used_is_written_back_and_seeds_the_next_dialog() {
        let settings = chosen();
        let still = still_from_controls(&still_controls(&settings), None, true);
        assert_eq!(still, settings.output.still);
        let tilt = tilt_from_controls(&tilt_controls(&settings), None, true);
        assert_eq!(tilt, settings.output.tilt_video);

        // The user changed controls in the dialog before starting.
        let mut controls = still_controls(&settings);
        controls.sample_exponent_raw = 6.0;
        controls.bounce_index = 0;
        controls.transfer_index = ExportTransfer::FullData.index();
        let written = still_from_controls(&controls, settings.output.still.transfer, true);
        assert_eq!((written.sample_exponent, written.bounces), (6, 4));
        assert_eq!(written.transfer, Some(ExportTransfer::FullData));
        let mut next = AppSettings::default();
        next.output.still = written;
        assert_eq!(
            still_controls(&next).sample_exponent_raw.to_bits(),
            6.0f32.to_bits()
        );
        assert_eq!(still_controls(&next).bounce_index, 0);
    }

    #[test]
    fn a_run_without_the_remote_does_not_pin_the_transfer() {
        let controls = StillControls {
            transfer_index: ExportTransfer::FullData.index(),
            ..still_controls(&AppSettings::default())
        };
        assert_eq!(still_from_controls(&controls, None, false).transfer, None);
        assert_eq!(
            still_from_controls(&controls, Some(ExportTransfer::FinalPicture), false).transfer,
            Some(ExportTransfer::FinalPicture),
            "an earlier choice survives a local run"
        );
        let tilt = tilt_controls(&AppSettings::default());
        assert_eq!(tilt_from_controls(&tilt, None, false).transfer, None);
    }

    #[test]
    fn a_transfer_nobody_chose_follows_the_remote_endpoints_default() {
        let mut settings = AppSettings {
            remote: Some(RemoteEndpoint {
                export_transfer: ExportTransfer::FinalPicture,
                ..RemoteEndpoint::default()
            }),
            ..AppSettings::default()
        };
        assert_eq!(
            still_controls(&settings).transfer_index,
            ExportTransfer::FinalPicture.index()
        );
        assert_eq!(
            tilt_controls(&settings).transfer_index,
            ExportTransfer::FinalPicture.index()
        );
        // A choice that was made wins over the endpoint.
        settings.output.still.transfer = Some(ExportTransfer::FullData);
        assert_eq!(
            still_controls(&settings).transfer_index,
            ExportTransfer::FullData.index()
        );
    }

    #[test]
    fn a_hand_edited_file_cannot_seed_an_impossible_control() {
        let mut settings = AppSettings::default();
        settings.output.still.sample_exponent = 99;
        settings.output.still.size_preset = 9;
        settings.output.still.bounces = 16;
        settings.output.tilt_video.resolution_preset = 99;
        settings.output.tilt_video.filename_template = "  ".to_string();
        let still = still_controls(&settings);
        assert_eq!(still.sample_exponent_raw.to_bits(), 15.0f32.to_bits());
        assert_eq!(still.size_preset, 2);
        assert_eq!(still.bounce_index, 2, "16 bounces sits on the 12 rung");
        let tilt = tilt_controls(&settings);
        assert_eq!(tilt.resolution_preset, params::CUSTOM_RESOLUTION_INDEX);
        assert_ne!(tilt.filename_template.trim(), "");
    }

    /// A slider position that drifted off an integer still lands on a legal exponent.
    #[test]
    fn the_samples_slider_is_remembered_as_a_whole_exponent_inside_the_export_range() {
        assert_eq!(exponent_of(10.4), 10);
        assert_eq!(exponent_of(10.6), 11);
        assert_eq!(exponent_of(-3.0), EXPORT_SAMPLES_EXPONENT_RANGE.0);
        assert_eq!(exponent_of(99.0), EXPORT_SAMPLES_EXPONENT_RANGE.1);
    }

    #[test]
    fn an_unknown_colour_space_reads_as_srgb() {
        assert_eq!(color_space_name(7), "sRGB");
        assert_eq!(color_space_name(-1), "sRGB");
    }
}
