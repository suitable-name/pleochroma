//! `pleochroma-cut --help` and `--version`: recognized and answered before any window.
//!
//! A release build uses the Windows GUI subsystem, so it has no console of its own and
//! `--help` used to open the editor. `main` calls [`answer_if_requested`] first of all, before
//! Slint, the GPU or the RADV workaround's restart wait are touched.
//!
//! Where the text goes:
//! - Windows with a standard output the process was started with (a redirect or a pipe):
//!   that handle.
//! - Windows started from a console (`cmd`, PowerShell): the parent's console, attached with
//!   `AttachConsole`. The shell does not wait for a GUI program, so its prompt may come back
//!   before the text.
//! - Windows with no console at all (a shortcut, Explorer): a message box.
//! - macOS and Linux: standard output.

use std::ffi::OsString;

/// What the command line asks for instead of the editor.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InfoRequest {
    /// `-h` or `--help`: the usage text.
    Help,
    /// `-V` or `--version`: the version line.
    Version,
}

/// The information request in a program's arguments (without the program name), if any.
///
/// The first of `-h`, `--help`, `-V` and `--version` wins, wherever it stands in the line
/// (`pleochroma-cut design.psdd --help` is a help request). After a bare `--` every argument
/// is a file name, so nothing after it counts. A file path, with or without `--log`, is not
/// a request.
#[must_use]
pub fn info_request(args: impl IntoIterator<Item = OsString>) -> Option<InfoRequest> {
    for arg in args {
        if arg == "--" {
            return None;
        }
        if arg == "-h" || arg == "--help" {
            return Some(InfoRequest::Help);
        }
        if arg == "-V" || arg == "--version" {
            return Some(InfoRequest::Version);
        }
    }
    None
}

/// The one-line answer to `--version`: program name and version.
#[must_use]
pub fn version_line() -> String {
    format!("pleochroma-cut {}", env!("CARGO_PKG_VERSION"))
}

/// The answer to `--help`.
#[must_use]
pub fn usage_text() -> String {
    format!(
        "{version}\n\
         \n\
         Usage: pleochroma-cut [OPTIONS] [FILE]\n\
         \n\
         Opens the Pleochroma Cut editor. FILE is a design to open once the window is up.\n\
         \n\
         Options:\n\
         \x20 --log           Also write a log file (pleochroma-cut.log) beside the program,\n\
         \x20                 or into the temp folder when that is not writable\n\
         \x20 -h, --help      Show this help and exit\n\
         \x20 -V, --version   Show the version and exit\n\
         \n\
         The RUST_LOG environment variable overrides the log filter.\n\
         \n\
         For batch rendering and scripting, see: pleochroma-cli --help\n",
        version = version_line()
    )
}

/// The text to show for `request`.
#[must_use]
pub fn text_for(request: InfoRequest) -> String {
    match request {
        InfoRequest::Help => usage_text(),
        InfoRequest::Version => format!("{}\n", version_line()),
    }
}

/// Answers `--help` or `--version` when the line asks for it.
///
/// Returns `true` when the request was answered and the program should exit with status 0
/// without opening anything. Call it before any window, Slint or GPU call.
#[must_use]
pub fn answer_if_requested(args: impl IntoIterator<Item = OsString>) -> bool {
    let Some(request) = info_request(args) else {
        return false;
    };
    show(&text_for(request), request);
    true
}

#[cfg(not(windows))]
fn show(text: &str, _request: InfoRequest) {
    use std::io::Write as _;
    // A closed pipe (`| head`) is no reason to fail.
    let _ = std::io::stdout().write_all(text.as_bytes());
}

#[cfg(windows)]
fn show(text: &str, request: InfoRequest) {
    windows_output::show(text, request);
}

/// The Windows output paths. Raw `kernel32` and `user32` declarations, as the worker's
/// console launcher does, so no extra crate is needed.
#[cfg(windows)]
mod windows_output {
    use super::InfoRequest;
    use std::{io::Write as _, os::windows::ffi::OsStrExt as _};

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetStdHandle(standard_handle: u32) -> isize;
        fn AttachConsole(process_id: u32) -> i32;
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        fn MessageBoxW(window: isize, text: *const u16, caption: *const u16, kind: u32) -> i32;
    }

    /// `STD_OUTPUT_HANDLE`, which is `(DWORD)-11`.
    const STD_OUTPUT_HANDLE: u32 = 0xFFFF_FFF5;
    /// `ATTACH_PARENT_PROCESS`, which is `(DWORD)-1`.
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    /// `INVALID_HANDLE_VALUE`, which is `(HANDLE)-1`.
    const INVALID_HANDLE_VALUE: isize = -1;
    /// `MB_OK | MB_ICONINFORMATION`.
    const MB_OK_INFORMATION: u32 = 0x0000_0040;

    /// Whether the process was started with a usable standard output (a redirect, a pipe, or a
    /// console program's own console).
    fn has_standard_output() -> bool {
        // SAFETY: `GetStdHandle` takes a constant selector and reads no memory of ours.
        let handle = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
        handle != 0 && handle != INVALID_HANDLE_VALUE
    }

    /// Writes `text` to the parent's console, which this process attaches to first. `false`
    /// when there is no parent console or it cannot be written.
    fn write_to_parent_console(text: &str) -> bool {
        // SAFETY: `AttachConsole` takes a process id selector and reads no memory of ours.
        // It fails, harmlessly, when there is no parent console or this process has one.
        if unsafe { AttachConsole(ATTACH_PARENT_PROCESS) } == 0 {
            return false;
        }
        // The standard handles of a GUI-subsystem process are not set by the attach, so open
        // the console's output buffer by name.
        let Ok(mut console) = std::fs::OpenOptions::new().write(true).open("CONOUT$") else {
            return false;
        };
        // A leading line break: the shell's prompt was printed before this text arrived.
        console.write_all(b"\r\n").is_ok() && console.write_all(text.as_bytes()).is_ok()
    }

    /// Shows `text` in a native message box.
    fn message_box(text: &str, request: InfoRequest) {
        let caption = match request {
            InfoRequest::Help => "Pleochroma Cut: help",
            InfoRequest::Version => "Pleochroma Cut: version",
        };
        let wide = |s: &str| -> Vec<u16> {
            std::ffi::OsStr::new(s)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect()
        };
        // A message box wants CRLF line breaks.
        let body = wide(&text.trim_end().replace('\n', "\r\n"));
        let caption = wide(caption);
        // SAFETY: both pointers come from live, NUL-terminated `Vec<u16>` buffers that outlive
        // the call (it blocks until the box is closed); a null owner window is allowed.
        unsafe {
            MessageBoxW(0, body.as_ptr(), caption.as_ptr(), MB_OK_INFORMATION);
        }
    }

    /// Shows `text` by the first way that works: standard output, the parent console, a
    /// message box.
    pub fn show(text: &str, request: InfoRequest) {
        if has_standard_output() {
            let _ = std::io::stdout().write_all(text.as_bytes());
            return;
        }
        if !write_to_parent_console(text) {
            message_box(text, request);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{InfoRequest, info_request, text_for, usage_text, version_line};
    use std::ffi::OsString;

    fn os(args: &[&str]) -> Vec<OsString> {
        args.iter().map(OsString::from).collect()
    }

    #[test]
    fn help_flags_are_recognized_anywhere() {
        assert_eq!(info_request(os(&["--help"])), Some(InfoRequest::Help));
        assert_eq!(info_request(os(&["-h"])), Some(InfoRequest::Help));
        assert_eq!(
            info_request(os(&["design.psdd", "--help"])),
            Some(InfoRequest::Help)
        );
        assert_eq!(
            info_request(os(&["--log", "-h", "design.psdd"])),
            Some(InfoRequest::Help)
        );
    }

    #[test]
    fn version_flags_are_recognized() {
        assert_eq!(info_request(os(&["--version"])), Some(InfoRequest::Version));
        assert_eq!(info_request(os(&["-V"])), Some(InfoRequest::Version));
        assert_eq!(
            info_request(os(&["--log", "-V"])),
            Some(InfoRequest::Version)
        );
    }

    #[test]
    fn the_first_request_wins() {
        assert_eq!(
            info_request(os(&["--version", "--help"])),
            Some(InfoRequest::Version)
        );
        assert_eq!(
            info_request(os(&["--help", "--version"])),
            Some(InfoRequest::Help)
        );
    }

    #[test]
    fn a_file_path_is_not_a_request() {
        assert_eq!(info_request(os(&[])), None);
        assert_eq!(info_request(os(&["C:\\designs\\round.psdd"])), None);
        assert_eq!(info_request(os(&["round.psdd", "--log"])), None);
        assert_eq!(info_request(os(&["--log", "round.psdd"])), None);
        assert_eq!(info_request(os(&["--restarted-from=4242"])), None);
    }

    #[test]
    fn lowercase_v_and_longer_look_alikes_are_not_requests() {
        assert_eq!(info_request(os(&["-v"])), None);
        assert_eq!(info_request(os(&["--helpful"])), None);
        assert_eq!(info_request(os(&["-H"])), None);
    }

    #[test]
    fn nothing_after_a_bare_double_dash_counts() {
        assert_eq!(info_request(os(&["--", "--help"])), None);
        assert_eq!(info_request(os(&["--", "-V"])), None);
        assert_eq!(
            info_request(os(&["--help", "--", "x"])),
            Some(InfoRequest::Help)
        );
    }

    #[test]
    fn the_usage_names_the_real_flags_and_the_cli() {
        let usage = usage_text();
        assert!(usage.contains("Usage: pleochroma-cut [OPTIONS] [FILE]"));
        assert!(usage.contains("--log"));
        assert!(usage.contains("--help"));
        assert!(usage.contains("--version"));
        assert!(usage.contains("pleochroma-cli --help"));
        assert!(usage.starts_with(&version_line()));
    }

    #[test]
    fn the_version_text_is_one_line() {
        let text = text_for(InfoRequest::Version);
        assert_eq!(text.lines().count(), 1);
        assert!(text.starts_with("pleochroma-cut "));
        assert!(text.contains(env!("CARGO_PKG_VERSION")));
    }
}
