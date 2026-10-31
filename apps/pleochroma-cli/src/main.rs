//! The `pleochroma-cli` program: runs the command line given to it and exits with its code.
//!
//! Everything else is in the library (`pleochroma_cli`): this only collects the arguments,
//! names a newer release on standard error first (`pleochroma_cli::take_update_check`), prints
//! the text a command returned and turns its exit code into the process's.

use std::io::Write;

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
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
