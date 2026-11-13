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

/// One output row and the mask of its pixels a re-shade covered.
type MaskedRow<'a> = (&'a mut [Vec3], &'a [bool]);

/// Sets every pixel `covered` leaves out to the sum of the frame's taps, as [`render_frame`] sums
/// them, and leaves the covered pixels alone. Rows are claimed by index, so the result does not
/// depend on the thread count.
///
/// Returns `false` when `cancel` was raised (the buffer is then partly written).
pub(super) fn render_uncovered(
    frame: &BackendFrame<'_>,
    prep: &WhittedPrep<'_>,
    planes: &pleochroma::simd::PlanesSoA32,
    covered: &[bool],
    accum: &mut [Vec3],
    cancel: &AtomicBool,
) -> bool {
    let width = frame.width as usize;
    if cancel.load(Ordering::Relaxed) {
        return false;
    }
    if width == 0 || covered.len() != accum.len() {
        return true;
    }
    let rows: Mutex<Vec<Option<MaskedRow<'_>>>> = Mutex::new(
        accum
            .chunks_mut(width)
            .zip(covered.chunks(width))
            .map(Some)
            .collect(),
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
                    let claimed = rows
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .get_mut(y)
                        .and_then(Option::take);
                    let Some((out, mask)) = claimed else {
                        continue;
                    };
                    for (x, (slot, &done)) in out.iter_mut().zip(mask).enumerate() {
                        if done {
                            continue;
                        }
                        let pixel = u32::try_from(y * width + x).unwrap_or(u32::MAX);
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
                        *slot = sum;
                    }
                }
            });
        }
    });
    !cancel.load(Ordering::Relaxed)
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
