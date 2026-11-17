//! `pleochroma-worker-console`: the console twin of `pleochroma-worker`.
//!
//! A Windows-subsystem program has no console, so a terminal returns its prompt at once and
//! it loses stdout and its exit code. This tiny launcher is built as a console program. It
//! finds the sibling program (see [`target_stem`]: the same name, `.exe`), runs it with the
//! same arguments and standard streams, waits for it and returns its exit code. Packaging
//! renames this binary to the worker's name with `.com` (`pleochroma-worker.com`, or
//! `pleochroma-worker-windows-scalar-gpu.com` beside `pleochroma-worker-windows-scalar-gpu.exe`),
//! so that `cmd` and PowerShell (which try `.COM` before `.EXE`) start it when someone types
//! the program's name.
//!
//! The arguments pass through unchanged; this launcher never adds `--headless`. A line with
//! a command (or `--headless`, `--help`, `--version`) is waited for. A bare line (nothing
//! or only `--minimized`, `--config <file>`, `--no-update-check`, `--radv-workaround <v>`)
//! asks for the window: the program is started detached, so the prompt returns, and this
//! launcher exits with 0.
//!
//! Ctrl-C reaches the child through the shared console. This launcher answers Ctrl-C and
//! Ctrl-Break itself with "handled", so it stays alive until the child has stopped and then
//! returns the child's exit code.
//!
//! On other platforms the launcher replaces itself with the sibling program (`exec`), a plain
//! pass-through. It is only built there so that one `cargo build` makes every target.
//!
//! The decision "bare line or command" repeats `pleochroma_worker::cli::requests_window`
//! on purpose: this launcher must not link the whole worker crate. Keep the two in step.

use std::{
    ffi::{OsStr, OsString},
    path::PathBuf,
    process::{Command, exit},
};

/// File name of the program this launcher starts when its own name gives none, without the
/// platform suffix.
const TARGET_NAME: &str = "pleochroma-worker";

/// The file name, without the platform suffix, of the program a launcher called `own_stem`
/// starts: its own name with the first `-console` taken out. So `pleochroma-worker.com`
/// starts `pleochroma-worker.exe`, a download renamed to
/// `pleochroma-worker-windows-scalar-gpu.com` starts `pleochroma-worker-windows-scalar-gpu.exe`,
/// and the build outputs `pleochroma-worker-console` and `pleochroma-worker-console-win-…` start
/// `pleochroma-worker` and `pleochroma-worker-win-…`.
fn target_stem(own_stem: &str) -> String {
    own_stem.replacen("-console", "", 1)
}

/// The path of the program next to this one: same folder, [`target_stem`] of this launcher's
/// name plus the platform's executable suffix (`.exe` on Windows). When no such file exists,
/// the plain [`TARGET_NAME`] is tried. The launcher never starts itself.
fn sibling() -> std::io::Result<PathBuf> {
    let me = std::env::current_exe()?;
    let folder = me.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "this program has no folder to look in",
        )
    })?;
    let suffix = std::env::consts::EXE_SUFFIX;
    let own_stem = me
        .file_stem()
        .and_then(OsStr::to_str)
        .unwrap_or(TARGET_NAME);
    let named = folder.join(format!("{}{suffix}", target_stem(own_stem)));
    if named != me && named.is_file() {
        return Ok(named);
    }
    let plain = folder.join(format!("{TARGET_NAME}{suffix}"));
    if plain == me {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "this launcher carries the program's own name",
        ));
    }
    Ok(plain)
}

/// Whether `args` ask for the window rather than a command. A non-text argument counts as
/// a command.
#[cfg_attr(not(windows), allow(dead_code))]
fn requests_window(args: &[OsString]) -> bool {
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        let Some(arg) = arg.to_str() else {
            return false;
        };
        match arg {
            "--headless" | "-h" | "--help" | "-V" | "--version" | "--config=" => {
                return false;
            }
            "--minimized" | "--no-update-check" => {}
            // These two take a value: skip it. Without one the program refuses the line with
            // its own message and exit status 2, so this is a command to wait for, not a window.
            "--config" => match args.next().and_then(|value| value.to_str()) {
                Some(value) if !value.starts_with('-') => {}
                _ => return false,
            },
            "--radv-workaround" => {
                if args.next().is_none() {
                    return false;
                }
            }
            other if other.starts_with("--config=") || other.starts_with("--radv-workaround=") => {}
            _ => return false,
        }
    }
    true
}

/// Keeps this launcher alive through Ctrl-C and Ctrl-Break (the child gets them from the
/// shared console and decides what to do).
#[cfg(windows)]
mod console_events {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetConsoleCtrlHandler(
            handler: Option<unsafe extern "system" fn(u32) -> i32>,
            add: i32,
        ) -> i32;
    }

    /// Answers Ctrl-C (0) and Ctrl-Break (1) with "handled"; close, logoff and shutdown
    /// events keep their default.
    unsafe extern "system" fn handler(event: u32) -> i32 {
        i32::from(event <= 1)
    }

    /// Installs [`handler`] for this process. Unlike "ignore Ctrl-C", a registered handler
    /// is not inherited, so the child keeps its own Ctrl-C behavior.
    pub fn keep_alive_on_ctrl_c() {
        // SAFETY: `handler` is a valid `HandlerRoutine` for the life of the process and
        // does nothing but compare an integer.
        unsafe {
            SetConsoleCtrlHandler(Some(handler), 1);
        }
    }
}

#[cfg(windows)]
fn run(target: &std::path::Path, args: &[OsString]) -> ! {
    use std::{os::windows::process::CommandExt, process::Stdio};

    /// `DETACHED_PROCESS`: the program does not share this console.
    const DETACHED_PROCESS: u32 = 0x0000_0008;

    if requests_window(args) {
        let started = Command::new(target)
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(DETACHED_PROCESS)
            .spawn();
        match started {
            Ok(_) => exit(0),
            Err(e) => fail(target, &e),
        }
    }
    console_events::keep_alive_on_ctrl_c();
    match Command::new(target).args(args).status() {
        // A child ended by a signal-like fault has no code on some systems: report failure.
        Ok(status) => exit(status.code().unwrap_or(1)),
        Err(e) => fail(target, &e),
    }
}

#[cfg(not(windows))]
fn run(target: &std::path::Path, args: &[OsString]) -> ! {
    use std::os::unix::process::CommandExt;

    // `exec` only returns on failure.
    let e = Command::new(target).args(args).exec();
    fail(target, &e)
}

/// Prints why the program could not be started and exits with 1.
fn fail(target: &std::path::Path, error: &std::io::Error) -> ! {
    eprintln!("error: cannot start {}: {error}", target.display());
    exit(1)
}

fn main() {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    match sibling() {
        Ok(target) => run(&target, &args),
        Err(e) => {
            eprintln!("error: cannot find pleochroma-worker next to this program: {e}");
            exit(1)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{requests_window, target_stem};
    use std::ffi::OsString;

    /// The launcher starts the program of its own name: a release `.com` beside its `.exe`, a
    /// renamed download beside its renamed program, and a build output without `-console`.
    #[test]
    fn the_launcher_starts_the_program_of_its_own_name() {
        assert_eq!(target_stem("pleochroma-worker"), "pleochroma-worker");
        assert_eq!(
            target_stem("pleochroma-worker-windows-scalar-gpu"),
            "pleochroma-worker-windows-scalar-gpu"
        );
        assert_eq!(
            target_stem("pleochroma-worker-console"),
            "pleochroma-worker"
        );
        assert_eq!(
            target_stem("pleochroma-worker-console-win-avx2-gpu"),
            "pleochroma-worker-win-avx2-gpu"
        );
    }

    fn args(items: &[&str]) -> Vec<OsString> {
        items.iter().map(OsString::from).collect()
    }

    #[test]
    fn a_bare_line_asks_for_the_window() {
        assert!(requests_window(&[]));
        assert!(requests_window(&args(&["--minimized"])));
        assert!(requests_window(&args(&[
            "--config",
            "x.toml",
            "--minimized"
        ])));
        assert!(requests_window(&args(&["--config=x.toml"])));
        assert!(requests_window(&args(&["--no-update-check"])));
    }

    #[test]
    fn a_command_or_help_is_waited_for() {
        assert!(!requests_window(&args(&["--headless", "serve"])));
        assert!(!requests_window(&args(&["serve"])));
        assert!(!requests_window(&args(&["--help"])));
        assert!(!requests_window(&args(&["--version"])));
        assert!(!requests_window(&args(&["--minimized", "cert", "init"])));
    }

    /// A `--config` without a value is an error the program reports itself (exit status 2).
    #[test]
    fn a_config_flag_without_a_value_is_waited_for() {
        assert!(!requests_window(&args(&["--config"])));
        assert!(!requests_window(&args(&["--config", "--minimized"])));
        assert!(!requests_window(&args(&["--config="])));
        assert!(!requests_window(&args(&["--minimized", "--config"])));
        assert!(!requests_window(&args(&["--radv-workaround"])));
    }
}
