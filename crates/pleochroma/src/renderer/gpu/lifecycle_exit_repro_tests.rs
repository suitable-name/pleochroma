//! Thread-exit arm of the stall reproduction run by `scripts/retest.sh` (`--exit-repro`).
//!
//! The stall has three parties: a thread in `vkDestroyDevice` holding the Vulkan loader's lock
//! and a driver mutex while it joins the device's worker thread, that worker spinning in the
//! driver's thread-exit destructor, and a finished thread blocked in the same destructor on the
//! driver mutex. The lifecycle turn orders creation and teardown, not thread exits, so the
//! `exit_*` tests (turn on) add thread exits at times the turn does not order. A late exit ends
//! a whole lifecycle a few milliseconds after its own turn; a hybrid helper compiles, dispatches
//! and reads back on a short-lived thread, as the app's hybrid frame does; a warm-up helper
//! acquires and compiles on a short-lived thread that hands the device over, as the app's
//! warm-up does.

use std::time::Duration;

use super::lifecycle_repro_tests::{acquire_or_skip, compile, dispatch_and_check, gpu_lifecycle};

/// A whole lifecycle on the test thread, which then exits `delay_ms` later instead of right after
/// its own turn: its exit can fall inside another thread's teardown.
fn late_exit(name: &str, cells: u32, delay_ms: u64) {
    gpu_lifecycle(name, cells);
    std::thread::sleep(Duration::from_millis(delay_ms));
}

/// The app's hybrid frame: the test thread owns the device; a named scoped thread compiles,
/// dispatches and reads back, then exits before the test thread tears the device down.
fn hybrid_helper(name: &str, cells: u32) {
    let Some(ctx) = acquire_or_skip(name) else {
        return;
    };
    std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("repro hybrid".to_owned())
            .spawn_scoped(scope, || {
                let pipeline = compile(&ctx, name);
                dispatch_and_check(&ctx, &pipeline, name, cells);
            })
            .expect("the hybrid helper thread starts")
            .join()
            .expect("the hybrid helper thread does not panic");
    });
}

/// The app's warm-up: a named thread acquires the device and compiles, hands both over and
/// exits; the test thread dispatches, reads back and tears the device down.
fn warm_up_helper(name: &str, cells: u32) {
    let acquired = std::thread::scope(|scope| {
        std::thread::Builder::new()
            .name("repro warm-up".to_owned())
            .spawn_scoped(scope, || {
                acquire_or_skip(name).map(|ctx| {
                    let pipeline = compile(&ctx, name);
                    (ctx, pipeline)
                })
            })
            .expect("the warm-up helper thread starts")
            .join()
            .expect("the warm-up helper thread does not panic")
    });
    let Some((ctx, pipeline)) = acquired else {
        return;
    };
    dispatch_and_check(&ctx, &pipeline, name, cells);
    drop(pipeline);
    drop(ctx);
}

macro_rules! exit_repro_tests {
    ($($name:ident => $body:expr;)*) => {
        $(
            #[test]
            #[ignore = "stall reproduction: run by scripts/retest.sh (--exit-repro)"]
            fn $name() {
                $body;
            }
        )*
    };
}

exit_repro_tests! {
    exit_00_late => late_exit("exit_00_late", 64, 1);
    exit_01_hybrid => hybrid_helper("exit_01_hybrid", 256);
    exit_02_warm_up => warm_up_helper("exit_02_warm_up", 4096);
    exit_03_late => late_exit("exit_03_late", 65_536, 3);
    exit_04_hybrid => hybrid_helper("exit_04_hybrid", 64);
    exit_05_warm_up => warm_up_helper("exit_05_warm_up", 256);
    exit_06_late => late_exit("exit_06_late", 4096, 7);
    exit_07_hybrid => hybrid_helper("exit_07_hybrid", 65_536);
    exit_08_warm_up => warm_up_helper("exit_08_warm_up", 64);
    exit_09_late => late_exit("exit_09_late", 256, 15);
    exit_10_hybrid => hybrid_helper("exit_10_hybrid", 4096);
    exit_11_warm_up => warm_up_helper("exit_11_warm_up", 65_536);
    exit_12_late => late_exit("exit_12_late", 64, 1);
    exit_13_hybrid => hybrid_helper("exit_13_hybrid", 256);
    exit_14_warm_up => warm_up_helper("exit_14_warm_up", 4096);
    exit_15_late => late_exit("exit_15_late", 65_536, 3);
    exit_16_hybrid => hybrid_helper("exit_16_hybrid", 64);
    exit_17_warm_up => warm_up_helper("exit_17_warm_up", 256);
    exit_18_late => late_exit("exit_18_late", 4096, 7);
    exit_19_hybrid => hybrid_helper("exit_19_hybrid", 65_536);
    exit_20_warm_up => warm_up_helper("exit_20_warm_up", 64);
    exit_21_late => late_exit("exit_21_late", 256, 15);
    exit_22_hybrid => hybrid_helper("exit_22_hybrid", 4096);
    exit_23_warm_up => warm_up_helper("exit_23_warm_up", 65_536);
}
