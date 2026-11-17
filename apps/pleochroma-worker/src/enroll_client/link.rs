//! The pairing link and the terminal QR code `cert issue-token` prints next to a code.
//!
//! Everything here is pure except [`coordinator_name`] and [`coordinator_port`] (read the
//! loaded settings and the computer's name), [`with_terminal_escapes`] (asks the console) and
//! the LAN lookup the caller passes in, so the tests need no network. The link and the code inside
//! it are secrets: nothing here logs them, and no type here implements `Debug`.

use pleochroma_net::{
    messages::PeerRole,
    pairing::{MAX_ADDRS, MAX_NAME_BYTES, PairingLink},
};
use qrcode::{Color, EcLevel, QrCode};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};

/// The quiet zone around the QR code, in modules (the standard asks for four).
const QUIET_MODULES: usize = 4;

/// Selects black text on a white background, so the code scans on a dark terminal too.
const ANSI_BLACK_ON_WHITE: &str = "\x1b[30;47m";

/// Resets the colors set by [`ANSI_BLACK_ON_WHITE`].
const ANSI_RESET: &str = "\x1b[0m";

/// The port of an enrollment listener address (`host:port`).
///
/// # Errors
///
/// A plain message if `admin_addr` has no valid port.
pub(super) fn enroll_port(admin_addr: &str) -> Result<u16, String> {
    admin_addr
        .rsplit_once(':')
        .and_then(|(_, port)| port.parse::<u16>().ok())
        .filter(|port| *port != 0)
        .ok_or_else(|| format!("--admin-addr {admin_addr:?} must be host:port"))
}

/// The coordinator port that belongs to an enrollment port.
///
/// When the settings are loaded and their enrollment port for `role` is `enroll_port`, the
/// coordinator port is the one the settings choose. Otherwise it is one below the enrollment
/// port (7879 -> 7878 for viewers, monitors and admins; 7881 -> 7880 for workers), the default
/// layout; a coordinator started with its own `--enroll-bind` needs `--address`.
pub(super) fn coordinator_port(enroll_port: u16, role: PeerRole) -> u16 {
    let ports = crate::settings::settings_file()
        .and_then(|_| crate::settings::current().effective_ports().ok());
    coordinator_port_in(enroll_port, role, ports)
}

/// [`coordinator_port`] for the given settings ports (`None`: no settings are loaded).
fn coordinator_port_in(
    enroll_port: u16,
    role: PeerRole,
    ports: Option<crate::settings::model::EffectivePorts>,
) -> u16 {
    let chosen = ports.and_then(|ports| match role {
        PeerRole::Worker => (ports.worker_enroll == enroll_port).then_some(ports.worker),
        PeerRole::Viewer | PeerRole::Monitor | PeerRole::Admin => {
            (ports.viewer_enroll == enroll_port).then_some(ports.viewer)
        }
    });
    chosen.unwrap_or_else(|| enroll_port.saturating_sub(1).max(1))
}

/// The addresses the link carries: the `--address` values when there are any, else this
/// machine's LAN addresses (default route first) on `port`, at most [`MAX_ADDRS`].
///
/// # Errors
///
/// A plain message when there is nothing to put in the link.
pub(super) fn link_addresses(
    explicit: &[SocketAddrV4],
    lan: &[Ipv4Addr],
    port: u16,
) -> Result<Vec<SocketAddr>, String> {
    let addrs: Vec<SocketAddr> = if explicit.is_empty() {
        lan.iter()
            .take(MAX_ADDRS)
            .map(|ip| SocketAddr::V4(SocketAddrV4::new(*ip, port)))
            .collect()
    } else {
        explicit
            .iter()
            .take(MAX_ADDRS)
            .copied()
            .map(SocketAddr::V4)
            .collect()
    };
    if addrs.is_empty() {
        return Err(
            "this machine has no LAN address to put in the pairing link -- pass the coordinator's \
             address with --address <ip:port>"
                .to_string(),
        );
    }
    Ok(addrs)
}

/// The coordinator's display name for the link, cut to what a link allows: the name in the
/// loaded settings, else the computer's own name, else "Pleochroma".
pub(super) fn coordinator_name() -> String {
    let chosen = crate::settings::settings_file().map(|_| crate::settings::current().name);
    let raw = chosen
        .filter(|name| !name.trim().is_empty())
        .or_else(crate::settings::model::computer_name)
        .unwrap_or_default();
    clean_name(&raw)
}

/// How the QR code is drawn: what the place it is printed to can show.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum QrStyle {
    /// A terminal that understands escape sequences: black on white lines, any theme scans.
    BlackOnWhite,
    /// A console without escape sequences (the classic Windows console), which is dark: the
    /// glyphs are the light modules, so the picture is the code in light on dark.
    LightOnDark,
    /// A pipe or a file: dark modules as glyphs, for dark text on a light page.
    Plain,
}

/// Runs `draw` with the [`QrStyle`] the terminal on standard output can show. A pipe or a file
/// gets [`QrStyle::Plain`]. On Windows the answer comes from the console itself: virtual
/// terminal processing is switched on for the call and put back after it, and a console that
/// refuses it (the classic one) gets [`QrStyle::LightOnDark`].
pub(super) fn with_terminal_escapes<R>(draw: impl FnOnce(QrStyle) -> R) -> R {
    if !std::io::IsTerminal::is_terminal(&std::io::stdout()) {
        return draw(QrStyle::Plain);
    }
    escapes_on_terminal(draw)
}

/// Other platforms: a terminal handles the escapes.
#[cfg(not(windows))]
fn escapes_on_terminal<R>(draw: impl FnOnce(QrStyle) -> R) -> R {
    draw(QrStyle::BlackOnWhite)
}

/// Windows: asks the console for `ENABLE_VIRTUAL_TERMINAL_PROCESSING` and restores its mode.
#[cfg(windows)]
fn escapes_on_terminal<R>(draw: impl FnOnce(QrStyle) -> R) -> R {
    use std::ffi::c_void;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetStdHandle(id: u32) -> *mut c_void;
        fn GetConsoleMode(handle: *mut c_void, mode: *mut u32) -> i32;
        fn SetConsoleMode(handle: *mut c_void, mode: u32) -> i32;
    }
    /// `STD_OUTPUT_HANDLE`, which is -11 as an unsigned value.
    const STD_OUTPUT_HANDLE: u32 = 0xFFFF_FFF5;
    /// `ENABLE_VIRTUAL_TERMINAL_PROCESSING`.
    const ENABLE_VT: u32 = 0x0004;

    let mut mode = 0_u32;
    // SAFETY: both calls only read or set the mode of the standard output handle; `mode` is a
    // valid pointer to a `u32` for the whole call.
    let handle = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    // SAFETY: as above.
    if handle.is_null() || unsafe { GetConsoleMode(handle, &raw mut mode) } == 0 {
        // Not a console after all (a redirected handle): a file or a pipe.
        return draw(QrStyle::Plain);
    }
    if mode & ENABLE_VT != 0 {
        return draw(QrStyle::BlackOnWhite);
    }
    // SAFETY: as above.
    if unsafe { SetConsoleMode(handle, mode | ENABLE_VT) } == 0 {
        return draw(QrStyle::LightOnDark);
    }
    let result = draw(QrStyle::BlackOnWhite);
    // SAFETY: as above; this puts the console back as it was.
    unsafe { SetConsoleMode(handle, mode) };
    result
}

/// Strips control characters, trims, and cuts `raw` to [`MAX_NAME_BYTES`] on a character
/// boundary. An empty result becomes "Pleochroma".
fn clean_name(raw: &str) -> String {
    let mut name = String::new();
    for c in raw.trim().chars().filter(|c| !c.is_control()) {
        if name.len() + c.len_utf8() > MAX_NAME_BYTES {
            break;
        }
        name.push(c);
    }
    if name.is_empty() {
        "Pleochroma".to_string()
    } else {
        name
    }
}

/// Builds the `pleochroma://pair` link text.
///
/// # Errors
///
/// A plain message naming what the link format refuses; it never contains the code.
pub(super) fn build_link(
    role: PeerRole,
    name: &str,
    addrs: Vec<SocketAddr>,
    enroll_port: u16,
    token: &str,
) -> Result<String, String> {
    PairingLink::new(role, name, addrs, enroll_port, token)
        .build()
        .map_err(|e| format!("could not build the pairing link: {e}"))
}

/// Draws `text` as a QR code in Unicode half blocks, two module rows per text line, with
/// the four-module quiet zone. With [`QrStyle::BlackOnWhite`] every line is black on white,
/// so the code scans on any terminal theme; [`QrStyle::Plain`] draws the dark modules as
/// glyphs (a pipe or a file), and [`QrStyle::LightOnDark`] draws the light modules, quiet zone
/// included, for a dark console that cannot color.
///
/// Returns `None` when the text does not fit the largest code.
pub(super) fn half_block_qr(text: &str, style: QrStyle) -> Option<String> {
    let ansi = style == QrStyle::BlackOnWhite;
    let invert = style == QrStyle::LightOnDark;
    let code = QrCode::with_error_correction_level(text.as_bytes(), EcLevel::M).ok()?;
    let modules = code.width();
    let colors = code.to_colors();
    let total = modules + 2 * QUIET_MODULES;
    let dark = |x: usize, y: usize| -> bool {
        x >= QUIET_MODULES
            && y >= QUIET_MODULES
            && x < QUIET_MODULES + modules
            && y < QUIET_MODULES + modules
            && colors[(y - QUIET_MODULES) * modules + (x - QUIET_MODULES)] == Color::Dark
    };
    // A glyph is drawn where a module is lit: dark modules, or (light on dark) the others
    // inside the picture.
    let lit = |x: usize, y: usize| -> bool {
        if invert {
            x < total && y < total && !dark(x, y)
        } else {
            dark(x, y)
        }
    };
    let mut out = String::new();
    for y in (0..total).step_by(2) {
        if ansi {
            out.push_str(ANSI_BLACK_ON_WHITE);
        }
        for x in 0..total {
            out.push(match (lit(x, y), lit(x, y + 1)) {
                (false, false) => ' ',
                (true, false) => '\u{2580}',
                (false, true) => '\u{2584}',
                (true, true) => '\u{2588}',
            });
        }
        if ansi {
            out.push_str(ANSI_RESET);
        }
        out.push('\n');
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v4(text: &str) -> SocketAddrV4 {
        text.parse().unwrap()
    }

    #[test]
    fn enroll_port_reads_the_port_and_refuses_the_rest() {
        assert_eq!(enroll_port("127.0.0.1:7879").unwrap(), 7879);
        assert_eq!(enroll_port("[::1]:7881").unwrap(), 7881);
        assert!(enroll_port("127.0.0.1").is_err());
        assert!(enroll_port("127.0.0.1:0").is_err());
        assert!(enroll_port("host:notaport").is_err());
    }

    #[test]
    fn the_coordinator_port_is_one_below_the_enrollment_port_without_settings() {
        assert_eq!(coordinator_port_in(7879, PeerRole::Viewer, None), 7878);
        assert_eq!(coordinator_port_in(7881, PeerRole::Worker, None), 7880);
        assert_eq!(coordinator_port_in(1, PeerRole::Admin, None), 1);
    }

    #[test]
    fn the_settings_name_the_coordinator_port_when_their_enrollment_port_matches() {
        use crate::settings::model::EffectivePorts;
        let ports = EffectivePorts {
            viewer: 9000,
            viewer_enroll: 9500,
            worker: 9100,
            worker_enroll: 9600,
        };
        let port = |enroll, role| coordinator_port_in(enroll, role, Some(ports));
        assert_eq!(port(9500, PeerRole::Viewer), 9000);
        assert_eq!(port(9500, PeerRole::Monitor), 9000);
        assert_eq!(port(9500, PeerRole::Admin), 9000);
        assert_eq!(port(9600, PeerRole::Worker), 9100);
        // A different enrollment port than the settings say (a hand-made `--enroll-bind`):
        // the default layout.
        assert_eq!(port(7879, PeerRole::Viewer), 7878);
        // A viewer code on the worker enrollment port is not the worker port.
        assert_eq!(port(9600, PeerRole::Viewer), 9599);
    }

    #[test]
    fn explicit_addresses_win_over_the_lan_list() {
        let lan = [Ipv4Addr::new(10, 0, 0, 5)];
        let addrs = link_addresses(&[v4("192.168.1.20:9000")], &lan, 7878).unwrap();
        assert_eq!(addrs, vec![SocketAddr::V4(v4("192.168.1.20:9000"))]);
    }

    #[test]
    fn the_lan_list_uses_the_coordinator_port_and_stops_at_three() {
        let lan = [
            Ipv4Addr::new(10, 0, 0, 1),
            Ipv4Addr::new(10, 0, 0, 2),
            Ipv4Addr::new(10, 0, 0, 3),
            Ipv4Addr::new(10, 0, 0, 4),
        ];
        let addrs = link_addresses(&[], &lan, 7878).unwrap();
        assert_eq!(addrs.len(), 3);
        assert_eq!(addrs[0], SocketAddr::V4(v4("10.0.0.1:7878")));
    }

    #[test]
    fn no_address_at_all_says_to_pass_one() {
        let err = link_addresses(&[], &[], 7878).unwrap_err();
        assert!(err.contains("--address"), "{err}");
    }

    #[test]
    fn names_are_cleaned_and_cut_on_a_character_boundary() {
        assert_eq!(clean_name("  Studio PC\n"), "Studio PC");
        assert_eq!(clean_name(""), "Pleochroma");
        let long = "\u{e9}".repeat(100);
        let cut = clean_name(&long);
        assert!(cut.len() <= MAX_NAME_BYTES);
        assert!(cut.chars().all(|c| c == '\u{e9}'));
    }

    #[test]
    fn the_link_carries_the_role_addresses_and_code() {
        let link = build_link(
            PeerRole::Admin,
            "Studio PC",
            vec![SocketAddr::V4(v4("192.168.1.20:7878"))],
            7879,
            "GW1-ABCDEF",
        )
        .unwrap();
        assert!(link.starts_with("pleochroma://pair?"), "{link}");
        let parsed = PairingLink::parse(&link).unwrap();
        assert_eq!(parsed.role, PeerRole::Admin);
        assert_eq!(parsed.enroll_port, 7879);
        assert_eq!(parsed.token(), "GW1-ABCDEF");
    }

    #[test]
    fn a_bad_link_error_never_echoes_the_code() {
        let err =
            build_link(PeerRole::Monitor, "x", Vec::new(), 7879, "GW1-SECRETCODE").unwrap_err();
        assert!(!err.contains("SECRETCODE"), "{err}");
    }

    #[test]
    fn the_qr_has_a_quiet_zone_and_one_line_per_two_module_rows() {
        let text = "pleochroma://pair?v=1&r=monitor&n=Studio&a=192.168.1.20:7878&e=7879&t=GW1-AAAA";
        let plain = half_block_qr(text, QrStyle::Plain).unwrap();
        let lines: Vec<&str> = plain.lines().collect();
        let width = lines[0].chars().count();
        assert!(lines.iter().all(|l| l.chars().count() == width));
        // The first quiet-zone line (modules 0 and 1 of 8 quiet rows) is blank.
        assert!(lines[0].chars().all(|c| c == ' '));
        // A version-3 code or larger, so more than a handful of lines.
        assert!(lines.len() > 10);
        assert!(plain.contains('\u{2588}') || plain.contains('\u{2580}'));
    }

    #[test]
    fn the_ansi_qr_wraps_every_line_in_black_on_white() {
        let text = "pleochroma://pair?v=1";
        let coloured = half_block_qr(text, QrStyle::BlackOnWhite).unwrap();
        for line in coloured.lines() {
            assert!(line.starts_with(ANSI_BLACK_ON_WHITE), "{line:?}");
            assert!(line.ends_with(ANSI_RESET), "{line:?}");
        }
    }

    /// A dark console without colors shows the light modules: the quiet zone is lit and the
    /// picture is the plain one with lit and unlit swapped.
    #[test]
    fn the_light_on_dark_qr_swaps_the_modules() {
        let text = "pleochroma://pair?v=1";
        let plain = half_block_qr(text, QrStyle::Plain).unwrap();
        let swapped = half_block_qr(text, QrStyle::LightOnDark).unwrap();
        assert!(!swapped.contains('\u{1b}'));
        let first = swapped.lines().next().unwrap();
        assert!(first.chars().all(|c| c == '\u{2588}'), "{first}");
        let plain_first = plain.lines().next().unwrap();
        assert!(plain_first.chars().all(|c| c == ' '));
        // Every cell differs in the way a swap changes it (the last line has a single module
        // row, since the picture has an odd number of rows, and is left out).
        let lines = plain.lines().count() - 1;
        let body = |text: &str| text.lines().take(lines).collect::<Vec<_>>().join("\n");
        for (p, s) in body(&plain).chars().zip(body(&swapped).chars()) {
            let expected = match p {
                ' ' => '\u{2588}',
                '\u{2588}' => ' ',
                '\u{2580}' => '\u{2584}',
                '\u{2584}' => '\u{2580}',
                other => other, // the line break
            };
            assert_eq!(s, expected);
        }
    }

    #[test]
    fn text_past_the_largest_code_gives_none() {
        assert!(half_block_qr(&"A".repeat(8000), QrStyle::Plain).is_none());
    }
}
