//! Batched preview tracing, including deterministic tap ranges.
use super::{
    AssertUnwindSafe, AtomicBool, BatchItem, Instant, Ordering, QueuedBatch, StageJob, TraceEnv,
    Vec3, assets, catch_unwind, check_item, render_core,
};
use pleochroma::{
    optics::raytracer::{Camera, DEFAULT_FOV_DEG, FacetFinish, surface::FacetSurface},
    renderer::gpu_backend::{GpuAccumulate, GpuBatchItem, GpuSceneRef},
};
/// The tracer thread's body for one batch.
pub(super) fn trace_batch(env: &TraceEnv, batch: &QueuedBatch) {
    let request_id = batch.request.request_id;
    let cancel: &AtomicBool = &batch.cancel;
    let started = Instant::now();

    let mut gpu_items: Vec<(usize, &BatchItem)> = Vec::new();
    let mut cpu_items: Vec<(usize, &BatchItem)> = Vec::new();
    // Each item's Stone Shadow and Tray Reflection (by item index), resolved once here, up front,
    // and held for the whole batch: a batch item is a finished picture, so it takes the Final
    // map. An item whose map or mirror cannot be built fails by itself with the reason; the
    // others are traced.
    let mut trays: Vec<assets::Tray> = vec![
        assets::Tray {
            shadow: None,
            mirror: None,
        };
        batch.request.items.len()
    ];
    for (index, item) in batch.request.items.iter().enumerate() {
        if let Err(reason) = check_item(item) {
            env.fail(request_id, item.item_id, reason);
            continue;
        }
        match assets::resolve_tray(&item.scene, None, cancel) {
            Ok(tray) => trays[index] = tray,
            // `cancel` was raised: the batch ends as cancelled below.
            Err(assets::TrayStop::Cancelled) => continue,
            Err(assets::TrayStop::Refused(message)) => {
                env.fail(request_id, item.item_id, message);
                continue;
            }
        }
        render_core::note_tray_route(&item.scene, &env.gpu);
        if render_core::scene_uses_gpu(&item.scene, &env.gpu) {
            gpu_items.push((index, item));
        } else {
            cpu_items.push((index, item));
        }
    }

    if !cancel.load(Ordering::Relaxed) {
        let declined = trace_on_gpu(env, request_id, &gpu_items, &trays, cancel);
        cpu_items.extend(declined);
        cpu_items.sort_by_key(|(index, _)| *index);
        trace_on_cpu(env, request_id, &cpu_items, &trays, cancel);
    }

    let cancelled = cancel.load(Ordering::Relaxed);
    let _ = env.stage.send(StageJob::End {
        request_id,
        cancelled,
    });
    tracing::info!(
        request_id,
        items = batch.request.items.len(),
        gpu_items = gpu_items.len(),
        cancelled,
        traced_in = ?started.elapsed(),
        "worker: traced a preview batch"
    );
}

/// Traces `items` in ONE `GpuBackend::try_accumulate_batch_cancellable` call and hands
/// every finished item to the encoder. Returns the items the GPU declined, for the CPU.
///
/// Delivery happens after the call returns, because the call holds every item's output
/// buffer for its whole duration; the encoder overlaps the NEXT batch's tracing instead.
fn trace_on_gpu<'a>(
    env: &TraceEnv,
    request_id: u32,
    items: &[(usize, &'a BatchItem)],
    trays: &[assets::Tray],
    cancel: &AtomicBool,
) -> Vec<(usize, &'a BatchItem)> {
    if items.is_empty() {
        return Vec::new();
    }
    let cameras: Vec<Camera> = items
        .iter()
        .map(|(_, item)| {
            Camera::new(
                item.scene.yaw,
                item.scene.pitch,
                item.scene.distance,
                DEFAULT_FOV_DEG,
            )
            .with_roll(item.scene.camera_roll)
        })
        .collect();
    let finishes: Vec<Vec<FacetFinish>> = items
        .iter()
        .map(|(_, item)| render_core::resolve_facet_finishes(&item.scene))
        .collect();
    let materials: Vec<_> = items
        .iter()
        .map(|(_, item)| item.scene.traced_material())
        .collect();
    let surfaces: Vec<Vec<FacetSurface>> = items
        .iter()
        .map(|(_, item)| item.scene.effects.surfaces())
        .collect();
    let mut outs: Vec<Vec<Vec3>> = items
        .iter()
        .map(|(_, item)| vec![Vec3::ZERO; item.scene.width as usize * item.scene.height as usize])
        .collect();
    let mut outcomes: Vec<Option<GpuAccumulate>> = vec![None; items.len()];

    let result = catch_unwind(AssertUnwindSafe(|| {
        let mut batch: Vec<GpuBatchItem<'_>> = items
            .iter()
            .zip(&cameras)
            .zip(&finishes)
            .zip(&materials)
            .zip(&surfaces)
            .zip(outs.iter_mut())
            .map(
                |(((((&(index, item), camera), finish), material), surface), out)| GpuBatchItem {
                    scene: GpuSceneRef {
                        camera,
                        width: item.scene.width,
                        height: item.scene.height,
                        planes: &item.scene.planes,
                        facet_finishes: finish,
                        material,
                        max_bounces: item.scene.max_bounces,
                        environment: assets::environment_for(
                            &item.scene,
                            None,
                            trays[index].view(),
                        ),
                        estimator: Some(item.scene.estimator),
                    },
                    tools: &item.scene.tools,
                    surface: item.scene.effects.surface_scene(surface),
                    first_sample: item.first_sample,
                    samples: item.samples,
                    out: out.as_mut_slice(),
                },
            )
            .collect();
        env.gpu
            .try_accumulate_batch_cancellable(&mut batch, cancel, &mut |index, outcome| {
                if let Some(slot) = outcomes.get_mut(index) {
                    *slot = Some(outcome);
                }
                if outcome == GpuAccumulate::Done
                    && let Some(&(_, item)) = items.get(index)
                {
                    env.traced(request_id, item);
                }
            });
    }));
    let panicked = result.is_err();

    let mut declined = Vec::new();
    for ((&(index, item), accum), outcome) in items.iter().zip(outs).zip(outcomes) {
        match outcome {
            Some(GpuAccumulate::Done) => env.picture(request_id, item, accum),
            Some(GpuAccumulate::Declined) => declined.push((index, item)),
            None if panicked => env.fail(
                request_id,
                item.item_id,
                "the GPU tracer panicked while tracing this batch",
            ),
            // The backend never reported this item (it stopped early without a cancel):
            // give it to the CPU rather than lose it.
            None if !cancel.load(Ordering::Relaxed) => declined.push((index, item)),
            Some(GpuAccumulate::Cancelled) | None => {}
        }
    }
    declined
}

/// Traces `items` one by one on the CPU, handing each finished item to the encoder.
fn trace_on_cpu(
    env: &TraceEnv,
    request_id: u32,
    items: &[(usize, &BatchItem)],
    trays: &[assets::Tray],
    cancel: &AtomicBool,
) {
    for &(index, item) in items {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let tray = trays[index].view();
        let traced = catch_unwind(AssertUnwindSafe(|| {
            if item.scene.engine.is_ray() {
                let mut out =
                    vec![Vec3::ZERO; item.scene.width as usize * item.scene.height as usize];
                return render_core::whitted::trace(
                    &item.scene,
                    tray,
                    item.first_sample,
                    item.samples,
                    env.threads,
                    env.prefer_ray_gpu,
                    cancel,
                    &mut out,
                )
                .then_some(out);
            }
            render_core::trace_samples_with_gpu_cancellable(
                &env.cpu,
                &item.scene,
                (item.first_sample, item.samples),
                env.threads,
                cancel,
                tray,
            )
        }));
        match traced {
            Ok(Some(accum)) => {
                env.traced(request_id, item);
                env.picture(request_id, item, accum);
            }
            // Cancelled mid-trace: nothing was added, and nothing is owed.
            Ok(None) if !item.scene.engine.is_ray() || cancel.load(Ordering::Acquire) => return,
            Ok(None) => env.fail(
                request_id,
                item.item_id,
                "the renderer could not trace this item's complete sample range",
            ),
            Err(_) => env.fail(
                request_id,
                item.item_id,
                "tracing panicked on a scene that passed validation",
            ),
        }
    }
}
