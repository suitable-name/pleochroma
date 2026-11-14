//! The per-item render engine and the local/remote lane runners that pull from a
//! shared [`WorkQueue`]. The design behind this shape is in `gui::batch::preview`'s
//! module doc.

use super::remote_lane::run_remote_lane;
use crate::{
    BatchModel, MainWindow,
    bridge::preview_render::{self, CacheKind, FingerprintMaterial, PreviewJob, PreviewView},
    gui::{
        batch::{
            batch_queue::WorkQueue,
            remote_dispatch::{DispatcherGroup, RemoteStatus},
        },
        progress_eta::{EtaEstimator, batch_eta_label},
    },
    settings::WorkerSettings,
};
use pleochroma::{
    optics::{materials::GemMaterial, raytracer::DEFAULT_MAX_BOUNCES},
    renderer::gpu_backend::GpuBackend,
};
use pleochroma_vault::{db::sqlite::Database, model::preview::PreviewMaterialState};
use slint::{ComponentHandle, Weak};
use std::{
    any::Any,
    collections::{BTreeSet, HashMap},
    panic::{self, AssertUnwindSafe},
    sync::{
        Mutex, OnceLock, PoisonError,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    thread,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tracing::warn;

mod accumulator;
mod ray;
mod resolve;

pub(super) use accumulator::{DesignAccum, RecordRevision};
use accumulator::{FinishedViews, record_item_result};
#[cfg(test)]
pub(super) use resolve::record_planes_with_frosting;
pub(super) use resolve::{ResolvedDesign, resolve_design};

/// Every preview render's bounce cap. Unlike `preview_size`/`preview_spp` it is not a
/// setting: it is the raytracer's [`DEFAULT_MAX_BOUNCES`], the live viewport's default.
pub(super) const PREVIEW_MAX_BOUNCES: u32 = DEFAULT_MAX_BOUNCES;

/// How close a `GemMaterial` preset's RI must be to a design's own RI to count as a match.
///
/// The closeness is an absolute refractive-index difference, used by
/// `Database::ensure_preview_material` and `pick_ri_preset`. A judgement, not a
/// measurement: most named species' RI bands are
/// separated by well over this (Quartz ~1.55, Beryl ~1.58), while color varieties of one
/// species deliberately tie and are settled by `gui::batch::material_choice`. `0.02` covers
/// one species' RI spread without merging two species.
///
/// `pub` because `gui::batch::tilt` uses the same tolerance: both batches must pick the same
/// material for a design, since the choice is persisted once and reused.
pub const RI_MATCH_TOLERANCE: f64 = 0.02;

/// The refractive index a design with no parseable `refractive_index` is matched against.
///
/// It is Diamond's sodium-D RI, so such a design still gets a stable, persisted preview
/// material instead of being skipped. A design's own RI always takes priority; this applies
/// only when that field is missing or unparseable.
///
/// `pub` because `gui::batch::tilt` shares it via [`target_ri_for_design`].
pub const FALLBACK_TARGET_RI: f64 = 2.417;

/// A design's target refractive index for `Database::ensure_preview_material` matching.
///
/// It is the design's own `refractive_index` when that parses as a finite number above `1.0`,
/// else [`FALLBACK_TARGET_RI`]. Shared with `gui::batch::tilt`, so both batches match a design
/// the same way (see [`RI_MATCH_TOLERANCE`]).
#[must_use]
pub fn target_ri_for_design(full: &pleochroma_vault::model::entry::FullDiagramRecord) -> f64 {
    full.refractive_index
        .as_deref()
        .and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|ri| ri.is_finite() && *ri > 1.0)
        .unwrap_or(FALLBACK_TARGET_RI)
}

/// How long the local lane sleeps before re-checking the queue when it finds nothing to claim
/// but the remote lane is not yet done (see `gui::batch::batch_queue`). Items take ~1-2 s, so
/// this poll only matters while local waits out the last items still in flight remotely.
const LOCAL_IDLE_POLL: Duration = Duration::from_millis(15);

/// Everything shared, read-only, across every design a batch processes, borrowed by both
/// lanes. It deliberately holds no `GpuBackend`: only the local lanes use the GPU, and
/// keeping it out (passed to them separately) means this struct need not be `Sync` for
/// `std::thread::scope`. `db`, `material_candidates` and `custom_materials` are acquired once
/// per batch (see `super::spawn_preview_batch`); `preview_size`/`preview_spp` are the settings
/// snapshot the whole batch started with.
pub(super) struct BatchContext<'a> {
    pub(super) db: &'a Mutex<Database>,
    pub(super) material_candidates:
        &'a [pleochroma_vault::model::material_match::RiPresetCandidate],
    /// The vault's custom materials (`catalogue_material::load_custom_materials`), against
    /// which a design file's material name is resolved before the built-ins, as the
    /// editor's viewport does.
    pub(super) custom_materials: &'a [GemMaterial],
    pub(super) preview_size: u32,
    pub(super) preview_spp: u32,
    pub(super) engine: pleochroma::renderer::engine::EngineSpec,
    pub(super) ray_draft: bool,
    /// Draw the fast solid stand-in pictures instead of the traced ones: no material is
    /// chosen, no GPU or remote lane is used, and the pictures are stored under a
    /// fingerprint that still counts the design as missing its traced preview.
    pub(super) solid: bool,
    /// Set once a caught local render panic names a wgpu or mapped-buffer failure (see
    /// [`catch_local_render`]). Never cleared for this batch: every local lane shares one
    /// `GpuBackend`, whose staging buffers may still be mapped, so none may dispatch into it
    /// again. A plain flag, since `GpuBackend` has no retire method.
    pub(super) gpu_retired: &'a AtomicBool,
    /// The designs whose geometry came from the angle table rather than a design file (see
    /// `gui::batch::record_planes_with_frosting_for_batch`). Read once for the summary line.
    pub(super) angle_table_entries: &'a Mutex<BTreeSet<i64>>,
}

/// One claimable unit of work: one view of one design, finer than the tilt batch's
/// whole-design item (see `gui::batch::preview`'s module doc).
#[derive(Debug, Clone, Copy)]
pub(super) struct PreviewItem {
    pub(super) entry_id: i64,
    pub(super) view: PreviewView,
}

/// Turns a `catch_unwind` payload into a message, as `catch_file_panic` does for imports.
pub(super) fn panic_message(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_string())
}

/// Runs `f` (one local view's render) under `catch_unwind`, so a panic cannot take the rest of
/// the batch down (see `gui::batch::preview`'s module doc). A panic whose message names a wgpu
/// or mapped-buffer failure also retires the shared `GpuBackend` for the rest of the batch: a
/// failed readback can leave a staging buffer mapped, and every later dispatch on that
/// renderer would fail the same way (see [`BatchContext`]).
fn catch_local_render(
    view: PreviewView,
    gpu_retired: &AtomicBool,
    f: impl FnOnce() -> Option<Vec<u8>>,
) -> Option<Vec<u8>> {
    panic::catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|payload| {
        let message = panic_message(&*payload);
        if message.contains("wgpu") || message.contains("still mapped") {
            tracing::error!(
                "Preview render panicked for a {view:?} view with a GPU-fatal message; \
                 retiring the shared GPU backend for the rest of this batch: {message}"
            );
            gpu_retired.store(true, Ordering::Relaxed);
        } else {
            warn!("Preview render panicked for a {view:?} view: {message}");
        }
        None
    })
}

/// Renders `resolved`'s `view` on the local engine (GPU, with CPU scanline fallback; see
/// `bridge::preview_render::render_view`). Declines without dispatching once
/// [`BatchContext::gpu_retired`] is set (see [`catch_local_render`]).
fn render_item_local(
    ctx: &BatchContext<'_>,
    gpu: &GpuBackend,
    resolved: &ResolvedDesign,
    view: PreviewView,
) -> Option<Vec<u8>> {
    if ctx.solid {
        return catch_local_render(view, ctx.gpu_retired, || {
            preview_render::render_view_solid_with_tools(
                &resolved.planes,
                &resolved.tools,
                ctx.preview_size,
                view,
            )
        });
    }
    if ctx.gpu_retired.load(Ordering::Relaxed) {
        return None;
    }
    let job = PreviewJob {
        engine: ctx.engine,
        planes: &resolved.planes,
        tools: &resolved.tools,
        frosted: &resolved.frosted,
        effects: &resolved.effects,
        material: &resolved.material,
        size: ctx.preview_size,
        spp: ctx.preview_spp,
        max_bounces: PREVIEW_MAX_BOUNCES,
    };
    catch_local_render(view, ctx.gpu_retired, || {
        preview_render::render_view(&job, view, gpu)
    })
}

/// Running totals both lanes update concurrently -- plain atomics (not a `Mutex`)
/// suffice since `generated`/`failed` are independent counters with no invariant
/// between them that needs atomic coupling.
#[derive(Default)]
pub(super) struct Tally {
    pub(super) generated: AtomicU32,
    pub(super) failed: AtomicU32,
}

/// Live status all lanes update as they claim and finish items. [`push_progress`] sends one
/// coherent snapshot of it to the UI thread. Local lanes are a busy count, not one current
/// title, because several run at once.
#[derive(Default, Clone)]
pub(super) struct LiveProgress {
    /// Designs whose BOTH views are fully accounted for so far, by any lane.
    completed: u32,
    /// How many local lanes currently have an item claimed -- out of
    /// `preview_batch_local_lane_total` (see [`push_progress`]'s `local_lane_total`
    /// parameter, fixed for the whole batch so it isn't duplicated into this
    /// per-update struct).
    local_active: u32,
    /// The remote dispatchers' side: how many run, how many items are on the remote,
    /// and the most recently started item's title. Changed only through
    /// [`update_remote`].
    remote: RemoteStatus,
    /// Fed `completed / design_total` on every [`push_progress`], so the remaining
    /// time reflects the mixed local + remote throughput of the whole batch.
    eta: EtaEstimator,
}

/// Pushes a snapshot of `progress` to the UI thread after any lane's status change.
/// `local_lane_total` (from `batch_queue::local_lane_count`) is fixed for the batch, so it is
/// a parameter here rather than a [`LiveProgress`] field, like `design_total`.
pub(super) fn push_progress(
    ui_weak: &Weak<MainWindow>,
    design_total: u32,
    local_lane_total: u32,
    progress: &Mutex<LiveProgress>,
) {
    let (snapshot, eta_text) = {
        let mut p = progress.lock().unwrap_or_else(PoisonError::into_inner);
        let now = std::time::Instant::now();
        if design_total > 0 {
            let fraction = f64::from(p.completed) / f64::from(design_total);
            p.eta.observe(now, fraction);
        }
        let eta_text = batch_eta_label(p.completed, design_total, p.eta.eta(now));
        (p.clone(), eta_text)
    };
    let ui_weak = ui_weak.clone();
    let _ = ui_weak.upgrade_in_event_loop(move |ui| {
        ui.global::<BatchModel>().set_preview_eta(eta_text.into());
        ui.global::<BatchModel>()
            .set_preview_design_index(slint_count(snapshot.completed));
        ui.global::<BatchModel>()
            .set_preview_design_total(slint_count(design_total));
        ui.global::<BatchModel>()
            .set_preview_local_active(slint_count(snapshot.local_active));
        ui.global::<BatchModel>()
            .set_preview_local_lane_total(slint_count(local_lane_total));
        ui.global::<BatchModel>()
            .set_preview_remote_title(snapshot.remote.title().into());
        ui.global::<BatchModel>()
            .set_preview_remote_active(snapshot.remote.is_active());
        ui.global::<BatchModel>()
            .set_preview_remote_in_flight(slint_count(snapshot.remote.in_flight()));
    });
}

/// A `u32` count as the `int` a Slint property holds, saturating instead of wrapping negative.
fn slint_count(count: u32) -> i32 {
    i32::try_from(count).unwrap_or(i32::MAX)
}

/// Applies `change` to the shared remote status -- every remote dispatcher goes through
/// here, so no two of them can interleave a half-finished update.
pub(super) fn update_remote(
    progress: &Mutex<LiveProgress>,
    change: impl FnOnce(&mut RemoteStatus),
) {
    change(
        &mut progress
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remote,
    );
}

fn increment_completed(progress: &Mutex<LiveProgress>) {
    progress
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .completed += 1;
}

/// Marks one local lane as having just claimed an item (`+1`) or finished one (`-1`). A signed
/// step, not separate increment and decrement functions (as in the tilt batch).
fn adjust_local_active(progress: &Mutex<LiveProgress>, delta: i32) {
    let mut p = progress.lock().unwrap_or_else(PoisonError::into_inner);
    p.local_active = p.local_active.saturating_add_signed(delta);
}

/// What both lane runners share by plain reference across the `std::thread::scope` in
/// `super::spawn_preview_batch`. It excludes `GpuBackend` (see [`BatchContext`]) and
/// `Weak<MainWindow>`, which each lane takes as its own clone. It also bundles the arguments
/// [`finish_item`] needs, to keep that call short.
pub(super) struct LaneShared<'a> {
    pub(super) ctx: &'a BatchContext<'a>,
    pub(super) queue: &'a WorkQueue<PreviewItem>,
    pub(super) design_state: &'a Mutex<HashMap<i64, DesignAccum>>,
    pub(super) tally: &'a Tally,
    pub(super) progress: &'a Mutex<LiveProgress>,
    pub(super) cancel: &'a AtomicBool,
    pub(super) design_total: u32,
    /// How many local lanes this batch spawned -- see `batch_queue::local_lane_count`'s
    /// own doc comment. Fixed for the whole batch; threaded through here (rather than
    /// read fresh by each lane) purely so every `push_progress` call site has it without
    /// needing its own separate parameter.
    pub(super) local_lane_total: u32,
    /// How many remote dispatchers this batch spawns -- see
    /// `batch_queue::remote_lane_count`. `0` when no remote lane runs (`LocalOnly`).
    pub(super) remote_lane_total: u32,
    /// Pictures one batched remote request carries (`AppSettings::remote_preview_batch_size`);
    /// `remote_lane::remote_batch` claims that many at a time.
    pub(super) remote_batch_size: usize,
    /// Set by the first remote dispatcher to start sitting a failing remote out, so the
    /// "remote lane paused" toast shows once per batch however many dispatchers (each
    /// with its own failure count) reach their own sit-out.
    pub(super) sit_out_toasted: AtomicBool,
    /// The library card thumbnail cache, invalidated per design in [`finish_item`] once
    /// its previews are saved.
    pub(super) thumbnail_cache: &'a crate::gui::batch::preview_cache::PreviewThumbnailCache,
}

/// The fingerprint a finished design's pair is stored under: the batch's picture kind
/// (traced at the batch's size and sample count, or the solid stand-in) and the design's
/// persisted `material` -- its name, stone size and provenance
/// (`bridge::preview_render::cache_fingerprint`), plus the `frosting` hash of its stored
/// frosting (`gui::batch::frosting_scan::stored_frosting_hash`; `0` for a polished design)
/// and the `zones` hash of its material's stored zones (`stored_zones_hash`; `0` for none).
/// The staleness scan rebuilds the same text from the same persisted rows.
pub(super) fn preview_fingerprint(
    ctx: &BatchContext<'_>,
    material: &PreviewMaterialState,
    frosting: u64,
    zones: u64,
    axis: Option<preview_render::AxisKey>,
    effects: preview_render::EffectsKey,
) -> String {
    let kind = ray::cache_kind(ctx);
    preview_render::cache_fingerprint(
        kind,
        FingerprintMaterial::from_state(material)
            .with_frosting(frosting)
            .with_zones(zones)
            .with_axis(axis)
            .with_effects(effects),
    )
}

/// Stores a finished design's pair, unless the design changed while it was rendered.
/// Returns whether the pair was stored.
///
/// The images go in only if the catalogue row still carries the `updated_at` the items
/// resolved their record at (`Database::save_preview_images`'s compare-and-swap), so a
/// re-import, metadata edit or re-sync that lands during the seconds of rendering is
/// never followed by a write of the old geometry's pictures. A design whose two items
/// disagree about the record version is refused the same way. An item pair that never
/// reported a revision (`RecordRevision::Unknown`) is saved against the row's current
/// stamp, i.e. without that protection.
fn save_finished_design(shared: &LaneShared<'_>, entry_id: i64, done: &FinishedViews) -> bool {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX));
    let guard = shared.ctx.db.lock().unwrap_or_else(PoisonError::into_inner);
    let expected_updated_at = match done.revision {
        RecordRevision::Stamp(stamp) => stamp,
        RecordRevision::Unknown => match guard.entry_updated_at(entry_id) {
            Ok(stamp) => stamp,
            Err(e) => {
                warn!("Could not save the previews for entry {entry_id}: {e}");
                return false;
            }
        },
        RecordRevision::Conflicting => {
            warn!(
                "Discarded the previews rendered for entry {entry_id}: the design changed \
                 between its two views"
            );
            return false;
        }
    };
    // The material, size and provenance `resolve_design` persisted for this design.
    let material = guard
        .get_preview_material_state(entry_id)
        .unwrap_or_default();
    // The frosting the design is stored with: the CAS on `updated_at` below refuses a result
    // rendered before a re-import or save moved it.
    let frosting = crate::gui::batch::frosting_scan::stored_frosting_hash(&guard, entry_id);
    let zones =
        crate::gui::batch::frosting_scan::stored_zones_hash(&guard, material.name.as_deref());
    // ... and the crystal axis: the design's own when its file sets one, else the material's
    // stored default, which the scan reads by name.
    let axis = crate::gui::batch::frosting_scan::effective_axis_key(
        &guard,
        entry_id,
        material.name.as_deref(),
    );
    // ... and the surface finish and abraded edge the design states, with the user's machine.
    let effects = crate::gui::batch::frosting_scan::stored_effects_key(&guard, entry_id);
    let fingerprint = preview_fingerprint(shared.ctx, &material, frosting, zones, axis, effects);
    let stored = guard.save_preview_images(
        entry_id,
        done.front.as_deref(),
        done.top.as_deref(),
        now,
        &fingerprint,
        expected_updated_at,
    );
    drop(guard);
    match stored {
        Ok(true) => true,
        Ok(false) => {
            warn!(
                "Discarded the previews rendered for entry {entry_id}: the design changed \
                 while they were rendering"
            );
            false
        }
        Err(e) => {
            warn!("Failed to save the previews for entry {entry_id}: {e}");
            false
        }
    }
}

/// [`finish_item_at_revision`] for an item with no record revision to report. Right for
/// a failure hand-back (`bytes` is `None`, so there is nothing to protect); a caller
/// holding rendered bytes should pass its design's [`ResolvedDesign::revision`] to
/// [`finish_item_at_revision`] instead, since an item without one leaves its half of the
/// pair unchecked against edits made during the render.
pub(super) fn finish_item(
    shared: &LaneShared<'_>,
    ui_weak: &Weak<MainWindow>,
    entry_id: i64,
    view: PreviewView,
    bytes: Option<Vec<u8>>,
) {
    finish_item_at_revision(
        shared,
        ui_weak,
        entry_id,
        view,
        bytes,
        RecordRevision::Unknown,
    );
}

/// Persists a finished design's front/top pair (whatever combination succeeded) and
/// folds the result into `shared.tally`/`shared.progress` -- called once a design's
/// LAST outstanding item comes back, from whichever lane that happens to be (see
/// [`record_item_result`]). `revision` is the record version this item's `bytes` were
/// rendered from ([`ResolvedDesign::revision`]); see [`save_finished_design`] for how
/// the design's items' revisions gate the save.
pub(super) fn finish_item_at_revision(
    shared: &LaneShared<'_>,
    ui_weak: &Weak<MainWindow>,
    entry_id: i64,
    view: PreviewView,
    bytes: Option<Vec<u8>>,
    revision: RecordRevision,
) {
    let Some(done) = record_item_result(shared.design_state, entry_id, view, bytes, revision)
    else {
        // This design still has its other view in flight elsewhere -- nothing to save
        // or tally yet.
        return;
    };

    let saved = (done.front.is_some() || done.top.is_some())
        && save_finished_design(shared, entry_id, &done);

    if saved {
        shared.tally.generated.fetch_add(1, Ordering::Relaxed);
        // The library card re-queries this design's thumbnails on its next frame,
        // replacing the placeholder or the stale image.
        shared.thumbnail_cache.invalidate(ui_weak, entry_id);
    } else {
        shared.tally.failed.fetch_add(1, Ordering::Relaxed);
    }
    increment_completed(shared.progress);
    push_progress(
        ui_weak,
        shared.design_total,
        shared.local_lane_total,
        shared.progress,
    );
}

/// The local lanes' GPU. The batch acquires it on a thread of its own, so neither the
/// remote dispatchers nor the first local pictures wait for adapter acquisition and the
/// megakernel compile. Until it is ready a local picture is traced on the CPU, the fallback
/// every picture the GPU declines takes anyway.
pub(super) struct LocalGpu {
    /// Set once, by [`Self::acquire`].
    acquired: OnceLock<GpuBackend>,
    /// What a picture renders with until then: a backend that always declines.
    pending: GpuBackend,
}

impl LocalGpu {
    /// A GPU not acquired yet.
    pub(super) const fn new() -> Self {
        Self {
            acquired: OnceLock::new(),
            pending: GpuBackend::disabled(),
        }
    }

    /// Acquires the adapter and compiles the megakernel. Slow: called once per batch, on a
    /// thread of its own.
    pub(super) fn acquire(&self) {
        let _ = self.acquired.set(GpuBackend::acquire());
    }

    /// The backend a local picture renders with now: the acquired one once it is ready,
    /// otherwise the declining stand-in.
    pub(super) fn current(&self) -> &GpuBackend {
        self.acquired.get().unwrap_or(&self.pending)
    }
}

/// Runs one local lane: claims items via `WorkQueue::claim_local` (remote-failed items
/// first, then fresh ones) until both piles are empty and `remote_lane_done` says no more
/// requeues are coming (see `gui::batch::batch_queue`). `gpu` is the [`LocalGpu`] every
/// local lane shares; each picture takes the backend it holds when the picture starts.
/// Concurrent use is safe because the GPU queue serializes the dispatches.
pub(super) fn run_local_lane(
    shared: &LaneShared<'_>,
    gpu: &LocalGpu,
    ui_weak: &Weak<MainWindow>,
    remote_lane_done: &AtomicBool,
) {
    loop {
        if shared.cancel.load(Ordering::Relaxed) {
            break;
        }
        // Read BEFORE the claim: a remote dispatcher raises the flag only after its last
        // requeue, so a claim that follows an observed `true` sees every requeue and an
        // empty answer is final. Read after the claim, a requeue landing between the two
        // would be stranded.
        let remote_finished = remote_lane_done.load(Ordering::Acquire);
        let Some(item) = shared.queue.claim_local() else {
            if remote_finished {
                break;
            }
            thread::sleep(LOCAL_IDLE_POLL);
            continue;
        };

        adjust_local_active(shared.progress, 1);
        push_progress(
            ui_weak,
            shared.design_total,
            shared.local_lane_total,
            shared.progress,
        );

        let resolved = resolve_design(shared.ctx, item.entry_id);
        let revision = resolved
            .as_ref()
            .map_or(RecordRevision::Unknown, |r| r.revision);
        let bytes =
            resolved.and_then(|r| render_item_local(shared.ctx, gpu.current(), &r, item.view));
        finish_item_at_revision(shared, ui_weak, item.entry_id, item.view, bytes, revision);

        adjust_local_active(shared.progress, -1);
        push_progress(
            ui_weak,
            shared.design_total,
            shared.local_lane_total,
            shared.progress,
        );
    }
}

/// One [`PreviewItem`] per view (front, then top) of every id in `entry_ids` -- the
/// initial contents of `super::spawn_preview_batch`'s shared queue.
pub(super) fn build_items(entry_ids: &[i64]) -> Vec<PreviewItem> {
    entry_ids
        .iter()
        .flat_map(|&entry_id| {
            [
                PreviewItem {
                    entry_id,
                    view: PreviewView::Front,
                },
                PreviewItem {
                    entry_id,
                    view: PreviewView::Top,
                },
            ]
        })
        .collect()
}

/// Runs every lane of one batch attempt inside a `std::thread::scope`, blocking until all of
/// them finish. Kept out of `super::spawn_preview_batch` to keep that function short.
///
/// `shared.remote_lane_total` remote dispatchers run at once, all claiming from the same
/// queue; `remote_lane_done` is raised when the last of them ends.
pub(super) fn run_batch_lanes(
    shared: &LaneShared<'_>,
    gpu: &LocalGpu,
    plan: super::super::batch_queue::LanePlan,
    remote_worker: Option<&WorkerSettings>,
    local_lane_total: u32,
    ui_weak: &Weak<MainWindow>,
    remote_lane_done: &AtomicBool,
) {
    // Built before the scope: the scoped dispatchers borrow it for the scope's whole
    // lifetime.
    let dispatchers = DispatcherGroup::new(shared.remote_lane_total as usize, remote_lane_done);
    let dispatchers = &dispatchers;
    std::thread::scope(|scope| {
        if plan.run_remote {
            for _ in 0..shared.remote_lane_total {
                let remote_ui_weak = ui_weak.clone();
                scope.spawn(move || {
                    run_remote_lane(
                        shared,
                        &remote_ui_weak,
                        remote_worker,
                        plan.fallback_to_local,
                        dispatchers,
                    );
                });
            }
        }
        if plan.run_local {
            // The local lanes all share the caller's `gpu` (see `run_local_lane`); the
            // queue is their only coordination.
            for _ in 0..local_lane_total {
                let local_ui_weak = ui_weak.clone();
                scope.spawn(move || {
                    run_local_lane(shared, gpu, &local_ui_weak, remote_lane_done);
                });
            }
        }
    });
}
