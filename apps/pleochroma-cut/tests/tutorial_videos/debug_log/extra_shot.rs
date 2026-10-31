//! An extra picture inside a step: `NN-<phase>-<window>.png`, taken on request of a route.
//!
//! The before and after pictures of a step show only where the step ended, and the "during" sets
//! only cover what `debug_overlays` counts as up. A route that walks several pages of one window
//! (the Rough Color wizard) or reads a popup that no table lists (Editor Details) takes its
//! picture with [`page_shot`], so every page the guide describes is on a picture.

use super::{skip_note, with_sink};
use crate::failure_capture;

/// Photographs the open window `window` (`main`, `rough-color`, ...; the names of the "during"
/// pictures) as `NN-<phase>-<window>.png` of the current step, with no pointer marks, so the page
/// is seen as the learner sees it. Does nothing outside debug mode; a privacy hit or a failed
/// capture is a note on the step.
pub fn page_shot(phase: &str, window: &str) {
    with_sink(|sink| {
        let Some(ui) = sink.ui.upgrade() else {
            return;
        };
        match failure_capture::capture_windows_for(&ui, &sink.words) {
            Ok(shots) => {
                let wanted = shots
                    .into_iter()
                    .filter(|(name, _)| name == window)
                    .collect();
                sink.write_shots(phase, wanted, &[]);
            }
            Err(reason) => sink.cur.notes.push(skip_note(phase, &reason)),
        }
    });
}
