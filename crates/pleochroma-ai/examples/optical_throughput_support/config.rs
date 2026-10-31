use serde::Serialize;
use std::{ffi::OsString, path::PathBuf};

pub const HELP: &str = "Native CUDA optical throughput and request-identity probe.
Run only one benchmark/trainer on the selected GPU at a time.

cargo run --release --manifest-path crates/pleochroma-ai/Cargo.toml --features cuda --example optical_throughput -- [OPTIONS]

  --output PATH          JSONL file, create_new; refuses an existing file (default stdout)
  --requests N           Requests per timed case, 1..1000000 (default 256)
  --repeats N            Warmed repetitions, 0..100 (default 2)
  --in-flight LIST       Unique comma-separated limits, each 1..1024 (default 1,8,32,128)
  --device N             Native CUDA device ordinal (default 0)
  --max-seconds SECONDS  Positive soft deadline per case, at most 86400 (default 300)
  --heartbeat-seconds N  Pending-work JSONL heartbeat interval, 1..5 (default 2)
  --help                 Print this help without initializing CUDA

A cold scalar case always runs first and supplies the same-GPU reference. Other first-wave
cases and warmed repetitions reuse that service. Timed cases use the same closed round
brilliant, sapphire material, Fast fidelity and IsoHemisphere lighting. A separately tagged
mixed-fixture audit verifies reply identity against two scalar GPU references. No CPU
optical fallback is allowed. Mathematical disagreement and infrastructure errors fail.

Every JSONL event is flushed. Events include Unix milliseconds and monotonic elapsed time
for alignment with external GPU telemetry. Capacity memory values are initialization-time
device readings, not a live memory trace. Batch maximum is explicitly lifetime-scoped.
The soft deadline cancels retained requests, but cannot interrupt an active CUDA driver call
or the service's shutdown join. Use a subprocess watchdog for a hard wall-clock deadline.";

#[derive(Debug, Serialize)]
pub struct Config {
    pub output: Option<PathBuf>,
    pub requests: usize,
    pub repeats: usize,
    pub in_flight: Vec<usize>,
    pub device: usize,
    pub max_seconds: f64,
    pub heartbeat_seconds: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            output: None,
            requests: 256,
            repeats: 2,
            in_flight: vec![1, 8, 32, 128],
            device: 0,
            max_seconds: 300.0,
            heartbeat_seconds: 2,
        }
    }
}

impl Config {
    pub fn parse(args: impl IntoIterator<Item = OsString>) -> Result<Option<Self>, String> {
        let mut config = Self::default();
        let mut args = args.into_iter();
        while let Some(flag) = args.next() {
            let flag = flag.to_str().ok_or("option names must be UTF-8")?;
            if flag == "--help" || flag == "-h" {
                return Ok(None);
            }
            let value = args
                .next()
                .ok_or_else(|| format!("missing value for {flag}"))?;
            if flag == "--output" {
                if value.is_empty() {
                    return Err("--output requires a path".into());
                }
                config.output = Some(PathBuf::from(value));
                continue;
            }
            let value = value
                .to_str()
                .ok_or_else(|| format!("{flag} requires a UTF-8 value"))?;
            match flag {
                "--requests" => config.requests = bounded(value, 1, 1_000_000, flag)?,
                "--repeats" => config.repeats = bounded(value, 0, 100, flag)?,
                "--device" => config.device = bounded(value, 0, usize::MAX, flag)?,
                "--heartbeat-seconds" => {
                    config.heartbeat_seconds = bounded(value, 1, 5, flag)? as u64
                }
                "--max-seconds" => {
                    config.max_seconds = value.parse().map_err(|_| "invalid --max-seconds")?;
                    if !config.max_seconds.is_finite()
                        || config.max_seconds <= 0.0
                        || config.max_seconds > 86_400.0
                    {
                        return Err("--max-seconds must be finite and in (0, 86400]".into());
                    }
                }
                "--in-flight" => {
                    config.in_flight.clear();
                    for part in value.split(',') {
                        let count = bounded(part, 1, 1024, flag)?;
                        if config.in_flight.contains(&count) {
                            return Err("duplicate --in-flight limit".into());
                        }
                        config.in_flight.push(count);
                    }
                }
                _ => return Err(format!("unknown argument {flag}")),
            }
        }
        Ok(Some(config))
    }
}

fn bounded(text: &str, minimum: usize, maximum: usize, flag: &str) -> Result<usize, String> {
    let value = text
        .parse::<usize>()
        .map_err(|_| format!("invalid integer for {flag}: {text}"))?;
    if !(minimum..=maximum).contains(&value) {
        return Err(format!("{flag} must be in {minimum}..={maximum}"));
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(args: &[&str]) -> Result<Option<Config>, String> {
        Config::parse(args.iter().map(|arg| OsString::from(*arg)))
    }
    #[test]
    fn defaults_and_collector_flags_are_stable() {
        let defaults = parse(&[]).unwrap().unwrap();
        assert_eq!((defaults.requests, defaults.repeats), (256, 2));
        assert_eq!(defaults.in_flight, [1, 8, 32, 128]);
        let quick = parse(&[
            "--requests",
            "128",
            "--repeats",
            "1",
            "--output",
            "fresh.jsonl",
        ])
        .unwrap()
        .unwrap();
        assert_eq!((quick.requests, quick.repeats), (128, 1));
        assert_eq!(quick.output, Some(PathBuf::from("fresh.jsonl")));
    }
    #[test]
    fn invalid_or_unbounded_work_is_refused() {
        for args in [
            vec!["--requests", "0"],
            vec!["--in-flight", "1,1"],
            vec!["--in-flight", "1025"],
            vec!["--in-flight", ""],
            vec!["--max-seconds", "NaN"],
            vec!["--max-seconds", "0"],
            vec!["--heartbeat-seconds", "6"],
            vec!["--requests"],
            vec!["--unknown", "1"],
        ] {
            assert!(parse(&args).is_err(), "{args:?}");
        }
        assert!(parse(&["--help"]).unwrap().is_none());
    }
}
