//! Opt-in wall-clock timing of the GPU frame path: where the fixed cost of a pass goes.
//!
//! Off unless the environment variable [`ENV_VAR`] (`PLEOCHROMA_GPU_TIMING`) is set to
//! anything but empty or `0`; read once per process. Disabled, every probe in the frame path is one
//! predictable branch on a cached `bool` (no clock read, no lock, no allocation) and nothing
//! it measures feeds back into scheduling, chunk sizes or numerics.
//!
//! # What is measured
//!
//! Per CHUNK (one `tracing` line on the `pleochroma_cut::render_bench` target, message
//! prefix `gpu_timing chunk`): the CPU time of `GpuFrameRenderer::dispatch_chunk` split into the
//! `queue.write_buffer` uploads, the three `create_bind_group`/encoder-`finish`/`queue.submit`
//! triples (transport, reduce, readback copy) and the `map_async` call; then the drain:
//! the blocking `device.poll` (the GPU wait), the map callback receive, the mapped copy and
//! the CPU accumulation loop; plus the chunk's submit-to-GPU-done wall time and the GPU idle gap
//! that preceded its submission.
//!
//! Per TURN (`gpu_timing turn`): wall time, turnstile wait, `prepare_turn` (scene
//! preparation, of which `FrameSceneBuffers::ensure` is the scene re-upload) and the final
//! drain of the turn (`run_turn_chunks`' tail).
//!
//! Per PASS (`gpu_timing pass`, one `GpuBackend` request): the totals, the scratch zero-fill
//! and the scratch add, and the derived percentages the decision rules of the perf review's
//! `MEASURE_GPU.md` read. [`take_last_pass_summary`] hands the same numbers to a caller that
//! wants them in a JSON report (`live_render_bench` does).
//!
//! All state is process-global: read it with ONE GPU request in flight at a time (the
//! benchmark does). Several concurrent requests interleave their chunks and the per-pass
//! totals mix.

use std::{
    cell::Cell,
    sync::{Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};

#[cfg(not(target_arch = "wasm32"))]
use std::sync::OnceLock;

/// The environment variable that turns the timing log on (anything but empty or `0`).
pub const ENV_VAR: &str = "PLEOCHROMA_GPU_TIMING";

/// Whether timing is on. Read once; the disabled path of every probe is this load plus a
/// branch.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn enabled() -> bool {
    static ENABLED: OnceLock<bool> = OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var_os(ENV_VAR).is_some_and(|v| !v.is_empty() && v != "0"))
}

/// The browser has no environment and no `std::time::Instant`: never on.
#[cfg(target_arch = "wasm32")]
pub(crate) const fn enabled() -> bool {
    false
}

/// Per-chunk phases (CPU or blocked time on the renderer's thread). Dispatch phases are
/// recorded while the chunk is submitted, drain phases while it is read back (one chunk
/// later); [`ChunkTiming`] carries the first group across.
#[derive(Clone, Copy)]
pub(crate) enum Slot {
    /// `queue.write_buffer` of the chunk's `GpuTransportParams`.
    ParamsWrite,
    /// `create_bind_group` for the transport dispatch.
    TransportBind,
    /// Encoder creation, compute pass recording and `finish` of the transport dispatch.
    TransportEncode,
    /// `queue.submit` of the transport dispatch.
    TransportSubmit,
    /// `queue.write_buffer` of the reduce params.
    ReduceWrite,
    /// `create_bind_group` for the reduce dispatch.
    ReduceBind,
    /// Encoder creation, pass recording and `finish` of the reduce dispatch.
    ReduceEncode,
    /// `queue.submit` of the reduce dispatch.
    ReduceSubmit,
    /// Encoder creation, `copy_buffer_to_buffer` and `finish` of the readback copy.
    CopyEncode,
    /// `queue.submit` of the readback copy.
    CopySubmit,
    /// `map_async` on the staging buffer.
    MapBegin,
    /// The whole of `dispatch_chunk` (contains every dispatch phase above and the rest).
    DispatchTotal,
    /// The blocking `device.poll(Wait)` on the chunk's copy submission: the GPU wait.
    PollWait,
    /// Receiving the `map_async` callback result after the poll.
    MapRecv,
    /// `get_mapped_range`, the prefix copy into the scratch `Vec` and `unmap`.
    MapCopy,
    /// The CPU loop adding the chunk's pixels into the accumulation buffer.
    Accumulate,
}

/// Number of [`Slot`] variants.
const SLOTS: usize = Slot::Accumulate as usize + 1;

/// Per-turn phases, recorded straight into the global state.
#[derive(Clone, Copy)]
pub(crate) enum TurnSlot {
    /// Ticket, turnstile wait and renderer lock before the turn (lands in the NEXT turn
    /// line to be printed, i.e. the turn it admitted).
    TurnstileWait,
    /// `prepare_turn` as a whole (pack, key hash, capacity checks, scene upload).
    Prepare,
    /// `FrameSceneBuffers::ensure`: the four-buffer scene re-upload (inside `Prepare`).
    SceneUpload,
    /// The final drain of the turn: the one pending chunk drained after the last dispatch.
    TailDrain,
}

/// Number of [`TurnSlot`] variants.
const TURN_SLOTS: usize = TurnSlot::TailDrain as usize + 1;

/// Per-request phases of `GpuBackend::accumulate_geom_surface` outside the turns.
#[derive(Clone, Copy)]
pub(crate) enum PassSlot {
    /// Taking and zero-filling the scratch buffer.
    ScratchTake,
    /// Adding the scratch buffer into the caller's accumulation buffer.
    ScratchAdd,
}

/// Number of [`PassSlot`] variants.
const PASS_SLOTS: usize = PassSlot::ScratchAdd as usize + 1;

/// Chunk sizes a [`PassSummary`] lists at most.
const MAX_LOGGED_CHUNKS: usize = 4096;

/// A running clock for one probe; `None` (no clock read at all) when timing is off.
pub(crate) struct Mark(Option<Instant>);

impl Mark {
    /// Starts a clock, unless timing is off.
    pub(crate) fn now() -> Self {
        Self(enabled().then(Instant::now))
    }

    /// The instant this clock started (or last lapped); `None` when timing is off.
    pub(crate) const fn instant(&self) -> Option<Instant> {
        self.0
    }

    /// Adds the time since the last lap to the chunk phase `slot` and restarts the clock.
    pub(crate) fn lap(&mut self, slot: Slot) {
        if let Some(ns) = self.restart() {
            CHUNK.with(|cell| {
                let mut slots = cell.get();
                slots[slot as usize] = slots[slot as usize].saturating_add(ns);
                cell.set(slots);
            });
        }
    }

    /// Adds the time since the last lap to the turn phase `slot` and restarts the clock.
    pub(crate) fn lap_turn(&mut self, slot: TurnSlot) {
        if let Some(ns) = self.restart() {
            let mut state = lock_state();
            state.pass.turn_ns[slot as usize] += ns;
            state.turn.turn_ns[slot as usize] += ns;
        }
    }

    /// Adds the time since the last lap to the request phase `slot` and restarts the clock.
    pub(crate) fn lap_pass(&mut self, slot: PassSlot) {
        if let Some(ns) = self.restart() {
            lock_state().pass_ns[slot as usize] += ns;
        }
    }

    fn restart(&mut self) -> Option<u64> {
        let start = self.0.as_mut()?;
        let now = Instant::now();
        let ns = nanos(now.saturating_duration_since(*start));
        *start = now;
        Some(ns)
    }
}

fn nanos(duration: Duration) -> u64 {
    u64::try_from(duration.as_nanos()).unwrap_or(u64::MAX)
}

fn count(n: usize) -> u64 {
    u64::try_from(n).unwrap_or(u64::MAX)
}

const fn micros(ns: u64) -> u64 {
    ns / 1_000
}

const fn millis(ns: u64) -> f64 {
    ns as f64 / 1.0e6
}

thread_local! {
    /// The chunk phases of the chunk being dispatched or drained on this thread. Taken
    /// (and zeroed) at the end of `dispatch_chunk` and at the end of the drain, so the two
    /// halves of one chunk never mix with its neighbours'.
    static CHUNK: Cell<[u64; SLOTS]> = const { Cell::new([0; SLOTS]) };
}

fn take_chunk_slots() -> [u64; SLOTS] {
    CHUNK.with(|cell| cell.replace([0; SLOTS]))
}

/// Discards stray chunk phases (a failed dispatch or drain leaves its half behind).
pub(crate) fn reset_chunk() {
    if enabled() {
        let _ = take_chunk_slots();
    }
}

/// The dispatch half of one chunk's timing, carried from `dispatch_chunk` to the drain in
/// `PendingChunk`.
#[derive(Clone, Copy)]
pub(crate) struct ChunkTiming {
    slots: [u64; SLOTS],
    chunk_index: usize,
    pixels: usize,
    tuples: usize,
}

/// Takes the dispatch phases of the chunk just submitted; `None` when timing is off.
pub(crate) fn take_dispatch(
    chunk_index: usize,
    pixels: usize,
    tuples: usize,
) -> Option<ChunkTiming> {
    enabled().then(|| ChunkTiming {
        slots: take_chunk_slots(),
        chunk_index,
        pixels,
        tuples,
    })
}

/// Sums and counts of a set of chunks (a turn or a whole request).
#[derive(Clone, Copy)]
struct Totals {
    chunks: u64,
    tuples: u64,
    pixels: u64,
    chunk_ns: [u64; SLOTS],
    turn_ns: [u64; TURN_SLOTS],
    /// Submit-to-poll-return wall time, summed over chunks (overlapping chunks overlap here).
    gpu_wall_ns: u64,
    /// Submit-to-drained wall time, the clock `ns_per_tuple_ema` is fed from.
    cycle_ns: u64,
    /// GPU idle time before submissions (see [`State::record_chunk`]).
    idle_ns: u64,
}

impl Totals {
    const EMPTY: Self = Self {
        chunks: 0,
        tuples: 0,
        pixels: 0,
        chunk_ns: [0; SLOTS],
        turn_ns: [0; TURN_SLOTS],
        gpu_wall_ns: 0,
        cycle_ns: 0,
        idle_ns: 0,
    };

    const fn slot(&self, slot: Slot) -> u64 {
        self.chunk_ns[slot as usize]
    }

    const fn turn_slot(&self, slot: TurnSlot) -> u64 {
        self.turn_ns[slot as usize]
    }
}

struct State {
    pass_began: Option<Instant>,
    turn_began: Option<Instant>,
    /// When the GPU last finished a chunk, as the CPU saw it (poll return).
    last_done: Option<Instant>,
    pass: Totals,
    turn: Totals,
    pass_ns: [u64; PASS_SLOTS],
    /// Idle time before the pass's first chunk (turnstile, scene preparation, first encode).
    startup_ns: u64,
    turn_wall_ns: u64,
    turns: u32,
    chunk_pixels: Vec<u64>,
    last: Option<PassSummary>,
}

impl State {
    const EMPTY: Self = Self {
        pass_began: None,
        turn_began: None,
        last_done: None,
        pass: Totals::EMPTY,
        turn: Totals::EMPTY,
        pass_ns: [0; PASS_SLOTS],
        startup_ns: 0,
        turn_wall_ns: 0,
        turns: 0,
        chunk_pixels: Vec::new(),
        last: None,
    };

    /// Books one drained chunk and returns `(idle_before_ns, gpu_wall_ns)`.
    ///
    /// `idle_before` is how long the GPU certainly had nothing queued before this chunk's
    /// submission: the time from the previous chunk's observed completion (or the request's
    /// start, for its first chunk) to this chunk's submission, zero when the chunk was
    /// queued while the previous one still ran. It is a LOWER bound (the CPU may notice a
    /// completion late), and it is exactly the pipeline-fill, turn-boundary and
    /// startup cost a fixed per-pass overhead is made of.
    fn record_chunk(
        &mut self,
        slots: &[u64; SLOTS],
        shape: (usize, usize),
        times: (Instant, Instant),
        cycle: Duration,
    ) -> (u64, u64) {
        let (submitted_at, poll_done) = times;
        let idle_before = self.last_done.or(self.pass_began).map_or(0, |since| {
            nanos(submitted_at.saturating_duration_since(since))
        });
        let gpu_wall = nanos(poll_done.saturating_duration_since(submitted_at));
        if self.pass.chunks == 0 {
            self.startup_ns = idle_before;
        } else {
            self.pass.idle_ns += idle_before;
        }
        self.turn.idle_ns += idle_before;
        self.last_done = Some(self.last_done.map_or(poll_done, |done| done.max(poll_done)));

        let (pixels, tuples) = (count(shape.0), count(shape.1));
        for totals in [&mut self.pass, &mut self.turn] {
            totals.chunks += 1;
            totals.pixels += pixels;
            totals.tuples += tuples;
            totals.gpu_wall_ns += gpu_wall;
            totals.cycle_ns += nanos(cycle);
            for (total, value) in totals.chunk_ns.iter_mut().zip(slots) {
                *total += value;
            }
        }
        // Bounded: a long batch outside a request (no `begin_pass`) must not grow it forever.
        if self.chunk_pixels.len() < MAX_LOGGED_CHUNKS {
            self.chunk_pixels.push(pixels);
        }
        (idle_before, gpu_wall)
    }
}

static STATE: Mutex<State> = Mutex::new(State::EMPTY);

fn lock_state() -> MutexGuard<'static, State> {
    STATE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Starts a request: clears every counter. Call at the top of `accumulate_geom_surface`.
pub(crate) fn begin_pass() {
    if enabled() {
        let mut state = lock_state();
        *state = State::EMPTY;
        state.pass_began = Some(Instant::now());
    }
}

/// Starts a turn (`accumulate_turn`).
pub(crate) fn begin_turn() {
    if enabled() {
        lock_state().turn_began = Some(Instant::now());
    }
}

/// Ends a turn: prints its line and clears the per-turn counters.
pub(crate) fn end_turn() {
    if !enabled() {
        return;
    }
    let mut state = lock_state();
    let wall = state
        .turn_began
        .take()
        .map_or(0, |began| nanos(began.elapsed()));
    state.turn_wall_ns += wall;
    let turn = state.turn;
    let index = state.turns;
    state.turns += 1;
    state.turn = Totals::EMPTY;
    drop(state);

    let wall_us = micros(wall);
    let chunks = turn.chunks;
    let tuples = turn.tuples;
    let wait_us = micros(turn.turn_slot(TurnSlot::TurnstileWait));
    let prepare_us = micros(turn.turn_slot(TurnSlot::Prepare));
    let upload_us = micros(turn.turn_slot(TurnSlot::SceneUpload));
    let tail_us = micros(turn.turn_slot(TurnSlot::TailDrain));
    let idle_us = micros(turn.idle_ns);
    let dispatch_us = micros(turn.slot(Slot::DispatchTotal));
    let gpu_wall_us = micros(turn.gpu_wall_ns);
    tracing::info!(
        target: "pleochroma_cut::render_bench",
        "gpu_timing turn turn={index} wall_us={wall_us} chunks={chunks} tuples={tuples} \
         turnstile_wait_us={wait_us} prepare_us={prepare_us} scene_upload_us={upload_us} \
         tail_drain_us={tail_us} gpu_idle_before_us={idle_us} dispatch_cpu_us={dispatch_us} \
         gpu_wall_sum_us={gpu_wall_us}"
    );
}

/// Books one drained chunk and prints its line. `pending` is the dispatch half
/// (`PendingChunk::timing`), `drain_began` the instant `drain_pending_chunk` started (from
/// its [`Mark`]), `ema_ns_per_tuple` the chunk-size EMA AFTER this chunk updated it.
pub(crate) fn finish_chunk(
    pending: Option<ChunkTiming>,
    submitted_at: Instant,
    drain_began: Option<Instant>,
    ema_ns_per_tuple: Option<f64>,
) {
    let (Some(pending), Some(began)) = (pending, drain_began) else {
        return;
    };
    let drain = take_chunk_slots();
    let mut slots = pending.slots;
    for (slot, value) in slots.iter_mut().zip(&drain) {
        *slot += value;
    }
    let poll_done = began + Duration::from_nanos(slots[Slot::PollWait as usize]);
    let cycle = submitted_at.elapsed();
    let drain_began_after = nanos(began.saturating_duration_since(submitted_at));
    let (turn, idle_before, gpu_wall) = {
        let mut state = lock_state();
        let turn = state.turns;
        let (idle, wall) = state.record_chunk(
            &slots,
            (pending.pixels, pending.tuples),
            (submitted_at, poll_done),
            cycle,
        );
        drop(state);
        (turn, idle, wall)
    };

    let us = |slot: Slot| micros(slots[slot as usize]);
    let (chunk, pixels, tuples) = (pending.chunk_index, pending.pixels, pending.tuples);
    let (gpu_wall_us, cycle_us, idle_us) =
        (micros(gpu_wall), micros(nanos(cycle)), micros(idle_before));
    let (began_us, ema) = (micros(drain_began_after), ema_ns_per_tuple.unwrap_or(0.0));
    let (dispatch, params, reduce_write) = (
        us(Slot::DispatchTotal),
        us(Slot::ParamsWrite),
        us(Slot::ReduceWrite),
    );
    let (t_bind, t_enc, t_sub) = (
        us(Slot::TransportBind),
        us(Slot::TransportEncode),
        us(Slot::TransportSubmit),
    );
    let (r_bind, r_enc, r_sub) = (
        us(Slot::ReduceBind),
        us(Slot::ReduceEncode),
        us(Slot::ReduceSubmit),
    );
    let (c_enc, c_sub, map_begin) = (
        us(Slot::CopyEncode),
        us(Slot::CopySubmit),
        us(Slot::MapBegin),
    );
    let (poll, recv, copy, acc) = (
        us(Slot::PollWait),
        us(Slot::MapRecv),
        us(Slot::MapCopy),
        us(Slot::Accumulate),
    );
    tracing::info!(
        target: "pleochroma_cut::render_bench",
        "gpu_timing chunk turn={turn} chunk={chunk} pixels={pixels} tuples={tuples} gpu_wall_us={gpu_wall_us} \
         cycle_us={cycle_us} gpu_idle_before_us={idle_us} ns_per_tuple_ema={ema:.1} \
         dispatch_cpu_us={dispatch} params_write_us={params} transport_bind_us={t_bind} \
         transport_encode_us={t_enc} transport_submit_us={t_sub} reduce_write_us={reduce_write} \
         reduce_bind_us={r_bind} reduce_encode_us={r_enc} reduce_submit_us={r_sub} \
         copy_encode_us={c_enc} copy_submit_us={c_sub} map_begin_us={map_begin} \
         drain_began_after_submit_us={began_us} poll_wait_us={poll} map_recv_us={recv} \
         map_copy_us={copy} accumulate_us={acc}"
    );
}

/// The totals of one `GpuBackend` request, in milliseconds unless named otherwise.
///
/// Times of different chunks overlap (the pipeline keeps one chunk queued behind the one being
/// read), so the `gpu_wall_ms`/`cycle_ms` sums can exceed `wall_ms`; the `p24_*` and
/// `h04_*` percentages are the ones the perf review's decision rules read.
#[derive(Clone, Debug, Default, serde::Serialize)]
pub struct PassSummary {
    /// The request completed (not cancelled or declined).
    pub completed: bool,
    /// Wall time of the whole request, scratch fill and add included.
    pub wall_ms: f64,
    pub turns: u32,
    pub chunks: u64,
    pub tuples: u64,
    /// Pixels of each chunk in dispatch order (the chunk-size ramp and the remainder chunk).
    pub chunk_pixels: Vec<u64>,
    pub scratch_take_ms: f64,
    pub scratch_add_ms: f64,
    /// Request start to the first chunk's submission: turnstile, scene preparation, first
    /// bind group and encode. GPU idle.
    pub startup_gap_ms: f64,
    /// GPU idle between chunks (lower bound): turn boundaries and pipeline refills.
    pub gpu_idle_between_ms: f64,
    pub turn_wall_ms: f64,
    pub turnstile_wait_ms: f64,
    pub prepare_turn_ms: f64,
    pub scene_upload_ms: f64,
    pub tail_drain_ms: f64,
    /// `dispatch_chunk` CPU time, all chunks.
    pub dispatch_cpu_ms: f64,
    pub bind_group_ms: f64,
    pub encode_ms: f64,
    pub submit_ms: f64,
    pub write_buffer_ms: f64,
    pub map_begin_ms: f64,
    /// Submit to `device.poll` return, summed over chunks.
    pub gpu_wall_ms: f64,
    /// Submit to drained (the `ns_per_tuple_ema` clock), summed over chunks.
    pub cycle_ms: f64,
    pub poll_wait_ms: f64,
    pub map_recv_ms: f64,
    pub map_copy_ms: f64,
    pub accumulate_ms: f64,
    /// P24: bind groups + encoders + submits as a percentage of the summed chunk wall time.
    pub p24_bind_encode_submit_pct_of_chunk_wall: f64,
    /// P24: the same as a percentage of the request's wall time.
    pub p24_bind_encode_submit_pct_of_wall: f64,
    /// P24: the whole `dispatch_chunk` CPU time as a percentage of the request's wall time.
    pub p24_dispatch_cpu_pct_of_wall: f64,
    /// H04: scene re-upload + final drain as a percentage of the turns' wall time.
    pub h04_upload_plus_tail_pct_of_turn_wall: f64,
    /// H04: GPU idle between chunks as a percentage of the request's wall time.
    pub h04_idle_between_pct_of_wall: f64,
    /// Idle startup gap as a percentage of the request's wall time.
    pub startup_pct_of_wall: f64,
}

const fn pct(part: f64, whole: f64) -> f64 {
    if whole > 0.0 {
        100.0 * part / whole
    } else {
        0.0
    }
}

impl State {
    /// The request totals as a [`PassSummary`] (`wall` is the request's wall time).
    fn summarise(&self, wall: Duration, completed: bool) -> PassSummary {
        let total = &self.pass;
        let wall_ms = millis(nanos(wall));
        let slot_ms = |slot: Slot| millis(total.slot(slot));
        let turn_ms = |slot: TurnSlot| millis(total.turn_slot(slot));
        let bind = slot_ms(Slot::TransportBind) + slot_ms(Slot::ReduceBind);
        let encode = slot_ms(Slot::TransportEncode)
            + slot_ms(Slot::ReduceEncode)
            + slot_ms(Slot::CopyEncode);
        let submit = slot_ms(Slot::TransportSubmit)
            + slot_ms(Slot::ReduceSubmit)
            + slot_ms(Slot::CopySubmit);
        let bes = bind + encode + submit;
        let gpu_wall_ms = millis(total.gpu_wall_ns);
        let turn_wall_ms = millis(self.turn_wall_ns);
        let idle_between_ms = millis(total.idle_ns);
        let startup_ms = millis(self.startup_ns);
        PassSummary {
            completed,
            wall_ms,
            turns: self.turns,
            chunks: total.chunks,
            tuples: total.tuples,
            chunk_pixels: self.chunk_pixels.clone(),
            scratch_take_ms: millis(self.pass_ns[PassSlot::ScratchTake as usize]),
            scratch_add_ms: millis(self.pass_ns[PassSlot::ScratchAdd as usize]),
            startup_gap_ms: startup_ms,
            gpu_idle_between_ms: idle_between_ms,
            turn_wall_ms,
            turnstile_wait_ms: turn_ms(TurnSlot::TurnstileWait),
            prepare_turn_ms: turn_ms(TurnSlot::Prepare),
            scene_upload_ms: turn_ms(TurnSlot::SceneUpload),
            tail_drain_ms: turn_ms(TurnSlot::TailDrain),
            dispatch_cpu_ms: slot_ms(Slot::DispatchTotal),
            bind_group_ms: bind,
            encode_ms: encode,
            submit_ms: submit,
            write_buffer_ms: slot_ms(Slot::ParamsWrite) + slot_ms(Slot::ReduceWrite),
            map_begin_ms: slot_ms(Slot::MapBegin),
            gpu_wall_ms,
            cycle_ms: millis(total.cycle_ns),
            poll_wait_ms: slot_ms(Slot::PollWait),
            map_recv_ms: slot_ms(Slot::MapRecv),
            map_copy_ms: slot_ms(Slot::MapCopy),
            accumulate_ms: slot_ms(Slot::Accumulate),
            p24_bind_encode_submit_pct_of_chunk_wall: pct(bes, gpu_wall_ms),
            p24_bind_encode_submit_pct_of_wall: pct(bes, wall_ms),
            p24_dispatch_cpu_pct_of_wall: pct(slot_ms(Slot::DispatchTotal), wall_ms),
            h04_upload_plus_tail_pct_of_turn_wall: pct(
                turn_ms(TurnSlot::SceneUpload) + turn_ms(TurnSlot::TailDrain),
                turn_wall_ms,
            ),
            h04_idle_between_pct_of_wall: pct(idle_between_ms, wall_ms),
            startup_pct_of_wall: pct(startup_ms, wall_ms),
        }
    }
}

/// Ends a request: prints the pass line and keeps the summary for
/// [`take_last_pass_summary`].
pub(crate) fn end_pass(completed: bool) {
    if !enabled() {
        return;
    }
    let mut state = lock_state();
    let Some(began) = state.pass_began else {
        return;
    };
    let summary = state.summarise(began.elapsed(), completed);
    state.last = Some(summary.clone());
    drop(state);

    let s = &summary;
    tracing::info!(
        target: "pleochroma_cut::render_bench",
        "gpu_timing pass completed={} wall_ms={:.3} turns={} chunks={} tuples={} \
         startup_gap_ms={:.3} gpu_idle_between_ms={:.3} scratch_take_ms={:.3} \
         scratch_add_ms={:.3} turn_wall_ms={:.3} turnstile_wait_ms={:.3} prepare_turn_ms={:.3} \
         scene_upload_ms={:.3} tail_drain_ms={:.3} dispatch_cpu_ms={:.3} bind_group_ms={:.3} \
         encode_ms={:.3} submit_ms={:.3} write_buffer_ms={:.3} map_begin_ms={:.3} \
         gpu_wall_ms={:.3} cycle_ms={:.3} poll_wait_ms={:.3} map_recv_ms={:.3} \
         map_copy_ms={:.3} accumulate_ms={:.3} p24_bes_pct_of_chunk_wall={:.2} \
         p24_bes_pct_of_wall={:.2} h04_upload_tail_pct_of_turn_wall={:.2} \
         h04_idle_between_pct_of_wall={:.2} startup_pct_of_wall={:.2} chunk_pixels={:?}",
        s.completed,
        s.wall_ms,
        s.turns,
        s.chunks,
        s.tuples,
        s.startup_gap_ms,
        s.gpu_idle_between_ms,
        s.scratch_take_ms,
        s.scratch_add_ms,
        s.turn_wall_ms,
        s.turnstile_wait_ms,
        s.prepare_turn_ms,
        s.scene_upload_ms,
        s.tail_drain_ms,
        s.dispatch_cpu_ms,
        s.bind_group_ms,
        s.encode_ms,
        s.submit_ms,
        s.write_buffer_ms,
        s.map_begin_ms,
        s.gpu_wall_ms,
        s.cycle_ms,
        s.poll_wait_ms,
        s.map_recv_ms,
        s.map_copy_ms,
        s.accumulate_ms,
        s.p24_bind_encode_submit_pct_of_chunk_wall,
        s.p24_bind_encode_submit_pct_of_wall,
        s.h04_upload_plus_tail_pct_of_turn_wall,
        s.h04_idle_between_pct_of_wall,
        s.startup_pct_of_wall,
        s.chunk_pixels,
    );
}

/// The summary of the most recent GPU request, once; `None` when timing is off, when no
/// GPU request ran since the last call, or when the last request never reached the GPU.
#[must_use]
pub fn take_last_pass_summary() -> Option<PassSummary> {
    if !enabled() {
        return None;
    }
    lock_state().last.take()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(base: Instant, ms: u64) -> Instant {
        base + Duration::from_millis(ms)
    }

    #[test]
    fn slot_counts_match_their_enums() {
        assert_eq!(SLOTS, 16);
        assert_eq!(TURN_SLOTS, 4);
        assert_eq!(PASS_SLOTS, 2);
    }

    #[test]
    fn a_disabled_mark_reads_no_clock_and_records_nothing() {
        // The test environment does not set PLEOCHROMA_GPU_TIMING.
        if enabled() {
            return;
        }
        let mut mark = Mark::now();
        assert!(mark.instant().is_none());
        mark.lap(Slot::PollWait);
        assert_eq!(take_chunk_slots(), [0; SLOTS]);
        assert!(take_dispatch(0, 1, 1).is_none());
        assert!(take_last_pass_summary().is_none());
    }

    #[test]
    fn idle_is_startup_for_the_first_chunk_and_the_gap_to_the_last_completion_after() {
        let base = Instant::now();
        let mut state = State::EMPTY;
        state.pass_began = Some(base);
        let slots = [0; SLOTS];

        // First chunk submitted 7 ms after the request began, done at 30 ms.
        let (idle, wall) = state.record_chunk(
            &slots,
            (100, 100),
            (at(base, 7), at(base, 30)),
            Duration::from_millis(25),
        );
        assert_eq!((idle, wall), (7_000_000, 23_000_000));
        assert_eq!(state.startup_ns, 7_000_000);
        assert_eq!(state.pass.idle_ns, 0);

        // Second chunk queued while the first still ran: no idle.
        let (idle, _) = state.record_chunk(
            &slots,
            (100, 100),
            (at(base, 10), at(base, 55)),
            Duration::from_millis(45),
        );
        assert_eq!(idle, 0);

        // Third chunk submitted 12 ms after the second completed (a turn boundary): idle.
        let (idle, _) = state.record_chunk(
            &slots,
            (60, 60),
            (at(base, 67), at(base, 80)),
            Duration::from_millis(13),
        );
        assert_eq!(idle, 12_000_000);
        assert_eq!(state.pass.idle_ns, 12_000_000);
        assert_eq!(state.pass.chunks, 3);
        assert_eq!(state.pass.pixels, 260);
        assert_eq!(state.chunk_pixels, vec![100, 100, 60]);
        assert_eq!(state.last_done, Some(at(base, 80)));
    }

    #[test]
    fn summary_derives_the_decision_percentages() {
        let base = Instant::now();
        let mut state = State::EMPTY;
        state.pass_began = Some(base);
        let mut slots = [0; SLOTS];
        slots[Slot::TransportBind as usize] = 1_000_000;
        slots[Slot::TransportEncode as usize] = 2_000_000;
        slots[Slot::TransportSubmit as usize] = 3_000_000;
        slots[Slot::DispatchTotal as usize] = 8_000_000;
        state.record_chunk(
            &slots,
            (10, 10),
            (at(base, 1), at(base, 101)),
            Duration::from_millis(100),
        );
        state.turns = 1;
        state.turn_wall_ns = 110_000_000;
        state.pass.turn_ns[TurnSlot::SceneUpload as usize] = 1_000_000;
        state.pass.turn_ns[TurnSlot::TailDrain as usize] = 10_000_000;

        let summary = state.summarise(Duration::from_millis(200), true);
        assert!(summary.completed);
        assert_eq!(summary.chunks, 1);
        assert!((summary.bind_group_ms - 1.0).abs() < 1e-9);
        assert!((summary.encode_ms - 2.0).abs() < 1e-9);
        assert!((summary.submit_ms - 3.0).abs() < 1e-9);
        assert!((summary.p24_bind_encode_submit_pct_of_chunk_wall - 6.0).abs() < 1e-9);
        assert!((summary.p24_bind_encode_submit_pct_of_wall - 3.0).abs() < 1e-9);
        assert!((summary.p24_dispatch_cpu_pct_of_wall - 4.0).abs() < 1e-9);
        assert!((summary.h04_upload_plus_tail_pct_of_turn_wall - 10.0).abs() < 1e-9);
        assert!((summary.startup_gap_ms - 1.0).abs() < 1e-9);
    }

    #[test]
    fn percentages_of_nothing_are_zero() {
        assert!(pct(1.0, 0.0).abs() < f64::EPSILON);
        assert!((pct(1.0, 4.0) - 25.0).abs() < 1e-12);
    }
}
