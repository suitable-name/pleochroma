//! The GPU Worker's `TraceFullPass` handling: takes the renderer and the cached scene out
//! of the shared state, traces one full-frame pass, puts both back and posts the answer.
//!
//! Split out of `lib.rs` (the message handler grew past the lint's line limit); the shared
//! state, `post` and the device flags live in the parent module.

use std::{cell::RefCell, rc::Rc};

use glam::Vec3;
use pleochroma::renderer::gpu::{
    event_wavefront::{EventWavefrontRenderer, EwError, kernels::needed_kernels_with_surface},
    frame::classify_material,
};
use pleochroma_web_core::{
    gpu_admission::{self, ISOTROPIC_CLASS, SELF_CHECK_SCENE_ID},
    protocol::{FromWorker, WorkerRole},
    render::{Accumulator, handle_trace_chunk},
};

use super::{CachedScene, DEVICE_LOST, GpuWorkerState, compile::ew_kernel_name, now_ms, post};
use crate::self_check;

/// The pass a `TraceFullPass` message asked for.
#[derive(Clone, Copy)]
pub(super) struct Pass {
    pub(super) scene_id: u64,
    pub(super) id: u64,
    pub(super) sample_offset: u32,
    pub(super) spp: u32,
}

/// `true` for a sum buffer the accumulator may merge: non-empty, every value finite
/// and non-negative.
fn sums_are_valid(sums: &[[f32; 3]]) -> bool {
    Accumulator::full_pass_sums_valid(sums)
}

/// Posts `ChunkDropped` for `pass`: the page hands the pass back to the CPU workers.
fn post_dropped(pass: Pass) {
    post(&FromWorker::ChunkDropped {
        scene_id: pass.scene_id,
        first_pixel: 0,
        sample_offset: pass.sample_offset,
    });
}

/// Refuses `pass` because of a property of the scene itself (the page's classifier or the browser
/// kernel variant excludes it): `SceneError` first, so the pool marks the scene rejected, then
/// `ChunkDropped` to free the pass. The pool then hands the pass to the CPU Workers and does not
/// count it as a lost scene, which would end GPU tracing for the whole session.
fn post_scene_refused(pass: Pass, message: String) {
    post(&FromWorker::SceneError {
        scene_id: pass.scene_id,
        message,
    });
    post_dropped(pass);
}

/// The kernels the cached scene's pass builds under `pipeline_class` that this Worker has not
/// compiled, by name. The list is the desktop's own selection for the scene
/// (`needed_kernels_with_surface`), so it cannot drift from what the pass compiles; the browser
/// has no ground surface, so the ground arm is never part of it.
fn missing_kernels(state: &GpuWorkerState, pipeline_class: u32) -> Vec<String> {
    let Some(cached) = state.cached_scene.as_ref() else {
        return Vec::new();
    };
    let needed: Vec<String> =
        needed_kernels_with_surface(pipeline_class, &cached.scene.frame_scene(), false)
            .into_iter()
            .map(ew_kernel_name)
            .collect();
    state
        .compiled
        .missing(pipeline_class, needed.iter().map(String::as_str))
        .into_iter()
        .map(str::to_owned)
        .collect()
}

/// Takes the renderer and the scene OUT of the shared state: no `RefCell` borrow may live
/// across the `.await` of the trace. `None` after posting why the pass cannot run.
fn take_pass_inputs(
    state_rc: &Rc<RefCell<GpuWorkerState>>,
    pass: Pass,
) -> Option<(EventWavefrontRenderer, CachedScene, u32)> {
    let mut state = state_rc.borrow_mut();
    if state.role != Some(WorkerRole::Gpu) {
        drop(state);
        post(&FromWorker::Error {
            message: "TraceFullPass before a successful Init".to_string(),
        });
        return None;
    }
    let scene_ok = state
        .cached_scene
        .as_ref()
        .is_some_and(|c| c.scene_id == pass.scene_id);
    if !scene_ok {
        drop(state);
        post_dropped(pass);
        return None;
    }
    let refusal = state.cached_scene.as_ref().and_then(|c| c.ineligible);
    if let Some(reason) = refusal {
        drop(state);
        post_scene_refused(pass, format!("the GPU worker refused the scene: {reason}"));
        return None;
    }
    // The pass traces with the variant the page's gate compiled and tested for the stone's
    // class (`browser_pass_class`), never with the desktop's own, larger variant, whose
    // pipelines were not compiled here (the all-features `Scatter` kernel exceeds the compile
    // deadline). A stone that needs a feature the variant compiled out is refused.
    let pipeline_class = state
        .cached_scene
        .as_ref()
        .and_then(|c| gpu_admission::browser_pass_class(classify_material(c.scene.material())));
    let Some(pipeline_class) = pipeline_class else {
        drop(state);
        post_scene_refused(
            pass,
            "the GPU worker refused the scene: the stone needs a kernel feature the browser GPU \
             does not compile"
                .to_string(),
        );
        return None;
    };
    // A scene whose kernels this Worker did not compile and test would compile them inside
    // the pass, past every deadline and crash marker of the page's gate. The base set of the
    // class and the kernels of each feature the stone reaches (scattering, frosted facets)
    // are compiled by separate requests; the pass needs all of them.
    let missing = missing_kernels(&state, pipeline_class);
    if !missing.is_empty() {
        drop(state);
        post(&FromWorker::Error {
            message: format!(
                "the GPU worker has not compiled the {} kernels this stone needs ({})",
                gpu_admission::class_label(gpu_admission::class_kind(pipeline_class)),
                missing.join(", ")
            ),
        });
        post_dropped(pass);
        return None;
    }
    let Some(renderer) = state.renderer.take() else {
        drop(state);
        post(&FromWorker::GpuLost {
            reason: "renderer is not initialized".to_string(),
        });
        return None;
    };
    let Some(cached) = state.cached_scene.take() else {
        state.renderer = Some(renderer);
        return None;
    };
    Some((renderer, cached, pipeline_class))
}

/// Posts the answer to a traced pass: its sums when they are valid, `GpuLost` for a lost
/// device or a pass with NaN, infinite or negative sums, and an error plus `ChunkDropped`
/// when only this pass cannot run. `pass_start_ms` is when the trace began.
fn report_pass(pass: Pass, result: Result<(), EwError>, sums: Vec<Vec3>, pass_start_ms: f64) {
    match result {
        Ok(()) => {
            let elapsed_ms = now_ms() - pass_start_ms;
            let sums_arr: Vec<[f32; 3]> = sums.iter().map(Vec3::to_array).collect();
            drop(sums);
            if sums_are_valid(&sums_arr) {
                post(&FromWorker::FullPassResult {
                    scene_id: pass.scene_id,
                    pass_id: pass.id,
                    sample_offset: pass.sample_offset,
                    spp: pass.spp,
                    sums: sums_arr,
                    elapsed_ms,
                });
            } else {
                // A pass with NaN, infinite or negative sums must never reach
                // the accumulator; GpuLost makes the pool hand the pass back
                // to the CPU workers.
                post(&FromWorker::GpuLost {
                    reason: "the GPU pass produced non-finite or negative sums".to_string(),
                });
            }
        }
        Err(
            error @ (EwError::DeviceLost(_) | EwError::ChunkTimeout | EwError::BufferMapping(_)),
        ) => {
            DEVICE_LOST.with(|l| l.set(true));
            post(&FromWorker::GpuLost {
                reason: format!("event wavefront accumulation failed: {error}"),
            });
        }
        Err(error) => {
            // An eligibility error (unsupported material, bad buffer size):
            // the device is fine, only this pass cannot run.
            post(&FromWorker::Error {
                message: format!("event wavefront pass refused: {error}"),
            });
            post_dropped(pass);
        }
    }
}

/// Handles `TraceFullPass`: traces `pass` on the GPU and posts the answer.
#[expect(clippy::future_not_send, reason = "wasm is single-threaded")]
pub(super) async fn handle_full_pass(state_rc: &Rc<RefCell<GpuWorkerState>>, pass: Pass) {
    let Some((mut renderer, cached, pipeline_class)) = take_pass_inputs(state_rc, pass) else {
        return;
    };

    let num_pixels = cached.width as usize * cached.height as usize;
    let mut sums = vec![Vec3::ZERO; num_pixels];
    let pass_start_ms = now_ms();

    let result = {
        let frame_scene = cached.scene.frame_scene();
        renderer
            .accumulate_via_class_with_tools(
                &frame_scene,
                cached.scene.tools(),
                pipeline_class,
                pass.sample_offset,
                pass.spp,
                &mut sums,
            )
            .await
    };

    // The self-check of an anisotropic class is compared with the CPU's render of the same
    // scene before its answer leaves the Worker; a frame that disagrees goes out blank, which
    // the page reads as a failed self-check (the reason is posted as an error first).
    let sums = if result.is_ok()
        && let Some(reason) = anisotropic_self_check_failure(&cached, pass, &sums)
    {
        post(&FromWorker::Error { message: reason });
        vec![Vec3::ZERO; num_pixels]
    } else {
        sums
    };

    // Put both back before anything else can look at the state.
    {
        let mut state = state_rc.borrow_mut();
        state.renderer = Some(renderer);
        state.cached_scene = Some(cached);
    }

    report_pass(pass, result, sums, pass_start_ms);
}

/// Why the GPU render of the self-check scene differs from the CPU render, for a scene of an
/// anisotropic class; `None` for every other pass, for a self-check that agrees, and for a GPU
/// frame with invalid values (which [`report_pass`] already treats as a lost device).
fn anisotropic_self_check_failure(
    cached: &CachedScene,
    pass: Pass,
    sums: &[Vec3],
) -> Option<String> {
    if pass.scene_id != SELF_CHECK_SCENE_ID {
        return None;
    }
    let class = cached
        .material_class
        .filter(|class| *class != ISOTROPIC_CLASS)?;
    let gpu: Vec<[f32; 3]> = sums.iter().map(Vec3::to_array).collect();
    if !sums_are_valid(&gpu) {
        return None;
    }
    let cpu: Vec<[f32; 3]> = handle_trace_chunk(
        &cached.scene,
        cached.scene.plane_soa(),
        0,
        1,
        pass.sample_offset,
        pass.spp,
    )
    .iter()
    .map(Vec3::to_array)
    .collect();
    let label = gpu_admission::class_label(class);
    match self_check::compare(&gpu, &cpu, cached.width, cached.height) {
        Ok(comparison) if comparison.passed() => None,
        Ok(comparison) => Some(format!(
            "the {label} GPU self-check differs from the CPU render: {}",
            comparison.describe()
        )),
        Err(reason) => Some(format!(
            "the {label} GPU self-check could not be compared with the CPU render: {reason}"
        )),
    }
}
