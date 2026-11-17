//! "Render on <computer>?" -- the question after a pairing succeeds.
//!
//! A newly paired computer only helps when the render settings let it: Live Render on "This
//! computer only" or an export Compute pill on "This computer only" keep it out. The default
//! (`Both`) already uses it, so then nothing is asked. [`excluded_targets`] decides, pure and
//! tested. Confirming sets every excluded target to `Both` (this computer keeps rendering, never
//! `RemoteOnly`) through the same updates the settings UI uses, so the pills show it.

use crate::{
    MainWindow, RemoteWorkerModel, SettingsModel, TiltVideoExportModel,
    bridge::render_thread::RenderContext,
    settings::{LiveComputeTarget, SettingsPersister, TiltVideoCompute, model::AppSettings},
};
use slint::ComponentHandle;
use std::sync::{Arc, Mutex};

/// Which render targets keep a paired computer unused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Excluded {
    /// Live Render runs on this computer only.
    pub live: bool,
    /// The still export or the tilt-video export runs on this computer only.
    pub final_pictures: bool,
}

/// The targets that would leave a newly paired computer unused, `None` when it is already used
/// (every target is `Both` or `Remote only`) and nothing needs asking.
#[must_use]
pub(super) fn excluded_targets(settings: &AppSettings) -> Option<Excluded> {
    let live = settings.live_compute_target == LiveComputeTarget::LocalOnly;
    let final_pictures = settings.output.still.compute == TiltVideoCompute::Local
        || settings.tilt_video_compute_target == TiltVideoCompute::Local;
    (live || final_pictures).then_some(Excluded {
        live,
        final_pictures,
    })
}

/// Sets every excluded target to `Both`; targets that already include the remote stay.
pub(super) fn include_remote(settings: &mut AppSettings) {
    if settings.live_compute_target == LiveComputeTarget::LocalOnly {
        settings.live_compute_target = LiveComputeTarget::Both;
    }
    if settings.output.still.compute == TiltVideoCompute::Local {
        settings.output.still.compute = TiltVideoCompute::Both;
    }
    if settings.tilt_video_compute_target == TiltVideoCompute::Local {
        settings.tilt_video_compute_target = TiltVideoCompute::Both;
    }
}

/// Asks "Render on <name>?" when the saved settings keep the computer out. Call after the
/// pairing was saved.
pub(super) fn ask_if_unused(ui: &MainWindow, settings: &AppSettings, name: &str) {
    let Some(excluded) = excluded_targets(settings) else {
        return;
    };
    let model = ui.global::<RemoteWorkerModel>();
    model.set_use_remote_name(name.into());
    model.set_use_remote_final_too(excluded.final_pictures);
    model.set_use_remote_prompt_open(true);
}

/// Wires `use_remote_answered`: closes the question, and on "Use <name>" saves and shows the
/// new targets.
pub(super) fn setup_use_remote_callback(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let render_ctx = render_ctx.clone();
    let settings_store = settings_store.clone();
    let ui_weak = ui.as_weak();
    ui.global::<RemoteWorkerModel>()
        .on_use_remote_answered(move |use_remote: bool| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            ui.global::<RemoteWorkerModel>()
                .set_use_remote_prompt_open(false);
            if !use_remote {
                return;
            }
            settings_store.update(|s| include_remote(&mut s.settings));
            let settings = settings_store.snapshot().settings;
            {
                // Like the Live Compute pill: an actual change releases a settled remote epoch.
                let mut ctx = RenderContext::lock(&render_ctx);
                if ctx.live_compute_target != settings.live_compute_target {
                    if ctx.remote_active || ctx.live_epoch.is_some() {
                        ctx.release_remote();
                    }
                    ctx.live_compute_target = settings.live_compute_target;
                }
            }
            ui.global::<SettingsModel>().set_live_compute_target_index(
                super::live_compute_target_index(settings.live_compute_target),
            );
            // The still export dialog is seeded from the saved values each time it opens; the
            // tilt-video section is seeded once, so its pill is moved here.
            ui.global::<TiltVideoExportModel>()
                .set_compute_target_index(settings.tilt_video_compute_target.index());
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_is_asked_when_the_defaults_already_use_the_remote() {
        assert_eq!(excluded_targets(&AppSettings::default()), None);
    }

    #[test]
    fn remote_only_targets_are_never_asked_about() {
        let mut s = AppSettings {
            live_compute_target: LiveComputeTarget::RemoteOnly,
            tilt_video_compute_target: TiltVideoCompute::Remote,
            ..AppSettings::default()
        };
        s.output.still.compute = TiltVideoCompute::Remote;
        assert_eq!(excluded_targets(&s), None);
    }

    #[test]
    fn live_on_this_computer_only_asks_about_live_alone() {
        let s = AppSettings {
            live_compute_target: LiveComputeTarget::LocalOnly,
            ..AppSettings::default()
        };
        assert_eq!(
            excluded_targets(&s),
            Some(Excluded {
                live: true,
                final_pictures: false
            })
        );
    }

    #[test]
    fn a_local_only_export_asks_about_final_pictures() {
        let s = AppSettings {
            tilt_video_compute_target: TiltVideoCompute::Local,
            ..AppSettings::default()
        };
        assert_eq!(
            excluded_targets(&s),
            Some(Excluded {
                live: false,
                final_pictures: true
            })
        );
        let mut s = AppSettings::default();
        s.output.still.compute = TiltVideoCompute::Local;
        assert!(excluded_targets(&s).is_some_and(|e| e.final_pictures));
    }

    #[test]
    fn confirming_sets_excluded_targets_to_both_and_leaves_the_rest() {
        let mut s = AppSettings {
            live_compute_target: LiveComputeTarget::LocalOnly,
            tilt_video_compute_target: TiltVideoCompute::Local,
            ..AppSettings::default()
        };
        s.output.still.compute = TiltVideoCompute::Remote;
        include_remote(&mut s);
        assert_eq!(s.live_compute_target, LiveComputeTarget::Both);
        assert_eq!(s.tilt_video_compute_target, TiltVideoCompute::Both);
        assert_eq!(s.output.still.compute, TiltVideoCompute::Remote);
        assert_eq!(excluded_targets(&s), None);
    }
}
