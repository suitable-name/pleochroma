//! The one-time "compiling the graphics card's shaders" line of `pleochroma-worker render`.
//!
//! The first GPU render after an install or an update compiles the kernels it needs, which
//! takes 30 to 60 seconds with the graphics card idle. The command traces the whole picture in
//! one call and reports nothing meanwhile, so it says so once, on standard error, so the wait
//! is not mistaken for a hang.
//!
//! The number of pipelines the process compiled from source instead of loading them from the
//! cache (`GpuBackend::fresh_pipeline_compiles`) is read when the render starts and polled by a
//! small watcher thread while the render runs. The line prints when the number has risen. A
//! graphics driver that keeps no cache never raises the number (it has no cache to miss), so it
//! prints nothing instead of promising a cache it lacks.

use pleochroma::renderer::gpu_backend::GpuBackend;
use std::{
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

/// The line, with the ellipsis the owner asked for.
pub(super) const SHADER_HINT: &str = "Compiling the graphics card's shaders (the first run after an install or an update; later runs load them from the cache)\u{2026}";

/// How often the watcher reads the counter. The counter is one atomic load, so this is cheap, and
/// it is also how long the end of a render waits for the watcher to notice it is over.
const POLL: Duration = Duration::from_millis(50);

/// Whether this process has printed the line already: it prints at most once.
static PRINTED: AtomicBool = AtomicBool::new(false);

/// Whether the line is due: shaders were compiled since the render started (`now` is above
/// `baseline`) and the line was not printed before.
pub(super) const fn should_print(baseline: u64, now: u64, printed: bool) -> bool {
    now > baseline && !printed
}

/// Tells the watcher the render is over when it drops, on every path out of the render,
/// a panic included.
struct FinishOnDrop<'a>(&'a AtomicBool);

impl Drop for FinishOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

/// Reads the counter until the render ends or the line is printed.
fn watch_counter(baseline: u64, finished: &AtomicBool) {
    while !finished.load(Ordering::Acquire) {
        let due = should_print(
            baseline,
            GpuBackend::fresh_pipeline_compiles(),
            PRINTED.load(Ordering::Relaxed),
        );
        if due {
            if !PRINTED.swap(true, Ordering::AcqRel) {
                eprintln!("{SHADER_HINT}");
            }
            return;
        }
        std::thread::sleep(POLL);
    }
}

/// Runs `render` and prints the shader line while it runs when shaders get compiled.
///
/// `watching` is `false` for a run that does not use the graphics card: the render then runs as
/// it is.
pub(super) fn while_rendering<R>(watching: bool, render: impl FnOnce() -> R) -> R {
    if !watching {
        return render();
    }
    let baseline = GpuBackend::fresh_pipeline_compiles();
    let finished = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| watch_counter(baseline, &finished));
        let _finish = FinishOnDrop(&finished);
        render()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_line_is_due_when_the_counter_rose_during_the_render() {
        assert!(should_print(3, 4, false));
        assert!(should_print(0, 40, false));
    }

    #[test]
    fn the_line_waits_while_the_counter_is_where_the_render_started() {
        assert!(!should_print(3, 3, false));
    }

    #[test]
    fn the_line_prints_once_per_process() {
        assert!(!should_print(3, 9, true));
    }

    #[test]
    fn a_render_that_is_not_watched_just_runs() {
        assert_eq!(while_rendering(false, || 7), 7);
    }

    #[test]
    fn the_watcher_ends_with_the_render() {
        assert_eq!(while_rendering(true, || "done"), "done");
    }

    #[test]
    fn the_line_names_the_cache_and_ends_in_an_ellipsis() {
        assert!(SHADER_HINT.starts_with("Compiling the graphics card's shaders"));
        assert!(SHADER_HINT.contains("later runs load them from the cache"));
        assert!(SHADER_HINT.ends_with('\u{2026}'));
    }
}
