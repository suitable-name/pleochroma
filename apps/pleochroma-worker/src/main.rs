//! `pleochroma-worker` CLI entry point. See `lib.rs` for the crate-level design notes and
//! `cli::HelpTopic`/its `USAGE_*` constants for full command-line help.
//!
//! On Windows, a build with the `gui` feature is a Windows-subsystem program (no console of
//! its own); `pleochroma-worker-console` (packaged as `pleochroma-worker.com`) is the
//! console twin that hands a command line to it. See `build.rs`. Without `gui` the binary
//! keeps its console, exactly as before.
//!
//! With `gui`, a bare command line opens the coordinator window (`gui::app::run`); every
//! command, and every command with `--headless`, runs headless as before.
#![cfg_attr(all(windows, feature = "gui"), windows_subsystem = "windows")]

use pleochroma_worker::cli::{self, Command, HelpTopic};
use tracing_subscriber::{EnvFilter, fmt, prelude::*};

/// The settings file this run reads, and the folder that is its data folder when the settings
/// choose none: the window's per-user folder (or the one `--config` names), or for a command
/// line the folder of the `--config` file. `None` when there is no such file to look at.
fn settings_location(
    globals: &cli::GlobalFlags,
    window: bool,
) -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    use std::path::{Path, PathBuf};
    #[cfg(feature = "gui")]
    {
        if window {
            let folder = pleochroma_worker::gui::local::CoordinatorFolder::locate(
                globals.config.as_deref(),
            )?;
            return Some((
                folder.settings_file().to_path_buf(),
                folder.data_dir().to_path_buf(),
            ));
        }
    }
    #[cfg(not(feature = "gui"))]
    let _ = window;
    let file = globals.config.clone()?;
    let beside = file
        .parent()
        .filter(|dir| !dir.as_os_str().is_empty())
        .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
    Some((file, beside))
}

/// The command word of `argv` (the first word that is not a start-up switch), or `None`.
fn command_word(argv: &[String]) -> Option<&str> {
    let mut words = argv.iter();
    while let Some(arg) = words.next() {
        match arg.as_str() {
            "--no-update-check" => {}
            "--radv-workaround" => {
                let _ = words.next();
            }
            other if other.starts_with("--radv-workaround=") => {}
            other => return Some(other),
        }
    }
    None
}

/// The folder the log file goes in, or `None` for no file.
///
/// The window logs to `logs/` in its data folder. A headless run has no data folder of its own:
/// it writes a file only when `--config <file>` names one and the command is `serve` or `join`,
/// the commands that run for a long time. Any other command (`cert issue-token --config`, a
/// help page) creates no folder and no file. In both cases the data folder is the one the
/// settings file chooses, when it chooses one (read without touching the file), and the folder
/// of the settings file otherwise.
fn log_directory(
    globals: &cli::GlobalFlags,
    window: bool,
    argv: &[String],
) -> Option<std::path::PathBuf> {
    use pleochroma_worker::settings::{CoordinatorSettings, log::log_dir};
    if !window && !matches!(command_word(argv), Some("serve" | "join")) {
        return None;
    }
    let (file, default_dir) = settings_location(globals, window)?;
    let chosen = CoordinatorSettings::peek(&file)
        .map_or_else(|| default_dir.clone(), |s| s.data_dir(&default_dir));
    Some(log_dir(&chosen))
}

/// Whether the settings allow the update check (the `update_check` setting, on unless the file
/// says otherwise). Read without touching the file; a missing or invalid file, or no settings
/// file at all (a command line without `--config`), allows it. The command-line switch and the
/// environment variable still win over a setting that allows it.
fn settings_allow_update_check(globals: &cli::GlobalFlags, window: bool) -> bool {
    use pleochroma_worker::settings::CoordinatorSettings;
    settings_location(globals, window)
        .and_then(|(file, _)| CoordinatorSettings::peek(&file))
        .is_none_or(|settings| settings.update_check)
}

/// Installs the tracing subscriber, in every build: the console lines (scrubbed of tokens,
/// pairing links and private keys), the ring buffer the Log page and a remote administrator
/// read, and a daily log file in `log_dir` (when there is one and it can be written).
fn install_tracing(log_dir: Option<&std::path::Path>) {
    use pleochroma_worker::settings::log::{console_writer, file_layer, ring_layer};
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let registry = tracing_subscriber::registry()
        .with(filter)
        .with(fmt::layer().with_target(true).with_writer(console_writer()))
        .with(ring_layer())
        .with(log_dir.and_then(file_layer));
    let _ = registry.try_init();
}

/// Points the GPU driver's pipeline cache at `<cache>/pleochroma/pipeline_cache`, so the
/// megakernel is not recompiled on every start. Must run before the first renderer is
/// created; a no-op without a cache folder.
///
/// While the RADV scheduler workaround is in force the folder is `pipeline_cache-<value>`,
/// so a pipeline compiled with the stock scheduler is never served to a run with the
/// workaround.
///
/// The cache folder is the platform's per-user one (`%LOCALAPPDATA%`, `~/Library/Caches`,
/// `$XDG_CACHE_HOME` or `~/.cache`), the same rule as the CLI's; an empty `XDG_CACHE_HOME`
/// counts as unset, so the cache never lands relative to the folder the worker started in.
#[cfg(feature = "gpu")]
fn enable_gpu_pipeline_cache() {
    use pleochroma::renderer::gpu_driver::{
        pipeline_cache_folder, platform_cache_dir, workaround_active,
    };
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

/// Applies the RADV scheduler workaround (Mesa's AMD driver mis-schedules the GPU kernel).
///
/// See `pleochroma::renderer::gpu_driver`. It sets `ACO_DEBUG` and the Mesa shader-cache
/// folder unless the switch says off or the user set `ACO_DEBUG` themselves. Without a
/// switch value (`setting` is `None`) it is on only when an AMD GPU on the `amdgpu` kernel
/// driver is present. Must run before any thread starts and before the GPU is touched.
#[cfg(feature = "gpu")]
fn apply_radv_workaround(setting: Option<bool>) {
    let wanted = setting.unwrap_or_else(|| {
        let amd = pleochroma::renderer::gpu_driver::amd_gpu_present();
        tracing::info!(
            amd_gpu = amd,
            "RADV workaround not set: on only with an AMD GPU on the amdgpu driver"
        );
        amd
    });
    let cache_root = pleochroma::renderer::gpu_driver::platform_cache_dir()
        .map(|dir| dir.join("pleochroma-worker"));
    let status = pleochroma::renderer::gpu_driver::apply(wanted, cache_root.as_deref());
    tracing::info!("{}", status.log_line());
}

/// Takes the RADV workaround switch out of `argv` and applies it.
///
/// Exits with the usage on a bad value. In a `gpu` build it also points the GPU pipeline cache at
/// its folder. Runs while the process is still single-threaded: the driver reads the environment
/// when the GPU device is created.
fn start_gpu_environment(argv: &mut Vec<String>) {
    let radv_setting = match pleochroma_worker::radv_workaround::take_setting(
        argv,
        std::env::var_os(pleochroma_worker::radv_workaround::ENV).as_deref(),
    ) {
        Ok(setting) => setting,
        Err(e) => {
            eprintln!("error: {e}\n");
            eprint!("{}", HelpTopic::from_argv(argv).usage());
            std::process::exit(2);
        }
    };
    #[cfg(feature = "gpu")]
    {
        apply_radv_workaround(radv_setting);
        enable_gpu_pipeline_cache();
    }
    #[cfg(not(feature = "gpu"))]
    let _ = radv_setting;
}

/// Runs `serve` headless until Ctrl-C or SIGTERM, then shuts it down (see
/// `pleochroma_worker::serve::run_until`) and returns, so the process exits with status 0.
///
/// If the signal handler cannot be installed, a warning is logged and `serve` runs until it
/// is killed, as before.
fn run_serve_until_signal(args: &pleochroma_worker::cli::ServeArgs) -> Result<(), String> {
    let (stop_tx, stop_rx) = std::sync::mpsc::channel::<()>();
    if let Err(e) = ctrlc::set_handler(move || {
        let _ = stop_tx.send(());
    }) {
        tracing::warn!("could not install the Ctrl-C / SIGTERM handler ({e}); stop with kill");
    }
    pleochroma_worker::serve::run_until(args, move || {
        if stop_rx.recv().is_ok() {
            return;
        }
        // No handler: wait for the process to be killed.
        loop {
            std::thread::park();
        }
    })
}

/// What D1 calls a bare command: the first word of the command line is a command (not a flag
/// or a help request) and nothing asks for help.
#[cfg(feature = "gui")]
fn bare_command(argv: &[String]) -> Option<&str> {
    let first = argv.first()?.as_str();
    let is_command = matches!(first, "serve" | "join" | "render" | "cert");
    let asks_for_help = argv.iter().any(|a| a == "-h" || a == "--help");
    (is_command && !asks_for_help).then_some(first)
}

/// Opens the coordinator window and returns the process exit status: 0 after a normal run,
/// 2 when there is no display (with the hint), 1 for any other failure.
///
/// Headless output (the other commands) writes to the standard handles the process was
/// started with. On Windows the console twin passes its own handles to this Windows-subsystem
/// program, and nothing here frees, allocates or redirects a console, so they stay valid.
#[cfg(feature = "gui")]
fn run_window(globals: &cli::GlobalFlags) -> i32 {
    use pleochroma_worker::{
        gui::{
            app::{self, GuiError, GuiOptions},
            local::CoordinatorFolder,
        },
        platform::instance::{self, Claim},
    };
    // One window per user and folder: a second start brings the first window forward and
    // exits (an auto-start with `--minimized` only asks, so a login never pops a window up).
    let guard = if let Some(folder) = CoordinatorFolder::locate(globals.config.as_deref()) {
        match instance::claim(folder.data_dir(), !globals.minimized) {
            Claim::Existing => return 0,
            Claim::First(guard) => Some(guard),
            Claim::Unavailable => None,
        }
    } else {
        None
    };
    match app::run_with_instance(&GuiOptions::from(globals), guard) {
        Ok(()) => 0,
        Err(GuiError::NoDisplay(detail)) => {
            eprintln!("No display found. On servers run \"pleochroma-worker --headless serve …\"");
            eprintln!("({detail})");
            2
        }
        Err(GuiError::Failed(message)) => {
            eprintln!("error: {message}");
            1
        }
    }
}

/// Prints `error: <message>` and exits with status 1.
fn fail(error: &dyn std::fmt::Display) -> ! {
    eprintln!("error: {error}");
    std::process::exit(1)
}

/// Exits with status 1 and the error when `result` is one.
fn finish<E: std::fmt::Display>(result: Result<(), E>) {
    if let Err(e) = result {
        fail(&e);
    }
}

/// Starts the update check: a newer release is named in one info line, if there is one (one
/// GitHub request, at most 2 s). The request runs on its own thread, so start-up never waits
/// on the network; the thread is not joined and ends with the process. `--no-update-check`
/// (removed from `argv` here, before parsing), `PLEOCHROMA_NO_UPDATE_CHECK` or the
/// `update_check` setting being off (`settings_allowed`) skip it.
fn start_update_check(argv: &mut Vec<String>, settings_allowed: bool) {
    let skip = std::env::var_os(pleochroma_update::NO_UPDATE_CHECK_ENV);
    // Called first, so the switch is removed from `argv` whatever the setting says.
    if pleochroma_update::take_update_check(argv, skip.as_deref()) && settings_allowed {
        std::thread::spawn(|| {
            if let Some(line) =
                pleochroma_update::cli_notice("pleochroma-worker", env!("CARGO_PKG_VERSION"))
            {
                tracing::info!("{line}");
            }
        });
    }
}

/// Runs the command line `argv` (the global flags and the start-up switches already taken
/// out): parses it and runs the command, or prints the usage and exits with status 2. `config`
/// is the `--config` file, if the line had one.
fn run_command(argv: &[String], config: Option<&std::path::Path>) {
    match cli::parse(argv) {
        Ok(Command::Help(topic)) => print!("{}", topic.usage()),
        Ok(Command::Version) => println!("{}", cli::version_line()),
        #[cfg(feature = "worker")]
        Ok(Command::Render(render_args)) => {
            finish(pleochroma_worker::render_cmd::run(&render_args));
        }
        #[cfg(not(feature = "worker"))]
        Ok(Command::Render(_)) => fail(
            &"this build has no render capacity -- rebuild with `--features worker` (see --help)",
        ),
        Ok(Command::Serve(serve_args)) => finish(run_serve_until_signal(&serve_args)),
        #[cfg(feature = "worker")]
        Ok(Command::Join(join_args)) => finish(pleochroma_worker::join::run(&join_args)),
        // Unreachable in practice: `cli::parse` already refuses `join` on this build.
        #[cfg(not(feature = "worker"))]
        Ok(Command::Join(_)) => fail(
            &"this build has no render capacity to contribute -- rebuild with `--features worker` (see --help)",
        ),
        Ok(Command::CertInit(init_args)) => finish(pleochroma_worker::pki::init(&init_args.dir)),
        Ok(Command::CertIssueServer(server_args)) => finish(pleochroma_worker::pki::issue_server(
            &server_args.dir,
            &server_args.hosts,
            &server_args.ips,
        )),
        Ok(Command::CertIssueClient(client_args)) => {
            finish(pleochroma_worker::pki::issue_client_with_role(
                &client_args.dir,
                &client_args.name,
                &client_args.out,
                client_args.role,
            ));
        }
        Ok(Command::CertIssueToken(token_args)) => {
            // `--config` names the coordinator's settings: the pairing link then carries its
            // name and its real ports instead of guesses.
            if let Some(file) = config
                && !pleochroma_worker::settings::init_read_only(file.to_path_buf())
            {
                tracing::debug!("the --config file holds no valid settings; using the defaults");
            }
            finish(pleochroma_worker::enroll_client::run_issue_token(
                &token_args,
            ));
        }
        Ok(Command::CertClaim(claim_args)) => {
            finish(pleochroma_worker::enroll_client::claim(&claim_args));
        }
        Err(e) => {
            eprintln!("error: {e}\n");
            // Same topic resolution as a real --help, so a typo'd flag on
            // `render` prints render's own page rather than the whole document.
            eprint!("{}", HelpTopic::from_argv(argv).usage());
            std::process::exit(2);
        }
    }
}

fn main() {
    let mut argv: Vec<String> = std::env::args().skip(1).collect();
    // `--headless`, `--minimized` and `--config` may stand anywhere: they are removed first,
    // so the parser (and the usage page of an error) sees the line as it always was.
    let globals = match cli::take_global_flags(&mut argv) {
        Ok(globals) => globals,
        Err(e) => {
            eprintln!("error: {e}\n");
            eprint!("{}", HelpTopic::from_argv(&argv).usage());
            std::process::exit(2);
        }
    };
    // `--version` is answered first: nothing may reach standard output before the version line
    // (the log lines of the GPU setup and the update check would).
    if cli::wants_version(&argv) {
        println!("{}", cli::version_line());
        return;
    }
    // The window or the command line? Decided on the line as typed (`requests_window` knows
    // every flag the next steps remove), so the log file can go to the right folder.
    let launch = cli::launch_mode(&globals, &argv);
    let window = launch == cli::LaunchMode::Gui;
    install_tracing(log_directory(&globals, window, &argv).as_deref());
    // The RADV switch is removed from `argv` before parsing, as `--no-update-check` is below.
    start_gpu_environment(&mut argv);
    start_update_check(&mut argv, settings_allow_update_check(&globals, window));
    // Without the `gui` feature there is no window and `launch` is always `Headless`.
    #[cfg(feature = "gui")]
    {
        if launch == cli::LaunchMode::Gui {
            std::process::exit(run_window(&globals));
        }
        // D1: a command without `--headless` still runs headless in this version, with a
        // warning; a later version exits with status 2 instead.
        if !globals.headless
            && let Some(command) = bare_command(&argv)
        {
            eprintln!("warning: \"{command}\" without --headless is deprecated; add --headless");
        }
    }
    #[cfg(not(feature = "gui"))]
    let _ = launch;
    run_command(&argv, globals.config.as_deref());
}
