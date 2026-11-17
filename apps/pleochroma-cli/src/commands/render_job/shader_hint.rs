//! The one-time "compiling the graphics card's shaders" line of `render` and `tilt-video`.
//!
//! The first GPU render after an install or an update compiles the kernels it needs, which
//! takes 30 to 60 seconds with the graphics card idle and no sample traced yet. The run says so
//! once, on standard error, so the wait is not mistaken for a hang.
//!
//! The number of pipelines the process compiled from source instead of loading them from the
//! cache (`GpuBackend::fresh_pipeline_compiles`) is read when the render starts and polled by a
//! small watcher thread while the render runs. The line prints when the number has risen and no
//! sample has been traced yet. A graphics driver that keeps no cache never raises the number
//! (it has no cache to miss), so it prints nothing instead of promising a cache it lacks.

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
/// `baseline`), no sample has been traced yet, and the line was not printed before.
pub(super) const fn should_print(
    baseline: u64,
    now: u64,
    samples_started: bool,
    printed: bool,
) -> bool {
    now > baseline && !samples_started && !printed
}

/// Tells the watcher the render is over when it drops, on every path out of the render,
/// a panic included.
struct FinishOnDrop<'a>(&'a AtomicBool);

impl Drop for FinishOnDrop<'_> {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

/// Reads the counter until the render ends, the first sample is traced or the line is printed.
fn watch_counter(baseline: u64, samples_started: &AtomicBool, finished: &AtomicBool) {
    loop {
        if finished.load(Ordering::Acquire) {
            return;
        }
        let started = samples_started.load(Ordering::Acquire);
        let due = should_print(
            baseline,
            GpuBackend::fresh_pipeline_compiles(),
            started,
            PRINTED.load(Ordering::Relaxed),
        );
        if due {
            if !PRINTED.swap(true, Ordering::AcqRel) {
                eprintln!("{SHADER_HINT}");
            }
            return;
        }
        if started {
            return;
        }
        std::thread::sleep(POLL);
    }
}

/// Runs `render` and prints the shader line while it runs when shaders get compiled.
///
/// `watching` is `false` for `--quiet` and for a run that does not use the graphics card (the
/// `--local cpu` choice, or a build without GPU support): the render then runs as it is.
/// `samples_started` is set by the progress sink at the first traced sample.
pub(super) fn while_rendering<R>(
    watching: bool,
    samples_started: &AtomicBool,
    render: impl FnOnce() -> R,
) -> R {
    if !watching {
        return render();
    }
    let baseline = GpuBackend::fresh_pipeline_compiles();
    let finished = AtomicBool::new(false);
    std::thread::scope(|scope| {
        scope.spawn(|| watch_counter(baseline, samples_started, &finished));
        let _finish = FinishOnDrop(&finished);
        render()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_line_is_due_when_the_counter_rose_before_the_first_sample() {
        assert!(should_print(3, 4, false, false));
        assert!(should_print(0, 40, false, false));
    }

    #[test]
    fn the_line_waits_while_the_counter_is_where_the_render_started() {
        assert!(!should_print(3, 3, false, false));
    }

    #[test]
    fn the_line_is_not_due_once_samples_are_being_traced() {
        assert!(!should_print(3, 9, true, false));
    }

    #[test]
    fn the_line_prints_once_per_process() {
        assert!(!should_print(3, 9, false, true));
    }

    #[test]
    fn a_render_that_is_not_watched_just_runs() {
        let started = AtomicBool::new(false);
        assert_eq!(while_rendering(false, &started, || 7), 7);
    }

    #[test]
    fn the_watcher_ends_with_the_render() {
        // A render that compiles nothing: the watcher polls, sees the end and returns.
        let started = AtomicBool::new(false);
        assert_eq!(while_rendering(true, &started, || "done"), "done");
    }

    #[test]
    fn the_line_names_the_cache_and_ends_in_an_ellipsis() {
        assert!(SHADER_HINT.starts_with("Compiling the graphics card's shaders"));
        assert!(SHADER_HINT.contains("later runs load them from the cache"));
        assert!(SHADER_HINT.ends_with('\u{2026}'));
    }
}
