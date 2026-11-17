//! Start at login (decision D10): off by default, and when on it starts the coordinator in the
//! tray only (`<program> --minimized`), so no window opens at login.
//!
//! The entry belongs to the current user and runs only while that user is logged in:
//!
//! - Windows: the value `PleochromaCoordinator` under
//!   `HKEY_CURRENT_USER\Software\Microsoft\Windows\CurrentVersion\Run`, written with `reg.exe`.
//! - macOS: the launch agent `~/Library/LaunchAgents/com.pleochroma.coordinator.plist`. It is
//!   read at the next login, so nothing is loaded now.
//! - Linux and other desktops: the XDG autostart file
//!   `$XDG_CONFIG_HOME/autostart/pleochroma-coordinator.desktop` (`~/.config/autostart`).
//!
//! The functions that build the command line and the file texts are pure, so they are tested on
//! every platform.

use std::{
    path::{Path, PathBuf},
    sync::OnceLock,
};

/// The flag the login entry passes: start in the tray, no window.
pub const MINIMIZED_FLAG: &str = "--minimized";

/// The flag that names the settings file, passed when this run was started with one.
const CONFIG_FLAG: &str = "--config";

/// The settings file this run was started with (`--config <file>`), so the login entry starts
/// the program with the same one. Set once by [`set_config`].
static CONFIG: OnceLock<PathBuf> = OnceLock::new();

/// Remembers the `--config` file of this run (absolute, so the login entry works from any
/// folder). Called once at start-up; `None` leaves the entry without `--config`.
pub fn set_config(config: Option<&Path>) {
    if let Some(file) = config {
        let full = std::path::absolute(file).unwrap_or_else(|_| file.to_path_buf());
        let _ = CONFIG.set(full);
    }
}

/// The remembered `--config` file, if the run has one.
fn config() -> Option<&'static Path> {
    CONFIG.get().map(PathBuf::as_path)
}

/// The Windows `Run` value's name.
#[cfg_attr(not(windows), allow(dead_code))]
const RUN_VALUE: &str = "PleochromaCoordinator";

/// The Windows `Run` key.
#[cfg_attr(not(windows), allow(dead_code))]
const RUN_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Run";

/// The launch agent's label and file stem.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const AGENT_LABEL: &str = "com.pleochroma.coordinator";

/// The XDG autostart file's name.
#[cfg_attr(not(unix), allow(dead_code))]
const DESKTOP_FILE: &str = "pleochroma-coordinator.desktop";

/// Makes the program start at login, in the tray.
///
/// # Errors
///
/// A message in plain words when the program's own path is unknown or the entry cannot be
/// written.
pub fn enable() -> Result<(), String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("could not find this program's own path: {e}"))?;
    enable_for(&exe, config())
}

/// Removes the login entry. Succeeds when there is none.
///
/// # Errors
///
/// A message in plain words when the entry exists and cannot be removed.
pub fn disable() -> Result<(), String> {
    disable_entry()
}

/// Whether the login entry exists now.
#[must_use]
pub fn is_enabled() -> bool {
    entry_exists()
}

/// Whether the login entry exists and starts this very program (the path of this executable, and
/// this run's `--config` file) the way a new entry would.
///
/// False when there is none, or when it is stale (the program was moved or updated to another
/// folder).
#[must_use]
pub fn is_current() -> bool {
    std::env::current_exe().is_ok_and(|exe| entry_is_current(&exe, config()))
}

/// The command a login entry runs: the program's full path in quotes, then `--config <file>`
/// when there is one, then `--minimized`.
#[must_use]
pub fn command_line(exe: &Path, config: Option<&Path>) -> String {
    config.map_or_else(
        || format!("\"{}\" {MINIMIZED_FLAG}", exe.display()),
        |file| {
            format!(
                "\"{}\" {CONFIG_FLAG} \"{}\" {MINIMIZED_FLAG}",
                exe.display(),
                file.display()
            )
        },
    )
}

/// The text of the macOS launch agent that starts `exe` at login with `--minimized` (and the
/// settings file `config`, when given).
#[must_use]
pub fn launch_agent_plist(exe: &Path, config: Option<&Path>) -> String {
    let program = xml_escape(&exe.display().to_string());
    let config_args = config.map_or_else(String::new, |file| {
        format!(
            "\t\t<string>{CONFIG_FLAG}</string>\n\t\t<string>{}</string>\n",
            xml_escape(&file.display().to_string())
        )
    });
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
         <plist version=\"1.0\">\n\
         <dict>\n\
         \t<key>Label</key>\n\
         \t<string>{AGENT_LABEL}</string>\n\
         \t<key>ProgramArguments</key>\n\
         \t<array>\n\
         \t\t<string>{program}</string>\n\
         {config_args}\
         \t\t<string>{MINIMIZED_FLAG}</string>\n\
         \t</array>\n\
         \t<key>RunAtLoad</key>\n\
         \t<true/>\n\
         </dict>\n\
         </plist>\n"
    )
}

/// The text of the XDG autostart file that starts `exe` at login with `--minimized` (and the
/// settings file `config`, when given).
#[must_use]
pub fn desktop_entry(exe: &Path, config: Option<&Path>) -> String {
    let config_args = config.map_or_else(String::new, |file| {
        format!(
            " {CONFIG_FLAG} {}",
            desktop_exec_arg(&file.display().to_string())
        )
    });
    format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Pleochroma Coordinator\n\
         Comment=Starts the Pleochroma coordinator in the tray at login\n\
         Exec={}{config_args} {MINIMIZED_FLAG}\n\
         Terminal=false\n\
         X-GNOME-Autostart-enabled=true\n",
        desktop_exec_arg(&exe.display().to_string())
    )
}

/// One program path as the `Exec` key of a desktop entry wants it: quoted when it has a
/// space or a reserved character, with the reserved characters escaped (the `Exec` value is
/// escaped twice, so a backslash is written four times) and `%` doubled.
fn desktop_exec_arg(path: &str) -> String {
    let reserved = |c: char| " \t\n\"'\\><~|&;$*?#()`".contains(c);
    let text = path.replace('%', "%%");
    if !text.chars().any(reserved) {
        return text;
    }
    let mut out = String::from("\"");
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\\\\\"),
            '"' | '`' | '$' => {
                out.push_str("\\\\");
                out.push(c);
            }
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// Escapes the five characters XML text cannot hold as they are.
fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            other => out.push(other),
        }
    }
    out
}

/// The user's home folder.
#[cfg_attr(windows, allow(dead_code))]
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Where the macOS launch agent file goes.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn launch_agent_path() -> Option<PathBuf> {
    Some(
        home_dir()?
            .join("Library")
            .join("LaunchAgents")
            .join(format!("{AGENT_LABEL}.plist")),
    )
}

/// Where the XDG autostart file goes.
#[cfg_attr(any(windows, target_os = "macos"), allow(dead_code))]
fn desktop_entry_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| home_dir().map(|home| home.join(".config")))?;
    Some(base.join("autostart").join(DESKTOP_FILE))
}

/// Writes `text` to `path`, creating the folder first.
#[cfg_attr(windows, allow(dead_code))]
fn write_entry(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    }
    std::fs::write(path, text).map_err(|e| format!("could not write {}: {e}", path.display()))
}

/// Removes a file; a file that is already gone is success.
#[cfg_attr(windows, allow(dead_code))]
fn remove_entry(path: &Path) -> Result<(), String> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(format!("could not remove {}: {e}", path.display())),
    }
}

#[cfg(windows)]
fn enable_for(exe: &Path, config: Option<&Path>) -> Result<(), String> {
    use super::common::{COMMAND_TIMEOUT, run_bounded};
    let command = command_line(exe, config);
    let done = run_bounded(
        "reg",
        &[
            "add",
            RUN_KEY,
            "/v",
            RUN_VALUE,
            "/t",
            "REG_SZ",
            "/d",
            command.as_str(),
            "/f",
        ],
        COMMAND_TIMEOUT,
    );
    match done {
        Some(out) if out.success => Ok(()),
        _ => Err("could not add the login entry to the registry".to_owned()),
    }
}

#[cfg(windows)]
fn disable_entry() -> Result<(), String> {
    use super::common::{COMMAND_TIMEOUT, run_bounded};
    if !entry_exists() {
        return Ok(());
    }
    let done = run_bounded(
        "reg",
        &["delete", RUN_KEY, "/v", RUN_VALUE, "/f"],
        COMMAND_TIMEOUT,
    );
    match done {
        Some(out) if out.success => Ok(()),
        _ => Err("could not remove the login entry from the registry".to_owned()),
    }
}

#[cfg(windows)]
fn entry_exists() -> bool {
    use super::common::{COMMAND_TIMEOUT, run_bounded};
    run_bounded("reg", &["query", RUN_KEY, "/v", RUN_VALUE], COMMAND_TIMEOUT)
        .is_some_and(|out| out.success)
}

#[cfg(windows)]
fn entry_is_current(exe: &Path, config: Option<&Path>) -> bool {
    use super::common::{COMMAND_TIMEOUT, run_bounded};
    // `reg query` prints in the OEM code page, which the captured text decodes as UTF-8, so a
    // non-ASCII character in a path arrives as replacement characters. Both sides are reduced
    // to their ASCII letters with one marker per run of other characters before they compare.
    run_bounded("reg", &["query", RUN_KEY, "/v", RUN_VALUE], COMMAND_TIMEOUT).is_some_and(|out| {
        out.success && ascii_shape(&out.stdout).contains(&ascii_shape(&command_line(exe, config)))
    })
}

/// `text` with every run of non-ASCII characters replaced by one `?`.
#[cfg(windows)]
fn ascii_shape(text: &str) -> String {
    let mut shape = String::with_capacity(text.len());
    let mut in_run = false;
    for ch in text.chars() {
        if ch.is_ascii() {
            shape.push(ch);
            in_run = false;
        } else if !in_run {
            shape.push('?');
            in_run = true;
        }
    }
    shape
}

#[cfg(target_os = "macos")]
fn enable_for(exe: &Path, config: Option<&Path>) -> Result<(), String> {
    let path = launch_agent_path().ok_or("could not find the home folder")?;
    write_entry(&path, &launch_agent_plist(exe, config))
}

#[cfg(target_os = "macos")]
fn entry_is_current(exe: &Path, config: Option<&Path>) -> bool {
    launch_agent_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .is_some_and(|text| text == launch_agent_plist(exe, config))
}

#[cfg(target_os = "macos")]
fn disable_entry() -> Result<(), String> {
    launch_agent_path().map_or(Ok(()), |path| remove_entry(&path))
}

#[cfg(target_os = "macos")]
fn entry_exists() -> bool {
    launch_agent_path().is_some_and(|path| path.exists())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn enable_for(exe: &Path, config: Option<&Path>) -> Result<(), String> {
    let path = desktop_entry_path().ok_or("could not find the configuration folder")?;
    write_entry(&path, &desktop_entry(exe, config))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn entry_is_current(exe: &Path, config: Option<&Path>) -> bool {
    desktop_entry_path()
        .and_then(|path| std::fs::read_to_string(path).ok())
        .is_some_and(|text| text == desktop_entry(exe, config))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn disable_entry() -> Result<(), String> {
    desktop_entry_path().map_or(Ok(()), |path| remove_entry(&path))
}

#[cfg(all(unix, not(target_os = "macos")))]
fn entry_exists() -> bool {
    desktop_entry_path().is_some_and(|path| path.exists())
}

#[cfg(not(any(windows, unix)))]
fn enable_for(_exe: &Path, _config: Option<&Path>) -> Result<(), String> {
    Err("starting at login is not available on this system".to_owned())
}

#[cfg(not(any(windows, unix)))]
const fn entry_is_current(_exe: &Path, _config: Option<&Path>) -> bool {
    false
}

#[cfg(not(any(windows, unix)))]
const fn disable_entry() -> Result<(), String> {
    Ok(())
}

#[cfg(not(any(windows, unix)))]
const fn entry_exists() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn command_line_quotes_the_program_and_adds_the_flag() {
        let exe = Path::new(r"C:\Program Files\Pleochroma\pleochroma-worker.exe");
        assert_eq!(
            command_line(exe, None),
            r#""C:\Program Files\Pleochroma\pleochroma-worker.exe" --minimized"#
        );
        assert_eq!(
            command_line(exe, Some(Path::new(r"D:\pw\coordinator.toml"))),
            r#""C:\Program Files\Pleochroma\pleochroma-worker.exe" --config "D:\pw\coordinator.toml" --minimized"#
        );
    }

    #[test]
    fn the_config_file_is_passed_on_to_every_entry_kind() {
        let exe = Path::new("/opt/pw/worker");
        let config = Some(Path::new("/srv/pw/coordinator.toml"));
        assert!(
            desktop_entry(exe, config)
                .contains("\nExec=/opt/pw/worker --config /srv/pw/coordinator.toml --minimized\n")
        );
        let plist = launch_agent_plist(exe, config);
        assert!(plist.contains(
            "<string>/opt/pw/worker</string>\n\t\t<string>--config</string>\n\t\t<string>/srv/pw/coordinator.toml</string>\n\t\t<string>--minimized</string>"
        ));
        assert!(!launch_agent_plist(exe, None).contains("--config"));
    }

    #[test]
    fn plist_runs_the_program_with_the_flag_at_load() {
        let text = launch_agent_plist(Path::new("/Applications/Pleochroma & Co/worker"), None);
        assert!(text.contains("<string>com.pleochroma.coordinator</string>"));
        assert!(text.contains("<string>/Applications/Pleochroma &amp; Co/worker</string>"));
        assert!(text.contains("<string>--minimized</string>"));
        assert!(text.contains("<key>RunAtLoad</key>\n\t<true/>"));
        assert!(text.starts_with("<?xml"));
        assert!(text.ends_with("</plist>\n"));
    }

    #[test]
    fn desktop_entry_has_the_exec_line_and_no_terminal() {
        let text = desktop_entry(Path::new("/opt/pleochroma/pleochroma-worker"), None);
        assert!(text.starts_with("[Desktop Entry]\n"));
        assert!(text.contains("\nExec=/opt/pleochroma/pleochroma-worker --minimized\n"));
        assert!(text.contains("\nTerminal=false\n"));
        assert!(text.contains("\nType=Application\n"));
    }

    #[test]
    fn desktop_exec_quotes_and_escapes_reserved_characters() {
        assert_eq!(desktop_exec_arg("/usr/bin/worker"), "/usr/bin/worker");
        assert_eq!(desktop_exec_arg("/home/a b/worker"), "\"/home/a b/worker\"");
        assert_eq!(desktop_exec_arg("/tmp/50%/w"), "/tmp/50%%/w");
        assert_eq!(desktop_exec_arg("/tmp/a\"b"), "\"/tmp/a\\\\\"b\"");
        assert_eq!(desktop_exec_arg("/tmp/a$b"), "\"/tmp/a\\\\$b\"");
    }

    #[test]
    fn the_entry_files_have_their_own_names() {
        assert_eq!(DESKTOP_FILE, "pleochroma-coordinator.desktop");
        assert_eq!(AGENT_LABEL, "com.pleochroma.coordinator");
    }

    #[test]
    fn write_and_remove_round_trip_in_a_folder() {
        let dir = std::env::temp_dir().join(format!("pleochroma-autostart-{}", std::process::id()));
        let file = dir.join("autostart").join(DESKTOP_FILE);
        write_entry(&file, "text").unwrap();
        assert!(file.exists());
        remove_entry(&file).unwrap();
        remove_entry(&file).unwrap();
        assert!(!file.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
