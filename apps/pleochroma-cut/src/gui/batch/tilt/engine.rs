//! The per-design resolve, compute and dispatch engine, and the local and remote lane
//! runners that pull designs from a shared [`WorkQueue`].

use crate::{
    BatchModel,
    bridge::preview_render::{EffectsKey, FingerprintMaterial},
    gui::{
        batch::{
            batch_queue::WorkQueue,
            catalogue_material::{CatalogueMaterialSource, catalogue_material},
            preview::target_ri_for_design,
            remote_dispatch::{DispatcherGroup, RemoteStatus},
        },
        editor::CataloguePlanes,
        progress_eta::{EtaEstimator, batch_eta_label},
    },
    settings::WorkerSettings,
};
use pleochroma::{
    color::metrics::{FinishedStone, SweepProgress},
    geometry::{
        facet_finishes_with_frosted,
        plane::GpuFacetPlane,
        tool::{StoneGeometry, ToolPrimitive},
    },
    optics::{materials::GemMaterial, raytracer::FacetFinish},
};
use pleochroma_vault::model::tilt_curves::TiltPerformanceCurves;
use slint::{ComponentHandle, Weak};
use std::{
    any::Any,
    collections::BTreeSet,
    sync::{
        Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    thread,
    time::Duration,
};
use tracing::warn;

#[cfg(test)]
mod frosting_tests;
mod save;
mod sweep;

use super::remote_lane::run_remote_lane;
use save::save_curves;
pub use save::save_tilt_curves_for_entry;
use sweep::{
    compute_tilt_curves_locally, compute_tilt_curves_locally_stepped, fetch_tilt_curves_remote,
};

/// How long a local lane sleeps when the queue is empty but a remote lane may still requeue
/// a design (see `gui::batch::batch_queue`'s module doc). Short, so the batch ends promptly.
const LOCAL_IDLE_POLL: Duration = Duration::from_millis(15);

/// Read-only data shared by every design in a batch.
///
/// It holds no worker, queue or progress state (those live in [`LaneShared`]), so both lanes
/// can borrow `&BatchContext` across the `std::thread::scope` in `super::spawn_tilt_batch`
/// without an `Arc`.
pub(super) struct BatchContext<'a> {
    pub(super) db: &'a Mutex<pleochroma_vault::db::sqlite::Database>,
    pub(super) material_candidates:
        &'a [pleochroma_vault::model::material_match::RiPresetCandidate],
    /// The vault's custom materials, read once per batch by
    /// `catalogue_material::load_custom_materials`. A design file's material name is
    /// resolved against these before the built-ins.
    pub(super) custom_materials: &'a [GemMaterial],
    /// The designs whose geometry came from the angle table rather than a design file
    /// (see `gui::batch::record_planes_with_frosting_for_batch`). Read once at the end for
    /// the batch's summary line.
    pub(super) angle_table_entries: &'a Mutex<BTreeSet<i64>>,
}

/// One design's resolved geometry and material, the prelude of both [`process_local_entry`]
/// and [`process_remote_entry`].
struct ResolvedDesign {
    title: String,
    planes: Vec<GpuFacetPlane>,
    /// The concave tools cut into `planes` (`CataloguePlanes::render_tools`), empty when
    /// the design is planar: the curves are those of the stone WITH its tools.
    tools: Vec<ToolPrimitive>,
    /// The planes the design frosts (ascending indices into `planes`, from its stored
    /// frosting); empty for a polished design and on the angle-table path. A frosted facet
    /// scatters light, so the curves are those of the frosted stone.
    frosted: Vec<u32>,
    material: GemMaterial,
    /// Name of `material`, part of the fingerprint the curves are stored with.
    material_name: String,
    /// Girdle diameter in millimetres `material` was sized with (`0.0`: not set).
    girdle_mm: f64,
    /// How `material_name` was decided.
    material_source: CatalogueMaterialSource,
    /// The catalogue row's `updated_at`, read with the record `planes` come from. It is the
    /// compare-and-swap token of `Database::save_tilt_curves`: curves swept from a superseded
    /// record are refused.
    updated_at: Option<i64>,
}

impl ResolvedDesign {
    /// The stone the curves are swept over: the planes with their concave tools.
    const fn stone(&self) -> StoneGeometry<'_> {
        StoneGeometry {
            planes: self.planes.as_slice(),
            tools: self.tools.as_slice(),
        }
    }

    /// The per-facet finishes of [`Self::stone`]: frosted where the design authors frosting,
    /// empty (every facet polished) otherwise. The girdle toggle is a live-view setting no
    /// catalogue design stores, so only the authored frosting counts.
    fn finishes(&self) -> Vec<FacetFinish> {
        facet_finishes_with_frosted(&self.planes, false, &self.frosted)
    }

    /// The material part of the fingerprint the design's curves are stored with: the same
    /// name, size and source the scans rebuild from the persisted row.
    fn fingerprint_material(&self) -> FingerprintMaterial<'_> {
        FingerprintMaterial {
            name: Some(self.material_name.as_str()),
            girdle_mm: self.girdle_mm,
            from_design_file: self.material_source == CatalogueMaterialSource::DesignFile,
            // The design's stored frosting is added when the curves are saved
            // (`save::save_curves`), under the same lock as the write; the compare-and-swap
            // on `updated_at` there refuses curves swept before a re-import or save moved it.
            frosting: 0,
            // The material's stored zones are added the same way, in `save::save_curves`.
            zones: 0,
            // The custom material's stored default axis, likewise.
            axis: None,
            // The design's render physics, likewise (none until the design carries any).
            effects: EffectsKey::NONE,
        }
    }
}

/// `full`'s facet planes for a tilt-curve sweep.
///
/// The design file comes first and the angle table only as the fallback
/// (`gui::batch::record_planes_with_frosting_for_batch`, shared with the preview batch and
/// the library detail view). A remote `TILT_CURVES` request ships these planes in its scene,
/// so the worker never resolves a record itself.
///
/// The second value is the design's frosted planes: the ascending indices into the returned
/// planes the design frosts, the form the sweep's finishes and a remote scene's
/// `frosted_planes` take. Empty for a polished design and on the angle-table path.
pub(super) fn record_planes_with_frosting(
    ctx: &BatchContext<'_>,
    full: &pleochroma_vault::model::entry::FullDiagramRecord,
) -> Option<(CataloguePlanes, Vec<u32>)> {
    super::super::record_planes_with_frosting_for_batch(full, ctx.angle_table_entries)
}

/// Resolves `entry_id`'s geometry and material, or `None` if either step fails.
///
/// A step fails on an unreadable row, unreconstructable geometry or no matched material. Both
/// lanes call it, so each resolves its own copy; nothing is cached across lanes.
///
/// The material is the one `gui::batch::catalogue_material::catalogue_material` decides:
/// the design file's own, else the refractive-index pick, sized by the girdle diameter.
/// The curves are thus scored on the stone the design's thumbnail shows.
fn resolve_design(ctx: &BatchContext<'_>, entry_id: i64) -> Option<ResolvedDesign> {
    // The record and its revision stamp are read under one lock hold, so the stamp
    // describes exactly this record.
    let (full, updated_at) = {
        let guard = ctx.db.lock().unwrap_or_else(PoisonError::into_inner);
        (
            guard.get_diagram_full(entry_id),
            guard.entry_updated_at(entry_id),
        )
    };
    let Ok(Some(full)) = full else {
        return None;
    };
    let Ok(updated_at) = updated_at else {
        return None;
    };
    let (resolved, frosted) = record_planes_with_frosting(ctx, &full)?;

    let target_ri = target_ri_for_design(&full);
    let chosen = {
        let guard = ctx.db.lock().unwrap_or_else(PoisonError::into_inner);
        catalogue_material(
            &guard,
            entry_id,
            &resolved,
            ctx.custom_materials,
            ctx.material_candidates,
            target_ri,
        )
    };
    let chosen = match chosen {
        Ok(Some(chosen)) => chosen,
        Ok(None) => return None,
        Err(e) => {
            warn!("Could not choose the tilt-curve material for entry {entry_id}: {e}");
            return None;
        }
    };

    Some(ResolvedDesign {
        title: full.title,
        planes: resolved.planes,
        tools: resolved.render_tools,
        frosted,
        material: chosen.material,
        material_name: chosen.name,
        girdle_mm: chosen.girdle_mm,
        material_source: chosen.source,
        updated_at,
    })
}

/// Resolves and computes `entry_id`'s tilt curves on the LOCAL engine. Returns `true`
/// iff curves were computed and saved.
///
/// Reports no title: with several local lanes no single title is meaningful. [`run_local_lane`]
/// reports only that a lane is busy (`local_active`).
fn process_local_entry(ctx: &BatchContext<'_>, entry_id: i64, cancel: &AtomicBool) -> bool {
    let Some(resolved) = resolve_design(ctx, entry_id) else {
        return false;
    };
    let finishes = resolved.finishes();
    let Some(curves) = compute_tilt_curves_locally(
        FinishedStone::new(resolved.stone(), &finishes),
        &resolved.material,
        cancel,
    ) else {
        return false;
    };
    save_curves(
        ctx.db,
        entry_id,
        &curves,
        resolved.fingerprint_material(),
        resolved.updated_at,
    )
}

/// Resolves `entry_id` and dispatches its tilt curves to `worker`.
///
/// It reports the design's title as soon as it is known. Returns `true` iff curves were
/// computed and saved. With no worker (`None`) it is an immediate shortfall that never touches
/// the network.
pub(super) fn process_remote_entry(
    ctx: &BatchContext<'_>,
    worker: Option<&WorkerSettings>,
    entry_id: i64,
    cancel: &AtomicBool,
    mut on_title: impl FnMut(&str),
) -> bool {
    let Some(resolved) = resolve_design(ctx, entry_id) else {
        return false;
    };
    on_title(&resolved.title);
    let Some(worker) = worker else {
        return false;
    };
    let Some(curves) = fetch_tilt_curves_remote(
        worker,
        resolved.stone(),
        &resolved.frosted,
        &resolved.material,
        cancel,
    ) else {
        return false;
    };
    save_curves(
        ctx.db,
        entry_id,
        &curves,
        resolved.fingerprint_material(),
        resolved.updated_at,
    )
}

pub(super) fn panic_message(payload: &(dyn Any + Send)) -> String {
    payload
        .downcast_ref::<&str>()
        .map(|s| (*s).to_string())
        .or_else(|| payload.downcast_ref::<String>().cloned())
        .unwrap_or_else(|| "unknown panic".to_string())
}

/// Running totals updated concurrently by all lanes. Plain atomics suffice: the two
/// counters have no invariant that needs them updated together.
#[derive(Default)]
pub(super) struct Tally {
    pub(super) computed: AtomicU32,
    pub(super) failed: AtomicU32,
}

/// Live status all lanes update as they claim and finish designs.
///
/// [`push_progress`] sends a snapshot of it to the UI thread. `completed` counts finished
/// designs, and local lanes are a busy count rather than a single title.
#[derive(Default, Clone)]
pub(super) struct LiveProgress {
    /// Designs fully accounted for so far, by any lane, success or failure.
    completed: u32,
    /// How many local lanes currently have a design claimed, out of
    /// `tilt_batch_local_lane_total` (passed to [`push_progress`]).
    local_active: u32,
    /// The remote dispatchers' side: how many run, how many designs are on the remote,
    /// and the most recently started design's title. Changed only through
    /// [`update_remote`].
    remote: RemoteStatus,
    /// Fed `completed / design_total` on every [`push_progress`], so the remaining
    /// time reflects the mixed local + remote throughput of the whole batch.
    eta: EtaEstimator,
}

/// Pushes a snapshot of `progress` to the UI thread, after any lane's status change.
///
/// `design_total` and `local_lane_total` are fixed for the batch, so they are passed in
/// rather than stored in [`LiveProgress`].
pub(super) fn push_progress(
    ui_weak: &Weak<crate::MainWindow>,
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
        ui.global::<BatchModel>().set_tilt_eta(eta_text.into());
        ui.global::<BatchModel>()
            .set_tilt_design_index(snapshot.completed.cast_signed());
        ui.global::<BatchModel>()
            .set_tilt_design_total(design_total.cast_signed());
        ui.global::<BatchModel>()
            .set_tilt_local_active(snapshot.local_active.cast_signed());
        ui.global::<BatchModel>()
            .set_tilt_local_lane_total(local_lane_total.cast_signed());
        ui.global::<BatchModel>()
            .set_tilt_remote_title(snapshot.remote.title().into());
        ui.global::<BatchModel>()
            .set_tilt_remote_active(snapshot.remote.is_active());
        ui.global::<BatchModel>()
            .set_tilt_remote_in_flight(snapshot.remote.in_flight().cast_signed());
    });
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

pub(super) fn increment_completed(progress: &Mutex<LiveProgress>) {
    progress
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .completed += 1;
}

/// Marks a local lane busy (`delta` `1`, after a claim) or idle again (`delta` `-1`, after
/// a design finishes).
fn adjust_local_active(progress: &Mutex<LiveProgress>, delta: i32) {
    let mut p = progress.lock().unwrap_or_else(PoisonError::into_inner);
    p.local_active = p.local_active.saturating_add_signed(delta);
}

/// Per-batch state every lane borrows across the `std::thread::scope`.
///
/// It excludes `Weak<MainWindow>`: each lane gets its own clone, so this struct needs no
/// `Sync` bound on Slint's `Weak`.
pub(super) struct LaneShared<'a> {
    pub(super) ctx: &'a BatchContext<'a>,
    pub(super) queue: &'a WorkQueue<i64>,
    pub(super) tally: &'a Tally,
    pub(super) progress: &'a Mutex<LiveProgress>,
    pub(super) cancel: &'a AtomicBool,
    pub(super) design_total: u32,
    /// Local lanes spawned for this batch (`batch_queue::local_lane_count`), fixed for the batch.
    pub(super) local_lane_total: u32,
    /// Remote dispatchers spawned for this batch (`batch_queue::remote_lane_count`). `0` when
    /// no remote lane runs (`LocalOnly`).
    pub(super) remote_lane_total: u32,
}

/// Runs one LOCAL lane.
///
/// It claims designs via `WorkQueue::claim_local` (requeued remote failures first, then fresh
/// ones) until both piles are empty and `remote_lane_done` says no more work is coming. See
/// `gui::batch::batch_queue`'s module doc for why both conditions are needed. N lanes share
/// `shared.queue` and balance through it alone.
pub(super) fn run_local_lane(
    shared: &LaneShared<'_>,
    ui_weak: &Weak<crate::MainWindow>,
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
        let Some(entry_id) = shared.queue.claim_local() else {
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

        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            process_local_entry(shared.ctx, entry_id, shared.cancel)
        }));

        let saved = match result {
            Ok(saved) => saved,
            Err(payload) => {
                warn!(
                    "Tilt-curve computation panicked for entry {entry_id}: {}",
                    panic_message(&*payload)
                );
                false
            }
        };

        if saved {
            shared.tally.computed.fetch_add(1, Ordering::Relaxed);
        } else {
            shared.tally.failed.fetch_add(1, Ordering::Relaxed);
        }
        adjust_local_active(shared.progress, -1);
        increment_completed(shared.progress);
        push_progress(
            ui_weak,
            shared.design_total,
            shared.local_lane_total,
            shared.progress,
        );
    }
}

/// Computes one design's tilt curves from an already-resolved stone and material.
///
/// `stone` holds the planes with their concave tools (a planar design passes
/// `StoneGeometry::planes_only`). It is typically the open design's solved stone with
/// `gui::editor::material_lookup::resolved_gem_material`, not a database row. That bypasses
/// [`resolve_design`], which only sees what the catalogue row stores. Routing the Edit tab's
/// "analyse the design on the bench" action through the batch would score a stale on-disk
/// copy, or fail for a design that was never saved.
///
/// Tries `worker` first when given and falls back to the local engine on any remote
/// shortfall, as [`LiveComputeTarget::Both`] does for the catalogue batch. `worker: None`
/// runs locally only, as [`LiveComputeTarget::LocalOnly`]. It reuses
/// [`fetch_tilt_curves_remote`] and [`compute_tilt_curves_locally`], so a single design and
/// a catalogue batch always compute curves the same way. Unlike [`process_local_entry`] and
/// [`process_remote_entry`], it never calls [`save_curves`]: unsaved or edited work must not
/// overwrite the stored catalogue curves unasked. A caller that wants to persist does so
/// itself, with an `entry_id`.
///
/// This blocks for a full local sweep or a remote round trip. Run it off the UI thread and
/// marshal the result back, like other worker calls such as
/// `gui::tilt::tilt_profile::spawn_tilt_profile_sweep`.
///
/// `frosted` lists the planes the design frosts (ascending indices into `stone.planes`, as
/// `Design::frosted_planes_from_solved` or `StoneGeometryBuf::frosted` give them; empty for a
/// polished design). A frosted facet scatters light, so the sweep is the frosted stone's.
///
/// `on_step` reports the LOCAL sweep's progress, before each raytrace evaluation (see
/// [`compute_tilt_curves_locally_stepped`]). A remote sweep is one blocking request and
/// reports nothing.
///
/// [`LiveComputeTarget::Both`]: crate::settings::LiveComputeTarget::Both
/// [`LiveComputeTarget::LocalOnly`]: crate::settings::LiveComputeTarget::LocalOnly
#[must_use]
pub fn tilt_curves_for_stone(
    stone: StoneGeometry<'_>,
    frosted: &[u32],
    material: &GemMaterial,
    worker: Option<&WorkerSettings>,
    cancel: &AtomicBool,
    on_step: &mut dyn FnMut(SweepProgress),
) -> Option<TiltPerformanceCurves> {
    // A `TILT_CURVES` request carries no colour zones (it is a bare `SceneState`; the
    // zoning payload is only sent with render requests), so a zoned material's curves are
    // always computed locally.
    let worker = worker.filter(|_| material.zoning.is_none());
    if let Some(worker) = worker
        && let Some(curves) = fetch_tilt_curves_remote(worker, stone, frosted, material, cancel)
    {
        return Some(curves);
    }
    let finishes = facet_finishes_with_frosted(stone.planes, false, frosted);
    compute_tilt_curves_locally_stepped(
        FinishedStone::new(stone, &finishes),
        material,
        cancel,
        on_step,
    )
}

/// Runs every lane of one batch in a `std::thread::scope` and blocks until all finish. It is
/// kept out of `super::spawn_tilt_batch` to stay under clippy's line-count limit.
///
/// `shared.remote_lane_total` remote dispatchers run at once, all claiming from the same
/// queue; `remote_lane_done` is raised when the last of them ends.
pub(super) fn run_batch_lanes(
    shared: &LaneShared<'_>,
    plan: super::super::batch_queue::LanePlan,
    remote_worker: Option<&WorkerSettings>,
    local_lane_total: u32,
    ui_weak: &Weak<crate::MainWindow>,
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
            // N independently claiming local lanes. Each runs the same `run_local_lane`;
            // `WorkQueue` keeps them from claiming the same design.
            for _ in 0..local_lane_total {
                let local_ui_weak = ui_weak.clone();
                scope.spawn(move || {
                    run_local_lane(shared, &local_ui_weak, remote_lane_done);
                });
            }
        }
    });
}
