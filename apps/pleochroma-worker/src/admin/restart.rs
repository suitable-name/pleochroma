//! Restarting the coordinator on a remote administrator's request.
//!
//! `serve` itself cannot restart: stopping the coordinator and starting it again with the
//! saved settings is the job of whoever owns it (the coordinator window's local backend).
//! That owner registers a hook with [`set_hook`]; a remote `Restart` request calls it after a
//! short delay, so the "done" answer reaches the administrator before the connection ends.
//! A coordinator with no owner (a headless `serve`) has no hook and refuses in plain words.

use std::{
    sync::{Arc, PoisonError, RwLock},
    thread,
    time::Duration,
};

/// What the owner registers: restart the coordinator now, or say why not in plain words.
type Hook = Arc<dyn Fn() -> Result<(), String> + Send + Sync>;

/// The registered hook.
static HOOK: RwLock<Option<Hook>> = RwLock::new(None);

/// How long a restart waits after the request was accepted, so the answer and the audit
/// event are on their way first.
pub const DELAY: Duration = Duration::from_millis(400);

/// What a headless coordinator answers.
pub const NO_OWNER: &str = "This coordinator was started from the command line and cannot restart itself. \
     Restart it on its own computer.";

/// Registers the function that restarts the coordinator. A second call replaces the first.
/// The coordinator window's local backend calls this once it exists.
pub fn set_hook(hook: impl Fn() -> Result<(), String> + Send + Sync + 'static) {
    *HOOK.write().unwrap_or_else(PoisonError::into_inner) = Some(Arc::new(hook));
}

/// Removes the registered function (the coordinator window closed).
pub fn clear_hook() {
    *HOOK.write().unwrap_or_else(PoisonError::into_inner) = None;
}

/// Whether a restart is possible.
#[must_use]
pub fn available() -> bool {
    HOOK.read()
        .unwrap_or_else(PoisonError::into_inner)
        .is_some()
}

/// Asks for a restart after [`DELAY`] and returns at once.
///
/// # Errors
///
/// [`NO_OWNER`] when nobody registered a hook, or a plain sentence when the thread that waits
/// could not start.
pub fn schedule() -> Result<(), String> {
    schedule_after(DELAY)
}

/// [`schedule`] with a chosen delay (tests use none).
///
/// # Errors
///
/// As [`schedule`].
pub fn schedule_after(delay: Duration) -> Result<(), String> {
    let hook = HOOK
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
        .ok_or_else(|| NO_OWNER.to_owned())?;
    thread::Builder::new()
        .name("admin-restart".to_owned())
        .spawn(move || {
            thread::sleep(delay);
            tracing::info!("admin restart: restarting the coordinator");
            if let Err(error) = hook() {
                tracing::warn!("admin restart: the coordinator could not restart ({error})");
            }
        })
        .map(|_| ())
        .map_err(|error| format!("The coordinator could not restart: {error}"))
}

/// The hook is process-wide: every test that sets it or relies on it being unset takes this
/// lock first.
#[cfg(test)]
pub(crate) static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, mpsc};

    #[test]
    fn without_a_hook_a_restart_is_refused_and_with_one_it_runs_once() {
        let _guard = TEST_LOCK.lock().unwrap_or_else(PoisonError::into_inner);
        clear_hook();
        assert!(!available());
        assert_eq!(schedule_after(Duration::ZERO), Err(NO_OWNER.to_owned()));

        let (tx, rx) = mpsc::channel();
        let tx = Mutex::new(tx);
        set_hook(move || tx.lock().unwrap().send(()).map_err(|e| e.to_string()));
        assert!(available());
        schedule_after(Duration::ZERO).unwrap();
        rx.recv_timeout(Duration::from_secs(5))
            .expect("the hook ran");
        assert!(
            rx.recv_timeout(Duration::from_millis(100)).is_err(),
            "only once"
        );
        clear_hook();
    }
}
