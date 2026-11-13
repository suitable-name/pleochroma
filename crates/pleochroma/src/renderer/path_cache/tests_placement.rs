//! Tests of the placement decision.

use super::{
    estimate::{Measurements, SizeEstimate},
    placement::{
        MARGIN_DIVISOR, MemoryBudget, ONE_SHOT_NOTE, OffReason, Placement, RAM_HEADROOM_BYTES,
        choose_placement, plan, sample_cost, status_text, with_margin,
    },
    positions::BuildSettings,
};

const GIB: u64 = 1024 * 1024 * 1024;

fn budget(vram_free: Option<u64>, vram_reserved: u64, ram_available: Option<u64>) -> MemoryBudget {
    MemoryBudget {
        vram_free,
        vram_reserved,
        ram_available,
    }
}

#[test]
fn the_margin_is_twenty_percent_rounded_up() {
    assert_eq!(MARGIN_DIVISOR, 5);
    assert_eq!(with_margin(0), 0);
    assert_eq!(with_margin(100), 120);
    assert_eq!(with_margin(101), 122);
    assert_eq!(with_margin(u64::MAX), u64::MAX);
    assert_eq!(RAM_HEADROOM_BYTES, 4 * GIB);
}

#[test]
fn a_cache_that_fits_video_memory_goes_there() {
    let memory = budget(Some(20 * GIB), 2 * GIB, Some(64 * GIB));
    assert_eq!(choose_placement(10 * GIB, &memory), Placement::Vram);
}

#[test]
fn a_cache_larger_than_video_memory_goes_to_system_ram() {
    let memory = budget(Some(20 * GIB), 2 * GIB, Some(64 * GIB));
    // 16 GiB plus the margin is more than the 18 GiB left in video memory.
    assert_eq!(choose_placement(16 * GIB, &memory), Placement::SystemRam);
    // Without a video memory figure, system RAM is the first tier.
    let no_gpu = budget(None, 0, Some(64 * GIB));
    assert_eq!(choose_placement(GIB, &no_gpu), Placement::SystemRam);
}

#[test]
fn a_cache_larger_than_both_is_off_with_a_reason() {
    let memory = budget(Some(20 * GIB), 2 * GIB, Some(16 * GIB));
    let placement = choose_placement(40 * GIB, &memory);
    assert_eq!(
        placement,
        Placement::Off(OffReason::TooLarge {
            required_bytes: with_margin(40 * GIB),
            best_available_bytes: 18 * GIB,
        })
    );
    assert!(!placement.is_cached());
}

#[test]
fn the_margin_and_the_headroom_decide_at_the_boundary() {
    // Video memory: 12 GiB usable fits exactly a 10 GiB cache, since 10 GiB plus 20 % is 12 GiB.
    let usable = 12 * GIB;
    let exact = usable / 6 * 5;
    assert!(with_margin(exact) <= usable);
    let fits = budget(Some(usable), 0, None);
    assert_eq!(choose_placement(exact, &fits), Placement::Vram);
    // One byte more needs more than the usable bytes.
    let too_big = exact + 8;
    assert!(with_margin(too_big) > usable);
    assert_eq!(
        choose_placement(too_big, &fits),
        Placement::Off(OffReason::TooLarge {
            required_bytes: with_margin(too_big),
            best_available_bytes: usable,
        })
    );

    // System RAM: the 4 GiB headroom comes off first.
    let ram = budget(None, 0, Some(RAM_HEADROOM_BYTES + usable));
    assert_eq!(choose_placement(exact, &ram), Placement::SystemRam);
    assert!(matches!(
        choose_placement(too_big, &ram),
        Placement::Off(OffReason::TooLarge { .. })
    ));
    let squeezed = budget(None, 0, Some(RAM_HEADROOM_BYTES));
    assert!(matches!(
        choose_placement(1, &squeezed),
        Placement::Off(OffReason::TooLarge {
            best_available_bytes: 0,
            ..
        })
    ));
}

#[test]
fn reserved_video_memory_is_not_available_to_the_cache() {
    let memory = budget(Some(8 * GIB), 6 * GIB, Some(64 * GIB));
    assert_eq!(memory.vram_usable(), Some(2 * GIB));
    assert_eq!(choose_placement(3 * GIB, &memory), Placement::SystemRam);
    // More reserved than free saturates at zero instead of wrapping.
    let over = budget(Some(GIB), 6 * GIB, None);
    assert_eq!(over.vram_usable(), Some(0));
}

#[test]
fn nothing_to_cache_and_unknown_memory_are_outcomes_not_errors() {
    let memory = budget(Some(20 * GIB), 0, Some(64 * GIB));
    assert_eq!(
        choose_placement(0, &memory),
        Placement::Off(OffReason::NothingToCache)
    );
    assert_eq!(
        choose_placement(GIB, &MemoryBudget::default()),
        Placement::Off(OffReason::MemoryUnknown)
    );
    assert_eq!(
        choose_placement(u64::MAX, &memory),
        Placement::Off(OffReason::TooLarge {
            required_bytes: u64::MAX,
            best_available_bytes: 60 * GIB,
        })
    );
}

#[test]
fn the_status_line_names_the_size_and_the_tier() {
    // 8000 x 4000 pixels, all stone: a few GB.
    let measured = Measurements {
        pixels: 1,
        stone_pixels: 1,
        edge_pixels: 0,
        positions: 4,
        records: 4,
        fire_milli: 0,
    };
    let estimate =
        SizeEstimate::from_measurements(&measured, (8000, 4000), &BuildSettings::default(), 2.0);
    let ram = budget(None, 0, Some(64 * GIB));
    let (placement, text) = plan(&estimate, &ram);
    assert_eq!(placement, Placement::SystemRam);
    assert!(text.starts_with("Path cache: about "), "{text}");
    assert!(text.ends_with(", in system RAM"), "{text}");

    let gpu = budget(Some(32 * GIB), GIB, Some(64 * GIB));
    assert!(plan(&estimate, &gpu).1.ends_with(", in video memory"));

    let none = budget(Some(GIB), 0, Some(5 * GIB));
    let (placement, text) = plan(&estimate, &none);
    assert!(matches!(
        placement,
        Placement::Off(OffReason::TooLarge { .. })
    ));
    assert!(text.ends_with("rendering without it"), "{text}");
    assert!(text.contains("safety margin"), "{text}");

    assert!(status_text(&estimate, &Placement::Off(OffReason::MemoryUnknown)).contains("unknown"));
    let empty = SizeEstimate::from_measurements(
        &Measurements::default(),
        (64, 64),
        &BuildSettings::default(),
        2.0,
    );
    assert_eq!(
        plan(&empty, &gpu).1,
        "Path cache: not needed, the stone is not in view"
    );
}

/// An estimate of 1000 positions with `records` records per position and a fired share of
/// `fire_milli` thousandths.
fn costed(records: u64, fire_milli: u64) -> SizeEstimate {
    let measured = Measurements {
        pixels: 1,
        stone_pixels: 1,
        edge_pixels: 0,
        positions: 1000,
        records: 1000 * records,
        fire_milli: 1000 * fire_milli,
    };
    SizeEstimate::from_measurements(&measured, (256, 256), &BuildSettings::default(), 2.0)
}

/// The cost rule refuses a stone whose cached sample costs one traced sample or more.
#[test]
fn a_stone_that_shades_slower_from_the_cache_is_off_with_a_reason() {
    let gpu = budget(Some(32 * GIB), GIB, Some(64 * GIB));
    // 100 records cost 0.6 samples: on.
    let (placement, _) = plan(&costed(100, 0), &gpu);
    assert!(placement.is_cached(), "{placement:?}");
    // 200 records cost 1.2 samples: off, whatever the memory.
    let (placement, text) = plan(&costed(200, 0), &gpu);
    assert_eq!(
        placement,
        Placement::Off(OffReason::SlowerThanTracing {
            records_per_position: 200
        })
    );
    assert!(
        text.starts_with(
            "Path cache: not used, this stone has about 200 exit records per position"
        ),
        "{text}"
    );
    assert!(
        text.ends_with("shades slower from the cache than it traces"),
        "{text}"
    );
    // 100 records and a fired share of 0.4 cost 0.6 + 0.48: off.
    let (placement, _) = plan(&costed(100, 400), &gpu);
    assert!(matches!(
        placement,
        Placement::Off(OffReason::SlowerThanTracing { .. })
    ));
    // The rule comes before the memory tiers.
    let none = MemoryBudget::default();
    assert!(matches!(
        plan(&costed(200, 0), &none).0,
        Placement::Off(OffReason::SlowerThanTracing { .. })
    ));
}

/// A cache with a large fired share stays on, on either tier, and the status line gives the share.
#[test]
fn a_cache_that_fires_often_stays_on_and_says_how_often() {
    let measured = Measurements {
        pixels: 1,
        stone_pixels: 1,
        edge_pixels: 0,
        positions: 100,
        records: 400,
        // A mean trace probability of 0.05.
        fire_milli: 5_000,
    };
    let estimate =
        SizeEstimate::from_measurements(&measured, (256, 256), &BuildSettings::default(), 2.0);
    assert!((estimate.fire_share - 0.05).abs() < 1e-12);
    let gpu = budget(Some(32 * GIB), GIB, Some(64 * GIB));
    let (placement, text) = plan(&estimate, &gpu);
    assert_eq!(placement, Placement::Vram);
    assert!(
        text.contains(", 5.0 % of samples traced again, in video memory"),
        "{text}"
    );
    let cpu_only = budget(None, 0, Some(64 * GIB));
    let (placement, text) = plan(&estimate, &cpu_only);
    assert_eq!(placement, Placement::SystemRam);
    assert!(
        text.ends_with("5.0 % of samples traced again, in system RAM"),
        "{text}"
    );

    // A share of 0.31 with 4 records per position costs 0.4 samples: still on.
    let often = Measurements {
        fire_milli: 31_000,
        ..measured
    };
    let estimate =
        SizeEstimate::from_measurements(&often, (256, 256), &BuildSettings::default(), 2.0);
    assert!(sample_cost(&estimate) < 1.0);
    let (placement, text) = plan(&estimate, &gpu);
    assert_eq!(placement, Placement::Vram);
    assert!(text.contains("31.0 % of samples traced again"), "{text}");

    let never = Measurements {
        fire_milli: 0,
        ..measured
    };
    let estimate =
        SizeEstimate::from_measurements(&never, (256, 256), &BuildSettings::default(), 2.0);
    let (_, text) = plan(&estimate, &gpu);
    assert!(!text.contains("traced again"), "{text}");
}

/// The note of a render that does not keep its cache says what to do instead.
#[test]
fn the_one_shot_note_names_the_box_and_the_flag() {
    assert!(ONE_SHOT_NOTE.starts_with("Path cache: not used, "));
    assert!(ONE_SHOT_NOTE.contains("keep box under Path Cache"));
    assert!(ONE_SHOT_NOTE.contains("--persist-cache"));
    // The benchmark reads these words as "the cache was used".
    assert!(!ONE_SHOT_NOTE.contains("in video memory") && !ONE_SHOT_NOTE.contains("in system RAM"));
}
