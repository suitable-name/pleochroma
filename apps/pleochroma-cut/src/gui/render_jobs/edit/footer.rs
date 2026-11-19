//! The footer line of the edit dialog: one sentence that says what Save will do, with real counts.

use super::seed::Controls;
use pleochroma_render_jobs::state::JobState;

/// What the footer needs to know about the edit in progress.
#[expect(
    clippy::struct_excessive_bools,
    reason = "independent facts of one edit, read by one sentence table"
)]
#[derive(Debug, Clone, Copy)]
pub(super) struct FooterFacts {
    /// The state the job had when the dialog opened.
    pub(super) state: JobState,
    /// A tilt video.
    pub(super) is_video: bool,
    /// The finished frames the row had when the dialog opened.
    pub(super) frames_done: u32,
    /// Whether anything was edited yet.
    pub(super) edited: bool,
    /// The edit holds only a label or an output name.
    pub(super) only_label: bool,
    /// The edit resets the progress (the picture changes, or a video's frame folder moves).
    pub(super) resets: bool,
    /// The first changed field that changes the picture, if any.
    pub(super) field: Option<&'static str>,
    /// A video's samples moved.
    pub(super) samples_moved: bool,
    /// A video's sweep or axes changed.
    pub(super) sweep_or_axes: bool,
    /// A video's output folder or name changed.
    pub(super) folder_changed: bool,
    /// The tilt curves are being measured.
    pub(super) measuring: bool,
    /// The frames the video will have after the edit.
    pub(super) total_frames: u32,
}

/// The name of the first control that changes the picture, in the order of the dialog.
#[must_use]
pub(super) fn first_picture_field(
    seeded: &Controls,
    now: &Controls,
    material_changed: bool,
) -> Option<&'static str> {
    let size_changed = (now.size_preset, now.custom_width, now.custom_height)
        != (
            seeded.size_preset,
            seeded.custom_width,
            seeded.custom_height,
        );
    let sweep_changed = (now.start_deg, now.end_deg, now.step_deg.to_bits())
        != (seeded.start_deg, seeded.end_deg, seeded.step_deg.to_bits());
    [
        (size_changed, "size"),
        (now.sample_exponent != seeded.sample_exponent, "samples"),
        (now.bounce_index != seeded.bounce_index, "bounces"),
        (
            now.color_space_index != seeded.color_space_index,
            "color space",
        ),
        (material_changed, "material"),
        (now.ray_taps_index != seeded.ray_taps_index, "taps"),
        (now.ray_fire != seeded.ray_fire, "fire"),
        (sweep_changed, "sweep"),
        (now.axes != seeded.axes, "axes"),
        (now.overlay != seeded.overlay, "overlay"),
    ]
    .into_iter()
    .find_map(|(changed, name)| changed.then_some(name))
}

/// The sentence above the buttons.
#[must_use]
pub(super) fn footer_text(facts: &FooterFacts) -> String {
    if facts.measuring {
        return "Measuring the tilt curves of this job's stone\u{2026}".to_string();
    }
    let done = facts.frames_done;
    if !facts.edited {
        return match facts.state {
            JobState::Queued if facts.is_video && done > 0 => format!(
                "Waiting video: Save keeps its {done} finished frames unless the picture changes."
            ),
            JobState::Queued | JobState::Running => {
                "Waiting job: Save changes it in place.".to_string()
            }
            JobState::Paused => {
                "Paused job: Save keeps its finished frames unless the picture changes.".to_string()
            }
            JobState::Failed => {
                "Failed job: a new compute, transfer or HDR map puts it back in the queue."
                    .to_string()
            }
            JobState::Done | JobState::Cancelled => {
                "Done job: a new label or name keeps it done; other changes render it again."
                    .to_string()
            }
        };
    }
    if facts.is_video
        && matches!(facts.state, JobState::Queued | JobState::Paused)
        && let Some(sentence) = video_sentence(facts)
    {
        return sentence;
    }
    let field = facts.field.unwrap_or("folder");
    match facts.state {
        JobState::Queued | JobState::Running => {
            if facts.resets && done > 0 {
                format!("Waiting job: the {field} changed, so it renders from the start.")
            } else if facts.is_video && done > 0 {
                format!(
                    "Waiting video: Save keeps its {done} finished frames unless the picture changes."
                )
            } else {
                "Waiting job: Save changes it in place.".to_string()
            }
        }
        JobState::Paused => {
            if facts.resets {
                format!("Paused job: the {field} changed, so it renders from the start.")
            } else {
                "Paused job: Save keeps its finished frames.".to_string()
            }
        }
        JobState::Failed => {
            if facts.resets {
                format!(
                    "Failed job: Save puts it back in the queue. The {field} changed, so it renders from the start."
                )
            } else if facts.only_label {
                "Failed job: a new label or name keeps it failed.".to_string()
            } else {
                "Failed job: Save puts it back in the queue.".to_string()
            }
        }
        JobState::Done | JobState::Cancelled => {
            if facts.resets {
                "Done job: Save puts it back in the queue and renders it again. The old file stays on disk."
                    .to_string()
            } else if facts.only_label {
                "Done job: a new label or name keeps it done.".to_string()
            } else {
                "Done job: a new compute asks whether to render it again.".to_string()
            }
        }
    }
}

/// The sentences special to a waiting or paused video.
fn video_sentence(facts: &FooterFacts) -> Option<String> {
    if facts.folder_changed {
        return Some(
            "The folder changed, so the video renders from the start. Its finished frames stay in the old folder."
                .to_string(),
        );
    }
    if facts.sweep_or_axes {
        return Some(format!(
            "The sweep changed, so the video renders from the start ({} frames).",
            facts.total_frames
        ));
    }
    if facts.samples_moved && facts.frames_done > 0 {
        return Some(format!(
            "Changing the samples renders the {} finished frames again.",
            facts.frames_done
        ));
    }
    None
}
