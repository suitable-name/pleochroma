//! The five-tap counter that unlocks the Developer page for one app session.

use std::sync::{
    Mutex, PoisonError,
    atomic::{AtomicBool, Ordering},
};

/// The most time that may pass between two taps of one series.
const TAP_WINDOW_MS: u64 = 2000;

/// How many taps in a row unlock the page.
const TAPS_TO_UNLOCK: u8 = 5;

/// What one tap on the name-and-version block did.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TapOutcome {
    /// Too early to say anything (taps one and two, or a series that restarted).
    Nothing,
    /// Taps left until the page unlocks: 2, then 1.
    Countdown(u8),
    /// This tap unlocked the page.
    Unlocked,
    /// The page was unlocked before this tap.
    AlreadyUnlocked,
}

/// The current series of taps: how many, and when the last one came.
#[derive(Debug, Default)]
struct Series {
    count: u8,
    last_ms: Option<u64>,
}

/// The unlock state of one session: locked at start, never saved.
#[derive(Debug, Default)]
pub struct Unlock {
    unlocked: AtomicBool,
    series: Mutex<Series>,
}

impl Unlock {
    /// A locked counter.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Counts one tap made at `now_ms` (any monotonic clock, in milliseconds).
    ///
    /// Five taps, each within two seconds of the one before, unlock. A longer gap starts the
    /// series again with this tap as its first.
    pub fn tap(&self, now_ms: u64) -> TapOutcome {
        if self.unlocked() {
            return TapOutcome::AlreadyUnlocked;
        }
        let mut series = self.series.lock().unwrap_or_else(PoisonError::into_inner);
        let within = series
            .last_ms
            .is_some_and(|last| now_ms.saturating_sub(last) <= TAP_WINDOW_MS);
        series.count = if within {
            series.count.saturating_add(1)
        } else {
            1
        };
        series.last_ms = Some(now_ms);
        let count = series.count;
        drop(series);
        if count >= TAPS_TO_UNLOCK {
            self.unlocked.store(true, Ordering::Relaxed);
            return TapOutcome::Unlocked;
        }
        // The countdown starts with the third tap: "2 taps to developer options".
        if count >= 3 {
            TapOutcome::Countdown(TAPS_TO_UNLOCK - count)
        } else {
            TapOutcome::Nothing
        }
    }

    /// Whether the page is unlocked.
    #[must_use]
    pub fn unlocked(&self) -> bool {
        self.unlocked.load(Ordering::Relaxed)
    }

    /// Unlocks without taps (the preview's `--developer` flag).
    pub fn force(&self) {
        self.unlocked.store(true, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_new_counter_is_locked() {
        assert!(!Unlock::new().unlocked());
    }

    #[test]
    fn four_taps_do_nothing_and_the_fifth_unlocks() {
        let unlock = Unlock::new();
        assert_eq!(unlock.tap(0), TapOutcome::Nothing);
        assert_eq!(unlock.tap(500), TapOutcome::Nothing);
        assert_eq!(unlock.tap(1000), TapOutcome::Countdown(2));
        assert_eq!(unlock.tap(1500), TapOutcome::Countdown(1));
        assert!(!unlock.unlocked());
        assert_eq!(unlock.tap(2000), TapOutcome::Unlocked);
        assert!(unlock.unlocked());
    }

    #[test]
    fn a_tap_exactly_two_seconds_after_the_last_still_counts() {
        let unlock = Unlock::new();
        for step in 0..4 {
            let _ = unlock.tap(step * 2000);
        }
        assert_eq!(unlock.tap(8000), TapOutcome::Unlocked);
    }

    #[test]
    fn a_three_second_gap_restarts_the_count() {
        let unlock = Unlock::new();
        let _ = unlock.tap(0);
        let _ = unlock.tap(100);
        let _ = unlock.tap(200);
        let _ = unlock.tap(300);
        assert_eq!(unlock.tap(3300), TapOutcome::Nothing);
        assert_eq!(unlock.tap(3400), TapOutcome::Nothing);
        assert_eq!(unlock.tap(3500), TapOutcome::Countdown(2));
        assert!(!unlock.unlocked());
    }

    #[test]
    fn a_sixth_tap_reports_that_the_page_is_already_unlocked() {
        let unlock = Unlock::new();
        for step in 0..5 {
            let _ = unlock.tap(step * 100);
        }
        assert_eq!(unlock.tap(600), TapOutcome::AlreadyUnlocked);
    }

    #[test]
    fn force_unlocks_without_taps() {
        let unlock = Unlock::new();
        unlock.force();
        assert!(unlock.unlocked());
        assert_eq!(unlock.tap(0), TapOutcome::AlreadyUnlocked);
    }
}
