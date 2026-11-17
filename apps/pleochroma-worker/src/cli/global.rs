//! Global flags: `--headless`, `--minimized` and `--config <file>`, and the choice between
//! the window and the command line.
//!
//! These three flags may stand anywhere in `argv`. [`take_global_flags`] removes them
//! before [`super::parse`] runs, so every command line that parsed before parses exactly
//! the same with them added: the subcommand parsers never see them.
//!
//! [`launch_mode`] is the one place that decides between the window and the command line.
//! A build without the `gui` feature has no window, so it always answers
//! [`LaunchMode::Headless`], with or without `--headless`.

use std::path::PathBuf;

/// The flag that runs the command line instead of opening the window.
pub const HEADLESS_FLAG: &str = "--headless";

/// The flag that starts the window minimized (to the tray). Ignored headless.
pub const MINIMIZED_FLAG: &str = "--minimized";

/// The flag that names the settings file (`--config <file>` or `--config=<file>`).
pub const CONFIG_FLAG: &str = "--config";

/// The global flags taken out of `argv` by [`take_global_flags`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GlobalFlags {
    /// `--headless` was given: never open a window.
    pub headless: bool,
    /// `--minimized` was given: the window starts minimized. Ignored headless.
    pub minimized: bool,
    /// The settings file from `--config`, the last one when repeated.
    pub config: Option<PathBuf>,
}

/// What the process does after start-up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LaunchMode {
    /// Run the command line (today's behavior, no window).
    Headless,
    /// Open the coordinator window.
    Gui,
}

/// Removes every global flag from `argv` and returns what they said.
///
/// `--headless` and `--minimized` are switches; `--config` takes a value, as the next
/// argument or after an `=`. Repeating a switch is harmless, and the last `--config` wins.
///
/// # Errors
///
/// A message when `--config` has no value (it is last, the next argument is another flag,
/// or the value after `=` is empty).
pub fn take_global_flags(argv: &mut Vec<String>) -> Result<GlobalFlags, String> {
    let mut flags = GlobalFlags::default();
    let mut index = 0;
    while index < argv.len() {
        let arg = argv[index].clone();
        match arg.as_str() {
            HEADLESS_FLAG => {
                argv.remove(index);
                flags.headless = true;
            }
            MINIMIZED_FLAG => {
                argv.remove(index);
                flags.minimized = true;
            }
            CONFIG_FLAG => {
                argv.remove(index);
                let value = match argv.get(index) {
                    Some(next) if !next.starts_with('-') => argv.remove(index),
                    _ => return Err(format!("{CONFIG_FLAG} needs a file path")),
                };
                flags.config = Some(PathBuf::from(value));
            }
            other => {
                if let Some(value) = other.strip_prefix("--config=") {
                    if value.is_empty() {
                        return Err(format!("{CONFIG_FLAG} needs a file path"));
                    }
                    flags.config = Some(PathBuf::from(value));
                    argv.remove(index);
                } else {
                    index += 1;
                }
            }
        }
    }
    Ok(flags)
}

/// Whether `argv` (global flags already taken out) asks for the window rather than a
/// command: no command, no `--help`/`--version`, only flags the window itself understands.
///
/// Pure rule, independent of the build's features. The Windows console launcher
/// (`src/bin/console.rs`) repeats this rule on the raw arguments; keep the two in step.
#[must_use]
pub fn requests_window(argv: &[String]) -> bool {
    let mut words = argv.iter();
    while let Some(arg) = words.next() {
        match arg.as_str() {
            HEADLESS_FLAG | "-h" | "--help" | "-V" | "--version" => return false,
            MINIMIZED_FLAG | "--no-update-check" => {}
            // These two take a value: skip it. Without one the line is an error, which the
            // command-line path reports, so it is not a window request.
            CONFIG_FLAG => match words.next() {
                Some(value) if !value.starts_with('-') => {}
                _ => return false,
            },
            "--radv-workaround" => {
                if words.next().is_none() {
                    return false;
                }
            }
            "--config=" => return false,
            other if other.starts_with("--config=") || other.starts_with("--radv-workaround=") => {}
            // A command, or a flag only a command understands.
            _ => return false,
        }
    }
    true
}

/// Decides between the window and the command line.
///
/// `argv` is what is left after [`take_global_flags`]. The answer is
/// [`LaunchMode::Gui`] only in a build with the `gui` feature, without `--headless`, and
/// with no command, `--help` or `--version` on the line. **A build without the `gui`
/// feature always returns [`LaunchMode::Headless`].** A command given without `--headless`
/// still runs headless; `main` prints the deprecation warning for it in a `gui` build.
#[must_use]
pub fn launch_mode(flags: &GlobalFlags, argv: &[String]) -> LaunchMode {
    if cfg!(feature = "gui") && !flags.headless && requests_window(argv) {
        LaunchMode::Gui
    } else {
        LaunchMode::Headless
    }
}
