//! Deterministic rows: scheduling changes which thread owns a row, never its sum.
use super::gpu_backend::{BackendFrame, FrameOutputs};
use glam::Vec3;
use pleochroma::{
    optics::raytracer::add_finite_sample,
    renderer::whitted::{WhittedPrep, whitted_tap_xyz},
};
use std::sync::{
    Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};

struct Row<'a> {
    accum: &'a mut [Vec3],
}

pub(super) fn render_frame(
    frame: &BackendFrame<'_>,
    prep: &WhittedPrep<'_>,
    planes: &pleochroma::simd::PlanesSoA32,
    outputs: &mut FrameOutputs<'_>,
    cancel: &AtomicBool,
) -> bool {
    let width = frame.width as usize;
    if cancel.load(Ordering::Relaxed) {
        return false;
    }
    if width == 0 {
        return true;
    }
    let rows = Mutex::new(
        outputs
            .accum
            .chunks_mut(width)
            .map(|accum| Some(Row { accum }))
            .collect::<Vec<_>>(),
    );
    let next = AtomicUsize::new(0);
    let scene = super::engine::frame_scene(frame);
    let _lease = pleochroma::renderer::tonemap::live_cpu_budget::single_core_lease();
    let _trace_lease = pleochroma::renderer::tonemap::live_cpu_budget::trace_lease();
    let threads = pleochroma::renderer::tonemap::live_cpu_budget::partition().0;
    std::thread::scope(|scope| {
        for _ in 0..threads {
            let (rows, next, planes, scene) = (&rows, &next, planes, &scene);
            scope.spawn(move || {
                loop {
                    let y = next.fetch_add(1, Ordering::Relaxed);
                    if y >= frame.height as usize || cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let row = rows
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)[y]
                        .take()
                        .expect("an atomic row index is claimed exactly once");
                    for x in 0..width {
                        let pixel = u32::try_from(y * width + x).expect("a frame pixel fits u32");
                        let mut sum = Vec3::ZERO;
                        for tap in frame.sample_offset..frame.sample_offset + frame.spp {
                            if cancel.load(Ordering::Relaxed) {
                                return;
                            }
                            add_finite_sample(
                                &mut sum,
                                whitted_tap_xyz(scene, frame.tools, planes, prep, pixel, tap, None),
                            );
                        }
                        row.accum[x] += sum;
                    }
                }
            });
        }
    });
    !cancel.load(Ordering::Relaxed)
}
