//! Bounded, observable production-pipeline diagnostics.
//!
//! Never evaluates held-out episodes or reads/writes a checkpoint. The database loader opens
//! the source library read-only.

pub mod dataset;
#[path = "runner.rs"]
mod runner;
pub mod stats;
mod throughput_args;
mod throughput_driver;
mod throughput_output;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<_> = std::env::args().skip(1).collect();
    match throughput_args::parse(&args) {
        Ok(None) => {
            print!("{}", throughput_args::HELP);
            ExitCode::SUCCESS
        }
        Ok(Some(args)) => match throughput_driver::run(&args) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("throughput probe failed: {error}");
                ExitCode::FAILURE
            }
        },
        Err(error) => {
            eprintln!("{error}\n\n{}", throughput_args::HELP);
            ExitCode::from(2)
        }
    }
}
