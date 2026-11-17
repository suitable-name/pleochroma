//! Callbacks of the "Remote coordinator" dialog and the related remote-render controls.
//!
//! They cover save and remove of the one remote endpoint, "Test connection", token
//! enrollment, the global denoise toggle, the live compute-target picker, and the
//! final-picture export switch.

use super::{live_compute_target_from_index, worker_settings::from_worker_item};
use crate::{
    MainWindow, RemoteWorkerModel, SettingsModel, WorkerItem,
    bridge::{export_thread, remote::remote_render, render_thread::RenderContext},
    gui::{
        remote::{RemoteOrchestratorHandle, refresh_remote_ui},
        show_toast,
    },
    settings::{SettingsPersister, WorkerSettings},
};
use slint::ComponentHandle;
use std::sync::{Arc, Mutex};

// ---- Remote endpoint form + "Test connection" + denoise toggle -------------------

/// Wires every callback of the "Remote coordinator" dialog and the global denoise toggle.
///
/// `orchestrator` is the handle `setup_remote_rendering` returns; the denoise toggle needs it (see
/// `setup_denoise_toggle_callback`).
pub fn setup_worker_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
    orchestrator: &RemoteOrchestratorHandle,
) {
    setup_save_remote_callback(ui, render_ctx, settings_store);
    setup_remove_remote_callback(ui, settings_store);
    setup_denoise_toggle_callback(ui, render_ctx, settings_store, orchestrator);
    setup_live_compute_target_callback(ui, render_ctx, settings_store);
    setup_contribute_to_final_picture_callback(ui, settings_store);
    setup_claim_token_callback(ui);
    super::pairing::setup_pairing_callbacks(ui, render_ctx, settings_store);
    super::use_remote::setup_use_remote_callback(ui, render_ctx, settings_store);
    super::pairing::install_rediscovery(ui, settings_store);
    setup_test_worker_connection_callback(ui);
    setup_cert_dir_picker_callback(ui);
    super::advanced_in_use::setup_advanced_in_use_callback(ui);
}

/// Wires `on_save_remote`, which replaces the one endpoint.
///
/// The form includes the merged live transfer. Forgets any remembered refusal of final
/// pictures, since the address may now point at an upgraded coordinator. A changed live
/// transfer releases a live settled epoch (`RenderContext::release_remote`), as a
/// live-compute-target change does; the orchestrator re-dispatches the view on its next tick.
fn setup_save_remote_callback(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let settings_store_save = settings_store.clone();
    let render_ctx_save = render_ctx.clone();
    let ui_weak_save = ui.as_weak();
    ui.global::<RemoteWorkerModel>()
        .on_save_remote(move |item: WorkerItem| {
            let Some(ui) = ui_weak_save.upgrade() else {
                return;
            };
            let mut live_changed = false;
            settings_store_save.update(|s| {
                let endpoint = from_worker_item(&item, s.settings.remote.as_ref());
                live_changed = s.settings.remote.as_ref().is_some_and(|old| {
                    old.live_transfer != endpoint.live_transfer
                        || old.connection.transfer_mode != endpoint.connection.transfer_mode
                });
                s.settings.remote = Some(endpoint);
            });
            if live_changed {
                let mut ctx = RenderContext::lock(&render_ctx_save);
                if ctx.remote_active || ctx.live_epoch.is_some() {
                    ctx.release_remote();
                }
            }
            export_thread::forget_final_picture_refusals();
            let settings = settings_store_save.snapshot().settings;
            refresh_remote_ui(&ui, settings.remote.as_ref());
            crate::gui::render::output_defaults::follow_remote_default_for_tilt(&ui, &settings);
            show_toast(&ui, "Remote coordinator saved.", "success");
        });
}

/// Wires `on_remove_remote`: renders stay local afterwards.
fn setup_remove_remote_callback(ui: &MainWindow, settings_store: &Arc<SettingsPersister>) {
    let settings_store_remove = settings_store.clone();
    let ui_weak_remove = ui.as_weak();
    ui.global::<RemoteWorkerModel>().on_remove_remote(move || {
        let Some(ui) = ui_weak_remove.upgrade() else {
            return;
        };
        let mut removed_cert_dir = None;
        settings_store_remove.update(|s| {
            removed_cert_dir = s
                .settings
                .remote
                .take()
                .map(|remote| remote.connection.cert_dir);
        });
        // The published route belonged to the removed remote; a later form that reuses its
        // folder must start as a hand-entered one.
        if let Some(cert_dir) = removed_cert_dir {
            crate::bridge::remote::route::remove(&cert_dir);
        }
        refresh_remote_ui(&ui, None);
        crate::gui::render::output_defaults::follow_remote_default_for_tilt(
            &ui,
            &settings_store_remove.snapshot().settings,
        );
        show_toast(
            &ui,
            "Remote coordinator removed -- renders stay local.",
            "info",
        );
    });
}

/// Wires the Render & Output tab's switch for this machine rendering a share of final-picture
/// exports.
///
/// Unlike [`setup_live_compute_target_callback`], this only persists the choice: an export reads
/// the settings snapshot each time it dispatches (`gui::render::render_export::wiring`,
/// `gui::tilt::video_export`), so there is no `RenderContext` field to keep in step.
fn setup_contribute_to_final_picture_callback(
    ui: &MainWindow,
    settings_store: &Arc<SettingsPersister>,
) {
    let settings_store = settings_store.clone();
    ui.global::<SettingsModel>()
        .on_contribute_to_final_picture_changed(move |on: bool| {
            settings_store.update(|s| s.settings.contribute_to_final_picture = on);
        });
}

/// Wires the global denoise toggle.
fn setup_denoise_toggle_callback(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
    orchestrator: &RemoteOrchestratorHandle,
) {
    let settings_store_denoise = settings_store.clone();
    let render_ctx_denoise = render_ctx.clone();
    let orchestrator_denoise = orchestrator.clone();
    let ui_weak_denoise = ui.as_weak();
    ui.global::<RemoteWorkerModel>()
        .on_denoise_toggled(move |enabled: bool| {
            // Updates `RenderContext` at once (the local render loop and the remote redraw in
            // `orchestrator::tick::update::redraw_from_epoch` both read it) and persists the
            // choice. Other live render settings take the same two steps (e.g.
            // `on_bounces_changed` in `gui::render::quality`).
            let mut ctx = RenderContext::lock(&render_ctx_denoise);
            ctx.denoise_enabled = enabled;
            // One-shot re-tonemap: a converged accumulation would otherwise never read
            // `denoise_enabled`. See `RenderContext::redisplay_requested`.
            ctx.redisplay_requested = true;
            drop(ctx);
            settings_store_denoise.update(|s| s.settings.denoise_enabled = enabled);
            if let Some(ui) = ui_weak_denoise.upgrade() {
                // Both switches (this dialog's and the Render & Output tab's) show
                // `denoise_enabled` and only ask for the flip through this callback,
                // so the property must follow the choice or neither switch moves.
                ui.global::<RemoteWorkerModel>()
                    .set_denoise_enabled(enabled);
                // `RemoteOnly` suspends the render loop, so the flag above is never read:
                // force the orchestrator's display path (a no-op in other targets, see
                // `request_redisplay`).
                orchestrator_denoise.request_redisplay(&ui, &render_ctx_denoise);
            }
        });
}

/// Wires `on_test_worker_connection`: probes the candidate address and cert dir on a
/// background thread and reports compatibility back on the UI thread.
fn setup_test_worker_connection_callback(ui: &MainWindow) {
    let ui_weak_test = ui.as_weak();
    ui.global::<RemoteWorkerModel>().on_test_worker_connection(
        move |address: slint::SharedString, cert_dir: slint::SharedString| {
            let Some(ui) = ui_weak_test.upgrade() else {
                return;
            };
            ui.global::<RemoteWorkerModel>()
                .set_testing_connection(true);
            let worker = WorkerSettings {
                address: address.to_string(),
                cert_dir: cert_dir.to_string(),
                ..WorkerSettings::default()
            };
            let ui_weak_result = ui.as_weak();
            std::thread::spawn(move || {
                let result = remote_render::test_connection(&worker);
                let _ = ui_weak_result.upgrade_in_event_loop(move |ui| {
                    ui.global::<RemoteWorkerModel>()
                        .set_testing_connection(false);
                    match result {
                        Ok(info) => {
                            ui.global::<RemoteWorkerModel>()
                                .set_test_connection_is_error(false);
                            ui.global::<RemoteWorkerModel>().set_test_connection_result(
                                format!(
                                    "Compatible -- {} (protocol v{})",
                                    backend_label(info.render.as_ref()),
                                    info.protocol_version
                                )
                                .into(),
                            );
                        }
                        Err(err) => {
                            ui.global::<RemoteWorkerModel>()
                                .set_test_connection_is_error(true);
                            ui.global::<RemoteWorkerModel>()
                                .set_test_connection_result(err.to_string().into());
                        }
                    }
                });
            });
        },
    );
}

/// Wires the native folder picker for `form_cert_dir` (the "Browse..." button beside the
/// certificate-bundle-folder field in `remote_worker_dialog.slint`).
///
/// Only fills the field; the form's save and test actions validate it, whichever way the
/// folder was typed, pasted, or claimed via enrollment token. Returns the SAME text on cancel,
/// so the Slint-side assignment is a no-op then.
///
/// The dialog runs off the UI thread through `gui::pickers::pick`. Slint cannot await a
/// value-returning callback, so `on_pick_cert_dir` is void (`ui/models/remote_worker.slint`):
/// this closure pushes the chosen folder into `RemoteWorkerModel.picked_cert_dir`, and
/// `RemoteWorkerDialog`'s `changed picked_cert_dir` handler copies it into `form_cert_dir`.
/// Left unchanged when the user cancels.
fn setup_cert_dir_picker_callback(ui: &MainWindow) {
    let ui_weak = ui.as_weak();
    ui.global::<RemoteWorkerModel>()
        .on_pick_cert_dir(move |current: slint::SharedString| {
            use crate::gui::pickers::{PickerKind, PickerRequest, pick};

            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            // Cleared before the dialog opens: Slint `changed` handlers do not fire for a
            // same-value write, so re-picking the folder already held would be dropped.
            // Clearing first makes the completion below a real `"" -> <folder>` change.
            ui.global::<RemoteWorkerModel>()
                .set_picked_cert_dir("".into());
            let starting_dir = crate::gui::starting_dir_from_picker_field(current.as_str());
            let request = PickerRequest {
                kind: PickerKind::PickFolder,
                title: None,
                filters: Vec::new(),
                default_file_name: None,
                starting_dir,
            };
            pick(&ui, request, |ui, picked| {
                if let Some(path) = picked {
                    ui.global::<RemoteWorkerModel>()
                        .set_picked_cert_dir(path.display().to_string().into());
                }
            });
        });
}

/// Wires the "Live Compute" picker (the Parameters panel's Local/Remote/Local+Remote pills).
///
/// Updates `RenderContext` and persists the choice, the same two steps as `on_denoise_toggled`.
/// The orchestrator reads the target at its next settle (`RenderContext::live_compute_target`).
/// An actual change while an epoch is live releases it (`RenderContext::release_remote`): local
/// claims indices from the epoch's cursor while combining, so switching mid-epoch would
/// re-trace indices already taken, and `RemoteOnly` <-> `Both` changes who owns the display.
/// The orchestrator cancels the epoch's remote lane on its next tick.
fn setup_live_compute_target_callback(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let settings_store_compute = settings_store.clone();
    let render_ctx_compute = render_ctx.clone();
    ui.global::<SettingsModel>()
        .on_live_compute_target_changed(move |index: i32| {
            let target = live_compute_target_from_index(index);
            {
                let mut ctx = RenderContext::lock(&render_ctx_compute);
                if ctx.live_compute_target != target
                    && (ctx.remote_active || ctx.live_epoch.is_some())
                {
                    ctx.release_remote();
                }
                ctx.live_compute_target = target;
            }
            settings_store_compute.update(|s| s.settings.live_compute_target = target);
        });
}

/// Wires the "Redeem token" action in the worker add/edit form: given a worker name, an
/// enrollment address, and a pasted `GW1-...` token, claims the token and writes the
/// resulting certificate bundle to disk -- so a user enrolling a new worker never has to
/// install `pleochroma-worker` or run `cert claim` in a terminal. See `bridge::remote::enroll`'s
/// module doc comment for where the bundle is written and why the user never chooses the
/// path.
///
/// Follows `on_test_worker_connection`'s exact shape, just above: the actual claim
/// (`bridge::remote::enroll::claim_and_write_bundle`, a blocking TCP connect and TLS handshake)
/// runs on a plain `std::thread::spawn` thread, with the result marshalled back to the
/// Slint event loop via `Weak::upgrade_in_event_loop` -- see `bridge::export_thread`'s
/// module doc comment for the general pattern this and `on_test_worker_connection` both
/// follow.
///
/// The token is read out of the callback's own argument and moved into the background
/// closure; nothing here logs it, stores it in `settings::WorkerSettings`, or otherwise
/// keeps a copy once the claim attempt (success or failure) completes -- `claim_result_text`/
/// `claimed_cert_dir` report the outcome back to the dialog, never the token itself. The
/// dialog clears its `form_token` field on success (its `changed claimed_cert_dir` handler).
fn setup_claim_token_callback(ui: &MainWindow) {
    let ui_weak = ui.as_weak();
    ui.global::<RemoteWorkerModel>().on_claim_token(
        move |worker_name: slint::SharedString,
              enroll_addr: slint::SharedString,
              token: slint::SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            ui.global::<RemoteWorkerModel>().set_claiming_token(true);
            ui.global::<RemoteWorkerModel>()
                .set_claim_result_text("".into());
            ui.global::<RemoteWorkerModel>()
                .set_claimed_cert_dir("".into());
            ui.global::<RemoteWorkerModel>()
                .set_claimed_address("".into());

            let settings_dir = crate::settings::store::default_settings_path()
                .parent()
                .map(std::path::Path::to_path_buf)
                .unwrap_or_default();
            let worker_name = worker_name.to_string();
            let enroll_addr = enroll_addr.to_string();
            let token = token.to_string();
            let suggested_address =
                crate::bridge::remote::enroll::suggested_serve_address(&enroll_addr);
            let ui_weak_result = ui.as_weak();
            std::thread::spawn(move || {
                let bundle_dir =
                    crate::bridge::remote::enroll::bundle_dir_for(&settings_dir, &worker_name);
                let result = crate::bridge::remote::enroll::claim_and_write_bundle(
                    &token,
                    &enroll_addr,
                    &bundle_dir,
                );
                let _ = ui_weak_result.upgrade_in_event_loop(move |ui| {
                    ui.global::<RemoteWorkerModel>().set_claiming_token(false);
                    match result {
                        Ok(dir) => {
                            ui.global::<RemoteWorkerModel>()
                                .set_claim_result_is_error(false);
                            ui.global::<RemoteWorkerModel>().set_claim_result_text(
                                "Token redeemed -- certificate folder filled in.".into(),
                            );
                            ui.global::<RemoteWorkerModel>()
                                .set_claimed_cert_dir(dir.display().to_string().into());
                            ui.global::<RemoteWorkerModel>()
                                .set_claimed_address(suggested_address.into());
                        }
                        Err(message) => {
                            ui.global::<RemoteWorkerModel>()
                                .set_claim_result_is_error(true);
                            ui.global::<RemoteWorkerModel>()
                                .set_claim_result_text(message.into());
                        }
                    }
                });
            });
        },
    );
}

/// Human-readable description of what a connected server can render.
///
/// Takes the whole `Option`, because a server may have no render capacity: a worker built
/// without its `worker` feature advertises `render: None`, and the label says so plainly
/// rather than implying a renderer.
pub(super) fn backend_label(render: Option<&pleochroma_net::messages::RenderCapability>) -> String {
    match render.map(|r| &r.backend) {
        Some(pleochroma_net::messages::Backend::Cpu { threads }) => {
            format!("CPU, {threads} threads")
        }
        Some(pleochroma_net::messages::Backend::Gpu { adapter }) => format!("GPU ({adapter})"),
        Some(pleochroma_net::messages::Backend::Coordinator { workers, .. }) => {
            coordinator_label(*workers)
        }
        None => "library only (no render capacity)".to_string(),
    }
}

/// "coordinator (N workers)" -- what "served by" names a coordinator, counting the
/// workers joined to it (its own `--render` lane, if any, is not a worker).
fn coordinator_label(workers: u32) -> String {
    if workers == 1 {
        "coordinator (1 worker)".to_string()
    } else {
        format!("coordinator ({workers} workers)")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_net::messages::{Backend, RenderCapability};

    fn capability(backend: Backend) -> RenderCapability {
        RenderCapability {
            backend,
            max_pixels: 1,
            min_cadence_ms: 100,
            hdr: false,
        }
    }

    #[test]
    fn served_by_names_a_coordinator_by_its_worker_count() {
        let three = capability(Backend::Coordinator {
            workers: 3,
            threads: 16,
            gpus: 1,
        });
        assert_eq!(backend_label(Some(&three)), "coordinator (3 workers)");
        let one = capability(Backend::Coordinator {
            workers: 1,
            threads: 0,
            gpus: 0,
        });
        assert_eq!(backend_label(Some(&one)), "coordinator (1 worker)");
        assert_eq!(
            backend_label(Some(&capability(Backend::Cpu { threads: 8 }))),
            "CPU, 8 threads"
        );
        assert_eq!(backend_label(None), "library only (no render capacity)");
    }
}
