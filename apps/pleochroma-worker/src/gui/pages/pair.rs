//! The pairing dialog's driver: makes a one-time code, shows it, waits for the claim.
//!
//! The Slint side (`ui/pair_dialog.slint`, `PairDialog` and the `PairState` global) only
//! draws; every rule lives here:
//!
//! - **The code shows at once** with a default name ("Phone 1"). The name the owner types on
//!   the paired screen is applied to the device afterwards (`rename`), it is never part of the
//!   code, so the code does not change while the camera is aimed at it.
//! - **Cancel on close.** Closing the dialog, switching tabs, refreshing and pairing all
//!   cancel the code that was live, so no old code stays valid.
//! - **Refresh only while the dialog has the focus**, and at most five times in a row; then
//!   the dialog says "Code paused" and waits for a click. A code that runs out while the
//!   window is in the background shows "Code expired" and is replaced when the owner returns.
//! - **The phone tab never offers Copy.** The computer tab copies the link with the
//!   clipboard-history exclusion of the platform, and clears the clipboard when the code
//!   expires, is cancelled or is used, if it still holds that link.
//! - **Paired from <address>.** The claim carries only the code, never a name, so the screen
//!   shows the address that claimed it and keeps "Not my device — remove access" one click
//!   away (`revoke`).
//!
//! A token or a pairing link is never logged, never put in a window title and never has a
//! `Debug` output here: no type in this file derives `Debug`, the link is held in a wiping
//! string, and the computer tab shows the link with its code masked.

use crate::{
    control::backend::{
        CoordinatorEvent, CoordinatorState, Fingerprint, IssuedToken, PeerRole, TokenId,
    },
    gui::{
        app::{ActiveBackend, AppWindow, PairState},
        qr,
        util::{clock_text, copy_secret, mask_link},
    },
};
use pleochroma_net::tls::fingerprint_from_hex;
use slint::{ComponentHandle, Image, ModelRc, SharedString, Timer, TimerMode, VecModel};
use std::{
    cell::RefCell,
    rc::{Rc, Weak as RcWeak},
    sync::mpsc::{self, Receiver, Sender},
    time::{Duration, Instant, SystemTime},
};
use zeroize::Zeroizing;

/// How often the dialog looks at the clock, the events and the answers of its helper threads.
const TICK: Duration = Duration::from_millis(250);

/// How many times in a row a code is replaced without a click before the dialog pauses.
const MAX_AUTO_REFRESHES: u32 = 5;

/// The phases of `PairState.phase`.
mod phase {
    /// A code is being made.
    pub const MAKING: i32 = 0;
    /// A code is on screen and waiting for the device.
    pub const WAITING: i32 = 1;
    /// A device claimed the code.
    pub const PAIRED: i32 = 2;
    /// The code ran out while the window was in the background.
    pub const EXPIRED: i32 = 3;
    /// Five automatic refreshes passed; a click shows a new code.
    pub const PAUSED: i32 = 4;
    /// No code could be made.
    pub const PROBLEM: i32 = 5;
}

/// The answer of a helper thread to the dialog.
enum Msg {
    /// A code was made (or not) for the request numbered `generation`.
    Issued {
        generation: u64,
        result: Result<IssuedToken, String>,
    },
    /// The answer to "Not my device — remove access".
    Revoked(Result<(), String>),
    /// The answer to a rename.
    Renamed(Result<(), String>),
}

/// The live code.
struct Live {
    id: TokenId,
    /// The pairing link, a secret; wiped when dropped.
    link: Zeroizing<String>,
    /// The name the device gets until the owner types another.
    default_name: String,
    expires: Instant,
    total: Duration,
}

/// What the dialog remembers between ticks.
struct Session {
    open: bool,
    /// 0 phone, 1 another computer.
    mode: i32,
    /// The number of the newest code request; older answers are cancelled on arrival.
    generation: u64,
    /// Automatic refreshes since the last click.
    refreshes: u32,
    live: Option<Live>,
    events: Option<Receiver<CoordinatorEvent>>,
    /// The device that claimed the code (64 hex characters).
    fingerprint: Option<String>,
    /// The name last applied to the paired device.
    applied_name: String,
    removed: bool,
    /// The link last copied, kept only to recognize it on the clipboard for clearing.
    copied_link: Option<Zeroizing<String>>,
    clipboard: Option<arboard::Clipboard>,
}

struct Ctx {
    window: slint::Weak<AppWindow>,
    backend: ActiveBackend,
    state: RefCell<Session>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    timer: Timer,
}

/// Connects the dialog to the coordinator behind `backend`.
///
/// `PairState.open-requested(tab)` opens the dialog on a tab (0 phone, 1 another computer);
/// the top bar's "Connect a phone" button and the end of the wizard call it.
pub fn wire(window: &AppWindow, backend: &ActiveBackend) {
    let (tx, rx) = mpsc::channel();
    let ctx = Rc::new(Ctx {
        window: window.as_weak(),
        backend: backend.clone(),
        state: RefCell::new(Session {
            open: false,
            mode: 0,
            generation: 0,
            refreshes: 0,
            live: None,
            events: None,
            fingerprint: None,
            applied_name: String::new(),
            removed: false,
            copied_link: None,
            clipboard: None,
        }),
        tx,
        rx,
        timer: Timer::default(),
    });

    let me: RcWeak<Ctx> = Rc::downgrade(&ctx);
    ctx.timer.start(TimerMode::Repeated, TICK, move || {
        if let Some(ctx) = me.upgrade() {
            ctx.tick();
        }
    });

    let pair = window.global::<PairState>();
    pair.set_open(false);
    pair.on_open_requested({
        let ctx = Rc::clone(&ctx);
        move |tab| ctx.open(tab)
    });
    pair.on_close_requested({
        let ctx = Rc::clone(&ctx);
        move || ctx.close()
    });
    pair.on_mode_selected({
        let ctx = Rc::clone(&ctx);
        move |tab| ctx.switch_mode(tab)
    });
    pair.on_new_code({
        let ctx = Rc::clone(&ctx);
        move || ctx.new_code()
    });
    pair.on_copy_link({
        let ctx = Rc::clone(&ctx);
        move || ctx.copy_link()
    });
    pair.on_name_edited({
        let ctx = Rc::clone(&ctx);
        move |name| ctx.commit_name(name.as_str())
    });
    pair.on_remove_device({
        let ctx = Rc::clone(&ctx);
        move || ctx.remove_device()
    });
    pair.on_connect_another({
        let ctx = Rc::clone(&ctx);
        move || ctx.connect_another()
    });
}

impl Ctx {
    // ---------------------------------------------------------------- opening and closing

    fn open(&self, tab: i32) {
        let Some(win) = self.window.upgrade() else {
            return;
        };
        self.release_code();
        let mode = tab.clamp(0, 1);
        {
            let mut s = self.state.borrow_mut();
            s.open = true;
            s.mode = mode;
            s.refreshes = 0;
            s.fingerprint = None;
            s.applied_name.clear();
            s.removed = false;
            s.events = Some(self.backend.get().events());
        }
        let st = win.global::<PairState>();
        st.set_mode(mode);
        st.set_coordinator_name(SharedString::from(self.backend.get().status().name));
        st.set_removed(false);
        st.set_notice(SharedString::new());
        st.set_copied(false);
        st.set_focused(true);
        st.set_open(true);
        self.request_code();
    }

    fn close(&self) {
        let Some(win) = self.window.upgrade() else {
            return;
        };
        let st = win.global::<PairState>();
        if st.get_phase() == phase::PAIRED {
            self.commit_name(st.get_device_name().as_str());
        }
        self.release_code();
        {
            let mut s = self.state.borrow_mut();
            s.open = false;
            s.events = None;
            s.fingerprint = None;
            s.generation += 1;
        }
        st.set_open(false);
        st.set_qr(Image::default());
        st.set_link_shown(SharedString::new());
        st.set_copied(false);
    }

    fn switch_mode(&self, tab: i32) {
        let Some(win) = self.window.upgrade() else {
            return;
        };
        let tab = tab.clamp(0, 1);
        if self.state.borrow().mode == tab {
            return;
        }
        self.release_code();
        {
            let mut s = self.state.borrow_mut();
            s.mode = tab;
            s.refreshes = 0;
        }
        win.global::<PairState>().set_mode(tab);
        self.request_code();
    }

    /// A click on "Show a new code" or "Try again": a fresh code and a fresh count.
    fn new_code(&self) {
        self.release_code();
        self.state.borrow_mut().refreshes = 0;
        self.request_code();
    }

    /// "Connect another" on the paired screen: the name is applied, then a new code shows.
    fn connect_another(&self) {
        let Some(win) = self.window.upgrade() else {
            return;
        };
        let st = win.global::<PairState>();
        self.commit_name(st.get_device_name().as_str());
        {
            let mut s = self.state.borrow_mut();
            s.fingerprint = None;
            s.removed = false;
        }
        st.set_removed(false);
        st.set_notice(SharedString::new());
        self.new_code();
    }

    // ---------------------------------------------------------------- the code

    /// The name a new device gets until the owner types another: "Phone 3" or "Computer 3".
    fn default_name(&self, mode: i32) -> String {
        let status = self.backend.get().status();
        let count = status
            .devices
            .iter()
            .filter(|d| matches!(d.role, PeerRole::Viewer))
            .count()
            + 1;
        if mode == 1 {
            format!("Computer {count}")
        } else {
            format!("Phone {count}")
        }
    }

    /// Asks the coordinator for a code on a helper thread and shows "Making a code…".
    fn request_code(&self) {
        let Some(win) = self.window.upgrade() else {
            return;
        };
        let (mode, generation) = {
            let mut s = self.state.borrow_mut();
            s.generation += 1;
            (s.mode, s.generation)
        };
        let name = self.default_name(mode);
        let st = win.global::<PairState>();
        st.set_phase(phase::MAKING);
        st.set_qr(Image::default());
        st.set_link_shown(SharedString::new());
        st.set_copied(false);
        let backend = self.backend.get();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = backend.issue_token(PeerRole::Viewer, &name);
            let _ = tx.send(Msg::Issued { generation, result });
        });
    }

    /// Shows a code that was just made.
    fn show_code(&self, win: &AppWindow, issued: IssuedToken) {
        let IssuedToken {
            id,
            link,
            expires_at,
            ..
        } = issued;
        let remaining = expires_at
            .duration_since(SystemTime::now())
            .unwrap_or(Duration::ZERO);
        let mode = self.state.borrow().mode;
        let st = win.global::<PairState>();

        if mode == 0 {
            let Some(image) = qr::image(&link) else {
                self.cancel_in_background(id);
                self.show_problem(win, "The pairing code is too long to draw. Try again.");
                return;
            };
            st.set_qr(image);
            st.set_link_shown(SharedString::new());
        } else {
            st.set_qr(Image::default());
            st.set_link_shown(SharedString::from(mask_link(&link)));
        }
        st.set_addresses(ModelRc::new(VecModel::from(
            link_addresses(&link)
                .into_iter()
                .map(SharedString::from)
                .collect::<Vec<_>>(),
        )));
        st.set_fraction(1.0);
        st.set_remaining_text(SharedString::from(clock_text(remaining)));
        st.set_copied(false);
        st.set_phase(phase::WAITING);

        let default_name = self.default_name(mode);
        self.state.borrow_mut().live = Some(Live {
            id,
            link: Zeroizing::new(link),
            default_name,
            expires: Instant::now() + remaining,
            total: remaining.max(Duration::from_secs(1)),
        });
    }

    /// Shows why no code could be made, with the one action that helps.
    fn show_problem(&self, win: &AppWindow, error: &str) {
        let state = self.backend.get().status().state;
        let no_network = pleochroma_net::netinfo::lan_ipv4().is_empty();
        let st = win.global::<PairState>();
        st.set_problem_text(SharedString::from(problem_text(&state, no_network, error)));
        st.set_problem_action(SharedString::from("Try again"));
        st.set_qr(Image::default());
        st.set_link_shown(SharedString::new());
        st.set_phase(phase::PROBLEM);
    }

    /// Cancels the live code, if any, and clears the clipboard if it still holds its link.
    fn release_code(&self) {
        let live = self.state.borrow_mut().live.take();
        if let Some(live) = live {
            self.cancel_in_background(live.id);
        }
        self.clear_clipboard();
    }

    fn cancel_in_background(&self, id: TokenId) {
        let backend = self.backend.get();
        std::thread::spawn(move || backend.cancel_token(id));
    }

    /// The live code ran out.
    fn expire(&self, win: &AppWindow) {
        self.release_code();
        let st = win.global::<PairState>();
        st.set_qr(Image::default());
        st.set_link_shown(SharedString::new());
        st.set_copied(false);
        if self.state.borrow().refreshes >= MAX_AUTO_REFRESHES {
            st.set_phase(phase::PAUSED);
        } else if st.get_focused() {
            self.auto_refresh();
        } else {
            st.set_phase(phase::EXPIRED);
        }
    }

    fn auto_refresh(&self) {
        self.state.borrow_mut().refreshes += 1;
        self.request_code();
    }

    // ---------------------------------------------------------------- the clock

    fn tick(&self) {
        self.drain_messages();
        let Some(win) = self.window.upgrade() else {
            return;
        };
        if !self.state.borrow().open {
            return;
        }
        self.drain_events(&win);
        let st = win.global::<PairState>();
        match st.get_phase() {
            phase::WAITING => {
                let left = self.state.borrow().live.as_ref().map(|live| {
                    (
                        live.expires.saturating_duration_since(Instant::now()),
                        live.total,
                    )
                });
                match left {
                    Some((remaining, total)) if !remaining.is_zero() => {
                        st.set_fraction(remaining.as_secs_f32() / total.as_secs_f32());
                        st.set_remaining_text(SharedString::from(clock_text(remaining)));
                    }
                    _ => self.expire(&win),
                }
            }
            phase::EXPIRED
                if st.get_focused() && self.state.borrow().refreshes < MAX_AUTO_REFRESHES =>
            {
                self.auto_refresh();
            }
            _ => {}
        }
    }

    fn drain_messages(&self) {
        while let Ok(msg) = self.rx.try_recv() {
            let win = self.window.upgrade();
            match msg {
                Msg::Issued { generation, result } => {
                    let current = {
                        let s = self.state.borrow();
                        s.open && s.generation == generation
                    };
                    match (current, win, result) {
                        (true, Some(win), Ok(issued)) => self.show_code(&win, issued),
                        (true, Some(win), Err(error)) => self.show_problem(&win, &error),
                        // A code nobody asked for any more: it must not stay valid.
                        (_, _, Ok(issued)) => self.cancel_in_background(issued.id),
                        _ => {}
                    }
                }
                Msg::Revoked(result) => {
                    let Some(win) = win else { continue };
                    let st = win.global::<PairState>();
                    match result {
                        Ok(()) => {
                            self.state.borrow_mut().removed = true;
                            st.set_removed(true);
                            st.set_notice(SharedString::new());
                        }
                        Err(error) => st.set_notice(SharedString::from(error)),
                    }
                }
                Msg::Renamed(result) => {
                    if let (Err(error), Some(win)) = (result, win) {
                        win.global::<PairState>()
                            .set_notice(SharedString::from(error));
                    }
                }
            }
        }
    }

    fn drain_events(&self, win: &AppWindow) {
        let events: Vec<CoordinatorEvent> = {
            let s = self.state.borrow();
            s.events
                .as_ref()
                .map(|rx| rx.try_iter().collect())
                .unwrap_or_default()
        };
        for event in events {
            match event {
                CoordinatorEvent::TokenClaimed {
                    role: PeerRole::Viewer,
                    fingerprint,
                    peer,
                } => {
                    self.on_claimed(win, &fingerprint, &peer.ip().to_string());
                }
                CoordinatorEvent::TokenExpired { id } => {
                    let ours = self
                        .state
                        .borrow()
                        .live
                        .as_ref()
                        .is_some_and(|l| l.id == id);
                    if ours {
                        self.expire(win);
                    }
                }
                _ => {}
            }
        }
    }

    // ---------------------------------------------------------------- paired

    /// The live code was claimed by `fingerprint` from `address`.
    fn on_claimed(&self, win: &AppWindow, fingerprint: &str, address: &str) {
        let live = self.state.borrow_mut().live.take();
        let Some(live) = live else {
            // A claim with no live code (a code of an earlier dialog): not ours to show.
            return;
        };
        self.clear_clipboard();
        {
            let mut s = self.state.borrow_mut();
            s.fingerprint = Some(fingerprint.to_owned());
            s.applied_name.clone_from(&live.default_name);
            s.removed = false;
        }
        let st = win.global::<PairState>();
        st.set_qr(Image::default());
        st.set_link_shown(SharedString::new());
        st.set_device_ip(SharedString::from(address));
        st.set_device_name(SharedString::from(live.default_name.as_str()));
        st.set_device_certificate(SharedString::from(certificate_text(fingerprint)));
        st.set_device_access(SharedString::from("Viewer"));
        st.set_removed(false);
        st.set_notice(SharedString::new());
        st.set_phase(phase::PAIRED);
    }

    /// Applies the typed name to the paired device (a no-op when it did not change).
    fn commit_name(&self, typed: &str) {
        let typed = typed.trim();
        let (fingerprint, applied, removed) = {
            let s = self.state.borrow();
            (s.fingerprint.clone(), s.applied_name.clone(), s.removed)
        };
        let Some(hex) = fingerprint else {
            return;
        };
        if removed {
            return;
        }
        let Some(win) = self.window.upgrade() else {
            return;
        };
        if typed.is_empty() {
            // An empty name is not a name: put the applied one back.
            win.global::<PairState>()
                .set_device_name(SharedString::from(applied));
            return;
        }
        if typed == applied {
            return;
        }
        let Some(fp) = fingerprint_from_hex(&hex) else {
            return;
        };
        self.state.borrow_mut().applied_name = typed.to_string();
        let backend = self.backend.get();
        let tx = self.tx.clone();
        let name = typed.to_string();
        std::thread::spawn(move || {
            let result = backend.rename(&fp, &name);
            let _ = tx.send(Msg::Renamed(result));
        });
    }

    /// "Not my device — remove access".
    fn remove_device(&self) {
        let hex = self.state.borrow().fingerprint.clone();
        let Some(hex) = hex else {
            return;
        };
        let Some(fp): Option<Fingerprint> = fingerprint_from_hex(&hex) else {
            if let Some(win) = self.window.upgrade() {
                win.global::<PairState>().set_notice(SharedString::from(
                    "This device's certificate could not be read. Remove it in Devices.",
                ));
            }
            return;
        };
        let backend = self.backend.get();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = backend.revoke(&fp);
            let _ = tx.send(Msg::Revoked(result));
        });
    }

    // ---------------------------------------------------------------- the clipboard

    /// Copies the link of the live code. Only the computer tab has the button.
    fn copy_link(&self) {
        let Some(win) = self.window.upgrade() else {
            return;
        };
        let st = win.global::<PairState>();
        let mut s = self.state.borrow_mut();
        if s.mode != 1 {
            return;
        }
        let Some(link) = s.live.as_ref().map(|live| live.link.clone()) else {
            return;
        };
        if s.clipboard.is_none() {
            let Ok(clipboard) = arboard::Clipboard::new() else {
                st.set_notice(SharedString::from("The clipboard is not available."));
                return;
            };
            s.clipboard = Some(clipboard);
        }
        let copied = s
            .clipboard
            .as_mut()
            .is_some_and(|clipboard| copy_secret(clipboard, &link).is_ok());
        if copied {
            s.copied_link = Some(link);
            st.set_copied(true);
            st.set_notice(SharedString::new());
        } else {
            st.set_notice(SharedString::from("The link could not be copied."));
        }
    }

    /// Empties the clipboard if it still holds the link this dialog copied.
    fn clear_clipboard(&self) {
        let mut s = self.state.borrow_mut();
        let Some(link) = s.copied_link.take() else {
            return;
        };
        if let Some(clipboard) = s.clipboard.as_mut() {
            let holds_link = clipboard
                .get_text()
                .is_ok_and(|text| text.as_str() == link.as_str());
            if holds_link {
                let _ = clipboard.clear();
            }
        }
    }
}

// -------------------------------------------------------------------- pure helpers

/// The IPv4 addresses a pairing link carries (its `a=` parts), without ports.
fn link_ips(link: &str) -> Vec<String> {
    link.split('&')
        .filter_map(|part| part.strip_prefix("a="))
        .map(|addr| addr.rsplit_once(':').map_or(addr, |(ip, _)| ip).to_string())
        .collect()
}

/// The addresses of a link as chip texts: "192.168.178.20 · Wi-Fi" when the adapter is known.
fn link_addresses(link: &str) -> Vec<String> {
    let adapters = pleochroma_net::netinfo::lan_ipv4();
    link_ips(link)
        .into_iter()
        .map(|ip| {
            adapters
                .iter()
                .find(|a| a.addr.to_string() == ip)
                .map_or_else(|| ip.clone(), |a| format!("{ip} · {}", a.interface))
        })
        .collect()
}

/// A certificate fingerprint (hex) as "3B9E…4A21".
fn certificate_text(fingerprint: &str) -> String {
    if fingerprint.len() < 8 || !fingerprint.is_ascii() {
        return fingerprint.to_string();
    }
    format!(
        "{}…{}",
        fingerprint[..4].to_ascii_uppercase(),
        fingerprint[fingerprint.len() - 4..].to_ascii_uppercase()
    )
}

/// The cause shown when no code could be made, in plain words.
fn problem_text(state: &CoordinatorState, no_network: bool, error: &str) -> String {
    let error = error.trim();
    if matches!(state, CoordinatorState::Starting) {
        "The coordinator is still starting. Try again in a moment.".to_string()
    } else if !state.is_running() {
        "The coordinator is not running. Start it, then try again.".to_string()
    } else if no_network {
        "This computer has no network connection. Connect to a network, then try again.".to_string()
    } else if error.is_empty() {
        "Something went wrong. Try again.".to_string()
    } else {
        error.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LINK: &str = "pleochroma://pair?v=1&r=viewer&n=Phone+1&a=192.168.178.20:7878&a=10.0.0.5:7878&e=7879&t=GW1-SECRETSECRET";

    #[test]
    fn the_clock_rounds_up() {
        assert_eq!(clock_text(Duration::from_secs(180)), "3:00");
        assert_eq!(clock_text(Duration::from_millis(167_100)), "2:48");
        assert_eq!(clock_text(Duration::from_millis(500)), "0:01");
        assert_eq!(clock_text(Duration::ZERO), "0:00");
    }

    #[test]
    fn the_masked_link_hides_the_code_and_keeps_the_addresses() {
        let masked = mask_link(LINK);
        assert!(!masked.contains("SECRET"));
        assert!(!masked.contains("GW1-"));
        assert!(masked.contains("a=192.168.178.20:7878"));
        assert!(masked.ends_with("&t=•••••••"));
        assert_eq!(mask_link("not a link"), "pleochroma://pair?…");
    }

    #[test]
    fn the_addresses_of_a_link_lose_their_ports() {
        assert_eq!(link_ips(LINK), vec!["192.168.178.20", "10.0.0.5"]);
        assert!(link_ips("pleochroma://pair?v=1").is_empty());
    }

    #[test]
    fn a_certificate_shows_its_first_and_last_four_characters() {
        let hex = "3b9e0000000000000000000000000000000000000000000000000000000000004a21";
        assert_eq!(certificate_text(hex), "3B9E…4A21");
        assert_eq!(certificate_text("abc"), "abc");
    }

    #[test]
    fn the_problem_text_prefers_the_state_then_the_network_then_the_error() {
        assert!(problem_text(&CoordinatorState::Starting, false, "x").contains("starting"));
        assert!(problem_text(&CoordinatorState::Stopped, false, "x").contains("not running"));
        assert!(problem_text(&CoordinatorState::Running, true, "x").contains("no network"));
        assert_eq!(
            problem_text(&CoordinatorState::Running, false, " Pairing is off. "),
            "Pairing is off."
        );
        assert_eq!(
            problem_text(&CoordinatorState::Running, false, ""),
            "Something went wrong. Try again."
        );
    }
}
