//! H03 measurement: one analytical diagnostic image, serial and on a thread team
//! (`FABLE_perf_verification.md`, item H03).
//!
//! `evaluate_optical_image` (color/metrics/image.rs) is a serial pixel loop, and
//! `evaluate_optical_image_threaded` spreads the same rows over scoped workers with a
//! bit-identical result. The cut app's overlay worker
//! (apps/pleochroma-cut/src/bridge/optical_overlay/worker.rs) runs two passes per job on
//! the display share of the live CPU partition (a quarter of the cores, so 4 on 16): a draft
//! whose longer edge is 128 px at 1 sample, then a refinement at 320 px and 4 samples. Both
//! are sampled from the frame's
//! projection (`projection_size`), which is how the worker calls it. A 16:9 frame therefore
//! gives 128x72 and 320x180; the default 4:3 frame gives 128x96 and 320x240.
//!
//! The channels are the optional extra traces: Fire (`want_fire`) and the scintillation
//! temporal term (`want_scintillation`, which needs the orbit pose).
//!
//! Run: `cargo bench -p pleochroma --bench diagnostic_image`.

use divan::black_box;
use pleochroma::{
    color::metrics::{
        OpticalImageRequest, evaluate_optical_image, evaluate_optical_image_threaded,
    },
    geometry::{StandardGemCuts, tool::StoneGeometry},
    optics::{Camera, GemMaterial, raytracer::LightingPreset},
};

fn main() {
    divan::main();
}

/// One pass at `width` x `height`, `samples` per pixel, on a standard round brilliant.
fn pass(
    bencher: divan::Bencher,
    (width, height): (u32, u32),
    samples: u32,
    want_fire: bool,
    want_scintillation: bool,
    projection: [u32; 2],
    threads: usize,
) {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let pose = [0.60_f32, 0.45, 2.4];
    let camera = Camera::new(pose[0], pose[1], pose[2], 42.0);
    let environment = LightingPreset::IsoHemisphere.studio(1.0, 0.85, 0.95);
    bencher.bench_local(|| {
        let request = OpticalImageRequest {
            camera: black_box(&camera),
            width,
            height,
            samples,
            projection_size: Some(projection),
            tile_origin: None,
            geometry: StoneGeometry::planes_only(black_box(&planes)),
            finishes: &[],
            material: black_box(&material),
            environment,
            want_fire,
            want_scintillation,
            pose: Some(pose),
        };
        black_box(
            if threads == 1 {
                evaluate_optical_image(&request, || false)
            } else {
                evaluate_optical_image_threaded(&request, threads, || false)
            }
            .expect("the pass must complete"),
        )
    });
}

const FRAME_16_9: [u32; 2] = [1920, 1080];
const FRAME_4_3: [u32; 2] = [640, 480];

/// Draft pass, 16:9 frame (128x72, 1 sample), base classes only.
#[divan::bench(sample_count = 30)]
fn draft_16x9(bencher: divan::Bencher) {
    pass(bencher, (128, 72), 1, false, false, FRAME_16_9, 1);
}

/// Refinement pass, 16:9 frame (320x180, 4 samples), base classes only.
#[divan::bench(sample_count = 10)]
fn refined_16x9(bencher: divan::Bencher) {
    pass(bencher, (320, 180), 4, false, false, FRAME_16_9, 1);
}

/// Refinement pass, 16:9 frame, Fire on.
#[divan::bench(sample_count = 10)]
fn refined_16x9_fire(bencher: divan::Bencher) {
    pass(bencher, (320, 180), 4, true, false, FRAME_16_9, 1);
}

/// Refinement pass, 16:9 frame, scintillation on.
#[divan::bench(sample_count = 10)]
fn refined_16x9_scintillation(bencher: divan::Bencher) {
    pass(bencher, (320, 180), 4, false, true, FRAME_16_9, 1);
}

/// Refinement pass, 16:9 frame, Fire and scintillation on (every channel).
#[divan::bench(sample_count = 10)]
fn refined_16x9_all_channels(bencher: divan::Bencher) {
    pass(bencher, (320, 180), 4, true, true, FRAME_16_9, 1);
}

/// Draft pass, 4:3 frame (128x96, 1 sample), base classes only.
#[divan::bench(sample_count = 30)]
fn draft_4x3(bencher: divan::Bencher) {
    pass(bencher, (128, 96), 1, false, false, FRAME_4_3, 1);
}

/// Refinement pass, 4:3 frame (320x240, 4 samples), every channel.
#[divan::bench(sample_count = 10)]
fn refined_4x3_all_channels(bencher: divan::Bencher) {
    pass(bencher, (320, 240), 4, true, true, FRAME_4_3, 1);
}

/// Refinement pass, 16:9 frame, every channel, on 4 threads (the display share on 16 cores).
#[divan::bench(sample_count = 10)]
fn refined_16x9_all_channels_4_threads(bencher: divan::Bencher) {
    pass(bencher, (320, 180), 4, true, true, FRAME_16_9, 4);
}

/// Refinement pass, 16:9 frame, every channel, on 16 threads (the whole machine).
#[divan::bench(sample_count = 10)]
fn refined_16x9_all_channels_16_threads(bencher: divan::Bencher) {
    pass(bencher, (320, 180), 4, true, true, FRAME_16_9, 16);
}

/// Refinement pass, 4:3 frame, every channel, on 4 threads.
#[divan::bench(sample_count = 10)]
fn refined_4x3_all_channels_4_threads(bencher: divan::Bencher) {
    pass(bencher, (320, 240), 4, true, true, FRAME_4_3, 4);
}
