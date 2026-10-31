//! Direct native CUDA optical saturation probe, independent of projection/teacher work.
//! Run only one GPU benchmark/trainer at a time. See `--help` for output and timeout rules.
#[path = "optical_throughput_support/mod.rs"]
mod support;

use pleochroma_ai::gpu_optics::{CUDA_OPTICAL_PROVENANCE, CudaOpticalScorer};
use serde_json::json;
use std::{process::ExitCode, time::Instant};
use support::{CaseSpec, Config, Fixture, Score, Sink, run_case};

fn main() -> ExitCode {
    let config = match Config::parse(std::env::args_os().skip(1)) {
        Ok(Some(config)) => config,
        Ok(None) => {
            println!("{}", support::HELP);
            return ExitCode::SUCCESS;
        }
        Err(error) => {
            eprintln!("optical_throughput: {error}\nUse --help for arguments.");
            return ExitCode::from(2);
        }
    };
    let mut sink = match Sink::open(config.output.as_deref()) {
        Ok(sink) => sink,
        Err(error) => {
            eprintln!("optical_throughput: {error}");
            return ExitCode::FAILURE;
        }
    };
    match run(&config, &mut sink) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("optical_throughput failed: {error}");
            if let Err(write_error) = sink.event("error", json!({"message": error})) {
                eprintln!("cannot record error: {write_error}");
            }
            ExitCode::FAILURE
        }
    }
}

fn run(config: &Config, sink: &mut Sink) -> Result<(), String> {
    header(config, sink)?;
    let (scorer, fixtures) = initialize(config, sink)?;
    let mut reference: [Option<Score>; 2] = [None, None];
    let mut case_id = 0;
    // Only this case is service-cold. Later first-wave cases can grow native arenas,
    // but share the already initialized service; they are not mislabeled as cold.
    let cold = CaseSpec::new(case_id, "cold", 0, 1, config.requests, &[0]);
    run_case(&scorer, &fixtures, &mut reference, config, sink, &cold)?;
    sink.event(
        "scalar_reference",
        json!({"fixture_id": 0, "score": reference[0]}),
    )?;
    for &in_flight in config.in_flight.iter().filter(|&&value| value != 1) {
        case_id += 1;
        let case = CaseSpec::new(case_id, "first_wave", 0, in_flight, config.requests, &[0]);
        run_case(&scorer, &fixtures, &mut reference, config, sink, &case)?;
    }
    for repeat in 1..=config.repeats {
        for &in_flight in &config.in_flight {
            case_id += 1;
            let case = CaseSpec::new(case_id, "warmed", repeat, in_flight, config.requests, &[0]);
            run_case(&scorer, &fixtures, &mut reference, config, sink, &case)?;
        }
    }
    identity_audit(
        &scorer,
        &fixtures,
        &mut reference,
        config,
        sink,
        case_id + 1,
    )?;
    sink.event(
        "complete",
        json!({"status": "passed", "diagnostics": scorer.diagnostics()}),
    )
}

fn header(config: &Config, sink: &mut Sink) -> Result<(), String> {
    sink.event("run", json!({
        "schema_version": 1, "benchmark": "optical_throughput",
        "crate_version": env!("CARGO_PKG_VERSION"),
        "cuda_optical_provenance": CUDA_OPTICAL_PROVENANCE,
        "process_id": std::process::id(), "config": config,
        "origin_unix_ms": sink.origin_unix_ms(),
        "fidelity": "Fast", "lighting": "IsoHemisphere",
        "optical_percentage_order": ["windowing", "extinction", "tilt_brilliance"],
        "lch_order": ["lightness", "chroma", "hue_degrees"],
        "timed_fixture": "standard_round_brilliant/sapphire",
        "timing_scope": "owned preparation, queue admission, all completions, comparison and polling; excludes fixture/service initialization",
        "poll_backoff_microseconds": 50,
        "comparison": {
            "reference": "first scalar result on this CUDA service; not CPU parity certification",
            "exact_fields": ["optical percentages", "returned_fraction", "poses"],
            "lightness_chroma_absolute": 0.01, "hue_circular_degrees": 0.05,
            "srgb_codes": 1, "path_relative_absolute": 0.00001,
            "source": "crates/pleochroma-ai/src/gpu_optics/tests.rs::compare_tone"
        },
        "deadline": "soft per case; cancellation cannot interrupt an active CUDA driver call or shutdown join; use an external process watchdog",
        "max_batch_scope": "lifetime maximum; per-case mean uses exact counter deltas",
        "isolation": "run one GPU workload at a time"
    }))
}

fn initialize(
    config: &Config,
    sink: &mut Sink,
) -> Result<(CudaOpticalScorer, [Fixture; 2]), String> {
    let fixture_started = Instant::now();
    let fixtures = [Fixture::round()?, Fixture::emerald()?];
    sink.event(
        "fixture_initialization",
        json!({
            "wall_seconds": fixture_started.elapsed().as_secs_f64(),
            "fixtures": fixtures.iter().map(Fixture::description).collect::<Vec<_>>()
        }),
    )?;
    sink.event(
        "cuda_initialization_started",
        json!({"device_ordinal": config.device}),
    )?;
    eprintln!(
        "initializing native CUDA optics on device {}",
        config.device
    );
    let started = Instant::now();
    let scorer = match CudaOpticalScorer::new(config.device, 128) {
        Ok(scorer) => scorer,
        Err(error) => {
            sink.event(
                "cuda_initialization",
                json!({
                    "status": "failed", "wall_seconds": started.elapsed().as_secs_f64(),
                    "message": error.to_string()
                }),
            )?;
            return Err(error.to_string());
        }
    };
    sink.event(
        "cuda_initialization",
        json!({
            "status": "ready", "wall_seconds": started.elapsed().as_secs_f64(),
            "capacity": scorer.diagnostics().capacity
        }),
    )?;
    Ok((scorer, fixtures))
}

fn identity_audit(
    scorer: &CudaOpticalScorer,
    fixtures: &[Fixture],
    reference: &mut [Option<Score>],
    config: &Config,
    sink: &mut Sink,
    case_id: usize,
) -> Result<(), String> {
    let scalar = CaseSpec::new(case_id, "identity_reference", 0, 1, 1, &[1]);
    run_case(scorer, fixtures, reference, config, sink, &scalar)?;
    sink.event(
        "scalar_reference",
        json!({"fixture_id": 1, "score": reference[1]}),
    )?;
    let first = reference[0]
        .as_ref()
        .ok_or("missing round scalar reference")?;
    let second = reference[1]
        .as_ref()
        .ok_or("missing emerald scalar reference")?;
    if support::Agreement::default().observe(first, second).is_ok()
        || support::Agreement::default().observe(second, first).is_ok()
    {
        return Err("identity fixtures are indistinguishable under the comparison budgets".into());
    }
    let in_flight = config
        .in_flight
        .iter()
        .copied()
        .max()
        .unwrap_or(8)
        .clamp(8, 128);
    let audit = CaseSpec::new(
        case_id + 1,
        "identity_audit",
        0,
        in_flight,
        in_flight * 2,
        &[0, 1],
    );
    run_case(scorer, fixtures, reference, config, sink, &audit)
}
