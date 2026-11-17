//! Remote setup commands use a separate worker and reject replaced connection results.
//!
//! Forget Server also lives here: a confirmation dialog (id [`DIALOG_FORGET`], answered
//! in [`handle_dialog_result`]), then the worker erases the stored credentials and the UI
//! thread clears the saved server and leaves the Remote render backend.
//!
//! Pairing by link lives here too. The Android deep link `pleochroma://pair?...` reaches
//! Rust through the JNI native `MainActivity.nativePairingLink` (defined at the bottom of
//! this file, Android only), which calls [`receive_pairing_link`] from any thread; the
//! "Paste pairing link" field calls the same path. A link never connects by itself: it
//! opens the confirm sheet ("Pair with <name>?", plus "This replaces <name>" when the phone
//! is already paired), and only the Pair button starts [`remote::pair`].
//!
//! The "Scan QR code" button opens the Kotlin `ScannerActivity` ([`bridge::start_qr_scan`]);
//! a scanned code comes back through the same `nativePairingLink` as a deep link, so it also
//! ends in the confirm sheet. Finding the computer again lives here too: when a connection
//! test, a remote render, an export or a jobs request reaches none of the saved addresses
//! ([`connect_failed`]), [`begin_rediscovery`] browses the local network (Kotlin
//! `NsdDiscovery`), and an announcement of the paired CA is connected to with the saved
//! credentials before its addresses replace the saved ones
//! ([`pleochroma_mobile_core::remote_discovery`]). A browse ends by itself, and Kotlin
//! tells Rust when it does ([`rediscovery_ended`]), so the next failure starts a new one. A
//! remote render that failed on the old address starts over once the computer is found.
use crate::{AppWindow, JobRow, RemoteModel, Shell, push::toast};
use pleochroma_mobile_core::{
    app_state::AppState,
    layout::Destination,
    remote::{self, ConnectionReport, PairedServer, PairingOffer},
    remote_discovery::{self, Announcement},
    settings::{RemoteTransfer, RenderBackend},
    tasks::BackgroundQueue,
};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::{
    cell::RefCell,
    collections::VecDeque,
    rc::Rc,
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, AtomicU8, Ordering},
    },
};
use zeroize::Zeroizing;

/// A pairing link waiting for the window, and the window to hand it to. The deep link can
/// arrive on a JNI thread, or before the window exists (a cold start from a link).
struct Inbox {
    pending: Option<Zeroizing<String>>,
    window: Option<slint::Weak<AppWindow>>,
}
static INBOX: Mutex<Inbox> = Mutex::new(Inbox {
    pending: None,
    window: None,
});

fn inbox() -> MutexGuard<'static, Inbox> {
    INBOX.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Hands a pairing link to the app, from any thread. The text is a secret: it is wiped once
/// read and never logged. The confirm sheet opens on the UI thread; a link that arrives
/// before the Remote page is wired waits for [`wire`].
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub fn receive_pairing_link(text: &str) {
    let window = {
        let mut inbox = inbox();
        inbox.pending = Some(Zeroizing::new(text.to_owned()));
        inbox.window.clone()
    };
    if let Some(window) = window {
        let _queued = window.upgrade_in_event_loop(|window| take_pending_link(&window));
    }
}

/// Opens the sheet for the link waiting in the inbox, if any (on the UI thread).
fn take_pending_link(window: &AppWindow) {
    let pending = inbox().pending.take();
    if let Some(text) = pending {
        begin_pairing(window, &text);
    }
}

// ---------------------------------------------------------------------------------------
// Local network access
// ---------------------------------------------------------------------------------------
// Android 17 (API 37) blocks every connection to a device on the local network until the
// user allows it (`android.permission.ACCESS_LOCAL_NETWORK`, a runtime permission). Older
// versions never block, and Kotlin answers "granted" without a dialog. Everything that
// reaches the computer asks first through [`with_local_network`]: a pairing, a connection
// test, an enrollment, a server job request, a picture export, the live remote render and
// the network browse. A refusal is not remembered as final: the next attempt asks again.

/// The answer is not known yet (or a refusal may be asked again).
const LAN_UNKNOWN: u8 = 0;
/// The app may use the local network; it stays so until the process ends.
const LAN_GRANTED: u8 = 1;
/// The system dialog is open.
const LAN_ASKING: u8 = 2;
/// The user refused the last ask.
const LAN_DENIED: u8 = 3;
static LOCAL_NETWORK: AtomicU8 = AtomicU8::new(LAN_UNKNOWN);

/// What the user sees when the local network is refused.
pub const LOCAL_NETWORK_DENIED: &str =
    "Pleochroma needs access to devices on your local network to reach your computer.";

/// What waits for the answer of the system dialog.
type AfterAccess = Box<dyn FnOnce(&AppWindow) + Send>;
static AFTER_ACCESS: Mutex<Vec<AfterAccess>> = Mutex::new(Vec::new());

fn after_access() -> MutexGuard<'static, Vec<AfterAccess>> {
    AFTER_ACCESS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Whether the app may use the local network now.
#[must_use]
pub fn local_network_granted() -> bool {
    LOCAL_NETWORK.load(Ordering::Acquire) == LAN_GRANTED
}

/// Runs `then` (on the UI thread) once the app may use the local network. With the access
/// already there it runs at once; otherwise the system dialog opens first, and a refusal
/// shows [`LOCAL_NETWORK_DENIED`] and drops `then`. Several waiting calls share one dialog.
/// Call it before any state of the action changes, so a refusal leaves nothing half done.
pub fn with_local_network(window: &AppWindow, then: impl FnOnce(&AppWindow) + Send + 'static) {
    if local_network_granted() {
        then(window);
        return;
    }
    // Queued before the dialog opens, so an answer can never arrive ahead of it.
    after_access().push(Box::new(then));
    // Move to "asking" unless a dialog is already open or access is granted; only the call that
    // made the move opens the dialog. (A compare-exchange loop: `fetch_update` is deprecated.)
    let owns_the_ask = {
        let mut state = LOCAL_NETWORK.load(Ordering::Acquire);
        loop {
            if state == LAN_ASKING || state == LAN_GRANTED {
                break Err(state);
            }
            match LOCAL_NETWORK.compare_exchange_weak(
                state,
                LAN_ASKING,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(previous) => break Ok(previous),
                Err(current) => state = current,
            }
        }
    };
    match owns_the_ask {
        Ok(_) => match bridge::ensure_local_network() {
            Ok(false) => {}
            // Kotlin said yes without a dialog (older Android, or allowed before).
            Ok(true) => local_network_answered(true),
            Err(error) => {
                // Not being able to ask must not lock the user out: try the connection.
                tracing::warn!(%error, "could not ask for local network access");
                LOCAL_NETWORK.store(LAN_UNKNOWN, Ordering::Release);
                release_waiting(true);
            }
        },
        Err(LAN_GRANTED) => release_waiting(true),
        // The dialog is open: its answer runs this call too.
        Err(_) => {}
    }
}

/// For starts the user did not just ask for (the live remote render, the network browse):
/// true when the access is there. While it is unknown the dialog opens once and `then`
/// runs after a yes; after a refusal nothing asks again until [`local_network_retry`].
#[must_use]
pub fn local_network_for_background(
    window: &AppWindow,
    then: impl FnOnce(&AppWindow) + Send + 'static,
) -> bool {
    match LOCAL_NETWORK.load(Ordering::Acquire) {
        LAN_GRANTED => true,
        LAN_UNKNOWN => {
            with_local_network(window, then);
            false
        }
        _ => false,
    }
}

/// The user touched the picture again: a refusal stands no longer, the next remote start
/// asks again.
pub fn local_network_retry() {
    let _swapped = LOCAL_NETWORK.compare_exchange(
        LAN_DENIED,
        LAN_UNKNOWN,
        Ordering::AcqRel,
        Ordering::Acquire,
    );
}

/// The system dialog was answered (any thread). A yes lets everything that waited go on; a
/// no tells the user once and drops it.
fn local_network_answered(granted: bool) {
    LOCAL_NETWORK.store(
        if granted { LAN_GRANTED } else { LAN_DENIED },
        Ordering::Release,
    );
    release_waiting(granted);
}

/// Lets what waited for the dialog go on (`granted`) or tells the user it did not (on any
/// thread; the work itself runs on the UI thread).
fn release_waiting(granted: bool) {
    let queued = std::mem::take(&mut *after_access());
    if queued.is_empty() {
        return;
    }
    let window = inbox().window.clone();
    let Some(window) = window else {
        return;
    };
    let _queued = window.upgrade_in_event_loop(move |window| {
        if granted {
            for then in queued {
                then(&window);
            }
        } else {
            local_network_refused(&window);
        }
    });
}

/// The user refused (on the UI thread): says why nothing happened and frees the sheet.
fn local_network_refused(window: &AppWindow) {
    let model = window.global::<RemoteModel>();
    let pairing = RUNTIME.with(|value| {
        value
            .borrow()
            .as_ref()
            .is_some_and(|runtime| runtime.pairing)
    });
    if !pairing {
        model.set_pair_busy(false);
    }
    model.set_status(LOCAL_NETWORK_DENIED.into());
    toast::show(window, LOCAL_NETWORK_DENIED);
}

/// The dialog id of "Forget Server?" (ids 4000-4099 belong to this page; `design::wire`
/// routes the answer to [`handle_dialog_result`]).
pub const DIALOG_FORGET: i32 = 4001;
/// The dialog after a new pairing that asks whether to render on that computer; answered
/// in [`handle_dialog_result`] (inside the Remote range 4000..=4099 of `design::dialog_owner`).
pub const DIALOG_RENDER_REMOTE: i32 = 4002;
/// The dialog after a new pairing that asks whether to copy the computer's designs to this
/// phone (a sync). It comes after the render question and the connection test; answered in
/// [`handle_dialog_result`] (inside the Remote range 4000..=4099 of `design::dialog_owner`).
pub const DIALOG_LIBRARY_SYNC: i32 = 4003;

/// The question the dialog asks. It names what is deleted, that it cannot be undone and
/// what stays on the server (the phone cannot revoke its own certificate there).
const FORGET_BODY: &str = concat!(
    "This deletes the pairing credentials on this phone, including the Android Keystore ",
    "key that protects them, and clears the saved server name and address. It cannot be ",
    "undone: to use the server again you must pair this phone with a new pairing code. ",
    "If Remote rendering is selected, it switches back to Automatic. The server keeps its ",
    "own list of paired phones and any server jobs until its operator removes them.",
);

/// What a new pairing still waits for before it may offer to copy the computer's designs:
/// the answer to "Render on <name>?" (when that was asked) and the connection test, which
/// says whether the computer shares a library at all.
#[derive(Debug, Clone, PartialEq, Eq)]
struct PostPair {
    /// The computer's name for the offer.
    name: String,
    /// "Render on <name>?" is on screen and not answered yet.
    render_pending: bool,
    /// What the connection test learned: `None` until it has finished.
    library: Option<bool>,
}

impl PostPair {
    /// A pairing that just installed; `render_asked` is whether 4002 was shown.
    fn new(name: &str, render_asked: bool) -> Self {
        Self {
            name: name.to_owned(),
            render_pending: render_asked,
            library: None,
        }
    }

    /// Whether the offer may show now: the render question is answered and the test found a
    /// library on the computer. A computer that only renders, or a test that failed (which
    /// drops the whole record), never gets the offer.
    const fn should_offer(&self) -> bool {
        !self.render_pending && matches!(self.library, Some(true))
    }
}

struct Runtime {
    window: slint::Weak<AppWindow>,
    state: Rc<RefCell<AppState>>,
    queue: BackgroundQueue,
    next: u64,
    active: Option<(u64, Arc<AtomicBool>)>,
    installing: bool,
    /// The worker is erasing the stored credentials; setup requests wait for it.
    forgetting: bool,
    /// The pairing link the confirm sheet is asking about; taken when the user confirms.
    offer: Option<PairingOffer>,
    /// A pairing is claiming its certificate on the worker (the sheet shows "Pairing…").
    pairing: bool,
    /// The computer a pairing just installed: the next connection test says "Connected to
    /// <name>." instead of the technical summary.
    announce: Option<String>,
    /// What the offer to copy the computer's designs waits for; set by a new pairing.
    post_pair: Option<PostPair>,
}
thread_local! { static RUNTIME: RefCell<Option<Runtime>> = const { RefCell::new(None) }; }
thread_local! {
    // A saved address alone does not establish an authenticated render connection.
    static RENDER_CONNECTION: RefCell<Option<String>> = const { RefCell::new(None) };
}

/// Whether this endpoint passed a render-capable handshake or delivered a render frame.
pub fn render_connected(app: &AppState) -> bool {
    app.settings.remote.enrolled
        && RENDER_CONNECTION
            .with(|value| value.borrow().as_deref() == Some(app.settings.remote.address.trim()))
}

/// Records connection evidence for the current endpoint, never across app restarts.
pub fn record_render_connection(app: &AppState, connected: bool) {
    RENDER_CONNECTION.with(|value| {
        *value.borrow_mut() = connected.then(|| app.settings.remote.address.trim().to_owned());
    });
}

pub fn wire(window: &AppWindow, state: &Rc<RefCell<AppState>>) {
    RUNTIME.with(|value| {
        *value.borrow_mut() = Some(Runtime {
            window: window.as_weak(),
            state: Rc::clone(state),
            queue: BackgroundQueue::new("remote-setup"),
            next: 0,
            active: None,
            installing: false,
            forgetting: false,
            offer: None,
            pairing: false,
            announce: None,
            post_pair: None,
        });
    });
    inbox().window = Some(window.as_weak());
    let model = window.global::<RemoteModel>();
    let app = state.borrow();
    model.set_name(app.settings.remote.name.as_str().into());
    model.set_address(app.settings.remote.address.as_str().into());
    model.set_paired(app.settings.remote.enrolled);
    model.set_transfer(i32::from(
        app.settings.remote.transfer == RemoteTransfer::FullData,
    ));
    model.set_cadence(i32::try_from(app.settings.remote.cadence_ms).unwrap_or(500));
    model.set_allow_metered(app.settings.remote.allow_metered);
    drop(app);
    crate::remote_jobs::wire(window, state);
    let weak = window.as_weak();
    model.on_save(move || {
        if let Some(window) = weak.upgrade() {
            save(&window);
        }
    });
    let weak = window.as_weak();
    model.on_enroll(move || {
        if let Some(window) = weak.upgrade() {
            with_local_network(&window, |window| start(window, true));
        }
    });
    let weak = window.as_weak();
    model.on_test(move || {
        if let Some(window) = weak.upgrade() {
            with_local_network(&window, |window| start(window, false));
        }
    });
    let weak = window.as_weak();
    model.on_cancel(move || {
        interrupt();
        if let Some(window) = weak.upgrade() {
            window.global::<RemoteModel>().set_token("".into());
        }
    });
    let weak = window.as_weak();
    model.on_hidden(move || {
        if let Some(window) = weak.upgrade() {
            dismiss_pairing(&window);
        }
        interrupt();
        if let Some(window) = weak.upgrade() {
            let model = window.global::<RemoteModel>();
            model.set_token("".into());
            model.set_link_text("".into());
        }
    });
    let weak = window.as_weak();
    model.on_close(move || {
        if let Some(window) = weak.upgrade() {
            // Back closes the confirm sheet first and leaves the page on the next press.
            if window.global::<RemoteModel>().get_pair_open() && dismiss_pairing(&window) {
                return;
            }
        }
        interrupt();
        if let Some(window) = weak.upgrade() {
            let model = window.global::<RemoteModel>();
            model.set_token("".into());
            model.set_link_text("".into());
            window.global::<Shell>().set_more_page(0);
        }
    });
    wire_pairing(window);
    let weak = window.as_weak();
    model.on_forget_server(move || {
        if let Some(window) = weak.upgrade() {
            ask_forget(&window);
        }
    });
    // A link that started the app arrived before this page existed; open its sheet once the
    // event loop runs, so the start-up code cannot move the page away afterwards.
    let weak = window.as_weak();
    let _queued = slint::invoke_from_event_loop(move || {
        if let Some(window) = weak.upgrade() {
            take_pending_link(&window);
        }
    });
}

/// Connects the pairing callbacks: the pasted link, the scanner and the confirm sheet.
fn wire_pairing(window: &AppWindow) {
    let model = window.global::<RemoteModel>();
    let weak = window.as_weak();
    model.on_pair_from_text(move || {
        if let Some(window) = weak.upgrade() {
            let model = window.global::<RemoteModel>();
            let text = Zeroizing::new(model.get_link_text().to_string());
            model.set_link_text("".into());
            begin_pairing(&window, &text);
        }
    });
    let weak = window.as_weak();
    model.on_scan_qr(move || {
        if let Some(window) = weak.upgrade() {
            // The camera permission is asked inside the scanner, at this tap.
            if let Err(message) = bridge::start_qr_scan() {
                toast::show(&window, &message);
            }
        }
    });
    let weak = window.as_weak();
    model.on_pair_confirm(move || {
        if let Some(window) = weak.upgrade() {
            if !local_network_granted() {
                // The sheet waits for the system dialog; a second tap does nothing.
                window.global::<RemoteModel>().set_pair_busy(true);
            }
            with_local_network(&window, confirm_pairing);
        }
    });
    let weak = window.as_weak();
    model.on_pair_cancel(move || {
        if let Some(window) = weak.upgrade() {
            dismiss_pairing(&window);
        }
    });
}

/// Shows the sheet's plain-word error (the sheet's question is replaced by it).
fn show_pair_error(model: &RemoteModel<'_>, message: &str) {
    model.set_pair_busy(false);
    model.set_pair_name("".into());
    model.set_pair_address("".into());
    model.set_pair_replaces("".into());
    model.set_pair_error(message.into());
    model.set_pair_open(true);
}

/// Reads a pairing link and opens the confirm sheet for it (on the UI thread). Nothing
/// connects until the user taps Pair. A link that cannot be used, or one that is for a
/// render computer, opens the sheet with a plain sentence instead.
fn begin_pairing(window: &AppWindow, text: &str) {
    let shell = window.global::<Shell>();
    shell.set_destination(3);
    shell.set_more_page(4);
    let model = window.global::<RemoteModel>();
    // The core's destination follows the shell's (the navigation bar sets both).
    if let Some(state) = RUNTIME.with(|value| {
        value
            .borrow()
            .as_ref()
            .map(|runtime| Rc::clone(&runtime.state))
    }) && let Ok(mut app) = state.try_borrow_mut()
    {
        app.set_destination(Destination::More);
    }
    let state = RUNTIME.with(|value| {
        let guard = value.borrow();
        let runtime = guard.as_ref()?;
        if runtime.installing || runtime.forgetting || runtime.pairing {
            return None;
        }
        Some(Rc::clone(&runtime.state))
    });
    let Some(state) = state else {
        show_pair_error(
            &model,
            "Credentials are being changed. Open the link again in a moment.",
        );
        return;
    };
    match remote::read_pairing_link(text) {
        Ok(offer) => {
            let replaces = {
                let app = state.borrow();
                let remote = &app.settings.remote;
                if !remote.enrolled {
                    String::new()
                } else if remote.name.trim().is_empty() {
                    "the computer this phone is paired with".to_owned()
                } else {
                    remote.name.trim().to_owned()
                }
            };
            model.set_pair_error("".into());
            model.set_pair_busy(false);
            model.set_pair_name(offer.name().as_str().into());
            model.set_pair_address(offer.address_summary().as_str().into());
            model.set_pair_replaces(replaces.as_str().into());
            model.set_pair_open(true);
            RUNTIME.with(|value| {
                if let Some(runtime) = value.borrow_mut().as_mut() {
                    runtime.offer = Some(offer);
                }
            });
        }
        Err(message) => {
            RUNTIME.with(|value| {
                if let Some(runtime) = value.borrow_mut().as_mut() {
                    runtime.offer = None;
                }
            });
            show_pair_error(&model, &message);
        }
    }
}

/// Closes the confirm sheet and drops the link it held; a pairing in flight is cancelled
/// and its result discarded. Returns false while the credentials are being installed,
/// which cannot be stopped (the sheet then stays until it finishes).
fn dismiss_pairing(window: &AppWindow) -> bool {
    let model = window.global::<RemoteModel>();
    let outcome = RUNTIME.with(|value| {
        let mut guard = value.borrow_mut();
        let Some(runtime) = guard.as_mut() else {
            return Some(false);
        };
        if runtime.installing && runtime.pairing {
            return None;
        }
        runtime.offer = None;
        let was_pairing = runtime.pairing;
        if was_pairing {
            runtime.pairing = false;
            if let Some((_, cancel)) = runtime.active.take() {
                cancel.store(true, Ordering::Release);
            }
        }
        Some(was_pairing)
    });
    let Some(was_pairing) = outcome else {
        return false;
    };
    if was_pairing {
        model.set_busy(false);
        model.set_status("Pairing cancelled.".into());
    }
    model.set_pair_open(false);
    model.set_pair_busy(false);
    model.set_pair_error("".into());
    model.set_pair_name("".into());
    model.set_pair_address("".into());
    model.set_pair_replaces("".into());
    true
}

/// The user tapped Pair or Replace and pair: claims the certificate over every address of
/// the link on a worker. The old pairing stays untouched until the new credentials are
/// installed.
fn confirm_pairing(window: &AppWindow) {
    let model = window.global::<RemoteModel>();
    // Server work that uses the old credentials stops first, as when forgetting a server.
    interrupt();
    stop_rediscovery();
    crate::design::render::cancel_remote_export();
    // The old computer's library goes with its pairing: a running sync stops first.
    crate::library::remote_changed(window);
    RUNTIME.with(|value| {
        let mut guard = value.borrow_mut();
        let Some(runtime) = guard.as_mut() else {
            return;
        };
        if runtime.installing || runtime.forgetting || runtime.pairing {
            return;
        }
        let Some(offer) = runtime.offer.take() else {
            return;
        };
        runtime.post_pair = None;
        runtime.next = runtime.next.wrapping_add(1);
        let id = runtime.next;
        let cancelled = Arc::new(AtomicBool::new(false));
        runtime.active = Some((id, Arc::clone(&cancelled)));
        runtime.pairing = true;
        model.set_busy(true);
        model.set_pair_busy(true);
        model.set_status("Pairing…".into());
        let weak = window.as_weak();
        runtime.queue.submit(move || {
            let outcome = remote::pair(&offer, &cancelled);
            drop(offer);
            let _queued = weak.upgrade_in_event_loop(move |window| {
                finish_pairing(&window, id, outcome);
            });
        });
    });
}

/// The claim ended (on the UI thread). A failure shows its plain sentence in the sheet; a
/// success installs the credentials on the worker and goes on in [`finish_install`].
fn finish_pairing(
    window: &AppWindow,
    id: u64,
    outcome: Result<(remote::Enrollment, PairedServer), String>,
) {
    RUNTIME.with(|value| {
        let mut guard = value.borrow_mut();
        let Some(runtime) = guard.as_mut() else {
            return;
        };
        if !runtime
            .active
            .as_ref()
            .is_some_and(|(run, cancel)| *run == id && !cancel.load(Ordering::Acquire))
        {
            // Cancelled or replaced: the claimed credentials are dropped (and wiped) here.
            return;
        }
        let model = window.global::<RemoteModel>();
        match outcome {
            Err(message) => {
                runtime.active = None;
                runtime.pairing = false;
                model.set_busy(false);
                model.set_status("Pairing did not finish.".into());
                show_pair_error(&model, &message);
            }
            Ok((enrollment, server)) => {
                // UI acceptance is the commit boundary, as for hand enrollment.
                runtime.installing = true;
                model.set_installing(true);
                model.set_status("Installing encrypted credentials…".into());
                let platform = Arc::clone(runtime.state.borrow().platform());
                let weak = window.as_weak();
                runtime.queue.submit(move || {
                    let result = enrollment.install(platform.as_ref());
                    let _queued = weak.upgrade_in_event_loop(move |window| {
                        finish_install(&window, &server, result);
                    });
                });
            }
        }
    });
}

/// The credentials of a pairing are installed or not (on the UI thread). On success the
/// settings remember the computer, the sheet closes and the connection is tested at once.
fn finish_install(window: &AppWindow, server: &PairedServer, result: Result<(), String>) {
    let model = window.global::<RemoteModel>();
    let state = RUNTIME.with(|value| {
        let mut guard = value.borrow_mut();
        let runtime = guard.as_mut()?;
        runtime.installing = false;
        runtime.pairing = false;
        runtime.active = None;
        Some(Rc::clone(&runtime.state))
    });
    model.set_installing(false);
    model.set_busy(false);
    let Some(state) = state else {
        return;
    };
    if let Err(error) = result {
        model.set_status("Pairing did not finish.".into());
        show_pair_error(&model, &format!("{error} The old pairing was not changed."));
        return;
    }
    let (replaced, stop_render, already_remote) = {
        let mut app = state.borrow_mut();
        record_render_connection(&app, false);
        let replaced = app.settings.remote.enrolled;
        let mut remote_backend = false;
        app.update_settings(|settings| {
            remote_backend = remote::remember_paired(settings, server);
        });
        if remote_backend && replaced {
            // A render that talked to the computer just replaced must not go on.
            app.render.set_user_paused(false);
            app.render.stop();
        }
        (replaced, remote_backend && replaced, remote_backend)
    };
    model.set_name(server.name.as_str().into());
    set_address_from_app(&model, server.addresses.first().map_or("", String::as_str));
    model.set_enrollment_address("".into());
    model.set_token("".into());
    model.set_identity("".into());
    model.set_paired(true);
    crate::remote_jobs::clear();
    model.set_jobs(ModelRc::new(VecModel::from(Vec::<JobRow>::new())));
    model.set_jobs_status("Refresh to see the server jobs of this computer.".into());
    model.set_pair_open(false);
    model.set_pair_error("".into());
    model.set_pair_name("".into());
    model.set_pair_address("".into());
    model.set_pair_replaces("".into());
    crate::design::refresh(window, &state);
    let message = match (replaced, stop_render) {
        (_, true) => format!(
            "Paired with {}. Rendering restarts on request.",
            server.name
        ),
        (true, false) => format!("Paired with {}. The old pairing was replaced.", server.name),
        (false, false) => format!("Paired with {}.", server.name),
    };
    toast::show(window, &message);
    let ask_render = should_ask_render_remote(already_remote);
    RUNTIME.with(|value| {
        if let Some(runtime) = value.borrow_mut().as_mut() {
            runtime.announce = Some(server.name.clone());
            runtime.post_pair = Some(PostPair::new(&server.name, ask_render));
        }
    });
    if ask_render {
        ask_render_remote(window, &server.name);
    }
    // The first test proves the new pairing works and fills in the certificate identity.
    start(window, false);
}

/// Writes the address field on behalf of the app (a pairing, a computer found again). The
/// page's `changed address` handler leaves its "Address changed" note and its identity reset
/// alone for such a write; the flag is only set when the text really changes, so it can
/// never wait for a change that does not come.
fn set_address_from_app(model: &RemoteModel<'_>, address: &str) {
    if model.get_address().as_str() != address {
        model.set_address_set_by_app(true);
        model.set_address(address.into());
    }
}

/// Answers this page's dialog (Forget Server). Returns false for any other dialog id.
pub fn handle_dialog_result(window: &AppWindow, id: i32, confirmed: bool) -> bool {
    match id {
        DIALOG_FORGET => {
            if confirmed {
                start_forget(window);
            }
            true
        }
        DIALOG_RENDER_REMOTE => {
            if confirmed {
                // The render page's own backend picker: it saves the settings, stops the
                // running render and refreshes the page.
                window
                    .global::<crate::RenderModel>()
                    .invoke_backend_selected(RenderBackend::Remote.index());
            }
            // The render question is settled: the library offer may follow (once the
            // connection test has said whether the computer shares one).
            RUNTIME.with(|value| {
                if let Some(runtime) = value.borrow_mut().as_mut()
                    && let Some(post) = runtime.post_pair.as_mut()
                {
                    post.render_pending = false;
                }
            });
            queue_library_known(window);
            true
        }
        DIALOG_LIBRARY_SYNC => {
            if confirmed {
                crate::library::start_sync(window, crate::library::SyncStart::AfterPairing);
            } else {
                toast::show(window, "You can sync any time from the Library page.");
            }
            true
        }
        _ => false,
    }
}

/// Whether a new pairing asks to render on the computer: not when the render backend is
/// already Remote (`already_remote`, as [`remote::remember_paired`] reports it).
const fn should_ask_render_remote(already_remote: bool) -> bool {
    !already_remote
}

/// Asks after a new pairing whether the picture should be rendered on that computer. Only
/// the answer in [`handle_dialog_result`] changes the render settings.
fn ask_render_remote(window: &AppWindow, name: &str) {
    let name = if name.trim().is_empty() {
        "this computer"
    } else {
        name.trim()
    };
    window.global::<Shell>().invoke_show_dialog(
        DIALOG_RENDER_REMOTE,
        format!("Render on {name}?").into(),
        concat!(
            "The computer renders the pictures and this phone sends it the design. ",
            "You can change this later in the render settings."
        )
        .into(),
        format!("Use {name}").into(),
        "Keep on this phone".into(),
    );
}

/// What the offer to copy the computer's designs says: the title and the body. A computer
/// without a name is "this computer".
fn library_offer_texts(name: &str) -> (String, String) {
    let name = name.trim();
    let (title_name, lead) = if name.is_empty() {
        ("this computer", "This computer")
    } else {
        (name, name)
    };
    (
        format!("Copy designs from {title_name}?"),
        format!(
            concat!(
                "{} shares a design library. Copy its designs to this phone to search and ",
                "open them without a connection. The first copy can take a few minutes and ",
                "uses storage on this phone. Later, tap Sync on the Library page to fetch ",
                "only what changed.",
            ),
            lead
        ),
    )
}

/// Asks after a new pairing whether to copy the computer's designs to this phone. Only the
/// answer in [`handle_dialog_result`] starts a sync.
fn ask_library_sync(window: &AppWindow, name: &str) {
    let (title, body) = library_offer_texts(name);
    window.global::<Shell>().invoke_show_dialog(
        DIALOG_LIBRARY_SYNC,
        title.into(),
        body.into(),
        "Copy Designs".into(),
        "Not Now".into(),
    );
}

/// Queues [`library_known`]: the code that calls this may still hold the runtime, and the
/// dialog that was just answered must be closed before another one opens.
fn queue_library_known(window: &AppWindow) {
    let weak = window.as_weak();
    let _queued = slint::invoke_from_event_loop(move || {
        if let Some(window) = weak.upgrade() {
            library_known(&window);
        }
    });
}

/// Something about the computer's library changed (the test said whether it shares one, or
/// the render question was answered): the Library page reads the saved answer, and the
/// offer to copy the designs shows when everything it waits for is in.
fn library_known(window: &AppWindow) {
    let state = RUNTIME.with(|value| {
        value
            .borrow()
            .as_ref()
            .map(|runtime| Rc::clone(&runtime.state))
    });
    if let Some(state) = state {
        crate::design::refresh(window, &state);
    }
    offer_after_pairing(window);
}

/// Shows the offer to copy the computer's designs once, when a new pairing waits for
/// nothing more. Another dialog on screen keeps it from showing.
fn offer_after_pairing(window: &AppWindow) {
    if window.global::<Shell>().get_dialog_visible() {
        return;
    }
    let name = RUNTIME.with(|value| {
        let mut guard = value.borrow_mut();
        let runtime = guard.as_mut()?;
        let name = runtime
            .post_pair
            .as_ref()
            .filter(|post| post.should_offer())?
            .name
            .clone();
        runtime.post_pair = None;
        Some(name)
    });
    if let Some(name) = name {
        ask_library_sync(window, &name);
    }
}

/// Asks before the pairing credentials are deleted. The answer arrives in
/// [`handle_dialog_result`].
fn ask_forget(window: &AppWindow) {
    window.global::<Shell>().invoke_show_dialog(
        DIALOG_FORGET,
        "Forget Server?".into(),
        FORGET_BODY.into(),
        "Forget Server".into(),
        "Cancel".into(),
    );
}

/// Starts erasing the credentials once the user confirmed (on the UI thread). Nothing in
/// the settings changes until the worker reports success, so a failure can be retried.
fn start_forget(window: &AppWindow) {
    let model = window.global::<RemoteModel>();
    let Some(busy) = RUNTIME.with(|value| {
        value
            .borrow()
            .as_ref()
            .map(|runtime| runtime.installing || runtime.forgetting)
    }) else {
        return;
    };
    if busy {
        model.set_status(
            "Credentials are being changed. Try Forget Server again in a moment.".into(),
        );
        return;
    }
    // A connection test, a jobs request, or a picture export or Send To Server upload that
    // talks to the server must not hold the credentials while they are erased: each is
    // cancelled and its result dropped. A render on this phone is left alone; a Remote render
    // is stopped by `finish_forget` once the erase succeeds.
    interrupt();
    stop_rediscovery();
    crate::design::render::cancel_remote_export();
    // The computer's library goes with its pairing: a running sync stops first.
    crate::library::remote_changed(window);
    RUNTIME.with(|value| {
        let mut guard = value.borrow_mut();
        let Some(runtime) = guard.as_mut() else {
            return;
        };
        runtime.post_pair = None;
        runtime.forgetting = true;
        model.set_forgetting(true);
        model.set_status("Deleting the stored pairing credentials…".into());
        let platform = Arc::clone(runtime.state.borrow().platform());
        let weak = window.as_weak();
        runtime.queue.submit(move || {
            let result = remote::forget(platform.as_ref());
            let _queued = weak.upgrade_in_event_loop(move |window| finish_forget(&window, result));
        });
    });
}

/// The erase ended (on the UI thread). On success the saved server is cleared, Remote
/// rendering is left and the page is emptied; on failure everything stays as it was.
fn finish_forget(window: &AppWindow, result: Result<(), String>) {
    let model = window.global::<RemoteModel>();
    let state = RUNTIME.with(|value| {
        let mut guard = value.borrow_mut();
        let runtime = guard.as_mut()?;
        runtime.forgetting = false;
        Some(Rc::clone(&runtime.state))
    });
    model.set_forgetting(false);
    let Some(state) = state else {
        return;
    };
    if let Err(error) = result {
        let message = format!("{error} The server is still remembered; try Forget Server again.");
        model.set_status(message.as_str().into());
        toast::show(window, &message);
        return;
    }
    let left_remote = {
        let mut app = state.borrow_mut();
        record_render_connection(&app, false);
        let mut switched = false;
        app.update_settings(|settings| {
            switched = remote::clear_server_settings(settings);
        });
        if switched {
            // The same steps as picking another backend in the Render view.
            app.render.set_user_paused(false);
            app.render.stop();
        }
        switched
    };
    model.set_name("".into());
    model.set_address("".into());
    model.set_paired(false);
    model.set_enrollment_address("".into());
    model.set_token("".into());
    model.set_identity("".into());
    crate::remote_jobs::clear();
    model.set_jobs(ModelRc::new(VecModel::from(Vec::<JobRow>::new())));
    model.set_jobs_status("No server is set up, so there are no server jobs to show.".into());
    model.set_status("Server forgotten. Pair this phone again to use a server.".into());
    crate::design::refresh(window, &state);
    toast::show(
        window,
        if left_remote {
            "Server forgotten. Rendering is back on Automatic."
        } else {
            "Server forgotten."
        },
    );
}

fn save(window: &AppWindow) {
    let model = window.global::<RemoteModel>();
    RUNTIME.with(|value| {
        let guard = value.borrow();
        let Some(runtime) = guard.as_ref() else {
            return;
        };
        let address_changed =
            runtime.state.borrow().settings.remote.address.trim() != model.get_address().trim();
        if address_changed {
            record_render_connection(&runtime.state.borrow(), false);
        }
        runtime.state.borrow_mut().update_settings(|settings| {
            model
                .get_name()
                .trim()
                .clone_into(&mut settings.remote.name);
            model
                .get_address()
                .trim()
                .clone_into(&mut settings.remote.address);
            if address_changed {
                // A hand-typed address replaces the list a pairing link brought.
                settings.remote.addresses.clear();
            }
            settings.remote.transfer = if model.get_transfer() == 1 {
                RemoteTransfer::FullData
            } else {
                RemoteTransfer::DisplayOnly
            };
            settings.remote.cadence_ms = u32::try_from(model.get_cadence())
                .unwrap_or(500)
                .clamp(100, 10000);
            settings.remote.allow_metered = model.get_allow_metered();
        });
        model.set_status("Connection settings saved. No design was uploaded.".into());
    });
}

enum Outcome {
    Claimed(remote::Enrollment),
    Enrolled,
    Tested(ConnectionReport),
}
fn start(window: &AppWindow, enrollment: bool) {
    save(window);
    let model = window.global::<RemoteModel>();
    let address = if enrollment {
        model.get_enrollment_address()
    } else {
        model.get_address()
    }
    .trim()
    .to_owned();
    let token = Zeroizing::new(model.get_token().to_string());
    model.set_token("".into());
    model.set_identity("".into());
    RUNTIME.with(|value| {
        let mut guard = value.borrow_mut();
        let Some(runtime) = guard.as_mut() else {
            return;
        };
        if runtime.installing || runtime.forgetting {
            return;
        }
        if let Some((_, old)) = runtime.active.take() {
            old.store(true, Ordering::Release);
        }
        runtime.next = runtime.next.wrapping_add(1);
        let id = runtime.next;
        let cancelled = Arc::new(AtomicBool::new(false));
        runtime.active = Some((id, Arc::clone(&cancelled)));
        let platform = Arc::clone(runtime.state.borrow().platform());
        // A test dials every saved address of a paired computer; a hand-typed one stands alone.
        let addresses = {
            let app = runtime.state.borrow();
            if address == app.settings.remote.address.trim() {
                app.settings.remote.endpoint_addresses()
            } else {
                vec![address.clone()]
            }
        };
        model.set_busy(true);
        let status = match (&runtime.announce, enrollment) {
            (_, true) => "Redeeming the enrollment code…".to_owned(),
            (Some(name), false) => format!("Connecting to {name}…"),
            (None, false) => "Checking the connection…".to_owned(),
        };
        model.set_status(status.as_str().into());
        let weak = window.as_weak();
        runtime.queue.submit(move || {
            let outcome = if enrollment {
                remote::enroll(&token, &address, &cancelled).map(Outcome::Claimed)
            } else {
                remote::test_connection_any(platform.as_ref(), &addresses, &cancelled)
                    .map(Outcome::Tested)
            };
            let _queued = weak.upgrade_in_event_loop(move |window| {
                finish(&window, id, address, enrollment, outcome);
            });
        });
    });
}

/// Whether a finished connection attempt still matches the page. When it does not (Remote was
/// left, or the address changed) the attempt is dropped and the page says so; returns `false`.
fn result_still_wanted(
    window: &AppWindow,
    runtime: &mut Runtime,
    address: &str,
    enrollment: bool,
) -> bool {
    let model = window.global::<RemoteModel>();
    if runtime.installing {
        runtime.installing = false;
        model.set_installing(false);
        return true;
    }
    let shell = window.global::<Shell>();
    if shell.get_destination() != 3 || shell.get_more_page() != 4 {
        runtime.active = None;
        runtime.post_pair = None;
        model.set_busy(false);
        model.set_status("Connection interrupted when leaving Remote.".into());
        return false;
    }
    let current = if enrollment {
        model.get_enrollment_address()
    } else {
        model.get_address()
    };
    if current.trim() != address {
        runtime.active = None;
        runtime.post_pair = None;
        model.set_busy(false);
        model.set_status("Address changed; connection result discarded.".into());
        return false;
    }
    true
}

fn finish(
    window: &AppWindow,
    id: u64,
    address: String,
    enrollment: bool,
    outcome: Result<Outcome, String>,
) {
    RUNTIME.with(|value| {
        let mut guard = value.borrow_mut();
        let Some(runtime) = guard.as_mut() else {
            return;
        };
        if !runtime
            .active
            .as_ref()
            .is_some_and(|(run, cancel)| *run == id && !cancel.load(Ordering::Acquire))
        {
            return;
        }
        let announce = runtime.announce.take();
        let model = window.global::<RemoteModel>();
        if !result_still_wanted(window, runtime, &address, enrollment) {
            return;
        }
        if let Ok(Outcome::Claimed(bundle)) = outcome {
            // UI acceptance is the commit boundary. Cancellation before this point
            // drops the claim; afterward the atomic install is allowed to finish.
            runtime.installing = true;
            model.set_installing(true);
            model.set_status("Installing encrypted credentials…".into());
            let platform = Arc::clone(runtime.state.borrow().platform());
            let weak = window.as_weak();
            runtime.queue.submit(move || {
                let result = bundle
                    .install(platform.as_ref())
                    .map(|()| Outcome::Enrolled);
                let _queued = weak.upgrade_in_event_loop(move |window| {
                    finish(&window, id, address, true, result);
                });
            });
            return;
        }
        runtime.active = None;
        model.set_busy(false);
        match outcome {
            Ok(Outcome::Claimed(_)) => unreachable!("claim handled above"),
            Ok(Outcome::Enrolled) => {
                record_render_connection(&runtime.state.borrow(), false);
                runtime.state.borrow_mut().update_settings(|settings| {
                    settings.remote.enrolled = true;
                    // Hand enrollment names one address; a list from an earlier link is stale.
                    settings.remote.addresses.clear();
                });
                model.set_paired(true);
                model.set_status(
                    "Credentials encrypted and installed. Test the rendering address next.".into(),
                );
            }
            Ok(Outcome::Tested(report)) => {
                // The computer answers at a saved address: nothing to look for.
                stop_rediscovery();
                record_render_connection(&runtime.state.borrow(), report.render.is_some());
                // Right after a pairing the plain sentence says it all.
                let status = announce
                    .map_or_else(|| report.summary(), |name| format!("Connected to {name}."));
                model.set_status(status.as_str().into());
                // The test is the moment the phone learns whether the computer shares a
                // design library (the Library page offers it only then).
                let serves = report.library;
                let known = runtime.state.borrow().settings.remote.serves_library;
                if known != Some(serves) {
                    runtime
                        .state
                        .borrow_mut()
                        .update_settings(|settings| settings.remote.serves_library = Some(serves));
                }
                if let Some(post) = runtime.post_pair.as_mut() {
                    post.library = Some(serves);
                }
                // Queued: this code still holds the runtime, and the page and the offer
                // read it.
                queue_library_known(window);
                model.set_identity(report.certificate_identity.into());
            }
            Err(error) => {
                runtime.post_pair = None;
                record_render_connection(&runtime.state.borrow(), false);
                model.set_status(format!("Connection unavailable: {error}").into());
                if !enrollment {
                    // No saved address answered: look for the computer on this network. Queued,
                    // because this code still holds the runtime.
                    let weak = window.as_weak();
                    let _queued = slint::invoke_from_event_loop(move || {
                        if let Some(window) = weak.upgrade() {
                            begin_rediscovery(&window);
                        }
                    });
                }
            }
        }
    });
}
pub fn interrupt() {
    crate::remote_jobs::interrupt();
    let window = RUNTIME.with(|value| {
        if let Some(runtime) = value.borrow_mut().as_mut() {
            if runtime.installing {
                return None;
            }
            if let Some((_, cancel)) = runtime.active.take() {
                cancel.store(true, Ordering::Release);
                // A test that was cancelled must not leave its "Connected to" name behind
                // for the next, unrelated test.
                runtime.announce = None;
                let was_pairing = std::mem::take(&mut runtime.pairing);
                return Some((runtime.window.clone(), was_pairing));
            }
        }
        None
    });
    if let Some((window, was_pairing)) = window
        && let Some(window) = window.upgrade()
    {
        let model = window.global::<RemoteModel>();
        model.set_busy(false);
        model.set_token("".into());
        model.set_status("Connection interrupted. Retry when ready.".into());
        if was_pairing {
            // The code may or may not have been used up before the stop.
            show_pair_error(
                &model,
                "Pairing was interrupted. If it did not finish, show a new code on the computer.",
            );
        }
    }
}
pub fn shutdown() {
    RENDER_CONNECTION.with(|value| *value.borrow_mut() = None);
    interrupt();
    stop_rediscovery();
    crate::remote_jobs::shutdown();
    RUNTIME.with(|value| drop(value.borrow_mut().take()));
    // The window is gone; a link that arrives now waits for the next start.
    inbox().window = None;
}

/// What a browse for the paired computer knows. Present from [`begin_rediscovery`] until the
/// computer is found or [`stop_rediscovery`]; it is shared with the JNI thread that reports
/// the services, so it sits in a static.
struct Rediscovery {
    /// The tags of the saved CA; empty until the worker has read them (nothing matches then).
    tags: Vec<String>,
    /// A matching announcement is being connected to; others wait in `waiting`.
    verifying: bool,
    /// Matching announcements that arrived while one was being connected to. The next one
    /// is tried when the current one fails, so a fake announcement cannot hide the real one.
    waiting: VecDeque<Announcement>,
    /// Kotlin ended the browse (time ran out, the app stopped) while a connection to an
    /// announcement was still being tried. The session stays until that is settled, so a
    /// computer that answers is still saved; nothing new is found meanwhile.
    ended: bool,
}
static REDISCOVERY: Mutex<Option<Rediscovery>> = Mutex::new(None);

/// How many announcements wait for their turn.
const MAX_WAITING: usize = 8;

fn rediscovery() -> MutexGuard<'static, Option<Rediscovery>> {
    REDISCOVERY.lock().unwrap_or_else(PoisonError::into_inner)
}

/// A render, an export or a jobs request failed with `message`. When the message says that
/// no saved address answered, the app looks for the computer on the network. Safe from any
/// place on the UI thread: the work is queued, because the caller may still hold its own
/// state.
pub fn connect_failed(window: &AppWindow, message: &str) {
    if !remote::is_unreachable(message) {
        return;
    }
    let weak = window.as_weak();
    let _queued = slint::invoke_from_event_loop(move || {
        if let Some(window) = weak.upgrade() {
            begin_rediscovery(&window);
        }
    });
}

/// Starts looking for the paired computer on the local network, because none of its saved
/// addresses answered (on the UI thread). Does nothing for a phone that is not paired, while
/// credentials are being changed, or while a browse already runs. The browse ends by itself
/// after 30 seconds, when the computer is found, or when the pairing changes; Kotlin then
/// calls [`rediscovery_ended`], so a later failure can start another browse. The browse needs
/// local network access: it is asked for first, once, and not again after a refusal until
/// the user touches the picture.
pub fn begin_rediscovery(window: &AppWindow) {
    // A phone that is not paired has no computer to look for: no question either.
    let paired = RUNTIME.with(|value| {
        value
            .borrow()
            .as_ref()
            .is_some_and(|runtime| runtime.state.borrow().settings.remote.enrolled)
    });
    if paired && local_network_for_background(window, begin_browse) {
        begin_browse(window);
    }
}

/// Opens the session for a browse and reads the saved CA on the worker (on the UI thread,
/// with the local network access in hand).
fn begin_browse(window: &AppWindow) {
    let Some(state) = RUNTIME.with(|value| {
        let guard = value.borrow();
        let runtime = guard.as_ref()?;
        if runtime.installing || runtime.forgetting || runtime.pairing {
            return None;
        }
        Some(Rc::clone(&runtime.state))
    }) else {
        return;
    };
    if !state.borrow().settings.remote.enrolled {
        return;
    }
    {
        let mut session = rediscovery();
        if session.is_some() {
            return;
        }
        *session = Some(Rediscovery {
            tags: Vec::new(),
            verifying: false,
            waiting: VecDeque::new(),
            ended: false,
        });
    }
    let platform = Arc::clone(state.borrow().platform());
    let weak = window.as_weak();
    RUNTIME.with(|value| {
        if let Some(runtime) = value.borrow_mut().as_mut() {
            runtime.queue.submit(move || {
                let tags = remote::saved_ca_tags(platform.as_ref());
                let _queued = weak.upgrade_in_event_loop(move |window| browse(&window, tags));
            });
        }
    });
}

/// The saved CA is known: starts the Kotlin browse (on the UI thread).
fn browse(window: &AppWindow, tags: Result<Vec<String>, String>) {
    let Ok(tags) = tags else {
        *rediscovery() = None;
        return;
    };
    let stored = rediscovery().as_mut().is_some_and(|session| {
        session.tags = tags;
        true
    });
    if !stored {
        // Stopped while the worker read the credentials.
        return;
    }
    if let Err(error) = bridge::start_rediscovery() {
        tracing::warn!(%error, "could not browse for the computer");
        *rediscovery() = None;
        return;
    }
    let model = window.global::<RemoteModel>();
    let current = model.get_status();
    if !current.ends_with(LOOKING) {
        let status = if current.is_empty() {
            LOOKING.to_owned()
        } else {
            format!("{current} {LOOKING}")
        };
        model.set_status(status.as_str().into());
    }
}

/// What the Remote page adds to its status while a browse runs.
const LOOKING: &str = "Looking for the computer on this network…";

/// Ends a browse and releases the multicast lock; safe to call at any time.
pub fn stop_rediscovery() {
    let was_running = rediscovery().take().is_some();
    if was_running && let Err(error) = bridge::stop_rediscovery() {
        tracing::warn!(%error, "could not stop browsing for the computer");
    }
}

/// Kotlin ended the browse by itself (its 30 seconds ran out, the app went to the
/// background, or the browse failed) and released the multicast lock. The session is
/// cleared, so the next failed connection starts a new browse, unless an announcement is
/// being connected to: that attempt may still find the computer, so the session stays until
/// it is settled ([`verification_failed`] or [`finish_rediscovery`]). A failure still on
/// show is looked at again when the app comes back to the foreground
/// ([`crate::design::render::forget_failure`], called from the lifecycle handler), not on a
/// timeout, so a computer that is away does not keep the network browse running forever.
/// Nothing is called back into Kotlin: this runs on its UI thread.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn rediscovery_ended() {
    let mut guard = rediscovery();
    if let Some(session) = guard.as_mut()
        && session.verifying
    {
        session.ended = true;
    } else {
        *guard = None;
    }
}

/// One service resolved by the Kotlin browse (any thread): the TXT values `v` and `ca`, and
/// its IPv4 addresses as a comma-separated `ip:port` list. Only an announcement of the
/// paired CA is looked at further, and even that is only a hint: its addresses are saved
/// after a real connection with the saved credentials succeeded ([`finish_rediscovery`]).
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn service_found(name: &str, version: &str, ca: &str, addresses: &str) {
    let Some(announcement) = remote_discovery::read_announcement(name, version, ca, addresses)
    else {
        return;
    };
    // No session (the browse was stopped) or another CA's announcement: nothing to start.
    let start_now = rediscovery().as_mut().is_some_and(|session| {
        if !remote_discovery::belongs_to(&announcement, &session.tags) {
            return false;
        }
        if session.verifying {
            // Another one is being connected to: this one waits for its turn, and is tried
            // when that one fails.
            if session.waiting.len() < MAX_WAITING && !session.waiting.contains(&announcement) {
                session.waiting.push_back(announcement.clone());
            }
            false
        } else {
            session.verifying = true;
            true
        }
    });
    if start_now {
        dispatch_verification(announcement);
    }
}

/// Hands an announcement to the UI thread to be connected to. The session already says that
/// one is being verified.
fn dispatch_verification(announcement: Announcement) {
    let window = inbox().window.clone();
    let queued = window.is_some_and(|window| {
        window
            .upgrade_in_event_loop(move |window| verify_announcement(&window, announcement))
            .is_ok()
    });
    if !queued {
        verification_failed();
    }
}

/// The announcement being verified came to nothing: the next waiting one is tried.
fn verification_failed() {
    let next = {
        let mut guard = rediscovery();
        let Some(session) = guard.as_mut() else {
            return;
        };
        let next = session.waiting.pop_front();
        session.verifying = next.is_some();
        if next.is_none() && session.ended {
            // The browse is over and nothing is left to try: the session ends here.
            *guard = None;
        }
        next
    };
    if let Some(next) = next {
        dispatch_verification(next);
    }
}

/// Connects to the announced addresses with the saved credentials (on the UI thread; the
/// handshake runs on the worker). A computer that is not the paired one fails here.
fn verify_announcement(window: &AppWindow, announcement: Announcement) {
    let submitted = RUNTIME.with(|value| {
        let guard = value.borrow();
        let runtime = guard.as_ref()?;
        let platform = Arc::clone(runtime.state.borrow().platform());
        let weak = window.as_weak();
        runtime.queue.submit(move || {
            let cancelled = AtomicBool::new(false);
            let result =
                remote::test_connection_any(platform.as_ref(), &announcement.addresses, &cancelled);
            let _queued = weak.upgrade_in_event_loop(move |window| {
                finish_rediscovery(&window, &announcement, result);
            });
        });
        Some(())
    });
    if submitted.is_none() {
        verification_failed();
    }
}

/// The announced addresses were tried (on the UI thread). On success they replace the saved
/// ones, the user is told and a failed remote render starts over; otherwise the next
/// waiting announcement is tried and the browse goes on.
fn finish_rediscovery(
    window: &AppWindow,
    announcement: &Announcement,
    result: Result<ConnectionReport, String>,
) {
    // Stopped meanwhile (forgotten, re-paired, left): the result is dropped.
    let stopped = rediscovery().is_none();
    if stopped {
        return;
    }
    let Ok(report) = result else {
        verification_failed();
        return;
    };
    let Some(state) = RUNTIME.with(|value| value.borrow().as_ref().map(|r| Rc::clone(&r.state)))
    else {
        return;
    };
    let moved = {
        let mut app = state.borrow_mut();
        let mut moved = None;
        app.update_settings(|settings| {
            moved = remote_discovery::apply(&mut settings.remote, announcement);
            // The computer answered with the saved credentials: what it shares is known.
            settings.remote.serves_library = Some(report.library);
        });
        record_render_connection(&app, report.render.is_some());
        moved
    };
    stop_rediscovery();
    let message = moved.as_ref().map_or_else(
        || "The computer is reachable again.".to_owned(),
        remote_discovery::moved_message,
    );
    let model = window.global::<RemoteModel>();
    if moved.is_some()
        && let Some(first) = announcement.addresses.first()
    {
        // The page does not add its "Address changed" note for a write by the app.
        set_address_from_app(&model, first);
    }
    model.set_status(message.as_str().into());
    model.set_identity(report.certificate_identity.as_str().into());
    toast::show(window, &message);
    // A remote render that failed on the old address starts over at the saved one.
    crate::design::render::restart_after_move();
    // The Library page reads what the computer shares, and a computer view that showed an
    // error asks again.
    crate::design::refresh(window, &state);
    crate::library::computer_found_again(window);
}

/// Calls into the Kotlin `MainActivity` (the scanner and the browse). Android keeps the
/// activity from `nativeAttachActivity`; the desktop preview has no scanner or browse.
#[cfg(target_os = "android")]
mod bridge {
    use jni::{
        Env, EnvUnowned, JValueOwned,
        errors::{LogErrorAndDefault, Result as JniResult},
        jni_sig, jni_str,
        objects::{JObject, JString},
        refs::{Global, Reference},
        sys::jint,
        vm::JavaVM,
    };
    use std::sync::{Arc, Mutex, PoisonError};

    /// The activity, set by `MainActivity.onCreate` and cleared by `onDestroy`. The reference
    /// is shared, so a call never holds the lock while it waits for Kotlin.
    static ACTIVITY: Mutex<Option<Arc<Global<JObject<'static>>>>> = Mutex::new(None);

    /// Runs `call` on the activity; a sentence from Kotlin (`String?` result) is the error.
    fn with_activity(
        call: impl FnOnce(&mut Env<'_>, &JObject<'_>) -> JniResult<Option<String>>,
    ) -> Result<(), String> {
        call_activity(call)?.map_or(Ok(()), Err)
    }

    /// Runs `call` on the activity and returns what it returns.
    fn call_activity<T>(
        call: impl FnOnce(&mut Env<'_>, &JObject<'_>) -> JniResult<T>,
    ) -> Result<T, String> {
        // The call can wait for the UI thread for seconds; the lock is not held meanwhile,
        // or `nativeAttachActivity` of a recreated activity would wait behind it.
        let activity = ACTIVITY
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone();
        let Some(activity) = activity else {
            return Err("The app is not ready yet. Try again in a moment.".to_owned());
        };
        let vm = JavaVM::singleton().map_err(|error| error.to_string())?;
        vm.attach_current_thread(|env| call(env, activity.as_obj()))
            .map_err(|error| error.to_string())
    }

    /// Reads the `String?` a Kotlin service method returned.
    fn read_string(env: &mut Env<'_>, value: JValueOwned<'_>) -> JniResult<Option<String>> {
        let object = value.l()?;
        if object.is_null() {
            return Ok(None);
        }
        let text = env.cast_local::<JString>(object)?;
        Ok(Some(text.try_to_string(env)?))
    }

    /// `startQrScan(): String?`: opens the QR scanner.
    pub(super) fn start_qr_scan() -> Result<(), String> {
        with_activity(|env, activity| {
            let value = env.call_method(
                activity,
                jni_str!("startQrScan"),
                jni_sig!("()Ljava/lang/String;"),
                &[],
            )?;
            read_string(env, value)
        })
    }

    /// `ensureLocalNetwork(): Int`: 1 when the app may use the local network already (or
    /// the Android version asks for nothing), 0 when the system dialog was opened (the answer
    /// comes back through `nativeLocalNetworkAnswered`), anything else when Kotlin could not
    /// ask.
    pub(super) fn ensure_local_network() -> Result<bool, String> {
        let code = call_activity(|env, activity| {
            env.call_method(
                activity,
                jni_str!("ensureLocalNetwork"),
                jni_sig!("()I"),
                &[],
            )?
            .i()
        })?;
        match code {
            1 => Ok(true),
            0 => Ok(false),
            _ => Err("the permission request could not be started".to_owned()),
        }
    }

    /// `startRediscovery(): String?`: starts browsing for the paired computer.
    pub(super) fn start_rediscovery() -> Result<(), String> {
        with_activity(|env, activity| {
            let value = env.call_method(
                activity,
                jni_str!("startRediscovery"),
                jni_sig!("()Ljava/lang/String;"),
                &[],
            )?;
            read_string(env, value)
        })
    }

    /// `stopRediscovery(): String?`: stops browsing and releases the multicast lock.
    pub(super) fn stop_rediscovery() -> Result<(), String> {
        with_activity(|env, activity| {
            let value = env.call_method(
                activity,
                jni_str!("stopRediscovery"),
                jni_sig!("()Ljava/lang/String;"),
                &[],
            )?;
            read_string(env, value)
        })
    }

    /// `private external fun nativeAttachActivity()` in `MainActivity.kt`.
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_com_indicatrixsuite_mobile_MainActivity_nativeAttachActivity<
        'caller,
    >(
        mut unowned: EnvUnowned<'caller>,
        this: JObject<'caller>,
    ) {
        unowned
            .with_env(|env| -> JniResult<()> {
                let global: Global<JObject<'static>> = env.new_global_ref(&this)?;
                *ACTIVITY.lock().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(global));
                Ok(())
            })
            .resolve::<LogErrorAndDefault>();
    }

    /// `private external fun nativeDetachActivity()` in `MainActivity.kt`: the activity is
    /// destroyed, so Rust lets go of its global reference.
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_com_indicatrixsuite_mobile_MainActivity_nativeDetachActivity<
        'caller,
    >(
        mut unowned: EnvUnowned<'caller>,
        _this: JObject<'caller>,
    ) {
        unowned
            .with_env(|_env| -> JniResult<()> {
                // A call in flight keeps its own reference until it returns.
                *ACTIVITY.lock().unwrap_or_else(PoisonError::into_inner) = None;
                Ok(())
            })
            .resolve::<LogErrorAndDefault>();
    }

    /// `private external fun nativeRediscoveryEnded()` in `MainActivity.kt`: the browse ended
    /// (by itself, or because it was stopped) and the multicast lock is released.
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_com_indicatrixsuite_mobile_MainActivity_nativeRediscoveryEnded<
        'caller,
    >(
        mut unowned: EnvUnowned<'caller>,
        _this: JObject<'caller>,
    ) {
        unowned
            .with_env(|_env| -> JniResult<()> {
                super::rediscovery_ended();
                Ok(())
            })
            .resolve::<LogErrorAndDefault>();
    }

    /// `private external fun nativeLocalNetworkAnswered(granted: Int)` in `MainActivity.kt`:
    /// the user answered the system dialog for the local network (1 yes, 0 no). Called on
    /// Kotlin's UI thread; it only records the answer and queues the work for the UI thread.
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_com_indicatrixsuite_mobile_MainActivity_nativeLocalNetworkAnswered(
        _env: EnvUnowned<'_>,
        _this: JObject<'_>,
        granted: jint,
    ) {
        super::local_network_answered(granted != 0);
    }

    /// `private external fun nativeServiceFound(name, version, ca, addresses)` in
    /// `MainActivity.kt`: one service of the browse. The values are public (a name, the
    /// protocol version, a CA tag, addresses); nothing secret is in them.
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_com_indicatrixsuite_mobile_MainActivity_nativeServiceFound<
        'caller,
    >(
        mut unowned: EnvUnowned<'caller>,
        _this: JObject<'caller>,
        name: JString<'caller>,
        version: JString<'caller>,
        ca: JString<'caller>,
        addresses: JString<'caller>,
    ) {
        unowned
            .with_env(|env| -> JniResult<()> {
                let name = name.try_to_string(env)?;
                let version = version.try_to_string(env)?;
                let ca = ca.try_to_string(env)?;
                let addresses = addresses.try_to_string(env)?;
                super::service_found(&name, &version, &ca, &addresses);
                Ok(())
            })
            .resolve::<LogErrorAndDefault>();
    }
}

/// The desktop preview has no camera scanner and no network browse.
#[cfg(not(target_os = "android"))]
mod bridge {
    /// There is no scanner outside Android.
    pub(super) fn start_qr_scan() -> Result<(), String> {
        Err("Scanning works in the Android app. Paste the pairing link instead.".to_owned())
    }

    /// There is no browse outside Android.
    pub(super) fn start_rediscovery() -> Result<(), String> {
        Err("Finding the computer works in the Android app.".to_owned())
    }

    /// Nothing runs, so nothing stops.
    #[expect(
        clippy::unnecessary_wraps,
        reason = "the same signature as the Android bridge, which can fail"
    )]
    pub(super) const fn stop_rediscovery() -> Result<(), String> {
        Ok(())
    }

    /// The desktop preview asks for nothing: the local network is open.
    #[expect(
        clippy::unnecessary_wraps,
        reason = "the same signature as the Android bridge, which can fail"
    )]
    pub(super) const fn ensure_local_network() -> Result<bool, String> {
        Ok(true)
    }
}

/// The deep link's way in from Kotlin. It lives here, not in `platform_android/natives.rs`,
/// so the pairing code stays in one place and needs no change in a file of another lane.
/// The JVM finds it by its mangled name; no registration is needed.
#[cfg(target_os = "android")]
mod deep_link {
    use jni::{
        EnvUnowned,
        errors::{LogErrorAndDefault, Result as JniResult},
        objects::{JObject, JString},
    };

    /// `private external fun nativePairingLink(link: String)` in `MainActivity.kt`.
    ///
    /// The `pleochroma://pair?...` link of an incoming intent. It holds a one-time code, so
    /// nothing here logs it.
    #[unsafe(no_mangle)]
    pub extern "system" fn Java_com_indicatrixsuite_mobile_MainActivity_nativePairingLink<
        'caller,
    >(
        mut unowned: EnvUnowned<'caller>,
        _this: JObject<'caller>,
        link: JString<'caller>,
    ) {
        unowned
            .with_env(|env| -> JniResult<()> {
                // The link holds the one-time code: this copy is wiped when it goes away.
                let text = zeroize::Zeroizing::new(link.try_to_string(env)?);
                super::receive_pairing_link(&text);
                Ok(())
            })
            .resolve::<LogErrorAndDefault>();
    }
}

#[cfg(test)]
mod pairing_render_tests {
    use super::should_ask_render_remote;
    use pleochroma_mobile_core::{
        remote::PairedServer,
        settings::{MobileSettings, RenderBackend},
    };

    #[test]
    fn asks_only_when_the_backend_is_not_already_remote() {
        let mut settings = MobileSettings::default();
        assert_eq!(settings.render.backend, RenderBackend::Automatic);
        let paired = PairedServer {
            name: "Studio".to_owned(),
            addresses: vec!["10.0.0.5:7878".to_owned()],
            enroll_port: 7879,
        };
        let already_remote =
            pleochroma_mobile_core::remote::remember_paired(&mut settings, &paired);
        assert!(should_ask_render_remote(already_remote));
        settings.render.backend = RenderBackend::Remote;
        let already_remote =
            pleochroma_mobile_core::remote::remember_paired(&mut settings, &paired);
        assert!(!should_ask_render_remote(already_remote));
    }
}

#[cfg(test)]
mod post_pair_tests {
    use super::{DIALOG_LIBRARY_SYNC, DIALOG_RENDER_REMOTE, PostPair, library_offer_texts};

    #[test]
    fn the_library_offer_waits_for_the_render_answer_and_the_test() {
        let mut post = PostPair::new("Studio", true);
        assert!(!post.should_offer(), "nothing is known yet");
        post.library = Some(true);
        assert!(
            !post.should_offer(),
            "the test is in but the render question is still open"
        );
        post.render_pending = false;
        assert!(post.should_offer());

        // The other order: the render question is answered before the test finishes.
        let mut post = PostPair::new("Studio", true);
        post.render_pending = false;
        assert!(!post.should_offer(), "the test has not finished");
        post.library = Some(true);
        assert!(post.should_offer());

        // No render question was asked (the backend was already Remote).
        let mut post = PostPair::new("Studio", false);
        assert!(!post.should_offer());
        post.library = Some(true);
        assert!(post.should_offer());
    }

    #[test]
    fn no_offer_for_a_rendering_only_computer() {
        let mut post = PostPair::new("Studio", false);
        post.library = Some(false);
        assert!(!post.should_offer());
        let mut asked = PostPair::new("Studio", true);
        asked.library = Some(false);
        asked.render_pending = false;
        assert!(
            !asked.should_offer(),
            "answering the question changes nothing"
        );
    }

    #[test]
    fn no_offer_when_the_test_failed() {
        // A failed test never fills in `library`; it also drops the record, but even a
        // record that stayed could not offer.
        let mut post = PostPair::new("Studio", false);
        assert_eq!(post.library, None);
        post.render_pending = false;
        assert!(!post.should_offer());
    }

    #[test]
    fn the_offer_is_worded_with_the_computers_name() {
        let (title, body) = library_offer_texts(" Studio PC ");
        assert_eq!(title, "Copy designs from Studio PC?");
        assert!(body.starts_with("Studio PC shares a design library."));
        assert!(body.contains("tap Sync on the Library page"));
        let (title, body) = library_offer_texts("  ");
        assert_eq!(title, "Copy designs from this computer?");
        assert!(body.starts_with("This computer shares a design library."));
    }

    #[test]
    fn the_dialog_ids_sit_in_the_remote_range() {
        for id in [DIALOG_RENDER_REMOTE, DIALOG_LIBRARY_SYNC] {
            assert!((4000..=4099).contains(&id), "dialog {id}");
        }
        assert_eq!(DIALOG_LIBRARY_SYNC, 4003);
    }
}
