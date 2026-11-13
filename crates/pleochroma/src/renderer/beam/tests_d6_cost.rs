//! Beam tree cost diagnostics for the re-light rig: the standard brilliant in glass on a 16 pixel
//! frame.
//!
//! The ignored tests measure the tree per tile across a ladder of bounce limits and settings, so
//! the log shows where the cost grows: one `COST` line per tile and per configuration, with the
//! time, the beam and leaf counts, the splits, the drops and the depth histogram. The tiles of one
//! configuration run in parallel under one time budget (`PLEOCHROMA_BEAM_COST_BUDGET_S`, default
//! 30 s); a tile still running at the budget is cancelled and reported over budget, and the deeper
//! rows of that settings variant are skipped. `scripts/run_beam_tests.sh --cost` (step 10) runs
//! them and writes everything to one log.

use std::{
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    thread,
    time::{Duration, Instant},
};

use glam::Vec3;

use super::{
    BeamSettings,
    frame::BeamFrame,
    leaf_cache::LeafCache,
    primary::TileRect,
    stats::BeamStats,
    tests_support::{BeamRig, MAX_BOUNCES, glass, show_beam_log},
};
use crate::{optics::raytracer::LightingPreset, renderer::frame_scene::FrameScene};

/// The side of the square frame in pixels.
const FRAME: u32 = 16;

/// The tile edge in pixels: four tiles per frame.
const TILE: u32 = 8;

/// The bounce limits of the ladder, ascending.
const BOUNCES: [u32; 6] = [2, 3, 4, 6, 8, 12];

/// The default time budget of one configuration in seconds.
const DEFAULT_BUDGET_S: u64 = 30;

/// The default time budget of the full Ray-Traced build in seconds.
const DEFAULT_BUILD_BUDGET_S: u64 = 300;

/// How often the budget watcher looks at the clock.
const WATCH_INTERVAL: Duration = Duration::from_millis(20);

/// Seconds from the environment variable `name`, or `default`.
fn seconds_from_env(name: &str, default: u64) -> Duration {
    let seconds = std::env::var(name)
        .ok()
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(default);
    Duration::from_secs(seconds.max(1))
}

/// The settings variants of the ladder: what each one changes against the defaults.
///
/// The variants isolate the knobs that set the entry pieces: `τ_dir`, `τ_w` and the split depth.
fn variants() -> [(&'static str, BeamSettings); 5] {
    let base = BeamSettings::default();
    [
        ("default", base),
        (
            "tau_dir 1e-3",
            BeamSettings {
                dir_tolerance_rad: 1e-3,
                ..base
            },
        ),
        (
            "tau_w 1e-2",
            BeamSettings {
                weight_tolerance: 1e-2,
                ..base
            },
        ),
        (
            "depth 12",
            BeamSettings {
                max_split_depth: 12,
                ..base
            },
        ),
        (
            "coarse",
            BeamSettings {
                dir_tolerance_rad: 1e-2,
                chord_tolerance_px: 1e-2,
                weight_tolerance: 1e-2,
                lighting_tolerance: 1e-2,
                cutoff: 1e-2,
                spectral_nodes: 8,
                absorption_subnodes: 1,
                max_split_depth: 8,
                max_fragment_levels: 3,
                ..base
            },
        ),
    ]
}

/// The re-light tests' frame of `rig` at `bounces`, under their Studio lighting.
fn scene_of(rig: &BeamRig, bounces: u32) -> FrameScene<'_> {
    FrameScene {
        environment: LightingPreset::RingLights.studio(1.0, 0.3, 0.9),
        ..rig.scene(FRAME, bounces)
    }
}

/// One tile's tree: its seconds and its counters, `None` when the budget cancelled it.
struct TileCost {
    /// Seconds the tile ran.
    seconds: f64,
    /// The tile's counters; `None` when it was over budget.
    stats: Option<BeamStats>,
}

/// Runs `work` and sets the cancel flag it gets once `budget` has passed; returns its result and
/// seconds.
fn within<T>(budget: Duration, work: impl FnOnce(&AtomicBool) -> T) -> (T, f64) {
    let cancel = AtomicBool::new(false);
    let done = AtomicBool::new(false);
    thread::scope(|scope| {
        scope.spawn(|| {
            let started = Instant::now();
            while !done.load(Ordering::Relaxed) {
                if started.elapsed() >= budget {
                    cancel.store(true, Ordering::Relaxed);
                    return;
                }
                thread::sleep(WATCH_INTERVAL);
            }
        });
        let started = Instant::now();
        let result = work(&cancel);
        done.store(true, Ordering::Relaxed);
        (result, started.elapsed().as_secs_f64())
    })
}

/// The trees of every tile of `rig` at `bounces` under `settings`, all tiles in parallel within
/// one `budget`. `Err` with the reason when the stone is not beam traced.
fn measure(
    rig: &BeamRig,
    settings: &BeamSettings,
    bounces: u32,
    budget: Duration,
) -> Result<Vec<TileCost>, String> {
    let scene = scene_of(rig, bounces);
    let frame = BeamFrame::new(&scene, settings).map_err(|off| format!("{off:?}"))?;
    let (columns, rows) = TileRect::grid((FRAME, FRAME), TILE);
    let count = usize::try_from(columns * rows).unwrap_or(0);
    let finished = AtomicUsize::new(0);
    let (costs, _) = within(budget, |cancel| {
        thread::scope(|scope| {
            let workers: Vec<_> = (0..count)
                .map(|index| {
                    let (frame, finished) = (&frame, &finished);
                    scope.spawn(move || {
                        let started = Instant::now();
                        let traced = frame.tile_leaves(index, TILE, cancel);
                        finished.fetch_add(1, Ordering::Relaxed);
                        TileCost {
                            seconds: started.elapsed().as_secs_f64(),
                            stats: traced.map(|(_, stats, _)| stats),
                        }
                    })
                })
                .collect();
            workers
                .into_iter()
                .map(|worker| worker.join().expect("a tile thread"))
                .collect::<Vec<_>>()
        })
    });
    Ok(costs)
}

/// The `COST` line of one tile.
fn tile_line(label: &str, tile: usize, cost: &TileCost) -> String {
    let Some(stats) = &cost.stats else {
        return format!(
            "COST {label} tile {tile}: OVER BUDGET after {:.1} s",
            cost.seconds
        );
    };
    let drops: u64 = stats.drops.iter().sum();
    let deepest = stats
        .depth_histogram
        .iter()
        .rposition(|&count| count > 0)
        .unwrap_or(0);
    format!(
        "COST {label} tile {tile}: {:.2} s, primaries {}, beams {}, leaves {}, partitions {}, \
         repartitions {}, apex splits {}, tir splits {}, entry pieces {}, drops {drops}, \
         attempted beams {}, attempted pieces {}, lost nodes {}, deepest bin {deepest}, \
         depth {:?}",
        cost.seconds,
        stats.primary_beams,
        stats.beams,
        stats.leaves,
        stats.partitions,
        stats.repartitions,
        stats.apex_splits,
        stats.tir_splits,
        stats.entry_pieces,
        stats.attempted_beams,
        stats.attempted_pieces,
        stats.lost_nodes,
        &stats.depth_histogram[..=deepest],
    )
}

/// The attempted-work counters are monotone sums and `allowance_spent` flips exactly at
/// `TILE_ALLOWANCE`. A fast unit test, not a diagnostic row.
#[test]
fn the_tile_allowance_is_spent_exactly_at_the_constant() {
    use super::stats::TILE_ALLOWANCE;

    let mut stats = BeamStats::default();
    assert!(!stats.allowance_spent());
    stats.record_beam(0);
    stats.record_attempt();
    assert_eq!(stats.attempted_work(), 2);
    stats.attempted_pieces = TILE_ALLOWANCE - 2;
    assert!(!stats.allowance_spent());
    stats.record_attempt();
    assert!(stats.allowance_spent());
    // An attempt that carries the work and is discarded still leaves the work on the tile.
    let mut attempt = stats.carrying_attempts();
    attempt.record_attempt();
    let mut tile = BeamStats::default();
    tile.adopt_attempts(&attempt);
    assert_eq!(tile.attempted_work(), stats.attempted_work() + 1);
    assert_eq!(tile.beams, 0);
}

/// `allowance_spent_at` flips exactly at the allowance it is given.
#[test]
fn a_given_allowance_is_spent_exactly_at_its_value() {
    let mut stats = BeamStats {
        attempted_pieces: 4095,
        ..BeamStats::default()
    };
    assert!(!stats.allowance_spent_at(4096));
    assert!(stats.allowance_spent_at(4095));
    stats.record_attempt();
    assert!(stats.allowance_spent_at(4096));
    assert!(!stats.allowance_spent());
}

/// The lost-node counter merges by sum and is not a drop.
#[test]
fn lost_nodes_merge_by_sum_and_are_not_drops() {
    let mut total = BeamStats::default();
    let tile = BeamStats {
        lost_nodes: 3,
        ..BeamStats::default()
    };
    total.merge(&tile);
    total.merge(&tile);
    assert_eq!(total.lost_nodes, 6);
    assert_eq!(total.drops.iter().sum::<u64>(), 0);
}

/// The reflection collector counts its leaf attempts on the tile allowance: at two bounces a tile
/// holds reflection leaves only, and each is an attempt.
#[test]
fn the_reflection_collector_counts_against_the_tile_allowance() {
    let rig = BeamRig::cube(glass());
    let scene = rig.scene(FRAME, 2);
    let frame = BeamFrame::new(&scene, &BeamSettings::default()).expect("eligible");
    let cancel = AtomicBool::new(false);
    let (leaves, stats, _) = frame.tile_leaves(0, FRAME, &cancel).expect("tile");
    assert_ne!(leaves.len(), 0, "the tile keeps leaves");
    assert!(stats.attempted_pieces >= stats.leaves);
    assert!(stats.attempted_work() > stats.attempted_beams);
}

#[test]
#[ignore = "beam diagnostic: scripts/run_beam_tests.sh --cost (step 10) runs it"]
fn tree_cost_by_bounces_and_settings_for_the_relight_rig() {
    show_beam_log();
    let budget = seconds_from_env("PLEOCHROMA_BEAM_COST_BUDGET_S", DEFAULT_BUDGET_S);
    let rig = BeamRig::brilliant(glass());
    eprintln!(
        "COST rig: glass brilliant, {FRAME} px frame, {TILE} px tiles, budget {} s per row",
        budget.as_secs()
    );
    for (name, settings) in variants() {
        for bounces in BOUNCES {
            let label = format!("{name:<12} bounces {bounces:>2}");
            let costs = match measure(&rig, &settings, bounces, budget) {
                Ok(costs) => costs,
                Err(off) => {
                    eprintln!("COST {label}: not beam traced: {off}");
                    break;
                }
            };
            for (tile, cost) in costs.iter().enumerate() {
                eprintln!("{}", tile_line(&label, tile, cost));
            }
            let slowest = costs.iter().map(|cost| cost.seconds).fold(0.0, f64::max);
            let beams: u64 = costs
                .iter()
                .filter_map(|cost| cost.stats.as_ref())
                .map(|stats| stats.beams)
                .sum();
            let over = costs.iter().any(|cost| cost.stats.is_none());
            eprintln!(
                "COST {label}: row {slowest:.2} s, beams {beams}{}",
                if over { ", OVER BUDGET" } else { "" }
            );
            if over {
                eprintln!("COST {name}: deeper rows skipped");
                break;
            }
        }
    }
}

#[test]
#[ignore = "beam diagnostic: scripts/run_beam_tests.sh --cost (step 10) runs it"]
fn the_ray_traced_build_of_the_relight_rig_within_a_time_limit() {
    show_beam_log();
    let budget = seconds_from_env("PLEOCHROMA_BEAM_BUILD_BUDGET_S", DEFAULT_BUILD_BUDGET_S);
    let rig = BeamRig::brilliant(glass());
    let scene = scene_of(&rig, MAX_BOUNCES);
    // The settings of the re-light tests (`tests_d6_relight::beam_settings`).
    let settings = BeamSettings {
        residual_min_spp: 16,
        batch_tiles: 1,
        ..BeamSettings::default()
    };
    let background = vec![Vec3::ZERO; (FRAME * FRAME) as usize];
    let mut cache = LeafCache::new(u64::MAX);
    let (built, seconds) = within(budget, |cancel| {
        cache.relight_ray_traced(&scene, &settings, &background, cancel)
    });
    match built {
        Some(pixels) => eprintln!(
            "COST ray-traced build: {seconds:.2} s, {} pixels, {} bytes retained",
            pixels.len(),
            cache.bytes()
        ),
        None => eprintln!(
            "COST ray-traced build: no image after {seconds:.1} s (budget {} s: over budget, or \
             declined)",
            budget.as_secs()
        ),
    }
}
