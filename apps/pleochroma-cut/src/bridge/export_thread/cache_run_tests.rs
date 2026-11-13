//! Tests of the path cache in the export thread (CPU only, no adapter needed): a render without
//! the cache is as before, an ineligible stone is untouched and says why, a cached render has
//! the uncached mean, a kept cache is loaded by the next render, and a corrupt chunk rebuilds
//! without a word.

use super::{
    ExportProgress, RenderTaps,
    params::{ComputeTarget, ExportParams},
    path_cache_memory::{ASSUMED_RAM_BYTES, gigabytes, meminfo_available},
    scene_snapshot::SceneSnapshot,
    worker::{Accumulation, AccumulationCarry, AccumulationOutcome, render_accumulation},
};
use crate::{bridge::render_thread::RenderContext, settings::LocalComputeTarget};
use glam::Vec3;
#[cfg(feature = "adaptive-sampling")]
use pleochroma::renderer::adaptive::{AdaptiveSettings, NoiseMetric};
use pleochroma::{
    optics::materials::GemMaterial,
    renderer::{
        accel::PathCacheSettings,
        gpu_backend::GpuBackend,
        path_cache::{PathCacheOff, persist::read_meta, placement::ONE_SHOT_NOTE},
    },
};
use std::{
    path::{Path, PathBuf},
    sync::{Mutex, atomic::AtomicBool},
};

const GIB: u64 = 1 << 30;

fn capture() -> SceneSnapshot {
    SceneSnapshot::capture(&Mutex::new(RenderContext::default())).expect("Diamond resolves")
}

fn sapphire() -> SceneSnapshot {
    let mut scene = capture();
    scene.material = GemMaterial::sapphire();
    scene
}

fn params(scene: &SceneSnapshot, size: u32, spp: u32) -> ExportParams {
    ExportParams {
        engine: scene.engine,
        width: size,
        height: size,
        samples_per_pixel: spp,
        max_bounces: scene.max_bounces,
    }
}

/// A CPU render of `scene` at `(0.3, 0.2)`; the finished accumulation and every progress note.
fn render(
    scene: &SceneSnapshot,
    size: u32,
    spp: u32,
    taps: RenderTaps,
) -> (Accumulation, Vec<String>) {
    let mut notes = Vec::new();
    let cancel = AtomicBool::new(false);
    let outcome = render_accumulation(
        scene,
        0.3,
        0.2,
        params(scene, size, spp),
        ComputeTarget::LocalOnly,
        None,
        &GpuBackend::disabled(),
        LocalComputeTarget::Cpu,
        &mut AccumulationCarry::with_taps(taps),
        &cancel,
        |progress: ExportProgress| notes.extend(progress.note),
    );
    match outcome {
        AccumulationOutcome::Completed(done) => (done, notes),
        AccumulationOutcome::Cancelled => panic!("the render was cancelled"),
        AccumulationOutcome::Failed(message) => panic!("the render failed: {message}"),
    }
}

fn cached() -> PathCacheSettings {
    PathCacheSettings::default()
}

/// Mean of the luminance (Y) of the central `edge` x `edge` pixels of a `size`-wide accumulation.
fn central_mean(done: &Accumulation, size: u32, edge: u32) -> f32 {
    let from = (size - edge) / 2;
    let mut total = 0.0;
    for y in from..from + edge {
        for x in from..from + edge {
            total += done.accum[(y * size + x) as usize].y;
        }
    }
    total / (edge * edge) as f32 / done.samples_per_pixel as f32
}

/// A scratch folder that is emptied before use and removed afterwards.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "pleochroma-cache-run-{}-{name}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("the scratch folder is made");
        Self(dir)
    }

    fn path(&self) -> &Path {
        &self.0
    }

    /// The one key folder the cache wrote.
    fn key_folder(&self) -> PathBuf {
        let mut folders: Vec<PathBuf> = std::fs::read_dir(&self.0)
            .expect("the folder reads")
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.is_dir())
            .collect();
        assert_eq!(folders.len(), 1, "one key folder: {folders:?}");
        folders.remove(0)
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn kept_in(scratch: &Scratch) -> RenderTaps {
    RenderTaps {
        path_cache: Some(PathCacheSettings {
            persist: true,
            ..PathCacheSettings::default()
        }),
        cache_dir: Some(scratch.path().to_path_buf()),
        // Not the settings file of whoever runs the test.
        cache_disk_limit_bytes: Some(u64::MAX / 2),
        ..RenderTaps::default()
    }
}

#[test]
fn a_render_without_the_cache_is_the_render_it_was() {
    let scene = capture();
    let (plain, plain_notes) = render(&scene, 16, 16, RenderTaps::default());
    // A cache folder or a limit without the cache switched on changes nothing.
    let taps = RenderTaps {
        path_cache: None,
        cache_dir: Some(std::env::temp_dir().join("never-used")),
        cache_disk_limit_bytes: Some(1),
        ..RenderTaps::default()
    };
    let (again, notes) = render(&scene, 16, 16, taps);
    assert_eq!(plain.accum, again.accum, "byte for byte");
    assert_eq!(plain.samples_per_pixel, again.samples_per_pixel);
    assert!(plain_notes.is_empty() && notes.is_empty(), "{notes:?}");
}

#[test]
fn an_ineligible_stone_renders_as_without_the_cache_and_says_why() {
    let mut scene = capture();
    scene.material.name = "Precious Opal (White)".to_owned();
    let (plain, _) = render(&scene, 16, 16, RenderTaps::default());
    let taps = RenderTaps {
        path_cache: Some(cached()),
        ..RenderTaps::default()
    };
    let (asked, notes) = render(&scene, 16, 16, taps);
    assert_eq!(plain.accum, asked.accum, "an opal is traced as before");
    let reason = PathCacheOff::MaterialLook.message();
    assert!(
        notes.iter().any(|note| note == reason),
        "the reason {reason:?} is one progress note, got {notes:?}"
    );
    assert_eq!(
        notes.iter().filter(|note| note.as_str() == reason).count(),
        1,
        "said once"
    );
}

#[test]
fn a_scattering_stone_is_refused_with_its_own_sentence() {
    let mut scene = capture();
    scene.material.scattering_sigma_s = 0.5;
    let taps = RenderTaps {
        path_cache: Some(cached()),
        ..RenderTaps::default()
    };
    let (_, notes) = render(&scene, 8, 4, taps);
    assert!(
        notes
            .iter()
            .any(|note| note == PathCacheOff::ScatteringMedium.message()),
        "{notes:?}"
    );
}

#[test]
fn a_cached_cpu_render_has_the_uncached_mean() {
    let scene = sapphire();
    let size = 16;
    let (plain, _) = render(&scene, size, 256, RenderTaps::default());
    let scratch = Scratch::new("mean");
    let taps = kept_in(&scratch);
    let (shaded, notes) = render(&scene, size, 64, taps);
    assert!(
        notes.iter().any(|note| note.starts_with("Path cache: ")),
        "the size line is one progress note, got {notes:?}"
    );
    assert!(
        shaded.accum.iter().all(|pixel| pixel.is_finite()),
        "finite sums"
    );
    let (want, got) = (
        central_mean(&plain, size, 8),
        central_mean(&shaded, size, 8),
    );
    assert!(want > 0.0, "the stone has light");
    assert!(
        (got - want).abs() <= 0.08 * want,
        "cached mean {got} against the traced mean {want}"
    );
}

#[cfg(feature = "adaptive-sampling")]
#[test]
fn a_cached_adaptive_render_is_deterministic_and_says_the_size() {
    let scene = sapphire();
    let settings = AdaptiveSettings {
        target_noise: 0.5,
        metric: NoiseMetric::RelativeLstar,
        min_spp: 16,
        max_spp: 64,
        pass_spp: 8,
        tile: 8,
    };
    let (scratch_a, scratch_b) = (Scratch::new("adaptive-a"), Scratch::new("adaptive-b"));
    let taps = |scratch: &Scratch| RenderTaps {
        adaptive: Some(settings),
        ..kept_in(scratch)
    };
    let (first, notes) = render(&scene, 24, 64, taps(&scratch_a));
    let (second, _) = render(&scene, 24, 64, taps(&scratch_b));
    assert_eq!(first.accum, second.accum, "same inputs, same bytes");
    let stats = first.adaptive.as_ref().expect("adaptive statistics");
    assert!(stats.counts.iter().all(|count| (16..=64).contains(count)));
    assert!(
        notes.iter().any(|note| note.starts_with("Path cache: ")),
        "{notes:?}"
    );
}

#[test]
fn a_kept_cache_is_loaded_by_the_next_render_and_gives_the_same_image() {
    let scratch = Scratch::new("kept");
    let scene = sapphire();
    let (first, first_notes) = render(&scene, 16, 32, kept_in(&scratch));
    assert!(
        first_notes
            .iter()
            .any(|note| note.starts_with("Path cache: ") && note.ends_with("kept for re-renders")),
        "{first_notes:?}"
    );
    let folder = scratch.key_folder();
    let meta = read_meta(&folder).expect("the meta is readable");
    assert!(meta.complete, "the writer finished the folder");
    assert!(meta.tile_count > 0 && meta.size_bytes > 0, "{meta:?}");

    let (second, second_notes) = render(&scene, 16, 32, kept_in(&scratch));
    assert!(
        second_notes
            .iter()
            .any(|note| note.ends_with("loaded from disk")),
        "no tile is built the second time: {second_notes:?}"
    );
    assert_eq!(
        first.accum, second.accum,
        "the loaded cache shades the same"
    );
}

#[test]
fn a_corrupt_chunk_is_rebuilt_without_an_error() {
    let scratch = Scratch::new("corrupt");
    let scene = sapphire();
    let (first, _) = render(&scene, 16, 16, kept_in(&scratch));
    let folder = scratch.key_folder();
    let tiles = folder.join("tiles");
    let chunk = std::fs::read_dir(&tiles)
        .expect("the tiles folder reads")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .min()
        .expect("a chunk was written");
    let mut bytes = std::fs::read(&chunk).expect("the chunk reads");
    let last = bytes.len() - 1;
    bytes[last] ^= 0x5A;
    std::fs::write(&chunk, bytes).expect("the chunk is damaged");

    let (second, notes) = render(&scene, 16, 16, kept_in(&scratch));
    assert!(
        notes.iter().all(|note| !note.contains("loaded from disk")),
        "a damaged cache is not loaded: {notes:?}"
    );
    assert!(
        notes
            .iter()
            .any(|note| note.ends_with("kept for re-renders")),
        "it is built again and kept: {notes:?}"
    );
    assert_eq!(
        first.accum, second.accum,
        "the rebuilt tiles shade the same"
    );
    // And the rebuilt folder is whole again.
    let (_, third_notes) = render(&scene, 16, 16, kept_in(&scratch));
    assert!(
        third_notes
            .iter()
            .any(|note| note.ends_with("loaded from disk")),
        "{third_notes:?}"
    );
}

#[test]
fn the_memory_numbers_read_from_text() {
    assert_eq!(gigabytes("12"), Some(12 * GIB));
    assert_eq!(gigabytes(" 0.5 "), Some(GIB / 2));
    assert_eq!(gigabytes("-1"), None);
    assert_eq!(gigabytes("many"), None);
    assert_eq!(gigabytes("inf"), None);

    let text = "MemTotal:       65843812 kB\nMemFree:         1234 kB\n\
                MemAvailable:   40000000 kB\nBuffers: 1 kB\n";
    assert_eq!(meminfo_available(text), Some(40_000_000 * 1024));
    assert_eq!(meminfo_available("MemTotal: 1 kB\n"), None);
    assert_eq!(meminfo_available("MemAvailable: lots kB\n"), None);
    assert_eq!(ASSUMED_RAM_BYTES, 32 * GIB);
}

/// The sums of a render are sums of finite, non-negative XYZ.
#[test]
fn a_cached_render_never_yields_a_negative_or_nan_pixel() {
    let scene = sapphire();
    let scratch = Scratch::new("finite");
    let taps = kept_in(&scratch);
    let (shaded, _) = render(&scene, 12, 16, taps);
    for pixel in &shaded.accum {
        assert!(pixel.is_finite() && pixel.min_element() >= 0.0, "{pixel:?}");
    }
    assert!(shaded.accum.iter().any(|pixel| *pixel != Vec3::ZERO));
}

/// A render that asks for the cache without keeping it is traced as without the cache, and says
/// why once: a cache only pays for a view that is kept and rendered again.
#[test]
fn a_render_that_does_not_keep_its_cache_is_traced_and_says_why() {
    let scene = sapphire();
    let taps = RenderTaps {
        path_cache: Some(cached()),
        ..RenderTaps::default()
    };
    let (shaded, notes) = render(&scene, 16, 4, taps);
    assert_eq!(
        notes
            .iter()
            .filter(|note| note.as_str() == ONE_SHOT_NOTE)
            .count(),
        1,
        "{notes:?}"
    );
    let (plain, _) = render(&scene, 16, 4, RenderTaps::default());
    assert_eq!(
        shaded.accum, plain.accum,
        "a refused cache leaves the render as it was"
    );
}
