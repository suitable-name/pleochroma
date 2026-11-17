//! The tray icon (Windows notification area, macOS menu bar, Linux status notifier item).
//!
//! It is Slint's own `SystemTrayIcon` (the `system-tray` feature, the same one Pleochroma Cut
//! turns on): `Shell_NotifyIcon` on Windows, an `NSStatusItem` on macOS and `ksni` (a pure D-Bus
//! status notifier item, no GTK) on Linux. The component below is a tray-rooted one with no
//! window: its menu holds Open, the status line, Connect a phone and Quit.
//!
//! # No tray host
//!
//! Windows and macOS always have one. A Linux desktop may not (no `StatusNotifierWatcher` on
//! the session bus, as on GNOME without its extension): [`host_available`] asks, and the
//! window then minimizes on close instead of hiding.
//!
//! The tray must be made on the UI thread, and its callbacks run there.

use std::time::Duration;

/// The generated tray component.
mod host {
    slint::slint! {
        export component TrayHost inherits SystemTrayIcon {
            in property <string> status-text: "Stopped";
            in property <bool> can-pair: false;
            in property <image> tray-icon;
            callback open();
            callback pair();
            callback quit();
            icon: root.tray-icon;
            tooltip: "Pleochroma Coordinator";
            clicked => { root.open(); }
            Menu {
                MenuItem {
                    title: "Open Pleochroma Coordinator";
                    activated => { root.open(); }
                }
                MenuItem {
                    title: root.status-text;
                    enabled: false;
                }
                MenuSeparator { }
                MenuItem {
                    title: "Connect a phone";
                    enabled: root.can-pair;
                    activated => { root.pair(); }
                }
                MenuSeparator { }
                MenuItem {
                    title: "Quit";
                    activated => { root.quit(); }
                }
            }
        }
    }
}

/// The application icon, the same picture as the window's.
const ICON_PNG: &[u8] = include_bytes!("../../assets/icon.png");

/// A live tray icon. Dropping it removes the icon.
pub struct Tray {
    host: host::TrayHost,
}

impl Tray {
    /// Puts the icon and its menu up. `open`, `pair` and `quit` run on the UI thread: the icon
    /// was clicked or "Open" was chosen, "Connect a phone" was chosen, "Quit" was chosen.
    ///
    /// # Errors
    ///
    /// A message when the icon picture cannot be decoded or the component cannot be built.
    pub fn new(
        open: impl Fn() + 'static,
        pair: impl Fn() + 'static,
        quit: impl Fn() + 'static,
    ) -> Result<Self, String> {
        let host = host::TrayHost::new().map_err(|e| e.to_string())?;
        host.set_tray_icon(icon()?);
        host.on_open(open);
        host.on_pair(pair);
        host.on_quit(quit);
        Ok(Self { host })
    }

    /// Shows the status line and whether "Connect a phone" works now.
    pub fn set_status(&self, line: &str, can_pair: bool) {
        self.host.set_status_text(line.into());
        self.host.set_can_pair(can_pair);
    }
}

/// The tray picture as a Slint image.
fn icon() -> Result<slint::Image, String> {
    let decoded = image::load_from_memory(ICON_PNG)
        .map_err(|e| format!("could not read the tray icon: {e}"))?
        .to_rgba8();
    let buffer = slint::SharedPixelBuffer::<slint::Rgba8Pixel>::clone_from_slice(
        decoded.as_raw(),
        decoded.width(),
        decoded.height(),
    );
    Ok(slint::Image::from_rgba8(buffer))
}

/// Whether something will show a tray icon: always on Windows and macOS; on other desktops
/// only when a status notifier host is on the session bus.
#[must_use]
pub fn host_available() -> bool {
    if cfg!(any(windows, target_os = "macos")) {
        return true;
    }
    let args = [
        "--session",
        "--dest=org.freedesktop.DBus",
        "--type=method_call",
        "--print-reply",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus.NameHasOwner",
        "string:org.kde.StatusNotifierWatcher",
    ];
    super::common::run_bounded("dbus-send", &args, Duration::from_secs(1))
        .is_some_and(|out| out.success && name_has_owner(&out.stdout))
}

/// Whether a `NameHasOwner` reply from `dbus-send` says the name has an owner.
fn name_has_owner(reply: &str) -> bool {
    reply.contains("boolean true")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_watcher_reply_is_read() {
        assert!(name_has_owner(
            "method return time=1.0 sender=org.freedesktop.DBus -> destination=:1.5 serial=3 reply_serial=2\n   boolean true\n"
        ));
        assert!(!name_has_owner("method return\n   boolean false\n"));
        assert!(!name_has_owner(""));
    }

    #[test]
    fn the_icon_picture_decodes() {
        assert!(icon().is_ok());
    }
}
