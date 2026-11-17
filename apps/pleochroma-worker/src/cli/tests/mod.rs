//! Parser tests for `crate::cli`, one file per subcommand family.
//!
//! - [`help`]: `--help` resolution and unknown subcommands;
//! - [`render`]: the one-shot `render` subcommand;
//! - [`serve`]: the coordinator (`serve`) flags;
//! - [`cert`]: the `cert` sub-subcommands;
//! - [`join`]: the `join` subcommand;
//! - [`global`]: `--headless`, `--minimized`, `--config` and the launch decision.

mod cert;
mod global;
mod help;
mod join;
mod render;
mod serve;
