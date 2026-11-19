//! The "still in <step>" line: a small thread that reports a preparing step that takes long.

use std::{
    sync::{Arc, Condvar, Mutex, PoisonError, Weak},
    thread,
    time::Duration,
};

use pleochroma::diag::{self, Engine, PrepareProgress, PrepareStep};

use super::session::Session;

/// How often the thread looks at the progress snapshot.
const POLL: Duration = Duration::from_secs(5);

/// The first line comes after this long in one step, then one every `LINE_EVERY_MS`.
const FIRST_LINE_MS: u64 = 30_000;

/// Time between two "still in" lines.
const LINE_EVERY_MS: u64 = 15_000;

/// The stop signal shared with the thread.
type Stop = Arc<(Mutex<bool>, Condvar)>;

/// The running ticker thread of a session. Dropping it asks the thread to end.
pub struct Ticker {
    stop: Stop,
}

impl Ticker {
    /// Starts the thread. It holds the session weakly and ends when the session is gone.
    #[must_use]
    pub fn start(session: &Arc<Session>) -> Self {
        let stop: Stop = Arc::new((Mutex::new(false), Condvar::new()));
        let weak = Arc::downgrade(session);
        let signal = Arc::clone(&stop);
        // A thread that cannot start only means no "still in" lines.
        let _ = thread::Builder::new()
            .name("diag-ticker".to_owned())
            .spawn(move || run(&weak, &signal));
        Self { stop }
    }
}

impl Drop for Ticker {
    fn drop(&mut self) {
        let (flag, wake) = &*self.stop;
        *flag.lock().unwrap_or_else(PoisonError::into_inner) = true;
        wake.notify_all();
    }
}

fn run(session: &Weak<Session>, stop: &Stop) {
    let (flag, wake) = &**stop;
    let mut last: Option<(u64, u64)> = None;
    loop {
        {
            let stopped = flag.lock().unwrap_or_else(PoisonError::into_inner);
            let (stopped, _) = wake
                .wait_timeout_while(stopped, POLL, |stopped| !*stopped)
                .unwrap_or_else(PoisonError::into_inner);
            if *stopped {
                return;
            }
        }
        let Some(session) = session.upgrade() else {
            return;
        };
        if !session.tracer_on() {
            continue;
        }
        let progress = diag::prepare_progress();
        if let Some(line) = still_line(&progress, diag::process_ms(), &mut last) {
            session.write_line(&line);
        }
    }
}

/// The line for a step that has run 30 s, 45 s, 60 s and so on, once per 15 s bucket.
///
/// `last` is the `(start_ms, bucket)` of the line written before; it is replaced when a line
/// is returned. A step that ended or restarted begins a new series.
#[must_use]
pub fn still_line(
    progress: &PrepareProgress,
    now_ms: u64,
    last: &mut Option<(u64, u64)>,
) -> Option<String> {
    if progress.step == PrepareStep::Idle {
        *last = None;
        return None;
    }
    let elapsed = now_ms.saturating_sub(progress.start_ms);
    if elapsed < FIRST_LINE_MS {
        return None;
    }
    let bucket = elapsed / LINE_EVERY_MS;
    if *last == Some((progress.start_ms, bucket)) {
        return None;
    }
    *last = Some((progress.start_ms, bucket));
    Some(format!(
        "still in {}, {} s",
        step_words(progress),
        bucket * LINE_EVERY_MS / 1000
    ))
}

/// The step in plain words, with its count when it has one.
fn step_words(progress: &PrepareProgress) -> String {
    let engine = match progress.engine {
        Engine::PathTraced => "Path-Traced",
        Engine::RayTraced => "Ray-Traced",
    };
    match progress.step {
        PrepareStep::Idle => "idle".to_owned(),
        PrepareStep::StartingGpu => format!("starting the graphics card ({engine})"),
        PrepareStep::BuildingShaders if progress.total > 1 => {
            format!(
                "building shaders ({engine}) {} of {}",
                progress.done, progress.total
            )
        }
        PrepareStep::BuildingShaders => format!("building the {engine} shader"),
        PrepareStep::Uploading => format!("uploading the stone ({engine})"),
        PrepareStep::FirstTrace => format!("the first trace ({engine})"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn progress(step: PrepareStep, start_ms: u64) -> PrepareProgress {
        PrepareProgress {
            engine: Engine::PathTraced,
            step,
            done: 3,
            total: 18,
            start_ms,
        }
    }

    #[test]
    fn nothing_is_written_before_thirty_seconds() {
        let mut last = None;
        let p = progress(PrepareStep::BuildingShaders, 1000);
        assert_eq!(still_line(&p, 1000, &mut last), None);
        assert_eq!(still_line(&p, 30_999, &mut last), None);
    }

    #[test]
    fn a_line_comes_at_thirty_seconds_then_every_fifteen_and_only_once_per_bucket() {
        let mut last = None;
        let p = progress(PrepareStep::BuildingShaders, 1000);
        let first = still_line(&p, 31_500, &mut last).expect("30 s line");
        assert_eq!(
            first,
            "still in building shaders (Path-Traced) 3 of 18, 30 s"
        );
        assert_eq!(still_line(&p, 36_500, &mut last), None);
        assert_eq!(
            still_line(&p, 46_000, &mut last).as_deref(),
            Some("still in building shaders (Path-Traced) 3 of 18, 45 s")
        );
        assert_eq!(still_line(&p, 46_100, &mut last), None);
    }

    #[test]
    fn idle_or_a_new_step_begins_a_new_series() {
        let mut last = None;
        let p = progress(PrepareStep::StartingGpu, 0);
        assert!(still_line(&p, 31_000, &mut last).is_some());
        assert_eq!(
            still_line(&progress(PrepareStep::Idle, 0), 40_000, &mut last),
            None
        );
        assert_eq!(last, None);
        assert!(still_line(&p, 61_000, &mut last).is_some());
        let next = progress(PrepareStep::Uploading, 61_000);
        assert_eq!(still_line(&next, 70_000, &mut last), None);
    }
}
