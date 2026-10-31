//! Bounded direct ticket production; neither Rayon nor projection is involved.
use super::{Agreement, Config, Fixture, Score, Sink, diagnostics_delta};
use pleochroma::optics::raytracer::LightingPreset;
use pleochroma_ai::gpu_optics::{CudaOpticalScorer, OpticalDiagnostics, OpticalWork};
use pleochroma_cut_core::optimize::ObjectiveFidelity;
use serde::Serialize;
use serde_json::json;
use std::time::{Duration, Instant};

#[derive(Serialize)]
pub struct CaseSpec<'a> {
    case_id: usize,
    phase: &'static str,
    repeat: usize,
    offered_in_flight: usize,
    requests: usize,
    fixture_cycle: &'a [usize],
}

impl<'a> CaseSpec<'a> {
    pub const fn new(
        case_id: usize,
        phase: &'static str,
        repeat: usize,
        offered_in_flight: usize,
        requests: usize,
        fixture_cycle: &'a [usize],
    ) -> Self {
        Self {
            case_id,
            phase,
            repeat,
            offered_in_flight,
            requests,
            fixture_cycle,
        }
    }
}

struct Live {
    id: usize,
    fixture: usize,
    work: OpticalWork,
    submitted: bool,
}

struct State {
    live: Vec<Live>,
    seen: Vec<bool>,
    prepared: usize,
    submitted: usize,
    completed: usize,
    backpressure_attempts: u64,
    peak_owned_tickets: usize,
    reference_established: usize,
    agreement: Agreement,
    started: Instant,
    heartbeat: Instant,
}

impl State {
    fn new(case: &CaseSpec<'_>) -> Self {
        Self {
            live: Vec::with_capacity(case.offered_in_flight.min(case.requests)),
            seen: vec![false; case.requests],
            prepared: 0,
            submitted: 0,
            completed: 0,
            backpressure_attempts: 0,
            peak_owned_tickets: 0,
            reference_established: 0,
            agreement: Agreement::default(),
            started: Instant::now(),
            heartbeat: Instant::now(),
        }
    }

    fn fill(
        &mut self,
        scorer: &CudaOpticalScorer,
        fixtures: &[Fixture],
        case: &CaseSpec<'_>,
    ) -> Result<bool, String> {
        let before = self.prepared;
        while self.prepared < case.requests && self.live.len() < case.offered_in_flight {
            let fixture = case.fixture_cycle[self.prepared % case.fixture_cycle.len()];
            let input = &fixtures[fixture];
            let mut work = scorer
                .prepare_with_width(
                    input.stone(),
                    &input.material,
                    ObjectiveFidelity::Fast,
                    LightingPreset::IsoHemisphere,
                    input.half_width,
                )
                .map_err(|e| format!("prepare request {} fixture {fixture}: {e}", self.prepared))?;
            let submitted = scorer.try_submit(&mut work).map_err(|e| e.to_string())?;
            self.submitted += usize::from(submitted);
            self.backpressure_attempts += u64::from(!submitted);
            self.live.push(Live {
                id: self.prepared,
                fixture,
                work,
                submitted,
            });
            self.prepared += 1;
            self.peak_owned_tickets = self.peak_owned_tickets.max(self.live.len());
        }
        Ok(before != self.prepared)
    }

    fn poll(
        &mut self,
        scorer: &CudaOpticalScorer,
        reference: &mut [Option<Score>],
    ) -> Result<bool, String> {
        let mut progressed = false;
        let mut index = 0;
        while index < self.live.len() {
            let entry = &mut self.live[index];
            if !entry.submitted {
                entry.submitted = scorer
                    .try_submit(&mut entry.work)
                    .map_err(|e| e.to_string())?;
                self.submitted += usize::from(entry.submitted);
                self.backpressure_attempts += u64::from(!entry.submitted);
                progressed |= entry.submitted;
            }
            if !entry.submitted || !entry.work.poll() {
                index += 1;
                continue;
            }
            // Swap removal deliberately permits out-of-order polling/completion.
            // The retained request identity chooses its scalar fixture reference.
            let entry = self.live.swap_remove(index);
            let result = entry
                .work
                .complete()
                .map_err(|e| format!("request {} fixture {}: {e}", entry.id, entry.fixture))?;
            let actual = Score::from(&result);
            if !actual.finite() {
                return Err(format!(
                    "request {} returned an invalid Fast score",
                    entry.id
                ));
            }
            if self.seen[entry.id] {
                return Err(format!("duplicate completion for request {}", entry.id));
            }
            if let Some(expected) = &reference[entry.fixture] {
                self.agreement
                    .observe(expected, &actual)
                    .map_err(|e| format!("request {} fixture {}: {e}", entry.id, entry.fixture))?;
            } else {
                reference[entry.fixture] = Some(actual);
                self.reference_established += 1;
            }
            self.seen[entry.id] = true;
            self.completed += 1;
            progressed = true;
        }
        Ok(progressed)
    }

    fn progress(&self, case: &CaseSpec<'_>, diagnostics: &OpticalDiagnostics) -> serde_json::Value {
        json!({"case": case, "wall_seconds": self.started.elapsed().as_secs_f64(),
            "prepared": self.prepared, "submitted": self.submitted, "completed": self.completed,
            "owned_tickets": self.live.len(), "backpressure_attempts": self.backpressure_attempts,
            "queued_requests": diagnostics.queued_requests, "active_requests": diagnostics.active_requests,
            "first_error": diagnostics.first_error})
    }
}

pub fn run_case(
    scorer: &CudaOpticalScorer,
    fixtures: &[Fixture],
    reference: &mut [Option<Score>],
    config: &Config,
    sink: &mut Sink,
    case: &CaseSpec<'_>,
) -> Result<(), String> {
    match execute(scorer, fixtures, reference, config, sink, case) {
        Ok(()) => Ok(()),
        Err(error) => {
            // Flush before the owning scorer joins its native worker on shutdown.
            sink.event(
                "case_error",
                json!({"case": case, "message": error,
                "diagnostics": scorer.diagnostics()}),
            )?;
            Err(format!("case {} ({}): {error}", case.case_id, case.phase))
        }
    }
}

fn execute(
    scorer: &CudaOpticalScorer,
    fixtures: &[Fixture],
    reference: &mut [Option<Score>],
    config: &Config,
    sink: &mut Sink,
    case: &CaseSpec<'_>,
) -> Result<(), String> {
    let before = scorer.diagnostics();
    if before.queued_requests != 0 || before.active_requests != 0 {
        return Err("previous case left native work pending".into());
    }
    sink.event(
        "case_started",
        json!({"case": case, "service_cold": case.phase == "cold",
        "effective_in_flight_limit": case.offered_in_flight.min(case.requests)}),
    )?;
    eprintln!(
        "case {}: {} repeat {} in-flight {} requests {}",
        case.case_id, case.phase, case.repeat, case.offered_in_flight, case.requests
    );
    let mut state = State::new(case);
    while state.completed < case.requests {
        if state.started.elapsed().as_secs_f64() >= config.max_seconds {
            sink.event("deadline", state.progress(case, &scorer.diagnostics()))?;
            return Err(format!(
                "soft deadline of {} seconds exceeded; retained tickets cancelled; active CUDA calls may delay shutdown",
                config.max_seconds
            ));
        }
        let mut progressed = state.fill(scorer, fixtures, case)?;
        progressed |= state.poll(scorer, reference)?;
        if state.heartbeat.elapsed() >= Duration::from_secs(config.heartbeat_seconds) {
            let diagnostics = scorer.diagnostics();
            sink.event("heartbeat", state.progress(case, &diagnostics))?;
            eprintln!(
                "case {}: {}/{} complete, {} tickets, native queued {} active {}",
                case.case_id,
                state.completed,
                case.requests,
                state.live.len(),
                diagnostics.queued_requests,
                diagnostics.active_requests
            );
            if let Some(error) = diagnostics.first_error {
                return Err(error);
            }
            state.heartbeat = Instant::now();
        }
        if !progressed {
            std::thread::sleep(Duration::from_micros(50));
        }
    }
    let wall_seconds = state.started.elapsed().as_secs_f64();
    let after = scorer.diagnostics();
    if let Some(error) = &after.first_error {
        return Err(error.clone());
    }
    if after.completed_requests - before.completed_requests != case.requests as u64
        || after.fast_batches == before.fast_batches
        || !state.seen.iter().all(|seen| *seen)
        || after.queued_requests != 0
        || after.active_requests != 0
    {
        return Err(
            "native completion counters or request identities do not match this case".into(),
        );
    }
    let rate = case.requests as f64 / wall_seconds;
    sink.event("case", json!({
        "case": case, "wall_seconds": wall_seconds, "requests_per_second": rate,
        "completed_requests": state.completed, "peak_owned_tickets": state.peak_owned_tickets,
        "backpressure_attempts": state.backpressure_attempts,
        "references_established": state.reference_established, "agreement": state.agreement,
        "request_identities_complete": true, "diagnostic_delta": diagnostics_delta(&before, &after)
    }))?;
    eprintln!(
        "case {} complete: {:.3}s, {:.1} requests/s",
        case.case_id, wall_seconds, rate
    );
    Ok(())
}
