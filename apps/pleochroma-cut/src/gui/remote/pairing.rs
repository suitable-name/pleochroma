//! "Paste pairing link" of the Remote Coordinator dialog, and finding a moved coordinator again.
//!
//! The dialog sends the pasted text to `on_read_pairing_link` and clears its field at once.
//! The link is parsed ([`crate::bridge::remote::pair::read_pairing_link`]) and, if it is a
//! viewer code, held as a [`PairOffer`] while the dialog asks "Pair with <name>?" (with "This
//! replaces <name>" when a coordinator is already saved). Only `on_confirm_pairing` connects:
//! it redeems the code over every address of the link on a background thread, writes the
//! certificate bundle, and saves the endpoint with its address list, enrollment port and the
//! fixed-name mark. The code is never logged, stored in the settings or kept after use.
//!
//! [`install_rediscovery`] gives `bridge::remote::rediscover` the function that runs when no
//! saved address answers: it searches the local network for the coordinator of the saved
//! certificate authority, checks it with a real handshake using the saved certificates, and
//! only then saves the address it moved to.

use super::worker_settings::{address_summary, publish_route, to_worker_item};
use crate::{
    MainWindow, RemoteWorkerModel,
    bridge::{
        export_thread,
        remote::{
            pair::{self, PairOffer, Paired},
            rediscover,
            remote_render::verify_announced,
        },
        render_thread::RenderContext,
    },
    gui::{remote::refresh_remote_ui, show_toast},
    settings::{RemoteEndpoint, SettingsPersister},
};
use pleochroma_net::discovery::Found;
use slint::ComponentHandle;
use std::sync::{Arc, Mutex, PoisonError};

/// `RemoteWorkerModel.pair_stage`: nothing is pending, the paste field shows.
const STAGE_PASTE: i32 = 0;
/// `RemoteWorkerModel.pair_stage`: the link was read and waits for the user to confirm.
const STAGE_CONFIRM: i32 = 1;
/// `RemoteWorkerModel.pair_stage`: pairing runs on a background thread.
const STAGE_PAIRING: i32 = 2;

/// The offer a read link produced, held until the user confirms or cancels.
type PendingOffer = Arc<Mutex<Option<PairOffer>>>;

/// Wires the three pairing callbacks of `RemoteWorkerModel`.
pub fn setup_pairing_callbacks(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
) {
    let pending: PendingOffer = Arc::new(Mutex::new(None));
    setup_read_link_callback(ui, settings_store, &pending);
    setup_confirm_callback(ui, render_ctx, settings_store, &pending);
    setup_cancel_callback(ui, &pending);
}

/// Wires `on_read_pairing_link`: parse the pasted text and show the confirm step.
fn setup_read_link_callback(
    ui: &MainWindow,
    settings_store: &Arc<SettingsPersister>,
    pending: &PendingOffer,
) {
    let settings_store = settings_store.clone();
    let pending = pending.clone();
    let ui_weak = ui.as_weak();
    ui.global::<RemoteWorkerModel>()
        .on_read_pairing_link(move |text: slint::SharedString| {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let model = ui.global::<RemoteWorkerModel>();
            match pair::read_pairing_link(text.as_str()) {
                Ok(offer) => {
                    let replaces = settings_store
                        .snapshot()
                        .settings
                        .remote
                        .as_ref()
                        .map(RemoteEndpoint::display_name)
                        .unwrap_or_default();
                    model.set_pair_name(offer.name().into());
                    model.set_pair_address_summary(offer.address_summary().into());
                    model.set_pair_replaces(replaces.into());
                    model.set_pair_error("".into());
                    model.set_pair_stage(STAGE_CONFIRM);
                    *pending.lock().unwrap_or_else(PoisonError::into_inner) = Some(offer);
                }
                Err(words) => {
                    *pending.lock().unwrap_or_else(PoisonError::into_inner) = None;
                    model.set_pair_error(words.into());
                    model.set_pair_stage(STAGE_PASTE);
                }
            }
        });
}

/// Wires `on_confirm_pairing`: redeem the code on a background thread, then save the endpoint
/// on the UI thread.
fn setup_confirm_callback(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
    pending: &PendingOffer,
) {
    let render_ctx = render_ctx.clone();
    let settings_store = settings_store.clone();
    let pending = pending.clone();
    let ui_weak = ui.as_weak();
    ui.global::<RemoteWorkerModel>()
        .on_confirm_pairing(move || {
            let Some(ui) = ui_weak.upgrade() else {
                return;
            };
            let Some(offer) = pending
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .take()
            else {
                return;
            };
            let model = ui.global::<RemoteWorkerModel>();
            model.set_pair_error("".into());
            model.set_pair_stage(STAGE_PAIRING);

            let settings_dir = crate::settings::store::default_settings_path()
                .parent()
                .map(std::path::Path::to_path_buf)
                .unwrap_or_default();
            let ui_weak_result = ui.as_weak();
            let render_ctx = render_ctx.clone();
            let settings_store = settings_store.clone();
            let pending = pending.clone();
            std::thread::spawn(move || {
                let result = pair::pair(&offer, &settings_dir);
                let _ = ui_weak_result.upgrade_in_event_loop(move |ui| {
                    finish_pairing(&ui, &render_ctx, &settings_store, &pending, offer, result);
                });
            });
        });
}

/// Wires `on_cancel_pairing`: forget the pending offer and show the paste field again.
fn setup_cancel_callback(ui: &MainWindow, pending: &PendingOffer) {
    let pending = pending.clone();
    let ui_weak = ui.as_weak();
    ui.global::<RemoteWorkerModel>().on_cancel_pairing(move || {
        *pending.lock().unwrap_or_else(PoisonError::into_inner) = None;
        if let Some(ui) = ui_weak.upgrade() {
            let model = ui.global::<RemoteWorkerModel>();
            model.set_pair_error("".into());
            model.set_pair_stage(STAGE_PASTE);
        }
    });
}

/// Applies the result of [`pair::pair`] on the UI thread.
///
/// On success the endpoint is saved like the by-hand form saves it (`save_remote`): the stream
/// and transfer preferences of a replaced coordinator stay, a live remote epoch bound to the
/// old coordinator is released, remembered refusals of final pictures are forgotten and the
/// dialog and the tilt-video default follow. On failure the offer is kept, so the same code
/// can be tried again, and the dialog stays on the confirm step with the reason. When the dialog
/// was closed meanwhile, the offer is dropped instead and the paste field shows next time.
fn finish_pairing(
    ui: &MainWindow,
    render_ctx: &Arc<Mutex<RenderContext>>,
    settings_store: &Arc<SettingsPersister>,
    pending: &PendingOffer,
    offer: PairOffer,
    result: Result<Paired, String>,
) {
    let model = ui.global::<RemoteWorkerModel>();
    match result {
        Ok(paired) => {
            let cert_dir = paired.cert_dir.display().to_string();
            let mut replaced = false;
            settings_store.update(|s| {
                let base = s.settings.remote.clone();
                replaced = base.is_some();
                let next = base.unwrap_or_default().paired(
                    &paired.name,
                    paired.addresses.clone(),
                    paired.enroll_port,
                    &cert_dir,
                );
                s.settings.remote = Some(next);
            });
            if replaced {
                let mut ctx = RenderContext::lock(render_ctx);
                if ctx.remote_active || ctx.live_epoch.is_some() {
                    ctx.release_remote();
                }
            }
            export_thread::forget_final_picture_refusals();
            let settings = settings_store.snapshot().settings;
            refresh_remote_ui(ui, settings.remote.as_ref());
            crate::gui::render::output_defaults::follow_remote_default_for_tilt(ui, &settings);
            model.set_pair_error("".into());
            model.set_pair_stage(STAGE_PASTE);
            show_toast(ui, &format!("Paired with {}.", paired.name), "success");
            // Only a pairing that the render settings would leave unused asks to be used.
            super::use_remote::ask_if_unused(ui, &settings, &paired.name);
        }
        Err(words) => {
            if model.get_pair_dialog_closed() {
                // Nobody is looking: the offer and its code are dropped, not kept for a retry.
                drop(offer);
                model.set_pair_error("".into());
                model.set_pair_stage(STAGE_PASTE);
            } else {
                *pending.lock().unwrap_or_else(PoisonError::into_inner) = Some(offer);
                model.set_pair_error(words.into());
                model.set_pair_stage(STAGE_CONFIRM);
            }
        }
    }
}

/// Installs what runs when no saved address of a link-paired coordinator answers: search the
/// local network for it ([`rediscover::find`], matched by the saved certificate authority and
/// proven by a handshake with the saved certificates), save the address list it announces and
/// say so in plain words, for example "Studio PC moved to 192.168.1.40". A fake announcement
/// fails the handshake and never changes the saved settings.
pub fn install_rediscovery(ui: &MainWindow, settings_store: &Arc<SettingsPersister>) {
    let settings_store = settings_store.clone();
    // `slint::Weak` is only `Send`; the mutex makes the handler `Sync`.
    let ui_weak = Mutex::new(ui.as_weak());
    rediscover::set_handler(move |cert_dir: &str| {
        let is_this_remote = |endpoint: &RemoteEndpoint| {
            endpoint.fixed_name && endpoint.connection.cert_dir == cert_dir
        };
        let Some(saved) = settings_store
            .snapshot()
            .settings
            .remote
            .filter(is_this_remote)
        else {
            return;
        };
        // An announcement is only a claim: anyone on the network can copy the public
        // certificate authority tag. Only a coordinator that completes the handshake with the
        // saved certificates (fixed-name verification) is accepted, and until then nothing is
        // saved. A failed candidate does not end the search.
        let worker = saved.connection.clone();
        let verify = |found: &Found| {
            let addresses = rediscover::addresses_for(found);
            !addresses.is_empty() && verify_announced(&worker, &addresses).is_ok()
        };
        let Ok(found) = rediscover::find(cert_dir, rediscover::SEARCH_TIME, verify) else {
            return;
        };
        let addresses = rediscover::addresses_for(&found);
        if addresses.is_empty() || addresses == saved.endpoint_addresses() {
            return;
        }
        let status = rediscover::moved_status(&saved.connection.name, &addresses);
        let mut moved = None;
        settings_store.update(|s| {
            // The saved remote may have changed while the search ran: only its own entry moves.
            moved = s
                .settings
                .remote
                .as_ref()
                .filter(|r| is_this_remote(r))
                .map(|r| r.moved_to(addresses.clone()));
            if let Some(next) = &moved {
                s.settings.remote = Some(next.clone());
            }
        });
        let Some(moved) = moved else {
            return;
        };
        publish_route(Some(&moved));
        tracing::info!(
            "remote coordinator found at a new address ({})",
            moved.connection.address
        );
        let ui_weak = ui_weak.lock().unwrap_or_else(PoisonError::into_inner);
        let _ = ui_weak.upgrade_in_event_loop(move |ui| {
            let model = ui.global::<RemoteWorkerModel>();
            model.set_endpoint(to_worker_item(&moved));
            model.set_endpoint_address_summary(address_summary(&moved).into());
            model.set_rediscovery_status(status.clone().into());
            show_toast(&ui, &status, "info");
        });
    });
}
