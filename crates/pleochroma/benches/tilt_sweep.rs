//! Cost of the tilt-profile sweep over all axes on a standard round brilliant.
//!
//! `evaluate_all_axes_profiles_geom` traces every axis at every tilt step. The serial run
//! (`Some(1)`) is the per-core baseline for the parallel entry, which uses the machine's
//! parallelism.
//!
//! Run: `cargo bench -p pleochroma --bench tilt_sweep`.

use divan::black_box;
use pleochroma::{
    color::metrics::{evaluate_all_axes_profiles_geom, evaluate_all_axes_profiles_geom_threaded},
    geometry::{StandardGemCuts, tool::StoneGeometry},
    optics::{GemMaterial, raytracer::LightingPreset},
};

fn main() {
    divan::main();
}

/// The parallel sweep with the machine's parallelism (the cut app's call).
#[divan::bench(sample_count = 5, sample_size = 1)]
fn all_axes_parallel(bencher: divan::Bencher) {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let env = LightingPreset::RingLights.studio(1.0, 0.85, 0.95);
    bencher.bench_local(|| {
        black_box(evaluate_all_axes_profiles_geom(
            black_box(StoneGeometry::planes_only(&planes)),
            black_box(&material),
            black_box(env),
        ))
    });
}

/// The serial sweep on the calling thread (`Some(1)`), the per-core baseline.
#[divan::bench(sample_count = 3, sample_size = 1)]
fn all_axes_serial(bencher: divan::Bencher) {
    let planes = StandardGemCuts::standard_round_brilliant();
    let material = GemMaterial::diamond();
    let env = LightingPreset::RingLights.studio(1.0, 0.85, 0.95);
    bencher.bench_local(|| {
        black_box(evaluate_all_axes_profiles_geom_threaded(
            black_box(StoneGeometry::planes_only(&planes)),
            black_box(&material),
            black_box(env),
            Some(1),
        ))
    });
}
