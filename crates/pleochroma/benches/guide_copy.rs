//! P08 (guide-copy part) measurement: the per-pass copy of the guide buffers
//! (`FABLE_perf_verification.md`, item P08).
//!
//! `PassBudget::commit` (apps/pleochroma-cut/src/bridge/render_thread/interactive.rs:98-104)
//! adds the pass radiance into the running total and then copies three guide buffers with
//! `copy_from_slice` on every committed pass:
//!
//! ```text
//! for (total, contribution) in outputs.accum.iter_mut().zip(&self.radiance) { *total += *contribution; }
//! outputs.depth.copy_from_slice(&self.depth);      // Vec<f32>   4 B / pixel
//! outputs.normal.copy_from_slice(&self.normal);    // Vec<Vec3> 12 B / pixel
//! outputs.facet_id.copy_from_slice(&self.facet);   // Vec<i32>   4 B / pixel
//! ```
//!
//! That is 20 B/pixel of guide copy, about 41 MB per pass at 1920x1080. `PassBudget` is private
//! to the app crate, so this bench reproduces the same pattern on same-typed buffers
//! (`glam::Vec3` is the type the app uses). The radiance accumulate (12 B/pixel read plus
//! 12 B/pixel read-modify-write) is timed too, as the yardstick the guide copy is judged
//! against: both run once per committed pass.
//!
//! Run: `cargo bench -p pleochroma --bench guide_copy`.

use divan::black_box;
use glam::Vec3;

fn main() {
    divan::main();
}

/// The buffers one commit touches at `pixels` pixels, filled with non-zero data so the
/// pages are really resident.
struct Buffers {
    radiance: Vec<Vec3>,
    depth: Vec<f32>,
    normal: Vec<Vec3>,
    facet: Vec<i32>,
    out_accum: Vec<Vec3>,
    out_depth: Vec<f32>,
    out_normal: Vec<Vec3>,
    out_facet: Vec<i32>,
}

impl Buffers {
    fn new(pixels: usize) -> Self {
        let ramp = |i: usize| f32::from(u16::try_from(i % 4096).unwrap_or(0)) * 0.001;
        Self {
            radiance: (0..pixels).map(|i| Vec3::splat(ramp(i))).collect(),
            depth: (0..pixels).map(ramp).collect(),
            normal: (0..pixels).map(|i| Vec3::new(ramp(i), 0.5, 0.25)).collect(),
            facet: (0..pixels)
                .map(|i| i32::from(u16::try_from(i % 64).unwrap_or(0)))
                .collect(),
            out_accum: vec![Vec3::ZERO; pixels],
            out_depth: vec![1.0e6; pixels],
            out_normal: vec![Vec3::ZERO; pixels],
            out_facet: vec![-1; pixels],
        }
    }
}

const P_800X600: usize = 800 * 600;
const P_1080P: usize = 1920 * 1080;

fn guide_copy(bencher: divan::Bencher, pixels: usize) {
    let mut b = Buffers::new(pixels);
    bencher.bench_local(|| {
        b.out_depth.copy_from_slice(black_box(&b.depth));
        b.out_normal.copy_from_slice(black_box(&b.normal));
        b.out_facet.copy_from_slice(black_box(&b.facet));
        black_box((&b.out_depth, &b.out_normal, &b.out_facet));
    });
}

fn radiance_accumulate(bencher: divan::Bencher, pixels: usize) {
    let mut b = Buffers::new(pixels);
    bencher.bench_local(|| {
        for (total, contribution) in b.out_accum.iter_mut().zip(black_box(&b.radiance)) {
            *total += *contribution;
        }
        black_box(&b.out_accum);
    });
}

/// The three guide copies of one commit, 800x600.
#[divan::bench(sample_count = 100)]
fn guide_copy_800x600(bencher: divan::Bencher) {
    guide_copy(bencher, P_800X600);
}

/// The three guide copies of one commit, 1920x1080.
#[divan::bench(sample_count = 100)]
fn guide_copy_1080p(bencher: divan::Bencher) {
    guide_copy(bencher, P_1080P);
}

/// The radiance accumulate of one commit, 800x600.
#[divan::bench(sample_count = 100)]
fn radiance_accumulate_800x600(bencher: divan::Bencher) {
    radiance_accumulate(bencher, P_800X600);
}

/// The radiance accumulate of one commit, 1920x1080.
#[divan::bench(sample_count = 100)]
fn radiance_accumulate_1080p(bencher: divan::Bencher) {
    radiance_accumulate(bencher, P_1080P);
}
