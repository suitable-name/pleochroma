//! Pure parameter validation and frame-pose enumeration for the tilt performance video
//! export: the angle sweep (start/end/step), the resolution preset table, and the
//! zero-padded frame-numbering width. Deliberately free of any Slint/render dependency
//! so `mod.rs`'s wiring and `render.rs`'s per-frame loop build on the exact same
//! numbers the dialog itself shows via `TiltVideoExportModel.frame_count`.

use std::collections::{HashMap, hash_map::Entry};

/// Minimum tilt-angle step the dialog must accept, in degrees -- the owner's own
/// requirement is that any value in `0.01..=1.0` works.
pub const MIN_STEP_DEG: f64 = 0.01;
/// Maximum step accepted at all. Above `1.0` this is only a "quick preview" allowance,
/// not a hard requirement, but still clamped rather than left unbounded.
pub const MAX_STEP_DEG: f64 = 5.0;
/// Frame count above which the dialog must ask the user to confirm before starting.
pub const CONFIRM_FRAME_THRESHOLD: usize = 2000;

/// Smallest tilt angle the video sweep accepts, in degrees.
pub const MIN_ANGLE_DEG: f64 = -90.0;
/// Largest tilt angle the video sweep accepts, in degrees.
pub const MAX_ANGLE_DEG: f64 = 90.0;

/// Clamps a requested step into the accepted range. A non-finite input (a half-typed
/// text field, e.g. `"-"` or `""` parsed as `NaN`) collapses to the `1.0` default
/// rather than propagating a `NaN` into `frame_count`'s division.
#[must_use]
pub const fn clamp_step_deg(step: f64) -> f64 {
    if step.is_finite() {
        step.clamp(MIN_STEP_DEG, MAX_STEP_DEG)
    } else {
        1.0
    }
}

/// Clamps a requested sweep endpoint into `[MIN_ANGLE_DEG, MAX_ANGLE_DEG]`.
#[must_use]
pub const fn clamp_angle_deg(angle: f64) -> f64 {
    if angle.is_finite() {
        angle.clamp(MIN_ANGLE_DEG, MAX_ANGLE_DEG)
    } else {
        0.0
    }
}

/// Number of frames the sweep `[start_deg, end_deg]` at `step_deg` produces:
/// `floor((end - start) / step) + 1`. `0` for a degenerate request (non-positive step,
/// or an end before the start).
#[must_use]
pub fn frame_count(start_deg: f64, end_deg: f64, step_deg: f64) -> usize {
    if step_deg <= 0.0 || end_deg < start_deg {
        return 0;
    }
    let span = end_deg - start_deg;
    // Epsilon guards against float division landing a hair under an exact multiple --
    // e.g. `180.0 / 0.01` can evaluate to `17999.999999999996` rather than `18000.0`,
    // which a bare `.floor()` would under-count by one frame.
    let steps = (span / step_deg + 1e-6).floor();
    if steps.is_finite() {
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "steps is finite and non-negative (end >= start, step > 0); `as` saturates"
        )]
        let steps = steps as usize;
        steps.saturating_add(1)
    } else {
        0
    }
}

/// The exact fractional tilt angle, in degrees, for frame `index` of a `total`-frame
/// sweep (`total` must equal `frame_count(start_deg, end_deg, step_deg)`). The LAST
/// frame (`index + 1 == total`) is pinned to exactly `end_deg` rather than
/// `start_deg + step_deg * index`, so it lands on the end angle within float epsilon
/// regardless of accumulated step error -- required so the camera pose (and the
/// overlay's own angle readout) never snaps to the nearest 1° sweep-grid sample.
#[must_use]
pub const fn frame_angle_deg(
    start_deg: f64,
    end_deg: f64,
    step_deg: f64,
    index: usize,
    total: usize,
) -> f64 {
    if total == 0 {
        start_deg
    } else if index + 1 >= total {
        end_deg
    } else {
        step_deg.mul_add(index as f64, start_deg)
    }
}

/// The full list of exact fractional tilt angles a sweep visits, in frame order.
#[must_use]
pub fn frame_angles(start_deg: f64, end_deg: f64, step_deg: f64) -> Vec<f64> {
    let total = frame_count(start_deg, end_deg, step_deg);
    (0..total)
        .map(|i| frame_angle_deg(start_deg, end_deg, step_deg, i, total))
        .collect()
}

/// One frame of a video: the sweep axis (an index into `PROFILE_AZIMUTHS_DEG`) and the
/// exact tilt about it, in degrees.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FramePose {
    pub axis_index: usize,
    pub tilt_deg: f64,
}

/// The angles of one leg from `from_deg` to `to_deg` at `step_deg`, both ends included and
/// in either direction; the last angle is pinned to `to_deg` like [`frame_angle_deg`].
fn leg_angles(from_deg: f64, to_deg: f64, step_deg: f64) -> Vec<f64> {
    let span = (to_deg - from_deg).abs();
    let count = frame_count(0.0, span, step_deg);
    let sign = if to_deg < from_deg { -1.0 } else { 1.0 };
    (0..count)
        .map(|i| from_deg + sign * frame_angle_deg(0.0, span, step_deg, i, count))
        .collect()
}

/// Every frame of a video over `axes`, in order.
///
/// One axis is one pass, `start -> end` (the video as it always was). Several axes rock
/// through face-up, each `0 -> end -> start -> 0`, one after the other, so every joint is
/// the table-up pose and the video has no cut (the frame loop turns the image so that pose
/// looks the same on every axis, see `frames::axis_roll`). A pose shared by two legs, or by
/// two axes at face-up, is drawn once. Empty for an empty range (an end before the start)
/// or a step that is not positive.
#[must_use]
pub fn frame_poses(axes: &[usize], start_deg: f64, end_deg: f64, step_deg: f64) -> Vec<FramePose> {
    if frame_count(start_deg, end_deg, step_deg) == 0 {
        return Vec::new();
    }
    if let [axis_index] = axes {
        return frame_angles(start_deg, end_deg, step_deg)
            .into_iter()
            .map(|tilt_deg| FramePose {
                axis_index: *axis_index,
                tilt_deg,
            })
            .collect();
    }
    let mut poses: Vec<FramePose> = Vec::new();
    for &axis_index in axes {
        for (from, to) in [(0.0, end_deg), (end_deg, start_deg), (start_deg, 0.0)] {
            for tilt_deg in leg_angles(from, to, step_deg) {
                // A leg starts where the last one ended; the next axis starts table-up,
                // where the last one ended too.
                let repeats = poses
                    .last()
                    .is_some_and(|last| (last.tilt_deg - tilt_deg).abs() < 1e-9);
                if !repeats {
                    poses.push(FramePose {
                        axis_index,
                        tilt_deg,
                    });
                }
            }
        }
    }
    poses
}

/// The number of frames [`frame_poses`] gives.
#[must_use]
pub fn pose_count(axes: &[usize], start_deg: f64, end_deg: f64, step_deg: f64) -> usize {
    frame_poses(axes, start_deg, end_deg, step_deg).len()
}

/// For every frame of `poses`, the earlier frame that shows the same picture, if any: the same
/// axis at the same tilt. A rocking axis passes every angle between Start and End twice and
/// face-up three times, so about half of a multi-axis video repeats an earlier frame; the frame
/// loop copies those instead of rendering them again (`frames::render_frames`).
///
/// Only the same axis counts. Face-up looks alike on every axis (`frames::axis_roll`), but each
/// axis's camera is turned by a computed angle, so its pixels are not guaranteed to match.
#[must_use]
pub fn earlier_same_frame(poses: &[FramePose]) -> Vec<Option<usize>> {
    let mut first_of: HashMap<(usize, i64), usize> = HashMap::with_capacity(poses.len());
    poses
        .iter()
        .enumerate()
        .map(
            |(index, pose)| match first_of.entry((pose.axis_index, tilt_key(pose.tilt_deg))) {
                Entry::Occupied(first) => Some(*first.get()),
                Entry::Vacant(slot) => {
                    slot.insert(index);
                    None
                }
            },
        )
        .collect()
}

/// A tilt as a whole number of millionths of a degree: the outward and the return leg reach
/// the same angle by different sums, which may differ in the last bits.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a tilt is within ±90 degrees, so its millionths fit an i64 by far"
)]
fn tilt_key(tilt_deg: f64) -> i64 {
    (tilt_deg * 1e6).round() as i64
}

/// How many frames of the video over `axes` are rendered: [`pose_count`] less the frames that
/// repeat an earlier one ([`earlier_same_frame`]).
#[must_use]
pub fn rendered_pose_count(axes: &[usize], start_deg: f64, end_deg: f64, step_deg: f64) -> usize {
    earlier_same_frame(&frame_poses(axes, start_deg, end_deg, step_deg))
        .iter()
        .filter(|same| same.is_none())
        .count()
}

/// How many decimal digits `total_frames` needs so every frame's zero-padded filename
/// (`frame_00001.png`) is exactly as wide as the LAST one -- e.g. `18001` frames need
/// width `5`.
#[must_use]
pub fn frame_number_digits(total_frames: usize) -> usize {
    total_frames.max(1).to_string().len()
}

/// One PNG frame's filename inside the export's frame subfolder, zero-padded to
/// [`frame_number_digits`] of `total_frames` -- `index` is 1-based (`frame_00001.png`
/// for the first frame), matching the `frame_%0Nd.png` pattern `encode::mux_mp4` feeds
/// ffmpeg.
#[must_use]
pub fn frame_file_name(index_one_based: usize, total_frames: usize) -> String {
    let digits = frame_number_digits(total_frames);
    format!("frame_{index_one_based:0digits$}.png")
}

/// One of the fixed resolution presets `TiltVideoExportModel.resolution_preset` indexes
/// into: seven square presets (128..8192), then five 16:9 presets (720p..8K) -- kept in
/// lock-step with the combo box's item order in `performance_graph_dialog.slint`.
pub const RESOLUTION_PRESETS: &[(u32, u32)] = &[
    (128, 128),
    (256, 256),
    (512, 512),
    (1024, 1024),
    (2048, 2048),
    (4096, 4096),
    (8192, 8192),
    (1280, 720),
    (1920, 1080),
    (2560, 1440),
    (3840, 2160),
    (7680, 4320),
];
/// The `resolution_preset` index meaning "Custom" (`custom_width`/`custom_height`) --
/// one past the end of [`RESOLUTION_PRESETS`].
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_possible_wrap,
    reason = "const context (no try_from); the preset table has 12 entries"
)]
pub const CUSTOM_RESOLUTION_INDEX: i32 = RESOLUTION_PRESETS.len() as i32;

const MIN_VIDEO_DIM: i32 = 128;
const MAX_VIDEO_DIM: i32 = 8192;

/// Rounds `dim` down to the nearest even value -- `encode::mux_mp4`'s libx264 pass
/// (`-pix_fmt yuv420p`) rejects an odd width or height outright (chroma
/// subsampling needs an even number of samples per axis), and previously fell back
/// to GIF with nothing but a `tracing::warn!` no one watching a release build ever
/// sees (see [`super::encode`]'s own doc comment for the other half of that fix).
/// Rounds DOWN, never up, so the result never exceeds an already-clamped bound
/// (every caller here clamps to `[128, 8192]` first, both already even, so rounding
/// down can never underflow below `MIN_VIDEO_DIM` either).
const fn round_down_to_even(dim: i32) -> i32 {
    dim & !1
}

/// Resolves `preset_index`/`custom_width`/`custom_height` into the actual pixel size to
/// render. A custom size is clamped to `[128, 8192]` per axis independently -- `128`
/// (not the still-image export's `16`) since the smallest preset this dialog itself
/// offers is 128x128 -- and then rounded to even (see [`round_down_to_even`]); every
/// fixed [`RESOLUTION_PRESETS`] entry is already even, so this only ever changes a
/// custom size.
#[must_use]
pub fn resolve_resolution(preset_index: i32, custom_width: i32, custom_height: i32) -> (u32, u32) {
    if let Some(&preset) = usize::try_from(preset_index)
        .ok()
        .and_then(|index| RESOLUTION_PRESETS.get(index))
    {
        preset
    } else {
        (
            round_down_to_even(custom_width.clamp(MIN_VIDEO_DIM, MAX_VIDEO_DIM)).cast_unsigned(),
            round_down_to_even(custom_height.clamp(MIN_VIDEO_DIM, MAX_VIDEO_DIM)).cast_unsigned(),
        )
    }
}

/// The `fps_index` pill (0/1/2) -> the actual frame rate.
#[must_use]
pub const fn resolve_fps(fps_index: i32) -> u32 {
    match fps_index {
        0 => 24,
        2 => 60,
        _ => 30,
    }
}

/// The bounce-ladder pill index -> the actual max-bounce cap, read off the one ladder
/// (`settings::model::BOUNCE_LADDER`) every picker shares. An index off the ends is limited to
/// the nearest rung.
#[must_use]
pub const fn resolve_max_bounces(bounce_index: i32) -> u32 {
    crate::settings::model::nearest_bounce_cap(bounce_index)
}

/// The `done`/`total` frame-count pair (`run::report_progress`'s own inputs) as a
/// `0.0..=1.0` completion fraction -- the number both `TiltVideoExportModel.
/// progress` (the dialog's own bar) and the status strip's `ActivityChip` (via
/// `ActivityModel.invoke_progress_external`) show, so the two can never
/// disagree. `total.max(1)` guards a
/// degenerate zero-frame request against a division by zero -- `frame_count`
/// itself already refuses to start a run at `0`, but this stays total rather than
/// assuming that guard always ran first.
#[must_use]
pub fn export_progress_fraction(done: usize, total: usize) -> f32 {
    done as f32 / total.max(1) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_progress_fraction_is_zero_before_any_frame() {
        assert_eq!(export_progress_fraction(0, 181), 0.0);
    }

    #[test]
    fn export_progress_fraction_is_one_on_the_last_frame() {
        assert_eq!(export_progress_fraction(181, 181), 1.0);
    }

    #[test]
    fn export_progress_fraction_is_proportional_partway_through() {
        assert_eq!(export_progress_fraction(50, 200), 0.25);
    }

    #[test]
    fn export_progress_fraction_never_divides_by_zero_for_a_degenerate_total() {
        assert_eq!(export_progress_fraction(0, 0), 0.0);
        assert_eq!(export_progress_fraction(5, 0), 5.0);
    }

    #[test]
    fn frame_count_matches_the_181_point_default_sweep() {
        // -90..=90 in 1 deg steps is exactly this dialog's own always-available sweep
        // resolution -- 181 points.
        assert_eq!(frame_count(-90.0, 90.0, 1.0), 181);
    }

    #[test]
    fn frame_count_reaches_18001_at_the_finest_required_step() {
        assert_eq!(frame_count(-90.0, 90.0, 0.01), 18001);
    }

    #[test]
    fn frame_count_is_zero_for_degenerate_requests() {
        assert_eq!(frame_count(0.0, -10.0, 1.0), 0, "end before start");
        assert_eq!(frame_count(-10.0, 10.0, 0.0), 0, "zero step");
        assert_eq!(frame_count(-10.0, 10.0, -1.0), 0, "negative step");
    }

    #[test]
    fn frame_count_handles_a_single_point_sweep() {
        assert_eq!(frame_count(5.0, 5.0, 1.0), 1);
    }

    /// Steps 0.01, 0.25 and 1.0 must
    /// each produce a frame list whose LAST angle lands on the end angle within 1e-9,
    /// not the nearest step-grid sample short of it.
    #[test]
    fn last_frame_lands_exactly_on_the_end_angle_for_every_required_step() {
        for step in [0.01, 0.25, 1.0] {
            let angles = frame_angles(-90.0, 90.0, step);
            let last = *angles.last().expect("non-empty sweep");
            assert!(
                (last - 90.0).abs() < 1e-9,
                "step {step}: last frame angle {last} did not land on 90.0 within 1e-9"
            );
            assert!(
                (angles[0] - -90.0).abs() < 1e-9,
                "step {step}: first frame must be the start angle"
            );
            assert_eq!(angles.len(), frame_count(-90.0, 90.0, step));
        }
    }

    #[test]
    fn frame_angles_are_monotonically_non_decreasing() {
        let angles = frame_angles(-10.0, 10.0, 0.3);
        for pair in angles.windows(2) {
            assert!(pair[1] >= pair[0]);
        }
    }

    #[test]
    fn one_axis_is_the_plain_start_to_end_pass() {
        let poses = frame_poses(&[2], -10.0, 10.0, 5.0);
        let tilts: Vec<f64> = poses.iter().map(|pose| pose.tilt_deg).collect();
        assert_eq!(tilts, vec![-10.0, -5.0, 0.0, 5.0, 10.0]);
        assert!(poses.iter().all(|pose| pose.axis_index == 2));
        assert_eq!(
            pose_count(&[2], -90.0, 90.0, 0.01),
            frame_count(-90.0, 90.0, 0.01)
        );
    }

    /// Two axes rock through face-up: `0 -> end -> start -> 0` each, no pose twice in a
    /// row, and every joint between legs and axes is table-up or a range end.
    #[test]
    fn several_axes_rock_through_face_up_without_repeating_a_pose() {
        let poses = frame_poses(&[0, 2], -10.0, 10.0, 5.0);
        let tilts: Vec<(usize, f64)> = poses
            .iter()
            .map(|pose| (pose.axis_index, pose.tilt_deg))
            .collect();
        let one_axis = |axis| {
            vec![
                (axis, 0.0),
                (axis, 5.0),
                (axis, 10.0),
                (axis, 5.0),
                (axis, 0.0),
                (axis, -5.0),
                (axis, -10.0),
                (axis, -5.0),
                (axis, 0.0),
            ]
        };
        let mut expected = one_axis(0);
        // The second axis starts table-up, where the first one ended: drawn once.
        expected.extend(one_axis(2).into_iter().skip(1));
        assert_eq!(tilts, expected);
        assert_eq!(pose_count(&[0, 2], -10.0, 10.0, 5.0), 17);
    }

    /// The way back of a rocking axis repeats the outward angles and face-up repeats on every
    /// leg: each repeat names the first frame of its pose, on the same axis only.
    #[test]
    fn a_rocked_angle_repeats_the_first_frame_of_its_pose_on_the_same_axis_only() {
        let poses = frame_poses(&[0, 2], -10.0, 10.0, 5.0);
        // Axis 0: 0, 5, 10, 5, 0, -5, -10, -5, 0; axis 2 without its table-up start: 5, 10, 5,
        // 0, -5, -10, -5, 0.
        assert_eq!(
            earlier_same_frame(&poses),
            vec![
                None,
                None,
                None,
                Some(1),
                Some(0),
                None,
                None,
                Some(5),
                Some(0),
                None,
                None,
                Some(9),
                None,
                None,
                None,
                Some(13),
                Some(12),
            ]
        );
        assert_eq!(rendered_pose_count(&[0, 2], -10.0, 10.0, 5.0), 10);
        // A step of 0.1 reaches the same angles by different sums on the way back.
        assert_eq!(pose_count(&[0, 1], -1.0, 1.0, 0.1), 81);
        assert_eq!(rendered_pose_count(&[0, 1], -1.0, 1.0, 0.1), 42);
        // One axis is one pass: nothing repeats.
        assert_eq!(rendered_pose_count(&[3], -90.0, 90.0, 1.0), 181);
        assert_eq!(pose_count(&[3], -90.0, 90.0, 1.0), 181);
    }

    #[test]
    fn a_rock_lands_exactly_on_the_ends_and_on_face_up() {
        let poses = frame_poses(&[0, 1], -30.0, 25.0, 0.3);
        for target in [25.0, -30.0, 0.0] {
            assert!(
                poses
                    .iter()
                    .any(|pose| (pose.tilt_deg - target).abs() < 1e-12),
                "the rock must visit {target} exactly"
            );
        }
        assert_eq!(poses.last().map(|pose| pose.tilt_deg), Some(0.0));
        // A leg's last frame is pinned to its end, so its final gap is under two steps.
        for pair in poses.windows(2) {
            assert!((pair[1].tilt_deg - pair[0].tilt_deg).abs() < 0.6);
        }
    }

    #[test]
    fn a_rock_with_a_range_on_one_side_of_face_up_still_starts_and_ends_table_up() {
        let tilts: Vec<f64> = frame_poses(&[0, 2], 10.0, 20.0, 10.0)
            .iter()
            .map(|pose| pose.tilt_deg)
            .collect();
        assert_eq!(
            tilts,
            vec![0.0, 10.0, 20.0, 10.0, 0.0, 10.0, 20.0, 10.0, 0.0]
        );
    }

    #[test]
    fn an_empty_range_gives_no_poses_for_any_axis_count() {
        assert!(frame_poses(&[0], 10.0, -10.0, 1.0).is_empty());
        assert!(frame_poses(&[0, 1], 10.0, -10.0, 1.0).is_empty());
        assert!(frame_poses(&[0, 1], -10.0, 10.0, 0.0).is_empty());
    }

    #[test]
    fn frame_number_digits_matches_the_last_frames_own_width() {
        assert_eq!(frame_number_digits(181), 3);
        assert_eq!(frame_number_digits(18001), 5);
        assert_eq!(frame_number_digits(1), 1);
        assert_eq!(frame_number_digits(0), 1);
    }

    #[test]
    fn frame_file_name_zero_pads_to_the_total_width() {
        assert_eq!(frame_file_name(1, 181), "frame_001.png");
        assert_eq!(frame_file_name(181, 181), "frame_181.png");
        assert_eq!(frame_file_name(7, 18001), "frame_00007.png");
    }

    #[test]
    fn clamp_step_deg_clamps_and_rejects_nan() {
        assert_eq!(clamp_step_deg(0.001), MIN_STEP_DEG);
        assert_eq!(clamp_step_deg(100.0), MAX_STEP_DEG);
        assert_eq!(clamp_step_deg(0.25), 0.25);
        assert_eq!(clamp_step_deg(f64::NAN), 1.0);
    }

    #[test]
    fn resolve_resolution_uses_the_preset_table_in_range() {
        assert_eq!(resolve_resolution(0, 999, 999), (128, 128));
        assert_eq!(resolve_resolution(8, 999, 999), (1920, 1080));
    }

    #[test]
    fn resolve_resolution_falls_back_to_clamped_custom_dimensions() {
        assert_eq!(
            resolve_resolution(CUSTOM_RESOLUTION_INDEX, 64, 20000),
            (128, 8192)
        );
        assert_eq!(resolve_resolution(-1, 500, 500), (500, 500));
    }

    /// libx264's `-pix_fmt yuv420p` rejects an odd width or height outright -- see
    /// `round_down_to_even`'s own doc comment. An odd custom dimension must round
    /// down to the nearest even one rather than reaching `encode::mux_mp4` unchanged
    /// and silently falling back to GIF.
    #[test]
    fn resolve_resolution_rounds_an_odd_custom_dimension_down_to_even() {
        assert_eq!(resolve_resolution(-1, 501, 723), (500, 722));
        // An odd dimension right at the upper clamp bound must round down, not
        // overflow past `MAX_VIDEO_DIM` by rounding up instead.
        assert_eq!(resolve_resolution(-1, 8191, 8191), (8190, 8190));
    }

    #[test]
    fn resolve_fps_covers_all_three_pills_and_falls_back_to_30() {
        assert_eq!(resolve_fps(0), 24);
        assert_eq!(resolve_fps(1), 30);
        assert_eq!(resolve_fps(2), 60);
        assert_eq!(resolve_fps(99), 30);
    }

    #[test]
    fn resolve_max_bounces_covers_the_full_six_rung_ladder() {
        assert_eq!(resolve_max_bounces(0), 4);
        assert_eq!(resolve_max_bounces(1), 8);
        assert_eq!(resolve_max_bounces(2), 12);
        assert_eq!(resolve_max_bounces(3), 24);
        assert_eq!(resolve_max_bounces(4), 64);
        assert_eq!(resolve_max_bounces(5), 128);
    }

    /// The tilt-video dialog keeps no ladder of its own: every pill index resolves to the
    /// rung `BOUNCE_LADDER` holds, and an index off either end is limited to the nearest.
    #[test]
    fn resolve_max_bounces_reads_the_one_ladder() {
        use crate::settings::model::BOUNCE_LADDER;
        for (index, rung) in BOUNCE_LADDER.iter().enumerate() {
            let index = i32::try_from(index).expect("a small index");
            assert_eq!(resolve_max_bounces(index), *rung);
        }
        assert_eq!(resolve_max_bounces(-3), BOUNCE_LADDER[0]);
        assert_eq!(
            resolve_max_bounces(99),
            BOUNCE_LADDER[BOUNCE_LADDER.len() - 1]
        );
    }
}
