//! The Tools page: render a scene file to a PNG on this computer, and the exact command that
//! runs this coordinator without the window.
//!
//! The render runs the one-shot `render` command ([`crate::render_cmd::run_cancellable`]) on its own
//! thread, so the window stays usable. Cancel sets its flag. The command reports no progress, so
//! the page shows the time spent. The headless command is built from the same
//! settings the window starts `serve` with ([`crate::settings::CoordinatorSettings::serve_args`]),
//! so it holds file paths and addresses only, never a token or a key.

use crate::{
    cli::{ComputeMode, RenderArgs, ServeArgs},
    gui::{
        app::{ActiveBackend, AppWindow, ToolsState},
        pages::picker::{self, Kind, Request},
    },
};
use slint::{ComponentHandle, SharedString, Timer, TimerMode};
use std::{
    cell::{Cell, RefCell},
    panic::AssertUnwindSafe,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

/// How often the page refreshes the time spent, the command and the "Copied" notice.
const TICK: Duration = Duration::from_secs(1);

/// How many ticks "Copied" stays up.
const NOTICE_TICKS: u32 = 3;

/// The Ray-Traced engine takes only these sample counts (anti-aliasing taps).
const RAY_SAMPLES: [u32; 4] = [1, 4, 8, 16];

/// The program name the command starts.
const PROGRAM: &str = "pleochroma-worker";

thread_local! {
    /// The refresh timer; a `Timer` stops when dropped, so it lives for the UI thread.
    static TIMER: RefCell<Option<Timer>> = const { RefCell::new(None) };
    /// When the running render started; `None` while idle.
    static STARTED: Cell<Option<Instant>> = const { Cell::new(None) };
    /// The running render's cancel flag; `None` while idle.
    static CANCEL: RefCell<Option<Arc<AtomicBool>>> = const { RefCell::new(None) };
    /// Ticks until "Copied" goes away.
    static NOTICE_LEFT: Cell<u32> = const { Cell::new(0) };
    /// The clipboard, kept open between copies (some platforms drop the text with it).
    static CLIPBOARD: RefCell<Option<arboard::Clipboard>> = const { RefCell::new(None) };
}

/// Connects the Tools page to the render command and fills in the headless command.
///
/// Page contract: called once by `gui::app::run`. The page needs nothing from the backend: the
/// render is local and the command comes from the settings.
pub fn wire(window: &AppWindow, _backend: &ActiveBackend) {
    let state = window.global::<ToolsState>();
    state.set_loaded(true);

    wire_pickers(window);
    wire_actions(window);

    refresh_command(&state);
    let timer = Timer::default();
    timer.start(TimerMode::Repeated, TICK, {
        let weak = window.as_weak();
        move || {
            if let Some(window) = weak.upgrade() {
                tick(&window.global::<ToolsState>());
            }
        }
    });
    TIMER.with(|slot| *slot.borrow_mut() = Some(timer));
}

/// Shows the problem of a file dialog that could not open.
fn picker_failed(weak: &slint::Weak<AppWindow>, started: Result<(), String>) {
    if let (Err(reason), Some(window)) = (started, weak.upgrade()) {
        window.global::<ToolsState>().set_error(reason.into());
    }
}

/// Wires the two file dialogs; each runs on a helper thread and fills its field when it ends.
fn wire_pickers(window: &AppWindow) {
    let state = window.global::<ToolsState>();
    state.on_browse_scene({
        let weak = window.as_weak();
        move || {
            let request =
                Request::new(Kind::OpenFile, "Choose a scene file").filter("Scene file", &["json"]);
            let started = picker::pick(&weak, &request, |window, path| {
                scene_chosen(&window.global::<ToolsState>(), &path);
            });
            picker_failed(&weak, started);
        }
    });
    state.on_browse_out({
        let weak = window.as_weak();
        move || {
            let Some(window) = weak.upgrade() else {
                return;
            };
            let current = PathBuf::from(window.global::<ToolsState>().get_out_path().as_str());
            let mut request =
                Request::new(Kind::SaveFile, "Save the picture as").filter("PNG picture", &["png"]);
            if let Some(dir) = current.parent() {
                request = request.start_in(dir);
            }
            if let Some(name) = current.file_name().and_then(std::ffi::OsStr::to_str) {
                request = request.file_name(name);
            }
            let started = picker::pick(&weak, &request, |window, path| {
                window
                    .global::<ToolsState>()
                    .set_out_path(shared(&with_png_extension(path)));
            });
            picker_failed(&weak, started);
        }
    });
}

/// Wires the engine choice, the render buttons, the picture buttons and Copy command.
fn wire_actions(window: &AppWindow) {
    let state = window.global::<ToolsState>();
    state.on_engine_picked({
        let weak = window.as_weak();
        move |engine| {
            if let Some(window) = weak.upgrade() {
                engine_chosen(&window.global::<ToolsState>(), engine);
            }
        }
    });
    state.on_start_render({
        let weak = window.as_weak();
        move || {
            if let Some(window) = weak.upgrade() {
                start_render(&window);
            }
        }
    });
    state.on_cancel_render(|| {
        CANCEL.with(|slot| {
            if let Some(flag) = slot.borrow().as_ref() {
                flag.store(true, Ordering::Relaxed);
            }
        });
    });
    state.on_open_picture({
        let weak = window.as_weak();
        move || {
            if let Some(window) = weak.upgrade() {
                let path = window.global::<ToolsState>().get_result_path();
                open_path(Path::new(path.as_str()));
            }
        }
    });
    state.on_show_folder({
        let weak = window.as_weak();
        move || {
            if let Some(window) = weak.upgrade() {
                let path = window.global::<ToolsState>().get_result_path();
                show_in_folder(Path::new(path.as_str()));
            }
        }
    });
    state.on_copy_command({
        let weak = window.as_weak();
        move || {
            if let Some(window) = weak.upgrade() {
                let state = window.global::<ToolsState>();
                let text = state.get_command();
                if text.is_empty() {
                    return;
                }
                let notice = match copy_text(text.as_str()) {
                    Ok(()) => {
                        NOTICE_LEFT.with(|left| left.set(NOTICE_TICKS));
                        "Copied"
                    }
                    Err(()) => "The clipboard is not available.",
                };
                state.set_copy_notice(notice.into());
            }
        }
    });
}

/// One second: the time spent, the command (the settings may have changed) and the notice.
fn tick(state: &ToolsState<'_>) {
    if state.get_rendering()
        && let Some(started) = STARTED.with(Cell::get)
    {
        state.set_status(format!("Rendering… {}", elapsed_text(started.elapsed())).into());
    }
    refresh_command(state);
    NOTICE_LEFT.with(|left| {
        let ticks = left.get();
        if ticks > 0 {
            left.set(ticks - 1);
            if ticks == 1 {
                state.set_copy_notice(SharedString::new());
            }
        }
    });
}

/// Writes the headless command (or why there is none) into the page, only when it changed.
fn refresh_command(state: &ToolsState<'_>) {
    let (command, problem) = match headless_command() {
        Ok(command) => (command, String::new()),
        Err(problem) => (String::new(), problem),
    };
    if state.get_command().as_str() != command {
        state.set_command(command.into());
    }
    if state.get_command_problem().as_str() != problem {
        state.set_command_problem(problem.into());
    }
}

/// A scene file was chosen: fill in the output name and the size the file carries.
fn scene_chosen(state: &ToolsState<'_>, path: &Path) {
    state.set_scene_path(shared(path));
    if state.get_out_path().is_empty() {
        state.set_out_path(shared(&with_png_extension(path.to_path_buf())));
    }
    if let Some((width, height)) = scene_size(path) {
        state.set_width_text(width.to_string().into());
        state.set_height_text(height.to_string().into());
    }
}

/// The width and height a scene file carries, or `None` when it cannot be read or has none.
fn scene_size(path: &Path) -> Option<(u32, u32)> {
    let text = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    let side = |key: &str| {
        value
            .get(key)?
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .filter(|n| *n > 0)
    };
    Some((side("width")?, side("height")?))
}

/// A path as the text the window shows.
fn shared(path: &Path) -> SharedString {
    path.to_string_lossy().into_owned().into()
}

/// The same path with the extension `png`.
fn with_png_extension(mut path: PathBuf) -> PathBuf {
    path.set_extension("png");
    path
}

/// The engine changed: keep the sample count one the engine takes.
fn engine_chosen(state: &ToolsState<'_>, engine: i32) {
    let samples = state.get_samples_text().trim().parse::<u32>().ok();
    if engine == 1 {
        if !samples.is_some_and(|n| RAY_SAMPLES.contains(&n)) {
            state.set_samples_text("4".into());
        }
    } else if samples.is_some_and(|n| RAY_SAMPLES.contains(&n)) {
        state.set_samples_text("256".into());
    }
}

/// Checks what was typed and turns it into the arguments of the `render` command.
///
/// # Errors
///
/// A message in plain words naming the field that is wrong.
fn render_args(state: &ToolsState<'_>) -> Result<RenderArgs, String> {
    let scene = PathBuf::from(state.get_scene_path().trim());
    if state.get_scene_path().trim().is_empty() {
        return Err("Choose a scene file first.".to_owned());
    }
    if !scene.is_file() {
        return Err("The scene file cannot be found. Choose it again.".to_owned());
    }
    let out_text = state.get_out_path();
    if out_text.trim().is_empty() {
        return Err("Choose where to save the picture.".to_owned());
    }
    let out = with_png_extension(PathBuf::from(out_text.trim()));
    if out == scene {
        return Err("The picture needs its own file name, not the scene file's.".to_owned());
    }
    if let Some(dir) = out.parent().filter(|dir| !dir.as_os_str().is_empty())
        && !dir.is_dir()
    {
        return Err("The folder for the picture does not exist.".to_owned());
    }
    let number = |what: &str, text: SharedString| {
        text.trim()
            .parse::<u32>()
            .map_err(|_| format!("{what} must be a whole number."))
    };
    let width = number("The width", state.get_width_text())?;
    let height = number("The height", state.get_height_text())?;
    let samples = number("The samples", state.get_samples_text())?;
    let ray = state.get_engine() == 1;
    if ray && !RAY_SAMPLES.contains(&samples) {
        return Err("The Ray-Traced engine takes 1, 4, 8 or 16 samples.".to_owned());
    }
    crate::render_cmd::validate_render_params(width, height, samples).map_err(|reason| {
        reason
            .replace("--width x --height", "Width x height")
            .replace("--width and --height", "Width and height")
            .replace("--samples", "Samples")
    })?;
    let settings = crate::settings::current();
    let threads = if settings.render_threads == 0 {
        settings.cpu_threads
    } else {
        settings.render_threads
    };
    Ok(RenderArgs {
        engine: Some(if ray { "ray" } else { "path" }.to_owned()),
        scene,
        out,
        width,
        height,
        samples,
        threads: usize::try_from(threads).unwrap_or(0),
        compute_mode: if state.get_device() == 1 {
            ComputeMode::OnlyCpu
        } else {
            ComputeMode::Hybrid
        },
    })
}

/// Starts the render on its own thread; the result comes back through the event loop.
fn start_render(window: &AppWindow) {
    let state = window.global::<ToolsState>();
    if state.get_rendering() {
        return;
    }
    state.set_error(SharedString::new());
    state.set_result_path(SharedString::new());
    let args = match render_args(&state) {
        Ok(args) => args,
        Err(reason) => {
            state.set_error(reason.into());
            return;
        }
    };
    state.set_out_path(shared(&args.out));
    let weak = window.as_weak();
    let cancel = Arc::new(AtomicBool::new(false));
    let thread_cancel = Arc::clone(&cancel);
    let spawned = std::thread::Builder::new()
        .name("tools-render".to_owned())
        .spawn(move || {
            let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| {
                crate::render_cmd::run_cancellable(&args, &thread_cancel)
            }))
            .unwrap_or_else(|_| Err("The render stopped unexpectedly.".to_owned()));
            let out = args.out;
            // A closed window has nobody to tell; the picture is on disk either way.
            let _ = weak.upgrade_in_event_loop(move |window| {
                finish(&window.global::<ToolsState>(), outcome, &out);
            });
        });
    match spawned {
        Ok(_) => {
            CANCEL.with(|slot| *slot.borrow_mut() = Some(cancel));
            STARTED.with(|started| started.set(Some(Instant::now())));
            state.set_rendering(true);
            state.set_status("Rendering… 0 s".into());
        }
        Err(e) => state.set_error(format!("The render could not start: {e}").into()),
    }
}

/// The render ended: show the picture or the reason it failed.
fn finish(state: &ToolsState<'_>, outcome: Result<(), String>, out: &Path) {
    STARTED.with(|started| started.set(None));
    CANCEL.with(|slot| *slot.borrow_mut() = None);
    state.set_rendering(false);
    state.set_status(SharedString::new());
    match outcome {
        Ok(()) => state.set_result_path(shared(out)),
        Err(reason) if reason == crate::render_cmd::CANCELLED => {
            state.set_status("Render cancelled".into());
        }
        Err(reason) => state.set_error(reason.into()),
    }
}

/// "12 s" or "2 min 05 s".
fn elapsed_text(spent: Duration) -> String {
    let secs = spent.as_secs();
    if secs < 60 {
        format!("{secs} s")
    } else {
        format!("{} min {:02} s", secs / 60, secs % 60)
    }
}

/// The command that runs this coordinator without the window, for the settings in use.
///
/// # Errors
///
/// A message in plain words when the settings cannot start a coordinator.
fn headless_command() -> Result<String, String> {
    let settings = crate::settings::current();
    let data_dir = crate::settings::default_data_dir().unwrap_or_else(|| PathBuf::from("."));
    let args = settings.serve_args(&data_dir)?;
    serve_command(&args, cfg!(windows))
}

/// `pleochroma-worker --headless serve ...` for `args`: every option that differs from `serve`'s
/// own defaults, with the bind address, the certificate files and the library always named.
/// `windows` picks the quoting (double quotes) over the shell's (single quotes).
///
/// # Errors
///
/// A message when the defaults of `serve` cannot be read (an internal fault).
fn serve_command(args: &ServeArgs, windows: bool) -> Result<String, String> {
    let defaults = match crate::cli::parse(&["serve".to_owned()]) {
        Ok(crate::cli::Command::Serve(defaults)) => *defaults,
        Ok(_) => {
            return Err("internal error: `serve` did not parse to the serve command".to_owned());
        }
        Err(e) => return Err(format!("internal error: {e}")),
    };
    let mut words: Vec<String> = vec![
        PROGRAM.to_owned(),
        crate::cli::HEADLESS_FLAG.to_owned(),
        "serve".to_owned(),
        "--bind".to_owned(),
        args.bind.clone(),
    ];
    words.extend(switch_options(args).into_iter().map(str::to_owned));
    let mut valued = address_options(args);
    valued.extend(tuning_options(args, &defaults));
    for (name, value) in valued {
        words.push(name.to_owned());
        words.push(value);
    }
    Ok(words
        .iter()
        .map(|word| quote(word, windows))
        .collect::<Vec<_>>()
        .join(" "))
}

/// The options of `serve` that take no value and are on in `args`.
fn switch_options(args: &ServeArgs) -> Vec<&'static str> {
    let mut flags = Vec::new();
    if args.allow_remote {
        flags.push("--allow-remote");
    }
    if args.render {
        flags.push(match args.compute_mode {
            ComputeMode::Hybrid => "--render",
            ComputeMode::OnlyGpu => "--only-gpu",
            ComputeMode::OnlyCpu => "--only-cpu",
        });
    }
    for (name, on) in [
        ("--no-enroll", args.no_enroll),
        ("--no-workers", args.no_workers),
        ("--no-library", args.no_library),
        ("--trust-any-client-cert", args.trust_any_client_cert),
        ("--allow-remote-admin", args.allow_remote_admin),
        ("--insecure-no-tls", args.insecure_no_tls),
    ] {
        if on {
            flags.push(name);
        }
    }
    flags
}

/// The options of `serve` that name files and addresses, when `args` sets them.
fn address_options(args: &ServeArgs) -> Vec<(&'static str, String)> {
    let mut valued: Vec<(&'static str, String)> = Vec::new();
    let paths: [(&'static str, &Option<PathBuf>); 6] = [
        ("--ca", &args.ca),
        ("--cert", &args.cert),
        ("--key", &args.key),
        ("--allowlist", &args.allowlist),
        ("--worker-allowlist", &args.worker_allowlist),
        ("--db", &args.db),
    ];
    for (name, path) in paths {
        if let Some(path) = path {
            valued.push((name, path.to_string_lossy().into_owned()));
        }
    }
    for (name, text) in [
        ("--enroll-bind", &args.enroll_bind),
        ("--worker-bind", &args.worker_bind),
        ("--worker-enroll-bind", &args.worker_enroll_bind),
        ("--pin-interactive-worker", &args.pin_interactive_worker),
    ] {
        if let Some(text) = text {
            valued.push((name, text.clone()));
        }
    }
    valued
}

/// The options of `serve` whose value differs from `serve`'s own default (`defaults`).
fn tuning_options(args: &ServeArgs, defaults: &ServeArgs) -> Vec<(&'static str, String)> {
    let mut valued: Vec<(&'static str, String)> = Vec::new();
    if args.threads != defaults.threads {
        valued.push(("--threads", args.threads.to_string()));
    }
    if args.max_connections != defaults.max_connections {
        valued.push(("--max-connections", args.max_connections.to_string()));
    }
    if args.max_preauth_per_ip != defaults.max_preauth_per_ip {
        valued.push(("--max-preauth-per-ip", args.max_preauth_per_ip.to_string()));
    }
    if args.interactive_workers != defaults.interactive_workers {
        valued.push((
            "--interactive-workers",
            args.interactive_workers.to_string(),
        ));
    }
    if args.max_job_memory_mib != defaults.max_job_memory_mib {
        valued.push(("--max-job-memory-mib", args.max_job_memory_mib.to_string()));
    }
    if args.whole_image_secs.to_bits() != defaults.whole_image_secs.to_bits() {
        valued.push(("--whole-image-secs", args.whole_image_secs.to_string()));
    }
    if args.whole_image_pixel_samples != defaults.whole_image_pixel_samples {
        valued.push((
            "--whole-image-pixel-samples",
            args.whole_image_pixel_samples.to_string(),
        ));
    }
    if args.jobs_per_viewer != defaults.jobs_per_viewer {
        valued.push(("--jobs-per-viewer", args.jobs_per_viewer.to_string()));
    }
    if args.payload_encoding != defaults.payload_encoding {
        valued.push(("--payload-encoding", args.payload_encoding.to_string()));
    }
    valued
}

/// `arg` as one shell word: bare when it has only safe characters, otherwise in double quotes
/// (`windows`) or single quotes.
fn quote(arg: &str, windows: bool) -> String {
    let safe =
        |c: char| c.is_ascii_alphanumeric() || "-_./:=@,+".contains(c) || (windows && c == '\\');
    if !arg.is_empty() && arg.chars().all(safe) {
        arg.to_owned()
    } else if windows {
        format!("\"{}\"", arg.replace('"', "\\\""))
    } else {
        format!("'{}'", arg.replace('\'', "'\\''"))
    }
}

/// Puts `text` on the clipboard.
///
/// # Errors
///
/// `Err` when there is no clipboard to use.
fn copy_text(text: &str) -> Result<(), ()> {
    CLIPBOARD.with(|slot| {
        let mut slot = slot.borrow_mut();
        if slot.is_none() {
            *slot = arboard::Clipboard::new().ok();
        }
        slot.as_mut().map_or(Err(()), |clipboard| {
            clipboard.set_text(text.to_owned()).map_err(|_| ())
        })
    })
}

/// The file manager or viewer command of this platform.
const fn opener() -> &'static str {
    if cfg!(target_os = "windows") {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    }
}

/// Opens `path` with the program the system chooses for it.
fn open_path(path: &Path) {
    if let Err(e) = std::process::Command::new(opener()).arg(path).spawn() {
        tracing::warn!("could not open {}: {e}", path.display());
    }
}

/// Shows the folder of `path`, with the file selected where the system can do that.
fn show_in_folder(path: &Path) {
    if cfg!(target_os = "windows") {
        let mut select = std::ffi::OsString::from("/select,");
        select.push(path);
        if let Err(e) = std::process::Command::new("explorer").arg(select).spawn() {
            tracing::warn!("could not show {}: {e}", path.display());
        }
    } else if let Some(dir) = path.parent() {
        open_path(dir);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_words_stay_bare_and_others_are_quoted() {
        assert_eq!(quote("--bind", false), "--bind");
        assert_eq!(quote("0.0.0.0:7878", false), "0.0.0.0:7878");
        assert_eq!(quote("/data/pki/ca.pem", false), "/data/pki/ca.pem");
        assert_eq!(quote("/my data/ca.pem", false), "'/my data/ca.pem'");
        assert_eq!(quote("it's", false), "'it'\\''s'");
        assert_eq!(
            quote("C:\\Users\\me\\ca.pem", true),
            "C:\\Users\\me\\ca.pem"
        );
        assert_eq!(
            quote("C:\\My Data\\ca.pem", true),
            "\"C:\\My Data\\ca.pem\""
        );
        assert_eq!(quote("", false), "''");
    }

    #[test]
    fn the_command_names_bind_certificates_and_no_secret() {
        let settings = crate::settings::CoordinatorSettings::default();
        let args = settings.serve_args(Path::new("data")).unwrap();
        let command = serve_command(&args, false).unwrap();
        assert!(
            command.starts_with("pleochroma-worker --headless serve --bind "),
            "{command}"
        );
        for flag in ["--ca ", "--cert ", "--key "] {
            assert!(command.contains(flag), "{command}");
        }
        assert!(!command.contains("--insecure-no-tls"), "{command}");
        assert!(!command.to_lowercase().contains("token"), "{command}");
    }

    #[test]
    fn only_options_that_differ_from_the_defaults_appear() {
        let Ok(crate::cli::Command::Serve(defaults)) = crate::cli::parse(&["serve".to_owned()])
        else {
            panic!("serve must parse");
        };
        let defaults = *defaults;
        let plain = serve_command(&defaults, false).unwrap();
        assert_eq!(
            plain,
            "pleochroma-worker --headless serve --bind 127.0.0.1:7878"
        );

        let mut changed = defaults;
        changed.max_connections = 10;
        changed.no_workers = true;
        changed.db = Some(PathBuf::from("lib.sqlite"));
        let command = serve_command(&changed, false).unwrap();
        assert!(command.contains("--max-connections 10"), "{command}");
        assert!(command.contains("--no-workers"), "{command}");
        assert!(command.contains("--db lib.sqlite"), "{command}");
    }

    #[test]
    fn a_long_render_reads_as_minutes_and_seconds() {
        assert_eq!(elapsed_text(Duration::from_secs(9)), "9 s");
        assert_eq!(elapsed_text(Duration::from_secs(125)), "2 min 05 s");
    }

    #[test]
    fn the_picture_always_ends_in_png() {
        assert_eq!(
            with_png_extension(PathBuf::from("a/b.json")),
            PathBuf::from("a/b.png")
        );
        assert_eq!(
            with_png_extension(PathBuf::from("c")),
            PathBuf::from("c.png")
        );
    }
}
