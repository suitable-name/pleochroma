//! The `pleochroma-cli` program: runs the command line given to it and exits with its code.
//!
//! Everything else is in the library (`pleochroma_cli`): this only collects the arguments,
//! names a newer release on standard error first (`pleochroma_cli::take_update_check`), prints
//! the text a command returned and turns its exit code into the process's. Before anything else
//! it takes `--radv-workaround` out of the arguments and sets up the GPU environment (the RADV
//! workaround and the pipeline cache folder).

use std::io::Write;

/// Points the GPU driver's pipeline cache at a folder under `<cache>/pleochroma`, so a GPU
/// render does not compile its shaders cold. Must run before the first renderer exists; does
/// nothing without a cache folder.
///
/// The folder name depends on the RADV workaround (`ACO_DEBUG` names it), so a pipeline compiled
/// one way is never served to a run the other way. An unwritable folder only costs start-up time.
#[cfg(feature = "gpu")]
fn enable_gpu_pipeline_cache() {
    use pleochroma::renderer::gpu_driver::{
        pipeline_cache_folder, platform_cache_dir, workaround_active,
    };
    if let Some(base) = platform_cache_dir() {
        let dir = base
            .join("pleochroma")
            .join(pipeline_cache_folder(workaround_active()));
        pleochroma::renderer::gpu::pipeline_cache::set_pipeline_cache_dir(dir);
    }
}

/// Applies the RADV scheduler workaround (Mesa's AMD driver mis-schedules the GPU kernel) the
/// way the worker does, through `pleochroma::renderer::gpu_driver`.
///
/// Sets `ACO_DEBUG` and the Mesa shader-cache folder (`<cache>/pleochroma-cli`) unless `setting`
/// (`--radv-workaround` or `PLEOCHROMA_RADV_WORKAROUND`) is off or the user set `ACO_DEBUG`
/// already. `None` means on only with an AMD GPU on the `amdgpu` kernel driver. Must run before
/// any thread starts and before the GPU is touched.
#[cfg(feature = "gpu")]
fn apply_radv_workaround(setting: Option<bool>) {
    use pleochroma::renderer::gpu_driver::{amd_gpu_present, apply, platform_cache_dir};
    let wanted = setting.unwrap_or_else(amd_gpu_present);
    let cache_root = platform_cache_dir().map(|dir| dir.join("pleochroma-cli"));
    // Nothing logs here: the command line has no subscriber, and its output stays deterministic.
    let _status = apply(wanted, cache_root.as_deref());
}

/// Takes the RADV workaround switch out of `args` and, in a `gpu` build, applies it and points
/// the GPU pipeline cache at its folder. Exits with the usage code on a bad value.
///
/// Must run first, while the process is still single-threaded: the driver reads the environment
/// when the GPU device is created.
fn start_gpu_environment(args: &mut Vec<String>) {
    let setting = match pleochroma_cli::radv_workaround::take_setting(
        args,
        std::env::var_os(pleochroma_cli::radv_workaround::ENV).as_deref(),
    ) {
        Ok(setting) => setting,
        Err(message) => {
            eprintln!("error: {message}\ntry: pleochroma-cli --help");
            std::process::exit(pleochroma_cli::EXIT_USAGE);
        }
    };
    #[cfg(feature = "gpu")]
    {
        apply_radv_workaround(setting);
        enable_gpu_pipeline_cache();
    }
    #[cfg(not(feature = "gpu"))]
    let _ = setting;
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    start_gpu_environment(&mut args);
    let skip = std::env::var_os(pleochroma_cli::NO_UPDATE_CHECK_ENV);
    if pleochroma_cli::take_update_check(&mut args, skip.as_deref()) {
        print_update_notice();
    }
    let outcome = pleochroma_cli::run(&args);
    // A closed pipe (`pleochroma-cli ... | head`) is not an error worth a message.
    let mut stdout = std::io::stdout().lock();
    let _ = stdout.write_all(outcome.stdout.as_bytes());
    let _ = stdout.flush();
    let _ = std::io::stderr().write_all(outcome.stderr.as_bytes());
    std::process::exit(outcome.exit);
}

/// Names a newer Pleochroma release in one `info:` line on standard error; prints nothing when
/// this is the latest release or GitHub cannot be asked (offline, timeout, rate limit).
fn print_update_notice() {
    if let Some(line) = pleochroma_update::cli_notice("pleochroma-cli", env!("CARGO_PKG_VERSION")) {
        eprintln!("info: {line}");
    }
}
