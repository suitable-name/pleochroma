//! The remote half of a beam render (protocol v38, beam guide section 14.8): a lane of joined
//! workers that render whole tiles, and [`render_shares`], which splits the frame's tiles between
//! this computer and the workers, merges the finished tiles in tile order and fails a lost
//! worker's tiles over.
//!
//! # Not a `BeamEngine`
//!
//! The local engines shade the fragments and run the residual of a *batch*, and the pipeline
//! combines the two. A worker renders whole tiles, beams and residual, with its own engines, and
//! returns finished pixels; nothing of the tree crosses the wire. So the remote engine
//! ([`RemoteBeamEngine`]) is a tile-level engine beside the batch-level ones: this computer
//! renders its tiles with the entry's tile restriction (`BeamRun::tiles`) and the workers render
//! theirs at the same time.
//!
//! # The split
//!
//! By tile, through `TileAssignment`: tile `i` starts on engine `i % engines` (this computer
//! first unless it sits out under Remote Only, then the workers in order). A worker that fails
//! (it disconnects, refuses, goes silent, or returns pixels that do not fit) is taken out
//! (`TileAssignment::fail`) and its tiles move to the next living engine in the cyclic order, in a
//! further round; with no engine left the render fails. Every pixel belongs to one tile and each
//! tile to one engine, so the merge does not depend on timing.
//!
//! # What equals what
//!
//! A tile's residual samples are a function of the pixel and the absolute sample index alone, so
//! with a fixed sample count the image equals the local render's bit for bit when every engine is
//! the CPU on one platform. With adaptive stopping the residual's dilation across a batch border
//! reads only the batch's own errors (the documented beam-mode difference), and the batches of a
//! share differ from those of the whole frame, so a pixel's count can differ within the noise
//! criterion. Across platforms the beam part is within the twin tolerance.

use super::{
    super::{
        super::types::AccumulationCarry,
        adaptive::assignment::TileAssignment,
        lanes::{BeamLaneCache, RenderJob, resolve_remote_capability},
    },
    Inputs, LocalHooks, render_local,
};
use crate::bridge::export_thread::{
    params::ComputeTarget,
    remote::{
        BeamClientPool, BeamTileClient, BeamTileFailure, BeamTileOrder, BeamTiles, LiveStream,
        RemoteCapability, scene_state_from_snapshot,
    },
};
use glam::Vec3;
use pleochroma::renderer::{
    accel_beam::BeamOff,
    adaptive::{AdaptiveImage, LiveTiles},
    beam::{
        BeamPhase, BeamProgress, leaf_cache::RetainedBatch, pipeline::tile_list, primary::TileRect,
        stats::BeamStats,
    },
};
use pleochroma_net::{SceneState, messages::BeamTilePixel};
use std::{
    cell::Cell,
    collections::BTreeMap,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    thread,
    time::Duration,
};

/// What a render says when it keeps its beams although remote compute was chosen.
pub(super) const NOTE_KEEP_LOCAL: &str = "Keeping the beams renders on this computer.";

/// How often the thread that waits for the workers reports progress.
const WAIT: Duration = Duration::from_millis(100);

/// One remote worker as the render sees it: it renders whole tiles.
pub(super) trait RemoteBeamEngine: Send {
    /// Renders the tiles `live` of `order`'s frame and returns their finished pixels in the order
    /// of `live`.
    ///
    /// # Errors
    ///
    /// Why the worker could not render them; the tiles then go to the next engine.
    fn render_tiles(
        &mut self,
        order: &BeamTileOrder<'_>,
        live: &LiveTiles,
        cancel: &AtomicBool,
    ) -> Result<BeamTiles, BeamTileFailure>;
}

impl<S: LiveStream + Send> RemoteBeamEngine for BeamTileClient<S> {
    fn render_tiles(
        &mut self,
        order: &BeamTileOrder<'_>,
        live: &LiveTiles,
        cancel: &AtomicBool,
    ) -> Result<BeamTiles, BeamTileFailure> {
        self.render(order, live, cancel)
    }
}

/// A client kept in the job's carry: the lane borrows it for one frame.
impl<S: LiveStream + Send> RemoteBeamEngine for &mut BeamTileClient<S> {
    fn render_tiles(
        &mut self,
        order: &BeamTileOrder<'_>,
        live: &LiveTiles,
        cancel: &AtomicBool,
    ) -> Result<BeamTiles, BeamTileFailure> {
        self.render(order, live, cancel)
    }
}

/// The remote engines of one beam render, and what went with opening them.
pub(super) struct BeamLane<'e> {
    /// The remote workers, in assignment order (after this computer, if it takes part).
    pub(super) engines: Vec<Box<dyn RemoteBeamEngine + 'e>>,
    /// The scene as the wire carries it; `Some` exactly when there are engines.
    pub(super) scene: Option<SceneState>,
    /// Said once, in the opening note: why there is no remote lane although one was asked for.
    pub(super) note: Option<String>,
    /// Remote compute was chosen but no worker is configured: the opening note says the render is
    /// on this computer.
    pub(super) not_local: bool,
    /// Whether this computer sits out (`RemoteOnly`): a remote failure then ends the render.
    pub(super) remote_only: bool,
}

impl BeamLane<'_> {
    /// No remote lane: every tile is rendered on this computer.
    #[must_use]
    pub(super) const fn local_only() -> Self {
        Self {
            engines: Vec::new(),
            scene: None,
            note: None,
            not_local: false,
            remote_only: false,
        }
    }

    /// The lane of a render that keeps its beams: this computer only, and the note says so when
    /// remote compute was chosen.
    #[must_use]
    pub(super) fn kept(target: ComputeTarget) -> Self {
        Self {
            note: (!matches!(target, ComputeTarget::LocalOnly)).then(|| NOTE_KEEP_LOCAL.to_owned()),
            ..Self::local_only()
        }
    }

    /// Whether this computer renders tiles: always, except for `RemoteOnly` with an engine to take
    /// them.
    #[must_use]
    pub(super) const fn local_participates(&self) -> bool {
        !self.remote_only || self.engines.is_empty()
    }
}

/// Opens the remote lane `job` asks for, or explains why it has none.
///
/// A render without a configured worker renders here and says so (`NOTE_BEAMS_LOCAL_ONLY`); a
/// worker that cannot take part (an HDR map, no zoning capability, too many pixels, unreachable)
/// gives a lane with a note under `Both` and an error under `RemoteOnly`.
///
/// # Errors
///
/// The sentence that fails a `RemoteOnly` render.
pub(super) fn open(job: &RenderJob<'_>) -> Result<BeamLane<'static>, String> {
    let mut carry = AccumulationCarry::default();
    let opening = opening(job, |note| {
        resolve_remote_capability(
            &mut carry,
            job.compute_target,
            job.worker,
            job.scene,
            job.width,
            job.height,
            note,
        )
    })?;
    match opening {
        Opening::Lane(lane) => Ok(*lane),
        Opening::Remote {
            capability,
            remote_only,
        } => {
            let mut pool = BeamClientPool::default();
            match pool.open(&capability) {
                Ok(_) => {
                    let engines = pool
                        .into_clients()
                        .into_iter()
                        .map(|client| Box::new(client) as Box<dyn RemoteBeamEngine>)
                        .collect();
                    Ok(remote_lane(job, remote_only, engines))
                }
                Err(why) => unreachable_lane(remote_only, &why),
            }
        }
    }
}

/// [`open`] over the job's carry: the probe is made once per carry, the connections are kept
/// across frames (a broken one is reopened), and a coordinator gets one per joined worker.
///
/// # Errors
///
/// The sentence that fails a `RemoteOnly` render.
pub(super) fn open_cached<'c>(
    job: &RenderJob<'_>,
    mut cache: BeamLaneCache<'c>,
) -> Result<BeamLane<'c>, String> {
    let opening = opening(job, |note| {
        cache.resolve(
            job.compute_target,
            job.worker,
            (job.scene, job.width, job.height),
            note,
        )
    })?;
    match opening {
        Opening::Lane(lane) => Ok(*lane),
        Opening::Remote {
            capability,
            remote_only,
        } => match cache.clients.open(&capability) {
            Ok(clients) => {
                let engines = clients
                    .iter_mut()
                    .map(|client| Box::new(client) as Box<dyn RemoteBeamEngine + 'c>)
                    .collect();
                Ok(remote_lane(job, remote_only, engines))
            }
            Err(why) => unreachable_lane(remote_only, &why),
        },
    }
}

/// What the checks before the connection decided.
enum Opening {
    /// The lane is settled without a connection (local only, or a note on why not). Boxed: a lane
    /// carries the wire scene.
    Lane(Box<BeamLane<'static>>),
    /// A worker that can take part; the connections are still to be made.
    Remote {
        /// The probed worker.
        capability: RemoteCapability,
        /// Whether this computer sits out (`RemoteOnly`).
        remote_only: bool,
    },
}

/// The checks of [`open`]: whether remote compute is asked for, configured and able to render this
/// scene. `resolve` probes the worker (or reads the carry's cached probe).
fn opening(
    job: &RenderJob<'_>,
    resolve: impl FnOnce(&mut Option<String>) -> Result<Option<RemoteCapability>, String>,
) -> Result<Opening, String> {
    if matches!(job.compute_target, ComputeTarget::LocalOnly) {
        return Ok(Opening::Lane(Box::new(BeamLane::local_only())));
    }
    if job.worker.is_none() {
        return Ok(Opening::Lane(Box::new(BeamLane {
            not_local: true,
            ..BeamLane::local_only()
        })));
    }
    let remote_only = matches!(job.compute_target, ComputeTarget::RemoteOnly);
    if job.scene.env_map.is_some() {
        return unavailable(
            remote_only,
            "A remote worker does not render beams of a scene with an HDR environment.",
        )
        .map(|lane| Opening::Lane(Box::new(lane)));
    }
    let mut pending_note = None;
    let Some(capability) = resolve(&mut pending_note)? else {
        return Ok(Opening::Lane(Box::new(BeamLane {
            note: pending_note,
            ..BeamLane::local_only()
        })));
    };
    Ok(Opening::Remote {
        capability,
        remote_only,
    })
}

/// The lane of a render that cannot use its worker (`why`): this computer only, with the note, or
/// the error that fails a `RemoteOnly` render.
fn unavailable(remote_only: bool, why: &str) -> Result<BeamLane<'static>, String> {
    if remote_only {
        Err(format!(
            "{why} Remote Only cannot render this scene; choose Local Only or Local + Remote."
        ))
    } else {
        Ok(BeamLane {
            note: Some(format!("{why} Rendering on this computer only.")),
            ..BeamLane::local_only()
        })
    }
}

/// [`unavailable`] for a worker that could not be connected to.
fn unreachable_lane(remote_only: bool, why: &str) -> Result<BeamLane<'static>, String> {
    unavailable(
        remote_only,
        &format!("The remote worker could not be reached ({why})."),
    )
}

/// The lane over `engines`, one slot each in assignment order.
fn remote_lane<'e>(
    job: &RenderJob<'_>,
    remote_only: bool,
    engines: Vec<Box<dyn RemoteBeamEngine + 'e>>,
) -> BeamLane<'e> {
    BeamLane {
        engines,
        scene: Some(scene_state_from_snapshot(
            job.scene,
            job.width,
            job.height,
            job.cam_yaw,
            job.cam_pitch,
        )),
        note: None,
        not_local: false,
        remote_only,
    }
}

/// The hooks of a render over several engines.
pub(super) struct ShareHooks<'h> {
    /// Progress: the phase, the share of the whole frame done and the time left.
    pub(super) progress: &'h mut dyn FnMut(BeamPhase, f32, Option<Duration>),
    /// Each finished piece's tiles and final pixels (local batches and remote tiles alike).
    pub(super) on_batch: &'h mut dyn FnMut(&[TileRect], &[Vec3]),
    /// Each local batch's leaves and kept index (a render that keeps its beams; local only).
    pub(super) retain: Option<&'h mut dyn FnMut(RetainedBatch)>,
}

/// What the shares of a render made.
pub(super) enum Shares {
    /// The merged frame, the counters and what was said about lost workers.
    Done {
        image: Box<AdaptiveImage>,
        stats: Box<BeamStats>,
        notes: Vec<String>,
    },
    /// The cancel flag was raised.
    Cancelled,
    /// The frame is not beam traced (the entry's reason).
    Off(BeamOff),
    /// The render failed (a Remote Only worker, with nobody to take over).
    Failed(String),
}

/// The tiles of one engine in one round.
pub(super) struct Share {
    /// The engine slot the tiles were given to.
    pub(super) slot: usize,
    /// The tiles.
    pub(super) live: LiveTiles,
}

/// The work of a round: `(engine slot, ascending tile indices)`, in slot order.
type Work = Vec<(usize, Vec<u32>)>;

/// Progress bookkeeping across rounds.
pub(super) struct Tally {
    pub(super) total: usize,
    /// Tiles merged in earlier rounds.
    pub(super) settled: Cell<usize>,
    /// Tiles of this computer's share of the round running now.
    pub(super) local: Cell<usize>,
}

impl Tally {
    /// The share of the frame done, with `local_done` of this computer's share (0 to 1) and
    /// `remote_done` worker tiles finished in this round.
    #[expect(
        clippy::cast_precision_loss,
        reason = "tile counts are far below 2^24, where f32 is exact"
    )]
    pub(super) fn mixed(&self, local_done: f32, remote_done: usize) -> f32 {
        if self.total == 0 {
            return 1.0;
        }
        let done = local_done.clamp(0.0, 1.0).mul_add(
            self.local.get() as f32,
            (self.settled.get() + remote_done) as f32,
        );
        (done / self.total as f32).clamp(0.0, 1.0)
    }
}

/// A frame-sized image with nothing rendered.
pub(super) fn empty_image(width: u32, height: u32) -> AdaptiveImage {
    let pixels = width as usize * height as usize;
    AdaptiveImage {
        width,
        height,
        sum: vec![Vec3::ZERO; pixels],
        half_b: vec![Vec3::ZERO; pixels],
        counts: vec![0; pixels],
        pixels: vec![Vec3::ZERO; pixels],
        samples_traced: 0,
    }
}

/// Copies the tiles `live` of `src` into `dst` (every per-pixel buffer).
pub(super) fn copy_tiles(dst: &mut AdaptiveImage, src: &AdaptiveImage, live: &LiveTiles) {
    let width = dst.width as usize;
    for &index in live.indices() {
        let (x0, y0, x1, y1) = live.tile_rect(index);
        for y in y0..y1 {
            let row = y as usize * width;
            for at in row + x0 as usize..row + x1 as usize {
                if let (Some(s), Some(h), Some(c), Some(p)) = (
                    src.sum.get(at),
                    src.half_b.get(at),
                    src.counts.get(at),
                    src.pixels.get(at),
                ) && let (Some(ds), Some(dh), Some(dc), Some(dp)) = (
                    dst.sum.get_mut(at),
                    dst.half_b.get_mut(at),
                    dst.counts.get_mut(at),
                    dst.pixels.get_mut(at),
                ) {
                    (*ds, *dh, *dc, *dp) = (*s, *h, *c, *p);
                }
            }
        }
    }
}

/// Writes a worker's `pixels` for the tiles `live` into `image`; `false` (and nothing written past
/// the mismatch) when they do not fit the tiles.
pub(super) fn scatter_tiles(
    image: &mut AdaptiveImage,
    live: &LiveTiles,
    pixels: &[BeamTilePixel],
) -> bool {
    if u64::try_from(pixels.len()) != Ok(live.pixel_count()) {
        return false;
    }
    let width = image.width as usize;
    let mut next = pixels.iter();
    for &index in live.indices() {
        let (x0, y0, x1, y1) = live.tile_rect(index);
        for y in y0..y1 {
            for x in x0..x1 {
                let Some(pixel) = next.next() else {
                    return false;
                };
                let at = y as usize * width + x as usize;
                if let (Some(slot), Some(count)) =
                    (image.pixels.get_mut(at), image.counts.get_mut(at))
                {
                    *slot = pixel.xyz;
                    *count = pixel.samples;
                }
            }
        }
    }
    true
}

/// The rectangles of the tiles `live`, in tile order.
fn rects_of(live: &LiveTiles) -> Vec<TileRect> {
    live.indices()
        .iter()
        .map(|&index| {
            let (x0, y0, x1, y1) = live.tile_rect(index);
            TileRect { x0, y0, x1, y1 }
        })
        .collect()
}

/// Renders the frame of `inputs` on this computer and the workers of `lane`.
///
/// Without workers (or with `hooks.retain`, which only this computer can serve) this is exactly
/// one local render of the whole frame.
pub(super) fn render_shares(
    inputs: &Inputs<'_, '_, '_>,
    (cancel, sample_offset): (&AtomicBool, u32),
    lane: &mut BeamLane<'_>,
    hooks: ShareHooks<'_>,
) -> Shares {
    let ShareHooks {
        progress,
        on_batch,
        retain,
    } = hooks;
    if lane.engines.is_empty() || retain.is_some() {
        let mut forward = |p: BeamProgress| progress(p.phase, p.done, p.eta);
        let local = LocalHooks {
            progress: &mut forward,
            on_batch,
            // Coerced to the local hooks' lifetime: an `Option<&mut dyn ..>` is invariant.
            retain: retain.map(|retain| retain as &mut dyn FnMut(RetainedBatch)),
        };
        return match render_local(inputs, (cancel, sample_offset), None, local) {
            Ok((image, stats)) => Shares::Done {
                image: Box::new(image),
                stats: Box::new(stats),
                notes: Vec::new(),
            },
            Err(off) => Shares::Off(off),
        };
    }
    run_rounds(inputs, (cancel, sample_offset), lane, (progress, on_batch))
}

/// What the rounds read and share.
struct Env<'a, 'f, 'e> {
    inputs: &'a Inputs<'a, 'f, 'e>,
    cancel: &'a AtomicBool,
    sample_offset: u32,
    order: BeamTileOrder<'a>,
    local: bool,
    first_remote: usize,
    tally: Tally,
}

/// The merged result so far.
struct Merged {
    image: AdaptiveImage,
    stats: BeamStats,
    notes: Vec<String>,
}

/// Rounds of shares until every tile is rendered: each round gives every engine with work its
/// tiles, at the same time; a failed worker's tiles go to the next engine in the next round.
fn run_rounds(
    inputs: &Inputs<'_, '_, '_>,
    (cancel, sample_offset): (&AtomicBool, u32),
    lane: &mut BeamLane<'_>,
    mut hooks: RoundHooks<'_>,
) -> Shares {
    let Some(scene) = lane.scene.as_ref() else {
        return Shares::Failed("The remote lane has no scene.".to_owned());
    };
    let (width, height, tile) = (
        inputs.frame.width,
        inputs.frame.height,
        inputs.adaptive.tile,
    );
    let all = tile_list((width, height), tile, None);
    let local = lane.local_participates();
    let first_remote = usize::from(local);
    let mut assignment = TileAssignment::new(all.len(), first_remote + lane.engines.len());
    let env = Env {
        inputs,
        cancel,
        sample_offset,
        order: BeamTileOrder {
            scene,
            beam: inputs.settings,
            adaptive: inputs.adaptive,
            sample_offset,
        },
        local,
        first_remote,
        tally: Tally {
            total: all.len(),
            settled: Cell::new(0),
            local: Cell::new(0),
        },
    };
    let mut work: Work = assignment
        .split(&all)
        .into_iter()
        .enumerate()
        .filter(|(_, tiles)| !tiles.is_empty())
        .collect();
    let mut merged = Merged {
        image: empty_image(width, height),
        stats: BeamStats::default(),
        notes: Vec::new(),
    };
    while !work.is_empty() {
        let shares: Vec<Share> = work
            .iter()
            .map(|(slot, tiles)| Share {
                slot: *slot,
                live: LiveTiles::from_indices(width, height, tile, tiles.clone()),
            })
            .collect();
        let (local_result, replies) = run_round(&env, &mut lane.engines, &shares, &mut hooks);
        match settle(
            &env,
            &mut assignment,
            &shares,
            (local_result, replies),
            (&mut merged, &mut *hooks.1),
        ) {
            Ok(next) => work = next,
            Err(stop) => return stop,
        }
    }
    Shares::Done {
        image: Box::new(merged.image),
        stats: Box::new(merged.stats),
        notes: merged.notes,
    }
}

/// What this computer's share of a round made.
type LocalResult = Option<Result<(AdaptiveImage, BeamStats), BeamOff>>;

/// What a worker answered for its share.
type Reply = (usize, Result<BeamTiles, BeamTileFailure>);

/// Hooks of the rounds: progress and the finished-pixels sink.
type RoundHooks<'h> = (
    &'h mut dyn FnMut(BeamPhase, f32, Option<Duration>),
    OnBatch<'h>,
);

/// The sink of finished pieces: their tiles and final pixels.
type OnBatch<'h> = &'h mut dyn FnMut(&[TileRect], &[Vec3]);

/// One round: the workers render their shares on threads while this computer renders its own (or
/// waits, reporting progress).
fn run_round(
    env: &Env<'_, '_, '_>,
    engines: &mut [Box<dyn RemoteBeamEngine + '_>],
    shares: &[Share],
    hooks: &mut RoundHooks<'_>,
) -> (LocalResult, Vec<Reply>) {
    let remote_done = AtomicUsize::new(0);
    let own = shares.iter().find(|share| env.local && share.slot == 0);
    env.tally
        .local
        .set(own.map_or(0, |share| share.live.indices().len()));
    let mut local_result: LocalResult = None;
    let mut replies: Vec<Reply> = Vec::new();
    thread::scope(|scope| {
        let mut running = Vec::new();
        for (index, engine) in engines.iter_mut().enumerate() {
            let slot = env.first_remote + index;
            let Some(share) = shares.iter().find(|share| share.slot == slot) else {
                continue;
            };
            let (order, cancel, live, done) = (env.order, env.cancel, &share.live, &remote_done);
            running.push((
                slot,
                scope.spawn(move || {
                    let reply = engine.render_tiles(&order, live, cancel);
                    if reply.is_ok() {
                        done.fetch_add(live.indices().len(), Ordering::Relaxed);
                    }
                    reply
                }),
            ));
        }
        let (progress, on_batch) = (&mut *hooks.0, &mut *hooks.1);
        if let Some(share) = own {
            let mut forward = |p: BeamProgress| {
                progress(
                    p.phase,
                    env.tally.mixed(p.done, remote_done.load(Ordering::Relaxed)),
                    p.eta,
                );
            };
            local_result = Some(render_local(
                env.inputs,
                (env.cancel, env.sample_offset),
                Some(&share.live),
                LocalHooks {
                    progress: &mut forward,
                    on_batch,
                    retain: None,
                },
            ));
        }
        // This computer's share is done (or it sits out): the bar keeps moving with the workers'
        // tiles until they are all in. With a share of its own the local part counts as whole.
        let phase = if own.is_some() {
            BeamPhase::Finishing
        } else {
            BeamPhase::Tracing
        };
        while running.iter().any(|(_, handle)| !handle.is_finished()) {
            progress(
                phase,
                env.tally.mixed(1.0, remote_done.load(Ordering::Relaxed)),
                None,
            );
            thread::sleep(WAIT);
        }
        for (slot, handle) in running {
            let reply = handle.join().unwrap_or_else(|_| {
                Err(BeamTileFailure::Broken(
                    "the remote lane stopped unexpectedly".to_owned(),
                ))
            });
            replies.push((slot, reply));
        }
    });
    (local_result, replies)
}

/// Merges a round's results into `merged` and works out the next round's work. `Err` ends the
/// render.
fn settle(
    env: &Env<'_, '_, '_>,
    assignment: &mut TileAssignment,
    shares: &[Share],
    (local_result, replies): (LocalResult, Vec<Reply>),
    (merged, on_batch): (&mut Merged, OnBatch<'_>),
) -> Result<Work, Shares> {
    let mut settled = 0_usize;
    if let Some(result) = local_result {
        let (image, stats) = result.map_err(Shares::Off)?;
        if let Some(share) = shares.iter().find(|share| env.local && share.slot == 0) {
            copy_tiles(&mut merged.image, &image, &share.live);
            settled += share.live.indices().len();
        }
        merged.image.samples_traced += image.samples_traced;
        merged.stats.merge(&stats);
    }
    if env.cancel.load(Ordering::Acquire) {
        return Err(Shares::Cancelled);
    }
    // The shares that failed this round; their tiles are queued after the loop, from the
    // assignment, because a later failure in the same loop can fail the heir of an earlier one.
    let mut failed: Vec<&Share> = Vec::new();
    for (slot, reply) in replies {
        let Some(share) = shares.iter().find(|share| share.slot == slot) else {
            continue;
        };
        let merged_ok = reply.and_then(|tiles| {
            if scatter_tiles(&mut merged.image, &share.live, &tiles.pixels) {
                Ok(tiles)
            } else {
                Err(BeamTileFailure::Broken(
                    "the worker's pixels do not fit the tiles it was asked for".to_owned(),
                ))
            }
        });
        match merged_ok {
            Ok(tiles) => {
                let xyz: Vec<Vec3> = tiles.pixels.iter().map(|pixel| pixel.xyz).collect();
                on_batch(&rects_of(&share.live), &xyz);
                merged.image.samples_traced += tiles.stats.residual_samples;
                merged.stats.beams += tiles.stats.beams;
                merged.stats.leaves += tiles.stats.leaves;
                merged.stats.fragments += tiles.stats.fragments;
                settled += share.live.indices().len();
            }
            Err(BeamTileFailure::Cancelled) => return Err(Shares::Cancelled),
            Err(failure) => {
                let Some(heir) = assignment.fail(slot) else {
                    return Err(Shares::Failed(format!(
                        "The remote worker failed: {failure}"
                    )));
                };
                let who = if env.local && heir == 0 {
                    "this computer renders its tiles from here on"
                } else {
                    "its tiles move to the next worker"
                };
                merged
                    .notes
                    .push(format!("The remote worker stopped ({failure}); {who}."));
                failed.push(share);
            }
        }
    }
    env.tally.settled.set(env.tally.settled.get() + settled);
    Ok(next_work(assignment, &failed))
}

/// The next round's work: every tile of the `failed` shares under the engine that owns it now
/// (`assignment` already moved the tiles of each failed engine on, however many failed).
pub(super) fn next_work(assignment: &TileAssignment, failed: &[&Share]) -> Work {
    let mut next: BTreeMap<usize, Vec<u32>> = BTreeMap::new();
    for share in failed {
        for &tile in share.live.indices() {
            if let Some(owner) = assignment.owner(tile) {
                next.entry(owner).or_default().push(tile);
            }
        }
    }
    next.into_iter()
        .map(|(slot, mut tiles)| {
            tiles.sort_unstable();
            tiles.dedup();
            (slot, tiles)
        })
        .collect()
}
