//! `pleochroma-worker` CLI entry point. See `lib.rs` for the crate-level design notes and
//! `cli::HelpTopic`/its `USAGE_*` constants for full command-line help.

use pleochroma_worker::cli::{self, Command, HelpTopic};
use tracing_subscriber::{EnvFilter, fmt};

/// The per-user cache folder of the platform: `%LOCALAPPDATA%`, `~/Library/Caches`,
/// `$XDG_CACHE_HOME` or `~/.cache`. `None` when none of those variables is set. The worker
/// has no data-directory flag of its own, so the platform default is the only source.
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

/// Points the GPU renderer's driver pipeline cache at `<cache>/pleochroma/pipeline_cache`
/// (review item GPU-1): without it every start recompiles the megakernel before the first
/// frame. Must run before the first renderer is created; a no-op without a cache folder.
///
/// While the RADV scheduler workaround is in force the folder is `pipeline_cache-<value>`
/// instead, so a pipeline compiled with the driver's stock scheduler is never served to a run
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

/// The RADV scheduler workaround (Mesa's AMD driver mis-schedules the GPU kernel).
///
/// See `pleochroma::renderer::gpu_driver`. Sets `ACO_DEBUG` and the Mesa shader-cache folder unless
/// the switch (`--radv-workaround`, `PLEOCHROMA_RADV_WORKAROUND`) says off or the user set
/// `ACO_DEBUG` themselves. Without the switch (`setting` is `None`) it is on only when the
/// machine has an AMD GPU on the `amdgpu` kernel driver. Must run before any thread starts
/// and before the GPU is touched.
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
    let cache_root = platform_cache_dir().map(|dir| dir.join("pleochroma-worker"));
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

fn main() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let _ = fmt().with_env_filter(filter).with_target(true).try_init();

    let mut argv: Vec<String> = std::env::args().skip(1).collect();
    // First: the RADV switch is removed from `argv` before parsing, like `--no-update-check`
    // below.
    start_gpu_environment(&mut argv);
    // The first log line names a newer release, if any (one GitHub request, at most 2 s);
    // `--no-update-check` (removed here, before parsing) or PLEOCHROMA_NO_UPDATE_CHECK skip it.
    let skip = std::env::var_os(pleochroma_update::NO_UPDATE_CHECK_ENV);
    if pleochroma_update::take_update_check(&mut argv, skip.as_deref())
        && let Some(line) =
            pleochroma_update::cli_notice("pleochroma-worker", env!("CARGO_PKG_VERSION"))
    {
        tracing::info!("{line}");
    }

    match cli::parse(&argv) {
        Ok(Command::Help(topic)) => print!("{}", topic.usage()),
        #[cfg(feature = "worker")]
        Ok(Command::Render(render_args)) => {
            if let Err(e) = pleochroma_worker::render_cmd::run(&render_args) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        #[cfg(not(feature = "worker"))]
        Ok(Command::Render(_)) => {
            eprintln!(
                "error: this build has no render capacity -- rebuild with `--features worker` \
                 (see --help)"
            );
            std::process::exit(1);
        }
        Ok(Command::Serve(serve_args)) => {
            if let Err(e) = pleochroma_worker::serve::run(&serve_args) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        #[cfg(feature = "worker")]
        Ok(Command::Join(join_args)) => {
            if let Err(e) = pleochroma_worker::join::run(&join_args) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        // Unreachable in practice: `cli::parse` already refuses `join` on this build.
        #[cfg(not(feature = "worker"))]
        Ok(Command::Join(_)) => {
            eprintln!(
                "error: this build has no render capacity to contribute -- rebuild with `--features worker` (see --help)"
            );
            std::process::exit(1);
        }
        Ok(Command::CertInit(init_args)) => {
            if let Err(e) = pleochroma_worker::pki::init(&init_args.dir) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Ok(Command::CertIssueServer(server_args)) => {
            if let Err(e) = pleochroma_worker::pki::issue_server(
                &server_args.dir,
                &server_args.hosts,
                &server_args.ips,
            ) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Ok(Command::CertIssueClient(client_args)) => {
            if let Err(e) = pleochroma_worker::pki::issue_client_with_role(
                &client_args.dir,
                &client_args.name,
                &client_args.out,
                client_args.role,
            ) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Ok(Command::CertIssueToken(token_args)) => {
            if let Err(e) = pleochroma_worker::enroll_client::run_issue_token(&token_args) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Ok(Command::CertClaim(claim_args)) => {
            if let Err(e) = pleochroma_worker::enroll_client::claim(&claim_args) {
                eprintln!("error: {e}");
                std::process::exit(1);
            }
        }
        Err(e) => {
            eprintln!("error: {e}\n");
            // Same topic resolution as a real --help, so a typo'd flag on
            // `render` prints render's own page rather than the whole document.
            eprint!("{}", HelpTopic::from_argv(&argv).usage());
            std::process::exit(2);
        }
    }
}
