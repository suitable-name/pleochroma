//! The one interface every coordinator page talks to.
//!
//! `LocalBackend` (the coordinator in this process) and `RemoteBackend` (a coordinator
//! managed over the administration protocol) both implement [`CoordinatorBackend`]. The GUI
//! holds the selected one as `Arc<dyn CoordinatorBackend>` and binds only to this trait, so
//! one UI serves both.
//!
//! Every method may be called from any thread and must not block for long: a remote
//! backend answers from its last pushed status and waits only for the commands' replies.
//! Errors are plain words for the user and never contain a token, a pairing link or a key.

pub use pleochroma_net::{
    admin::status::{
        AccessRole, Capabilities, CoordinatorEvent, CoordinatorState, CoordinatorStatus,
        DeviceStatus, IssuedToken, JobId, LogLevel, LogLine, SettingsPatch, TokenId, WorkerId,
    },
    messages::PeerRole,
    tls::Fingerprint,
};

/// What the coordinator pages can read and do.
pub trait CoordinatorBackend: Send + Sync {
    /// The latest snapshot of everything the pages show. Cheap; callable every second.
    fn status(&self) -> CoordinatorStatus;

    /// A new stream of events from now on. Each call returns its own receiver; a receiver
    /// that is dropped is forgotten by the sender.
    fn events(&self) -> std::sync::mpsc::Receiver<CoordinatorEvent>;

    /// The newest `limit` log lines at `min` severity or more severe, oldest first.
    fn logs(&self, min: tracing::Level, limit: usize) -> Vec<LogLine>;

    /// Makes a one-time pairing code for a Viewer or Worker (and, on the local backend only,
    /// a Monitor or Admin). `default_name` is the name the new device gets until renamed.
    ///
    /// # Errors
    ///
    /// Plain words when the role is not allowed for this caller, pairing is switched off,
    /// or the enrollment listener is not running.
    fn issue_token(&self, role: PeerRole, default_name: &str) -> Result<IssuedToken, String>;

    /// Cancels a pairing code that has not been used. Unknown ids are ignored.
    fn cancel_token(&self, id: TokenId);

    /// Removes a device's access and disconnects it at once.
    ///
    /// # Errors
    ///
    /// Plain words when the device is unknown, the caller may not, or the allowlist cannot
    /// be rewritten.
    fn revoke(&self, fp: &Fingerprint) -> Result<(), String>;

    /// Renames a device.
    ///
    /// # Errors
    ///
    /// Plain words when the device is unknown, the name is empty or too long, or the caller
    /// may not.
    fn rename(&self, fp: &Fingerprint, name: &str) -> Result<(), String>;

    /// Cancels a queued or running job (any owner).
    ///
    /// # Errors
    ///
    /// Plain words when the job is unknown, already finished, or the caller may not.
    fn cancel_job(&self, id: JobId) -> Result<(), String>;

    /// Deletes a job and its result (any owner).
    ///
    /// # Errors
    ///
    /// Plain words when the job is unknown or the caller may not.
    fn delete_job(&self, id: JobId) -> Result<(), String>;

    /// Disconnects a joined render computer. It may rejoin.
    ///
    /// # Errors
    ///
    /// Plain words when it is not joined or the caller may not.
    fn disconnect_worker(&self, id: WorkerId) -> Result<(), String>;

    /// Applies the set fields of `patch` after validating all of them; applies nothing when
    /// any field is invalid.
    ///
    /// # Errors
    ///
    /// Plain words naming the first invalid field, or that the caller may not.
    fn apply_settings(&self, patch: &SettingsPatch) -> Result<(), String>;

    /// Stops and starts the coordinator again with the same settings.
    ///
    /// # Errors
    ///
    /// Plain words when it cannot restart or the caller may not.
    fn restart(&self) -> Result<(), String>;

    /// What this caller may do; the pages grey out what is false.
    fn capabilities(&self) -> Capabilities;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::Arc, time::SystemTime};

    /// A backend that knows nothing; proves the trait is object safe and `Send + Sync`.
    struct Null;

    impl CoordinatorBackend for Null {
        fn status(&self) -> CoordinatorStatus {
            CoordinatorStatus::minimal("Test", "0", 45, CoordinatorState::Stopped)
        }
        fn events(&self) -> std::sync::mpsc::Receiver<CoordinatorEvent> {
            std::sync::mpsc::channel().1
        }
        fn logs(&self, _min: tracing::Level, _limit: usize) -> Vec<LogLine> {
            Vec::new()
        }
        fn issue_token(&self, _role: PeerRole, _default_name: &str) -> Result<IssuedToken, String> {
            Ok(IssuedToken {
                id: TokenId(1),
                token: "secret".to_string(),
                link: "link".to_string(),
                expires_at: SystemTime::UNIX_EPOCH,
            })
        }
        fn cancel_token(&self, _id: TokenId) {}
        fn revoke(&self, _fp: &Fingerprint) -> Result<(), String> {
            Ok(())
        }
        fn rename(&self, _fp: &Fingerprint, _name: &str) -> Result<(), String> {
            Ok(())
        }
        fn cancel_job(&self, _id: JobId) -> Result<(), String> {
            Ok(())
        }
        fn delete_job(&self, _id: JobId) -> Result<(), String> {
            Ok(())
        }
        fn disconnect_worker(&self, _id: WorkerId) -> Result<(), String> {
            Ok(())
        }
        fn apply_settings(&self, _patch: &SettingsPatch) -> Result<(), String> {
            Ok(())
        }
        fn restart(&self) -> Result<(), String> {
            Ok(())
        }
        fn capabilities(&self) -> Capabilities {
            Capabilities::monitor()
        }
    }

    #[test]
    fn trait_is_object_safe_and_shareable() {
        fn assert_send_sync<T: Send + Sync + ?Sized>() {}
        assert_send_sync::<dyn CoordinatorBackend>();
        let backend: Arc<dyn CoordinatorBackend> = Arc::new(Null);
        assert_eq!(backend.status().state, CoordinatorState::Stopped);
        assert!(backend.capabilities().is_read_only());
        assert!(backend.events().try_recv().is_err());
        let issued = backend.issue_token(PeerRole::Viewer, "Phone").unwrap();
        assert!(!format!("{issued:?}").contains("secret"));
    }
}
