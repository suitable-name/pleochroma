// No console window in a release build: Rust binaries default to the Windows CONSOLE
// subsystem, and a launch of the `.exe` (Explorer, a shortcut, the Start menu) would open
// an empty console beside the window.
//
// Debug builds keep the console so `cargo run` shows panics and `tracing` output. A release
// build has no stderr, which is why `install_tracing_subscriber` and `install_panic_logger`
// also write to files.
//
// No effect on non-Windows targets: the attribute is Windows-specific.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod info_request;

/// Installs the `tracing` subscriber: stderr, plus a log file when the app was started with
/// `--log`.
///
/// Without a subscriber every `tracing` event is lost, the GPU backend's device-lost messages
/// included, and `RUST_LOG` has no effect. The default filter is `warn`, with `info` for
/// `pleochroma` and `pleochroma_cut`; `RUST_LOG` overrides it.
///
/// Without `--log` no file is created. The file goes beside the executable, or into the temp
/// directory when that is not writable. A failed setup never stops the app: it falls back to
/// stderr.
fn install_tracing_subscriber() {
    use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt as _};

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("warn,pleochroma=info,pleochroma_cut=info"));

    let file_name = "pleochroma-cut.log";
    let beside_exe = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(file_name)));
    let candidates = [beside_exe, Some(std::env::temp_dir().join(file_name))];
    // Only a run started with `--log` writes the log file; the panic log below is
    // separate and exists only after a crash.
    let log_file = std::env::args()
        .any(|arg| arg == "--log")
        .then(|| {
            candidates.into_iter().flatten().find_map(|path| {
                std::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(path)
                    .ok()
            })
        })
        .flatten();

    let stderr_layer = fmt::layer().with_writer(std::io::stderr).with_target(true);
    // `with_ansi(false)`: color escape codes have no business in a plain-text log
    // file a user might open in Notepad.
    let file_layer = log_file.map(|file| {
        fmt::layer()
            .with_writer(file)
            .with_ansi(false)
            .with_target(true)
    });

    let _ = tracing_subscriber::registry()
        .with(filter)
        .with(stderr_layer)
        .with(file_layer)
        .try_init();
}

/// Writes panics to a file: a release build has no console for them.
///
/// A panic is not a `tracing` event, so a panic on a background thread would otherwise leave
/// no trace. Chains to the previous hook, so a debug build still prints to its console.
/// Failing to write the log is ignored: a panic handler that panics is worse than silence.
fn install_panic_logger() {
    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Beside the executable first, where a user finds it; then the temp directory, for
        // an install location that is not writable.
        let file_name = "pleochroma-cut-panic.log";
        let beside_exe = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(|dir| dir.join(file_name)));
        let candidates = [beside_exe, Some(std::env::temp_dir().join(file_name))];

        let thread = std::thread::current();
        let entry = format!(
            "---\nthread: {}\n{info}\nbacktrace: {}\n",
            thread.name().unwrap_or("<unnamed>"),
            std::backtrace::Backtrace::force_capture()
        );

        for path in candidates.into_iter().flatten() {
            use std::io::Write as _;
            if let Ok(mut f) = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&path)
                && f.write_all(entry.as_bytes()).is_ok()
            {
                break;
            }
        }

        previous(info);
    }));
}

/// Passes the saved UI scale (Edit > Preferences...) to the windowing layer as
/// `SLINT_SCALE_FACTOR`.
///
/// The windowing layer reads it when the first window is created, so a change takes effect at
/// the next start. Does nothing for the Automatic setting, and leaves a `SLINT_SCALE_FACTOR`
/// the user set.
///
/// Every picking and raster path reads the window's real scale factor, so one value here
/// keeps them consistent.
fn apply_saved_ui_scale() {
    if let Some(factor) = pleochroma_cut::gui::preferences::saved_ui_scale_factor() {
        // SAFETY: this is the first thing `main` does, before the tracing subscriber, the
        // panic hook, the window or any worker thread exists, so nothing else can be
        // reading or writing the process environment while it changes.
        unsafe { std::env::set_var("SLINT_SCALE_FACTOR", factor) };
    }
}

/// The per-user cache folder of the platform: `%LOCALAPPDATA%`, `~/Library/Caches`,
/// `$XDG_CACHE_HOME` or `~/.cache`. `None` when none of those variables is set.
#[cfg(feature = "gpu")]
fn platform_cache_dir() -> Option<std::path::PathBuf> {
    use std::path::PathBuf;
    if cfg!(target_os = "windows") {
        std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
    } else if cfg!(target_os = "macos") {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Library").join("Caches"))
    } else if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME") {
        Some(PathBuf::from(xdg))
    } else {
        std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache"))
    }
}

/// Points the GPU driver's pipeline cache at `<cache>/pleochroma/pipeline_cache`.
///
/// Without it every start recompiles the megakernel before the first pixel. Must run before the
/// first renderer is created; a no-op without a cache folder.
///
/// While the RADV scheduler workaround is active (`ACO_DEBUG` names it) the folder is
/// `pipeline_cache-<value>`, so a pipeline compiled with the stock scheduler is never reused
/// with the workaround.
#[cfg(feature = "gpu")]
fn enable_gpu_pipeline_cache() {
    use pleochroma::renderer::gpu_driver::{pipeline_cache_folder, workaround_active};
    let Some(base) = platform_cache_dir() else {
        return;
    };
    let dir = base
        .join("pleochroma")
        .join(pipeline_cache_folder(workaround_active()));
    if pleochroma::renderer::gpu::pipeline_cache::set_pipeline_cache_dir(dir.clone()) {
        tracing::info!(dir = %dir.display(), "GPU pipeline cache directory set");
    }
}

fn main() -> anyhow::Result<()> {
    // `--help` and `--version` answer and exit before anything else: no RADV restart wait, no
    // Slint, no GPU. A release build has no console, so the answer finds its own way out.
    if info_request::answer_if_requested(std::env::args_os().skip(1)) {
        return Ok(());
    }
    // Before ANY thread, Slint call or GPU call: the environment is only safe to change while
    // the process is still single-threaded, and Mesa reads `ACO_DEBUG` when the Vulkan device
    // is created. The RADV scheduler workaround comes first of all: when this instance
    // replaces another one (a restart) it also waits here until that one is gone.
    let radv_status = pleochroma_cut::gui::radv_workaround::apply_at_startup();
    // The app's estimator is `Estimator::APP_DEFAULT`, set before any GPU work, warm-up or frame.
    pleochroma::optics::raytracer::set_default_estimator(
        pleochroma::optics::raytracer::Estimator::APP_DEFAULT,
    );
    // A `ui-harness` build started as the app: real file dialogs, clipboard, opener and
    // library (the harness itself never runs `main`). Before anything reads the library path.
    #[cfg(feature = "ui-harness")]
    pleochroma_cut::harness_support::disarm_seams();
    apply_saved_ui_scale();
    install_tracing_subscriber();
    install_panic_logger();
    pleochroma_cut::gui::radv_workaround::log_startup_status(&radv_status);
    #[cfg(feature = "gpu")]
    enable_gpu_pipeline_cache();
    // `pleochroma-cut <file>` (what an OS "open with" or double-click association
    // runs) opens that file once the window is up; flags such as `--log` are skipped.
    if let Some(path) =
        pleochroma_cut::gui::startup_file::path_from_args(std::env::args_os().skip(1))
    {
        pleochroma_cut::gui::startup_file::request_open_at_startup(path);
    }
    pleochroma_cut::gui::main()
}
