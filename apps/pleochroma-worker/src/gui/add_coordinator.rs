//! "Add a coordinator": the dialog that pairs this computer with another coordinator.
//!
//! The Slint side (`ui/add_coordinator.slint`, `AddCoordinatorDialog` and the `AddState` global)
//! only draws. This file checks what is typed, reads the clipboard for the Paste buttons, and
//! pairs on a helper thread: [`remotes::add_from_link`] for a pairing link, or
//! [`remotes::add_from_code`] for a code and an address. A new coordinator is shown at once
//! through the [`Switcher`].
//!
//! The link and the code are secrets. They are held in wiping strings, never logged, never put
//! in a window title, and removed from any error text before it is shown. The dialog empties the
//! fields when it closes and after a successful pairing.

use crate::{
    gui::{
        app::{AddState, AppWindow},
        switcher::{Switcher, access_label},
    },
    settings::remotes,
};
use pleochroma_net::{messages::PeerRole, pairing::PairingLink};
use slint::{ComponentHandle, SharedString, Timer, TimerMode};
use std::{
    cell::RefCell,
    rc::{Rc, Weak as RcWeak},
    sync::mpsc::{self, Receiver, Sender},
    time::Duration,
};
use zeroize::Zeroizing;

/// How often the dialog looks for the answer of its helper thread.
const TICK: Duration = Duration::from_millis(200);

/// The port a viewer address gets when none is typed (the coordinator's base port).
const DEFAULT_PORT: u16 = remotes::DEFAULT_VIEWER_PORT;

/// The answer of the pairing thread.
enum Msg {
    /// The coordinator is saved under this id, or the reason it could not be.
    Paired(Result<String, String>),
}

/// What the dialog remembers between ticks.
struct Ctx {
    window: slint::Weak<AppWindow>,
    switcher: Switcher,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    /// The link or code the running attempt used, to scrub it from an error text.
    secrets: RefCell<Vec<Zeroizing<String>>>,
    timer: Timer,
}

/// Connects the dialog. It opens through [`open`] (the switcher's "Add a coordinator…") and
/// selects a new coordinator through `switcher`.
pub fn wire(window: &AppWindow, switcher: &Switcher) {
    let (tx, rx) = mpsc::channel();
    let ctx = Rc::new(Ctx {
        window: window.as_weak(),
        switcher: switcher.clone(),
        tx,
        rx,
        secrets: RefCell::new(Vec::new()),
        timer: Timer::default(),
    });
    let me: RcWeak<Ctx> = Rc::downgrade(&ctx);
    ctx.timer.start(TimerMode::Repeated, TICK, move || {
        if let Some(ctx) = me.upgrade() {
            ctx.tick();
        }
    });

    let state = window.global::<AddState>();
    state.set_open(false);
    state.on_close_requested({
        let ctx = Rc::clone(&ctx);
        move || ctx.close()
    });
    state.on_edited({
        let ctx = Rc::clone(&ctx);
        move || {
            if let Some(window) = ctx.window.upgrade() {
                // Typing again leaves the last problem behind.
                window.global::<AddState>().set_error(SharedString::new());
                check(&window);
            }
        }
    });
    state.on_paste({
        let ctx = Rc::clone(&ctx);
        move |which| ctx.paste(which)
    });
    state.on_connect({
        let ctx = Rc::clone(&ctx);
        move || ctx.connect()
    });
}

/// Opens the dialog on the pairing-link tab with empty fields.
pub fn open(window: &AppWindow) {
    let state = window.global::<AddState>();
    state.set_mode(0);
    clear_fields(&state);
    state.set_error(SharedString::new());
    state.set_busy(false);
    check(window);
    state.set_open(true);
}

/// Empties the three fields.
fn clear_fields(state: &AddState<'_>) {
    state.set_link(SharedString::new());
    state.set_address(SharedString::new());
    state.set_code(SharedString::new());
}

impl Ctx {
    fn close(&self) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let state = window.global::<AddState>();
        if state.get_busy() {
            return;
        }
        clear_fields(&state);
        state.set_error(SharedString::new());
        state.set_open(false);
    }

    /// The Paste buttons: 0 the link field, 1 the code field.
    fn paste(&self, which: i32) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let state = window.global::<AddState>();
        let text = arboard::Clipboard::new().and_then(|mut clipboard| clipboard.get_text());
        let Ok(text) = text else {
            state.set_error("The clipboard is not available or holds no text.".into());
            return;
        };
        let text = Zeroizing::new(text);
        let text = text.trim();
        if which == 0 {
            state.set_link(text.into());
        } else {
            state.set_code(text.into());
        }
        state.set_error(SharedString::new());
        check(&window);
    }

    /// "Connect": pairs on a helper thread.
    fn connect(&self) {
        let Some(window) = self.window.upgrade() else {
            return;
        };
        let state = window.global::<AddState>();
        if state.get_busy() || !state.get_can_connect() {
            return;
        }
        let link_mode = state.get_mode() == 0;
        let link = Zeroizing::new(state.get_link().trim().to_owned());
        let code = Zeroizing::new(state.get_code().trim().to_owned());
        let address = state.get_address().trim().to_owned();
        {
            let mut secrets = self.secrets.borrow_mut();
            secrets.clear();
            secrets.push(link.clone());
            secrets.push(code.clone());
        }
        state.set_busy(true);
        state.set_error(SharedString::new());
        state.set_status_text("Connecting…".into());
        state.set_status_level(1);

        let tx = self.tx.clone();
        let spawned = std::thread::Builder::new()
            .name("add-coordinator".to_owned())
            .spawn(move || {
                let result = if link_mode {
                    remotes::add_from_link(&link)
                } else {
                    remotes::add_from_code(&code, &address)
                };
                let _ = tx.send(Msg::Paired(result.map(|id| id.0)));
            });
        if let Err(e) = spawned {
            state.set_busy(false);
            state.set_error(format!("Could not start the connection: {e}").into());
        }
    }

    /// Takes in the answer of the pairing thread.
    fn tick(&self) {
        while let Ok(Msg::Paired(result)) = self.rx.try_recv() {
            let Some(window) = self.window.upgrade() else {
                continue;
            };
            let state = window.global::<AddState>();
            state.set_busy(false);
            match result {
                Ok(id) => {
                    self.secrets.borrow_mut().clear();
                    clear_fields(&state);
                    state.set_error(SharedString::new());
                    state.set_open(false);
                    self.switcher.select_remote(&id);
                }
                Err(reason) => {
                    let shown = self.scrub(&reason);
                    state.set_error(shown.into());
                }
            }
        }
    }

    /// The error in plain words, with any text of the link or the code taken out.
    fn scrub(&self, reason: &str) -> String {
        let mut text = reason.trim().to_owned();
        for secret in self.secrets.borrow().iter() {
            if !secret.is_empty() {
                text = text.replace(secret.as_str(), "…");
            }
        }
        if text.is_empty() {
            "The coordinator could not be paired. Check the code and the address, then try again."
                .to_owned()
        } else {
            text
        }
    }
}

/// Checks what is typed in the current tab and sets the footer line, the "this link contains"
/// card and whether Connect is possible.
fn check(window: &AppWindow) {
    let state = window.global::<AddState>();
    let outcome = if state.get_mode() == 0 {
        check_link(state.get_link().as_str())
    } else {
        check_manual(state.get_address().as_str(), state.get_code().as_str())
    };
    state.set_link_ok(outcome.card.is_some());
    if let Some(card) = outcome.card {
        state.set_link_name(card.name.into());
        state.set_link_place(card.place.into());
        state.set_link_access(card.access.into());
        state.set_link_expires(
            format!(
                "{} min, one use",
                crate::enroll::TOKEN_TTL_SECS.div_ceil(60)
            )
            .into(),
        );
    }
    state.set_status_text(outcome.text.into());
    state.set_status_level(outcome.level);
    state.set_can_connect(outcome.ok);
}

/// What a checked link names.
#[derive(Debug, PartialEq, Eq)]
struct Card {
    name: String,
    place: String,
    access: String,
}

/// The result of checking the typed text.
#[derive(Debug, PartialEq, Eq)]
struct Outcome {
    /// Connect may be pressed.
    ok: bool,
    /// The footer line.
    text: String,
    /// 0 fine, 1 information, 2 problem.
    level: i32,
    /// What the link names (link tab only).
    card: Option<Card>,
}

impl Outcome {
    fn info(text: &str) -> Self {
        Self {
            ok: false,
            text: text.to_owned(),
            level: 1,
            card: None,
        }
    }

    const fn problem(text: String) -> Self {
        Self {
            ok: false,
            text,
            level: 2,
            card: None,
        }
    }
}

/// Reads a pasted pairing link without using it: the footer says whether it can be used.
fn check_link(text: &str) -> Outcome {
    let text = text.trim();
    if text.is_empty() {
        return Outcome::info("Paste the pairing link from the other coordinator.");
    }
    match PairingLink::parse(text) {
        Err(e) => Outcome::problem(e.to_string()),
        Ok(link) => match link.role {
            PeerRole::Monitor | PeerRole::Admin => {
                let access = if link.role == PeerRole::Admin {
                    access_label(crate::control::backend::AccessRole::Admin)
                } else {
                    access_label(crate::control::backend::AccessRole::Monitor)
                };
                let name = if link.name.trim().is_empty() {
                    "Coordinator".to_owned()
                } else {
                    link.name.clone()
                };
                let place = link
                    .addrs
                    .first()
                    .map(ToString::to_string)
                    .unwrap_or_default();
                Outcome {
                    ok: true,
                    text: "Link looks valid".to_owned(),
                    level: 0,
                    card: Some(Card {
                        name,
                        place,
                        access: access.to_owned(),
                    }),
                }
            }
            PeerRole::Viewer | PeerRole::Worker => Outcome::problem(
                "This link is for a phone or a render computer. Make a monitor or administrator code on the other coordinator instead."
                    .to_owned(),
            ),
        },
    }
}

/// Checks a typed address and code.
fn check_manual(address: &str, code: &str) -> Outcome {
    let address = address.trim();
    let code = code.trim();
    if address.is_empty() && code.is_empty() {
        return Outcome::info("Type the address and the code from the other coordinator.");
    }
    if address.is_empty() {
        return Outcome::info("Type the address of the other coordinator.");
    }
    if !valid_address(address) {
        return Outcome::problem(
            "The address is written like 192.168.178.40:7878 (an IPv4 address, then the port)."
                .to_owned(),
        );
    }
    if code.is_empty() {
        return Outcome::info("Type or paste the code.");
    }
    if !code.to_ascii_uppercase().starts_with("GW1-") {
        return Outcome {
            ok: true,
            text: "Codes start with GW1-. Check that you copied all of it.".to_owned(),
            level: 1,
            card: None,
        };
    }
    Outcome {
        ok: true,
        text: "Address and code look valid".to_owned(),
        level: 0,
        card: None,
    }
}

/// True for `a.b.c.d` or `a.b.c.d:port` with a port from 1 to 65535.
fn valid_address(text: &str) -> bool {
    let (host, port) = text
        .rsplit_once(':')
        .map_or((text, Some(DEFAULT_PORT)), |(host, port)| {
            (host, port.parse::<u16>().ok().filter(|p| *p != 0))
        });
    port.is_some() && host.parse::<std::net::Ipv4Addr>().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ADMIN_LINK: &str = "pleochroma://pair?v=1&r=admin&n=Garage+server&a=192.168.178.40:7878&e=7879&t=GW1-AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

    #[test]
    fn an_empty_link_asks_for_one() {
        let outcome = check_link("  ");
        assert!(!outcome.ok);
        assert_eq!(outcome.level, 1);
    }

    #[test]
    fn something_that_is_no_link_says_so() {
        let outcome = check_link("hello");
        assert!(!outcome.ok);
        assert_eq!(outcome.level, 2);
        assert!(outcome.text.contains("not a Pleochroma pairing link"));
    }

    #[test]
    fn a_phone_link_is_turned_away() {
        let phone = ADMIN_LINK.replace("r=admin", "r=viewer");
        if let Ok(link) = PairingLink::parse(&phone) {
            assert_eq!(link.role, PeerRole::Viewer);
            let outcome = check_link(&phone);
            assert!(!outcome.ok);
            assert!(outcome.text.contains("phone or a render computer"));
        }
    }

    #[test]
    fn addresses_need_an_ipv4_host_and_a_real_port() {
        assert!(valid_address("192.168.178.40:7878"));
        assert!(valid_address("192.168.178.40"));
        assert!(!valid_address("192.168.178.40:0"));
        assert!(!valid_address("192.168.178.40:99999"));
        assert!(!valid_address("garage.local:7878"));
        assert!(!valid_address(""));
    }

    #[test]
    fn the_manual_tab_walks_the_user_through_the_fields() {
        assert_eq!(check_manual("", "").level, 1);
        assert!(!check_manual("", "GW1-X").ok);
        assert_eq!(check_manual("192.168.1.2:7878", "").level, 1);
        assert_eq!(check_manual("nonsense", "GW1-X").level, 2);
        let fine = check_manual("192.168.1.2:7878", "GW1-ABC");
        assert!(fine.ok);
        assert_eq!(fine.level, 0);
        let odd = check_manual("192.168.1.2:7878", "ABC");
        assert!(odd.ok);
        assert_eq!(odd.level, 1);
    }
}
