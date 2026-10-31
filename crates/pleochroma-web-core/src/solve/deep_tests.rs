//! Deep Solve through the solve Worker's handler, and the wire bytes of its two variants.

use super::*;
use crate::{
    protocol::{
        FromWorker, PROTOCOL_VERSION, ToWorker, WorkerRole, decode_from_worker, decode_to_worker,
        encode_from_worker, encode_to_worker,
    },
    worker::{CancelProbe, WorkerHandler},
};
use pleochroma::geometry::{
    meet_solver::{
        MeetConstraint, SolveControl, SolvePhase, SolveProgress, VerifiedSolveReport,
        solve_meet_points_verified_with,
    },
    stone_metrics::ExternalProportions,
};
use pleochroma_editor::deep_solve::availability;
use std::cell::{Cell, RefCell};

fn hex(bytes: &[u8]) -> String {
    bytes
        .iter()
        .map(|b| format!("{b:02X}"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn printed() -> PrintedTargetsData {
    PrintedTargetsData {
        vol_w3: Some(0.62),
        lw: Some(1.0),
        ..PrintedTargetsData::default()
    }
}

fn params() -> DeepSolveParams {
    DeepSolveParams { targets: printed() }
}

fn request() -> SolveRequest {
    SolveRequest::DeepSolve { params: params() }
}

/// The concave fixture's five flat tiers with the first pavilion tier following a meet, so
/// Deep Solve has something to repair (a design from an `.asc` schedule has every tier
/// pinned).
fn concave_design() -> Design {
    let mut design = Design::concave_fixture();
    design.tiers[1].constraint = MeetConstraint::MeetExisting;
    design
}

fn flat_design() -> Design {
    let mut design = concave_design();
    design.concave_tiers.clear();
    design
}

fn deep_solved(response: SolveResponse) -> DeepSolveResultData {
    match response {
        SolveResponse::DeepSolved(data) => data,
        other => panic!("expected a Deep Solve answer, got {other:?}"),
    }
}

fn answer(design: &Design) -> SolveResponse {
    handle_solve(&design_to_toml(design).expect("encodes"), &request())
}

fn failure_of(response: SolveResponse) -> (String, bool) {
    match response {
        SolveResponse::AnalysisFailed {
            message,
            missing_anchor,
        } => (message, missing_anchor),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

// --- the wire ---

/// `SolveRequest::DeepSolve` is variant 8, the last: five `Option<f64>` (a tag byte and the
/// little-endian bits for each one that is present), no framing around the structs.
#[test]
fn the_deep_solve_request_bytes_are_pinned() {
    let request = SolveRequest::DeepSolve {
        params: DeepSolveParams {
            targets: PrintedTargetsData {
                vol_w3: Some(1.0),
                lw: Some(0.5),
                cw: None,
                pw: None,
                hw: Some(2.0),
            },
        },
    };
    let bytes = postcard::to_allocvec(&request).expect("encodes");
    assert_eq!(
        hex(&bytes),
        "08 01 00 00 00 00 00 00 F0 3F 01 00 00 00 00 00 00 E0 3F 00 00 \
         01 00 00 00 00 00 00 00 40"
    );
    let back: SolveRequest = postcard::from_bytes(&bytes).expect("decodes");
    assert_eq!(back, request);

    let message = ToWorker::Solve {
        job_id: 4,
        design_toml: String::new(),
        request,
    };
    let bytes = encode_to_worker(&message).expect("encodes");
    assert_eq!(decode_to_worker(&bytes).expect("decodes"), message);
}

/// `SolveResponse::DeepSolved` is variant 9, the last: the solved tiers (a length, then
/// mast, strategy, detail each), the baseline, then the report -- three `f64`, the accepted
/// flag and three `u32` varints (300 is `AC 02`).
#[test]
fn the_deep_solved_answer_bytes_are_pinned() {
    let response = SolveResponse::DeepSolved(DeepSolveResultData {
        solved: vec![SolvedTierData {
            mast: 1.0,
            strategy: StrategyData::ScaleReference,
            detail: "x".to_owned(),
        }],
        baseline: Vec::new(),
        report: VerifiedReportData {
            initial_score: 0.5,
            score_after_calibration: 0.25,
            final_score: 0.125,
            accepted: true,
            overrides_applied: 2,
            anchor_moves_applied: 1,
            pipeline_runs: 300,
        },
    });
    let bytes = postcard::to_allocvec(&response).expect("encodes");
    assert_eq!(
        hex(&bytes),
        "09 01 00 00 00 00 00 00 F0 3F 00 01 78 00 \
         00 00 00 00 00 00 E0 3F 00 00 00 00 00 00 D0 3F 00 00 00 00 00 00 C0 3F \
         01 02 01 AC 02"
    );
    let back: SolveResponse = postcard::from_bytes(&bytes).expect("decodes");
    assert_eq!(back, response);

    let message = FromWorker::SolveResult {
        job_id: 4,
        response,
        elapsed_ms: 1500.0,
    };
    let bytes = encode_from_worker(&message).expect("encodes");
    assert_eq!(decode_from_worker(&bytes).expect("decodes"), message);
}

#[test]
fn the_wire_mirrors_convert_back_losslessly() {
    let external = ExternalProportions {
        vol_w3: Some(0.62),
        lw: None,
        cw: Some(0.3),
        pw: None,
        hw: Some(0.9),
    };
    let back = PrintedTargetsData::from(&external).to_external();
    assert_eq!(
        (back.vol_w3, back.lw, back.cw, back.pw, back.hw),
        (
            external.vol_w3,
            external.lw,
            external.cw,
            external.pw,
            external.hw
        )
    );

    // An unmeasurable plain solve scores INFINITY; it must survive the wire.
    let report = VerifiedSolveReport {
        initial_score: f64::INFINITY,
        score_after_calibration: f64::INFINITY,
        final_score: 0.004,
        accepted: true,
        overrides_applied: 3,
        anchor_moves_applied: 0,
        pipeline_runs: 41,
    };
    let data = VerifiedReportData::from(&report);
    let bytes = postcard::to_allocvec(&data).expect("encodes");
    let decoded: VerifiedReportData = postcard::from_bytes(&bytes).expect("decodes");
    let back = decoded.to_report();
    assert_eq!(back.initial_score.to_bits(), report.initial_score.to_bits());
    assert_eq!(back.final_score.to_bits(), report.final_score.to_bits());
    assert_eq!(
        (
            back.accepted,
            back.overrides_applied,
            back.anchor_moves_applied,
            back.pipeline_runs
        ),
        (true, 3, 0, 41)
    );
}

#[test]
fn deep_solve_streams_progress_and_stops_with_a_cancelled_answer() {
    let request = request();
    assert!(request.streams_progress());
    assert!(request.supports_graceful_cancel());
}

// --- the handler ---

#[test]
fn a_design_the_availability_gate_refuses_is_declined_with_its_hint() {
    let design = flat_design();
    for targets in [
        PrintedTargetsData::default(),
        PrintedTargetsData {
            vol_w3: Some(0.0),
            ..PrintedTargetsData::default()
        },
        PrintedTargetsData {
            lw: Some(f64::NAN),
            ..PrintedTargetsData::default()
        },
    ] {
        let response = handle_solve(
            &design_to_toml(&design).expect("encodes"),
            &SolveRequest::DeepSolve {
                params: DeepSolveParams { targets },
            },
        );
        let (message, missing_anchor) = failure_of(response);
        let (available, hint) = availability(&design, Some(&targets.to_external()));
        assert!(!available);
        assert_eq!(message, hint);
        assert!(!missing_anchor);
    }
}

#[test]
fn a_design_with_every_tier_pinned_is_declined() {
    let mut design = flat_design();
    for tier in &mut design.tiers {
        tier.constraint = MeetConstraint::ScaleReference(0.5);
    }
    let (message, _) = failure_of(answer(&design));
    assert!(message.contains("pinned"), "{message}");
}

#[test]
fn a_set_cancel_flag_cancels_before_the_search() {
    let response = run_deep_solve(&flat_design(), &params(), &AtomicBool::new(true), &|_| {});
    assert_eq!(response, SolveResponse::Cancelled);
}

#[test]
fn a_cancel_raised_by_a_progress_report_ends_the_run_without_a_result() {
    let cancel = AtomicBool::new(false);
    let reports = Cell::new(0_usize);
    let response = run_deep_solve(&flat_design(), &params(), &cancel, &|_| {
        reports.set(reports.get() + 1);
        cancel.store(true, Ordering::Relaxed);
    });
    assert_eq!(response, SolveResponse::Cancelled);
    assert!(reports.get() >= 1, "the search must report at a checkpoint");
}

#[test]
fn bad_toml_is_reported_as_an_invalid_design() {
    assert!(matches!(
        handle_solve("this is not a design", &request()),
        SolveResponse::InvalidDesign { .. }
    ));
}

/// The Worker's answer is the plain solve plus the same verified search a direct call makes
/// (no adjustable anchors), tier for tier, and the run is deterministic.
#[test]
fn a_small_design_answers_with_its_baseline_and_the_verified_search() {
    let design = flat_design();
    let data = deep_solved(answer(&design));

    let direct_baseline = design.solve().expect("the fixture solves");
    assert_eq!(data.baseline.len(), direct_baseline.len());
    for (got, want) in data.baseline.iter().zip(&direct_baseline) {
        assert_eq!(got.mast.to_bits(), want.mast.to_bits());
    }

    let tiers = design.resolved_meet_tier_inputs().expect("resolves");
    let (direct, direct_report) = solve_meet_points_verified_with(
        design.meta.gear_teeth_abs(),
        &tiers,
        &printed().to_external(),
        &[],
        &SolveControl::default(),
    )
    .expect("the fixture verifies");
    assert_eq!(data.solved.len(), design.tiers.len());
    for (got, want) in data.solved.iter().zip(&direct) {
        assert_eq!(got.mast.to_bits(), want.mast.to_bits());
        assert_eq!(SolveStrategy::from(got.strategy), want.strategy);
    }
    assert_eq!(data.report, VerifiedReportData::from(&direct_report));
    assert!(data.report.pipeline_runs >= 1);

    assert_eq!(data, deep_solved(answer(&design)));
}

#[test]
fn a_running_search_reports_progress_and_still_finishes() {
    let design = flat_design();
    let reports = Cell::new(0_usize);
    let response = run_deep_solve(&design, &params(), &AtomicBool::new(false), &|_| {
        reports.set(reports.get() + 1);
    });
    assert!(reports.get() >= 1);
    assert_eq!(deep_solved(response), deep_solved(answer(&design)));
}

/// A concave design is verified on its flat stone (C1): the answer is the flat design's.
#[test]
fn a_concave_design_is_verified_on_its_flat_stone() {
    let concave = deep_solved(answer(&concave_design()));
    assert_eq!(concave, deep_solved(answer(&flat_design())));
}

// --- through the Worker state machine ---

/// A solve-role Worker that has been initialised.
fn solve_worker() -> WorkerHandler {
    let mut handler = WorkerHandler::new();
    let reply = handler.handle(
        ToWorker::Init {
            protocol_version: PROTOCOL_VERSION,
            role: WorkerRole::Solve,
            worker_index: 1,
        },
        &|| 0.0,
    );
    assert!(matches!(reply, Some(FromWorker::Ready { .. })), "{reply:?}");
    handler
}

fn deep_job(job_id: u64, design: &Design) -> ToWorker {
    ToWorker::Solve {
        job_id,
        design_toml: design_to_toml(design).expect("encodes"),
        request: request(),
    }
}

/// A clock 300 ms later at every reading, so every progress report is due.
fn slow_clock() -> impl Fn() -> f64 {
    let now = Cell::new(0.0);
    move || {
        now.set(now.get() + 300.0);
        now.get()
    }
}

#[test]
fn the_worker_streams_phase_text_without_a_fraction_and_answers_the_result() {
    let mut worker = solve_worker();
    let sent = RefCell::new(Vec::new());
    let reply = worker.handle_probed(
        deep_job(1, &flat_design()),
        &slow_clock(),
        &|message| sent.borrow_mut().push(message),
        &|_| CancelProbe::Live,
    );
    let Some(FromWorker::SolveResult {
        job_id, response, ..
    }) = reply
    else {
        panic!("expected the Deep Solve result, got {reply:?}");
    };
    assert_eq!(job_id, 1);
    assert_eq!(deep_solved(response), deep_solved(answer(&flat_design())));

    let sent = sent.into_inner();
    assert!(!sent.is_empty(), "the search must post progress");
    for message in &sent {
        let FromWorker::Progress {
            job_id,
            message,
            fraction,
        } = message
        else {
            panic!("only progress is posted before the result, got {message:?}");
        };
        assert_eq!(*job_id, 1);
        assert!(message.starts_with("Deep Solve..."), "{message}");
        assert_eq!(*fraction, None, "the search has no honest fraction");
    }
}

#[test]
fn a_revoked_cancel_url_ends_the_worker_run_with_a_cancelled_answer() {
    let mut worker = solve_worker();
    let clock = slow_clock();
    assert_eq!(
        worker.handle(
            ToWorker::WatchCancel {
                job_id: 1,
                url: "blob:deep".to_owned(),
            },
            &clock,
        ),
        None
    );
    let reply = worker.handle_probed(deep_job(1, &flat_design()), &clock, &|_| {}, &|_| {
        CancelProbe::Revoked
    });
    let Some(FromWorker::SolveResult { response, .. }) = reply else {
        panic!("expected the job's result, got {reply:?}");
    };
    assert_eq!(response, SolveResponse::Cancelled);
}

#[test]
fn a_job_cancelled_while_queued_is_skipped() {
    let mut worker = solve_worker();
    let clock = slow_clock();
    assert_eq!(worker.handle(ToWorker::Cancel { job_id: 2 }, &clock), None);
    let reply = worker.handle(deep_job(2, &flat_design()), &clock);
    let Some(FromWorker::SolveResult { response, .. }) = reply else {
        panic!("expected the job's result, got {reply:?}");
    };
    assert_eq!(response, SolveResponse::Cancelled);
}

#[test]
fn the_progress_text_names_the_phase_and_the_time() {
    let report = |phase| SolveProgress {
        phase,
        sweep: 2,
        max_sweeps: 4,
        blocks_done: 3,
        blocks_total: 9,
    };
    let refine = deep_progress_status(report(SolvePhase::Refine), 12.34);
    assert!(refine.starts_with("Deep Solve..."), "{refine}");
    assert!(refine.contains("refining, sweep 2 of up to 4"), "{refine}");
    assert!(refine.ends_with("12.3s elapsed"), "{refine}");
    assert!(
        deep_progress_status(report(SolvePhase::Constructive), 0.0)
            .contains("placing tiers, 3 of 9")
    );
    assert!(
        deep_progress_status(report(SolvePhase::LeastSquares), 0.0)
            .contains("estimating unsettled blocks")
    );
}
