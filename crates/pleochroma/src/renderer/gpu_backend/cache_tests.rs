//! Tests of the host side of the GPU path cache (no GPU needed; they run in both builds).
//!
//! They cover the slab plan, the placement tiers from fake budgets, the split of a pass's live
//! pixels into the pixels phase B shades and the pixels the path tracer keeps, the order of the
//! host-side residual completion, and the declines of a backend with no GPU.

use std::sync::atomic::AtomicBool;

use glam::Vec3;

use super::{
    CacheTier, GpuAccumulate, GpuBackend, GpuPathCache, GpuSceneRef, MemoryHints, RAM_SLAB_BYTES,
    VRAM_SLAB_BYTES, memory_budget, plan_span,
};
use crate::{
    geometry::{GpuFacetPlane, cuts::StandardGemCuts},
    optics::{
        fluorescence::Fluorescence,
        materials::GemMaterial,
        raytracer::{Camera, LightingPreset, build_plane_soa, surface::SurfaceScene},
    },
    renderer::{
        adaptive::LiveTiles,
        frame_scene::FrameScene,
        path_cache::{
            BuildSettings, CacheOptions, OffReason, PathCacheFrame, Placement, choose_placement,
        },
    },
    simd::PlanesSoA32,
};

const GIB: u64 = 1024 * 1024 * 1024;
const MIB: u64 = 1024 * 1024;

/// Frame of the fixture: 24 x 16 pixels in 8-pixel tiles, 3 x 2 tiles.
const FRAME: (u32, u32) = (24, 16);
const TILE: u32 = 8;
const TILES: u32 = 6;

struct Fixture {
    camera: Camera,
    planes: Vec<GpuFacetPlane>,
    material: GemMaterial,
    soa: PlanesSoA32,
}

impl Fixture {
    fn new() -> Self {
        let planes = StandardGemCuts::standard_round_brilliant();
        let soa = build_plane_soa(&planes);
        Self {
            camera: Camera::new(0.35, 0.28, 5.0, 18.0),
            planes,
            material: GemMaterial::new_custom("cache host test glass", 1.6, 0.0, 0.0, [0.0; 3]),
            soa,
        }
    }

    fn scene(&self) -> FrameScene<'_> {
        FrameScene {
            camera: &self.camera,
            width: FRAME.0,
            height: FRAME.1,
            planes: &self.planes,
            facet_finishes: &[],
            material: &self.material,
            max_bounces: 4,
            environment: LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
        }
    }

    fn options() -> CacheOptions {
        CacheOptions {
            settings: BuildSettings {
                tile: TILE,
                k_max: 8,
                ..BuildSettings::default()
            },
            ..CacheOptions::default()
        }
    }
}

fn frame<'s, 'a>(scene: &'s FrameScene<'a>, soa: &'s PlanesSoA32) -> PathCacheFrame<'s, 'a> {
    PathCacheFrame::new(
        scene,
        soa,
        (&[], Fluorescence::none(), SurfaceScene::IDEAL),
        Fixture::options(),
    )
    .expect("a plain glass stone may use the cache")
}

#[test]
fn a_slab_takes_tiles_while_they_fit_and_always_at_least_one() {
    let span = plan_span(5, [10, 10, 10, 10], 25);
    assert_eq!((span.first_tile, span.tile_count, span.bytes), (5, 2, 20));
    assert_eq!(span.end_tile(), 7);

    // A tile larger than the budget still forms a slab of its own.
    let big = plan_span(0, [100, 1, 1], 10);
    assert_eq!((big.tile_count, big.bytes), (1, 100));

    // Everything fits: one slab of all the tiles.
    assert_eq!(plan_span(0, [4, 4, 4], u64::MAX).tile_count, 3);
    // No tiles: an empty slab (the plan never asks for one).
    assert_eq!(plan_span(9, std::iter::empty::<u64>(), 10).tile_count, 0);
}

#[test]
fn the_tier_follows_the_free_memory_numbers() {
    // 4K frame: 8.3 M pixels x 48 B + 256 MiB reserved is about 650 MB of video memory.
    let (width, height) = (3840, 2160);
    let estimate = GIB;

    let roomy = memory_budget(
        MemoryHints {
            vram_free: Some(32 * GIB),
            ram_available: Some(64 * GIB),
        },
        width,
        height,
    );
    assert_eq!(choose_placement(estimate, &roomy), Placement::Vram);

    // Video memory is almost taken by the render itself: the cache goes to system RAM.
    let tight_vram = memory_budget(
        MemoryHints {
            vram_free: Some(700 * MIB),
            ram_available: Some(64 * GIB),
        },
        width,
        height,
    );
    assert_eq!(
        choose_placement(estimate, &tight_vram),
        Placement::SystemRam
    );

    // Neither fits (RAM less than its 4 GiB headroom plus the cache): off, with the reason.
    let nothing = memory_budget(
        MemoryHints {
            vram_free: Some(700 * MIB),
            ram_available: Some(5 * GIB),
        },
        width,
        height,
    );
    assert!(matches!(
        choose_placement(estimate, &nothing),
        Placement::Off(OffReason::TooLarge { .. })
    ));

    // Nothing known at all: off, never an error.
    let unknown = memory_budget(MemoryHints::default(), width, height);
    assert_eq!(
        choose_placement(estimate, &unknown),
        Placement::Off(OffReason::MemoryUnknown)
    );
}

#[test]
fn the_tiers_name_their_slab_size_and_residency() {
    assert_eq!(CacheTier::of(&Placement::Vram), Some(CacheTier::Vram));
    assert_eq!(
        CacheTier::of(&Placement::SystemRam),
        Some(CacheTier::SystemRam)
    );
    assert_eq!(
        CacheTier::of(&Placement::Off(OffReason::NothingToCache)),
        None
    );
    assert!(CacheTier::Vram.keeps_slabs_resident());
    assert!(!CacheTier::SystemRam.keeps_slabs_resident());
    assert_eq!(CacheTier::Vram.slab_budget_bytes(), VRAM_SLAB_BYTES);
    assert_eq!(CacheTier::SystemRam.slab_budget_bytes(), RAM_SLAB_BYTES);
    const {
        assert!(RAM_SLAB_BYTES < VRAM_SLAB_BYTES);
    }
}

#[test]
fn the_slab_plan_covers_every_tile_once_and_does_not_depend_on_the_asker() {
    let fixture = Fixture::new();
    let scene = fixture.scene();
    let cache_frame = frame(&scene, &fixture.soa);

    // One byte of budget: a slab per tile.
    let singles = GpuPathCache::new(&cache_frame, CacheTier::SystemRam).with_slab_budget_bytes(1);
    let mut next = 0;
    let mut index = 0;
    while let Some(span) = singles.slab(index) {
        assert_eq!(span.first_tile, next);
        assert_eq!(span.tile_count, 1);
        next = span.end_tile();
        index += 1;
    }
    assert_eq!((next, singles.planned_slabs()), (TILES, TILES as usize));

    // A huge budget: everything in one slab, however it is asked for.
    let whole = GpuPathCache::new(&cache_frame, CacheTier::Vram);
    let span = whole.slab(0).expect("one slab");
    assert_eq!((span.first_tile, span.tile_count), (0, TILES));
    assert_eq!(whole.slab(1), None);
    assert_eq!(whole.slab(0), Some(span));
    assert_eq!(usize::try_from(span.bytes), Ok(cache_frame.built_bytes()));

    // A second cache over the same frame plans the same slabs.
    let again = GpuPathCache::new(&cache_frame, CacheTier::SystemRam).with_slab_budget_bytes(1);
    for index in 0..TILES as usize {
        assert_eq!(again.slab(index), singles.slab(index));
    }
    assert_ne!(again.id(), singles.id());
}

#[test]
fn the_host_completion_hook_is_off_unless_asked_for() {
    let fixture = Fixture::new();
    let scene = fixture.scene();
    let cache_frame = frame(&scene, &fixture.soa);
    assert!(!GpuPathCache::new(&cache_frame, CacheTier::Vram).cpu_completion());
    assert!(
        GpuPathCache::new(&cache_frame, CacheTier::Vram)
            .with_cpu_completion()
            .cpu_completion()
    );
}

#[test]
fn a_prepared_slab_splits_the_live_pixels_without_loss() {
    let fixture = Fixture::new();
    let scene = fixture.scene();
    let cache_frame = frame(&scene, &fixture.soa);
    let cache = GpuPathCache::new(&cache_frame, CacheTier::Vram);

    // Tiles 1 and 4: an interior-ish column of the 3 x 2 grid.
    let live = LiveTiles::from_indices(FRAME.0, FRAME.1, TILE, vec![1, 4]);
    let host = cache.prepare_slab(0, &live).expect("slab 0");
    assert_eq!(host.tiles.len(), TILES as usize);
    assert_eq!(host.tile_offsets.len(), TILES as usize);
    assert_eq!(host.tile_offsets[0], 0);
    assert!(host.tile_offsets.windows(2).all(|w| w[0] < w[1]));
    assert_eq!(host.words(), cache_frame.built_bytes() / 4);

    let mut all: Vec<u32> = host.pixels.iter().chain(&host.rest).copied().collect();
    all.sort_unstable();
    let mut expected: Vec<u32> = (0..FRAME.1)
        .flat_map(|y| (0..FRAME.0).map(move |x| (y, x)))
        .filter(|&(_, x)| (TILE..2 * TILE).contains(&x))
        .map(|(y, x)| y * FRAME.0 + x)
        .collect();
    expected.sort_unstable();
    assert_eq!(all, expected, "each live pixel is in exactly one list");

    // The cached pixels are those the tiles hold positions for.
    for &pixel in &host.pixels {
        let (x, y) = (pixel % FRAME.0, pixel / FRAME.0);
        let tile_index = cache_frame.tile_of((x, y));
        let tile = cache_frame.tile(tile_index).expect("tile");
        let local = ((y - (y / TILE) * TILE) * TILE + (x - (x / TILE) * TILE)) as usize;
        assert!(!tile.pixel_positions(local).is_empty());
    }
    // Per tile the cached pixels come in row-major order.
    assert!(
        host.pixels
            .windows(2)
            .all(|w| w[0] / FRAME.0 / TILE <= w[1] / FRAME.0 / TILE)
    );
}

#[test]
fn prefetch_workers_leave_the_plan_and_the_value_of_the_body_alone() {
    let fixture = Fixture::new();
    let scene = fixture.scene();
    let cache_frame = frame(&scene, &fixture.soa);
    let cache = GpuPathCache::new(&cache_frame, CacheTier::SystemRam).with_slab_budget_bytes(1);
    let planned = cache.with_prefetch(TILES - 1, || {
        for index in 0..TILES as usize {
            cache.slab(index).expect("slab");
        }
        cache.planned_slabs()
    });
    assert_eq!(planned, TILES as usize);
    assert_eq!(cache_frame.built_tiles(), TILES as usize);
    // Every tile planned: no thread starts and the body still runs.
    assert_eq!(cache.with_prefetch(TILES - 1, || 7), 7);
}

#[test]
fn fired_samples_complete_in_sorted_order_whatever_order_the_gpu_reported() {
    let fixture = Fixture::new();
    let scene = fixture.scene();
    let cache_frame = frame(&scene, &fixture.soa);
    let cache = GpuPathCache::new(&cache_frame, CacheTier::Vram);
    let live = LiveTiles::all(FRAME.0, FRAME.1, TILE);
    let host = cache.prepare_slab(0, &live).expect("slab 0");
    assert!(
        host.pixels.len() > 4,
        "the fixture stone covers some pixels"
    );

    let mut shuffled = vec![[3, 5, 0, 0], [1, 2, 0, 0], [3, 4, 0, 0], [1, 2, 0, 0]];
    let mut sorted = vec![[1, 2, 0, 0], [3, 4, 0, 0], [3, 5, 0, 0]];
    let from_shuffled = cache.complete_residuals(&host, FRAME.0, &mut shuffled);
    let from_sorted = cache.complete_residuals(&host, FRAME.0, &mut sorted);
    let keys: Vec<(usize, u32)> = from_shuffled.iter().map(|&(v, s, _)| (v, s)).collect();
    assert_eq!(keys, vec![(1, 2), (3, 4), (3, 5)]);
    assert_eq!(from_shuffled, from_sorted, "same samples, same values");
    assert!(from_shuffled.iter().all(|(_, _, value)| value.is_finite()));
    assert_eq!(
        cache.complete_residuals(&host, FRAME.0, &mut Vec::new()),
        Vec::new()
    );
}

/// A backend with no GPU declines a cached pass and leaves both buffers alone, in either build.
#[test]
fn a_backend_without_a_gpu_declines_a_cached_pass_and_touches_nothing() {
    let fixture = Fixture::new();
    let scene = fixture.scene();
    let cache_frame = frame(&scene, &fixture.soa);
    let cache = GpuPathCache::new(&cache_frame, CacheTier::Vram);
    let gpu_scene = GpuSceneRef {
        camera: &fixture.camera,
        width: FRAME.0,
        height: FRAME.1,
        planes: &fixture.planes,
        facet_finishes: &[],
        material: &fixture.material,
        max_bounces: 4,
        environment: LightingPreset::Daylight.studio(1.0, 0.4, 0.35),
        estimator: None,
    };
    let live = LiveTiles::all(FRAME.0, FRAME.1, TILE);
    let sentinel = Vec3::new(1.5, -2.5, 3.5);
    let len = (FRAME.0 * FRAME.1) as usize;
    let mut accum = vec![sentinel; len];
    let mut half_b = vec![sentinel; len];
    let never_cancel = AtomicBool::new(false);
    let backend = GpuBackend::disabled();
    let outcome = backend.try_accumulate_cached_live_cancellable(
        &gpu_scene,
        &[],
        &SurfaceScene::IDEAL,
        0,
        4,
        &live,
        &cache,
        &mut accum,
        &mut half_b,
        &never_cancel,
    );
    assert_eq!(outcome, GpuAccumulate::Declined);
    assert!(accum.iter().chain(&half_b).all(|p| *p == sentinel));
    assert_eq!(backend.path_cache_slab_limit(), None);
    backend.release_path_cache(&cache);
}
