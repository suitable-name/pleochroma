//! Owned optical requests and nonblocking completion; no caller thread waits here.
use super::{OpticalError, ScoredOptics};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
    mpsc::{Receiver, TryRecvError},
};

/// One owned request, including its bounded one-result mailbox.
///
/// The scheduler retains this value on backpressure and retries `try_submit` later.
/// Dropping it cancels queued or active work. Completion order never implies commit order.
pub struct OpticalWork {
    #[cfg(feature = "cuda")]
    pub(super) request: Option<super::worker::Candidate>,
    receive: Receiver<Result<ScoredOptics, OpticalError>>,
    cancel: Arc<AtomicBool>,
    failure: Arc<Mutex<Option<String>>>,
    result: Option<Result<ScoredOptics, OpticalError>>,
}

impl OpticalWork {
    #[cfg(any(feature = "cuda", test))]
    pub(super) fn new(
        receive: Receiver<Result<ScoredOptics, OpticalError>>,
        cancel: Arc<AtomicBool>,
        failure: Arc<Mutex<Option<String>>>,
    ) -> Self {
        Self {
            #[cfg(feature = "cuda")]
            request: None,
            receive,
            cancel,
            failure,
            result: None,
        }
    }

    /// Check completion without sleeping or waiting for the CUDA owner.
    pub fn poll(&mut self) -> bool {
        if self.result.is_some() {
            return true;
        }
        if self.cancel.load(Ordering::Relaxed) {
            self.result = Some(Err(OpticalError::Cancelled));
            return true;
        }
        match self.receive.try_recv() {
            Ok(result) => self.result = Some(result),
            Err(TryRecvError::Empty) => return false,
            Err(TryRecvError::Disconnected) => {
                self.result = Some(Err(OpticalError::Runtime(
                    self.failure
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .clone()
                        .unwrap_or_else(|| {
                            "CUDA optical worker disconnected before completion".into()
                        }),
                )));
            }
        }
        true
    }

    /// Cancel this request explicitly; dropping it has the same effect.
    pub fn cancel(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// Consume a completed result. A premature completion is an explicit runtime error.
    ///
    /// # Errors
    /// Returns cancellation, service failure, or premature-completion errors.
    pub fn complete(mut self) -> Result<ScoredOptics, OpticalError> {
        self.poll();
        self.result
            .take()
            .unwrap_or_else(|| Err(OpticalError::Runtime("optical request is not ready".into())))
    }
}

impl Drop for OpticalWork {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

/// The same ownership-preserving admission primitive is exercised by the fake service.
#[cfg(any(feature = "cuda", test))]
pub(super) fn try_admit<T>(
    sender: &std::sync::mpsc::SyncSender<T>,
    request: &mut Option<T>,
) -> Result<bool, std::sync::mpsc::SendError<T>> {
    use std::sync::mpsc::{SendError, TrySendError};
    let Some(candidate) = request.take() else {
        return Ok(true);
    };
    match sender.try_send(candidate) {
        Ok(()) => Ok(true),
        Err(TrySendError::Full(candidate)) => {
            *request = Some(candidate);
            Ok(false)
        }
        Err(TrySendError::Disconnected(candidate)) => Err(SendError(candidate)),
    }
}

#[cfg(test)]
#[path = "submission_tests.rs"]
mod tests;
