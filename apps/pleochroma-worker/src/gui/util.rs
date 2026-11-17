//! Small helpers the window and its pages share.

use std::{path::Path, time::Duration};

/// Opens `dir` in the system's file manager, creating it first when it is missing.
///
/// Never blocks the caller: the file manager is started and waited for on a thread of its own,
/// and a failure is logged. Only for a folder on this computer; a page showing a remote
/// coordinator must not call it with that coordinator's paths.
pub fn open_folder(dir: &Path) {
    let _ = std::fs::create_dir_all(dir);
    let program = if cfg!(target_os = "windows") {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    let dir = dir.to_path_buf();
    let spawned = std::thread::Builder::new()
        .name("open-folder".to_owned())
        .spawn(move || {
            // `explorer` answers with a non-zero status even when it worked, so only a
            // failure to start the program is reported.
            if let Err(e) = std::process::Command::new(program).arg(&dir).status() {
                tracing::warn!("could not open {}: {e}", dir.display());
            }
        });
    if let Err(e) = spawned {
        tracing::warn!("could not start the folder opener: {e}");
    }
}

/// Puts `text` on the clipboard and keeps it out of the platform's clipboard history.
///
/// Windows: the `ExcludeClipboardContentFromMonitorProcessing` format (history, cloud
/// clipboard and monitors all skip it). macOS and Linux: the platform's history-exclusion
/// hint.
///
/// # Errors
///
/// The clipboard's own error when the text could not be set.
pub fn copy_secret(clipboard: &mut arboard::Clipboard, text: &str) -> Result<(), arboard::Error> {
    let set = clipboard.set();
    #[cfg(windows)]
    let set = {
        use arboard::SetExtWindows as _;
        set.exclude_from_monitoring()
    };
    #[cfg(target_os = "macos")]
    let set = {
        use arboard::SetExtApple as _;
        set.exclude_from_history()
    };
    #[cfg(all(
        unix,
        not(any(target_os = "macos", target_os = "android", target_os = "emscripten"))
    ))]
    let set = {
        use arboard::SetExtLinux as _;
        set.exclude_from_history()
    };
    set.text(text.to_owned())
}

/// `m:ss`, rounded up to whole seconds.
#[must_use]
pub fn clock_text(remaining: Duration) -> String {
    let secs = remaining.as_secs() + u64::from(remaining.subsec_nanos() > 0);
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// The link with its code replaced by dots, for the screen. The addresses stay visible.
#[must_use]
pub fn mask_link(link: &str) -> String {
    link.find("&t=").map_or_else(
        || "pleochroma://pair?\u{2026}".to_owned(),
        |at| {
            format!(
                "{}&t=\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}\u{2022}",
                &link[..at]
            )
        },
    )
}
