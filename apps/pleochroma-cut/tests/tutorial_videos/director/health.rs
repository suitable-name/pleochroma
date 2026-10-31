//! The health check after every step: what makes a video dishonest.

use super::Film;
use crate::{routes, support::WindowView};
use pleochroma_cut::harness_support;
use uiprobe::take_panics;

impl Film<'_> {
    /// Abandons the video on a panic, an unexpected file dialog, an error or warning toast, or
    /// private text on screen.
    pub(super) fn health(&self, label: &str) -> Result<(), String> {
        let panics = take_panics();
        if !panics.is_empty() {
            return Err(format!("{label}: panic: {}", panics.join("; ")));
        }
        let dialogs = harness_support::take_picker_failures();
        if !dialogs.is_empty() {
            return Err(format!(
                "{label}: unexpected file dialog: {}",
                dialogs.join("; ")
            ));
        }
        if self.ui.get_toast_visible() {
            let kind = self.ui.get_toast_type().to_string();
            if kind == "error" || kind == "warning" {
                return Err(format!(
                    "{label}: {kind} toast: {}",
                    self.ui.get_toast_message()
                ));
            }
        }
        let mut windows = vec![("main", routes::visible_texts(&self.view()))];
        if let Some(help) = harness_support::help_window() {
            windows.push((
                "Help",
                routes::visible_texts(&WindowView::of("HelpWindow", &help)),
            ));
        }
        if let Some(rough) = harness_support::rough_planner_window() {
            windows.push((
                "Rough Planner",
                routes::visible_texts(&WindowView::of("RoughPlannerWindow", &rough)),
            ));
        }
        for (window, texts) in windows {
            for text in texts {
                if let Some(word) = self.cfg.private_hit(&text) {
                    return Err(format!(
                        "{label}: private text in the {window} window ({word:?}): {text:?}"
                    ));
                }
            }
        }
        Ok(())
    }
}
