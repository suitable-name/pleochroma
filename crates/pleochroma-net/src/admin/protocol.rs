//! The remote administration messages: [`AdminRequest`] (administrator to coordinator) and
//! [`AdminPush`] (coordinator to administrator).
//!
//! # Framing
//!
//! Administration uses the crate's ordinary length-prefixed `postcard` frames
//! ([`crate::messages::write_message`] and [`crate::messages::read_control_message`]) on a
//! viewer-port connection whose HELLO role is `Monitor` or `Admin`. Like the library and
//! tilt-curve messages, each message is one `postcard` enum, so the variant index is the
//! message tag and the order of the variants below is wire-load-bearing: append only. There
//! is no shared tag byte to collide with, because after the handshake an administration
//! connection speaks only these two enums (administrator to coordinator: [`AdminRequest`];
//! coordinator to administrator: [`AdminPush`]).
//!
//! # Conversation
//!
//! Every request carries a `req_id` chosen by the administrator. The coordinator answers
//! each with exactly one [`AdminPush::Response`] naming that `req_id`, except
//! [`AdminRequest::FetchResult`], which is answered by a run of
//! [`AdminReply::ResultChunk`] responses (same `req_id`) ending with `last == true`.
//! [`AdminRequest::Subscribe`] starts the push stream: [`AdminPush::Status`] at most once a
//! second and only when something changed, [`AdminPush::Event`], [`AdminPush::Log`] at the
//! subscribed level, and a [`AdminPush::Heartbeat`] every [`HEARTBEAT_INTERVAL`].

use super::status::{
    CoordinatorEvent, CoordinatorStatus, IssuedToken, JobId, LogLevel, LogLine, SettingsPatch,
    TokenId, WorkerId,
};
use crate::messages::{PeerRole, error_codes};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// How often the coordinator sends [`AdminPush::Heartbeat`] on a subscribed administration
/// connection, so both sides notice a dead link long before TCP does.
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(15);

/// How long an administrator waits without any push (status, event, log or heartbeat)
/// before it treats the connection as lost: three missed heartbeats.
pub const HEARTBEAT_TIMEOUT: Duration = Duration::from_secs(45);

/// The least time between two [`AdminPush::Status`] pushes, in milliseconds. A
/// [`AdminRequest::Subscribe`] asking for less is raised to this.
pub const MIN_STATUS_INTERVAL_MS: u32 = 1000;

/// The most bytes of a job picture in one [`AdminReply::ResultChunk`]. Well under
/// [`crate::framing::MAX_CONTROL_FRAME_LEN`], so a chunk is always a legal control frame.
pub const RESULT_CHUNK_LEN: usize = 256 * 1024;

/// Whether the coordinator's idle close applies to an administration connection.
///
/// The coordinator closes a control connection that has been silent for five minutes. A
/// connection that has sent [`AdminRequest::Subscribe`] is exempt: the push stream and its
/// [`HEARTBEAT_INTERVAL`] heartbeat keep it alive (and prove it is). An administration
/// connection that never subscribed is closed like any other idle control connection.
#[must_use]
pub const fn idle_close_applies(subscribed: bool) -> bool {
    !subscribed
}

/// One request from a remote administrator. Every variant carries the `req_id` its
/// [`AdminPush::Response`] will name.
///
/// Variant order is wire-load-bearing: append only.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AdminRequest {
    /// Start the push stream.
    Subscribe {
        /// Names the response.
        req_id: u64,
        /// Minimum milliseconds between [`AdminPush::Status`] pushes; at least
        /// [`MIN_STATUS_INTERVAL_MS`].
        status_ms: u32,
        /// The least severe log level to push, or `None` for no log lines.
        level: Option<LogLevel>,
    },
    /// Ask for one status snapshot now.
    Status {
        /// Names the response.
        req_id: u64,
    },
    /// Make a pairing code for a device or a render computer. Monitor and administrator
    /// codes are refused (local only).
    IssueToken {
        /// Names the response.
        req_id: u64,
        /// The role the code is for.
        role: PeerRole,
        /// The default name the new device gets.
        name: String,
    },
    /// Cancel an unclaimed pairing code.
    CancelToken {
        /// Names the response.
        req_id: u64,
        /// The code to cancel.
        id: TokenId,
    },
    /// Remove a device's access.
    Revoke {
        /// Names the response.
        req_id: u64,
        /// The certificate fingerprint, 64 lowercase hex characters.
        fingerprint: String,
    },
    /// Rename a device.
    Rename {
        /// Names the response.
        req_id: u64,
        /// The certificate fingerprint, 64 lowercase hex characters.
        fingerprint: String,
        /// The new name.
        name: String,
    },
    /// Cancel a job.
    CancelJob {
        /// Names the response.
        req_id: u64,
        /// The job.
        id: JobId,
    },
    /// Delete a job and its result.
    DeleteJob {
        /// Names the response.
        req_id: u64,
        /// The job.
        id: JobId,
    },
    /// Disconnect a joined render computer.
    DisconnectWorker {
        /// Names the response.
        req_id: u64,
        /// The render computer's registry id.
        id: WorkerId,
    },
    /// Change the safe settings.
    ApplySettings {
        /// Names the response.
        req_id: u64,
        /// What to change.
        patch: SettingsPatch,
    },
    /// Restart the coordinator.
    Restart {
        /// Names the response.
        req_id: u64,
    },
    /// Stream a finished job's picture (PNG) back as [`AdminReply::ResultChunk`]s.
    FetchResult {
        /// Names the responses.
        req_id: u64,
        /// The job.
        job: JobId,
    },
}

impl AdminRequest {
    /// The id its response will name.
    #[must_use]
    pub const fn req_id(&self) -> u64 {
        match self {
            Self::Subscribe { req_id, .. }
            | Self::Status { req_id }
            | Self::IssueToken { req_id, .. }
            | Self::CancelToken { req_id, .. }
            | Self::Revoke { req_id, .. }
            | Self::Rename { req_id, .. }
            | Self::CancelJob { req_id, .. }
            | Self::DeleteJob { req_id, .. }
            | Self::DisconnectWorker { req_id, .. }
            | Self::ApplySettings { req_id, .. }
            | Self::Restart { req_id }
            | Self::FetchResult { req_id, .. } => *req_id,
        }
    }

    /// The action in plain words, as the audit line and the denial message name it
    /// ("revoke", "cancel job", "apply settings", ...).
    #[must_use]
    pub const fn action(&self) -> &'static str {
        match self {
            Self::Subscribe { .. } => "subscribe",
            Self::Status { .. } => "read status",
            Self::IssueToken { .. } => "make pairing code",
            Self::CancelToken { .. } => "cancel pairing code",
            Self::Revoke { .. } => "remove access",
            Self::Rename { .. } => "rename device",
            Self::CancelJob { .. } => "cancel job",
            Self::DeleteJob { .. } => "delete job",
            Self::DisconnectWorker { .. } => "disconnect render computer",
            Self::ApplySettings { .. } => "apply settings",
            Self::Restart { .. } => "restart",
            Self::FetchResult { .. } => "fetch picture",
        }
    }

    /// True for the requests that change something (everything except `Subscribe`,
    /// `Status` and `FetchResult`). Only these are audited as administrator actions.
    #[must_use]
    pub const fn is_command(&self) -> bool {
        !matches!(
            self,
            Self::Subscribe { .. } | Self::Status { .. } | Self::FetchResult { .. }
        )
    }
}

/// A request failed. `code` is one of [`crate::messages::error_codes`] (a refusal by the
/// permission table uses `PERMISSION_DENIED`); `message` is plain words the GUI shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdminError {
    /// One of [`crate::messages::error_codes`].
    pub code: u32,
    /// Plain words for the person.
    pub message: String,
}

impl AdminError {
    /// An error with a code and a message.
    #[must_use]
    pub fn new(code: u32, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    /// A `PERMISSION_DENIED` error with the reason the GUI shows.
    #[must_use]
    pub fn denied(reason: impl Into<String>) -> Self {
        Self::new(error_codes::PERMISSION_DENIED, reason)
    }

    /// A `VALIDATION_FAILED` error (a setting or name out of range).
    #[must_use]
    pub fn invalid(reason: impl Into<String>) -> Self {
        Self::new(error_codes::VALIDATION_FAILED, reason)
    }

    /// True if the permission table refused the request.
    #[must_use]
    pub const fn is_denied(&self) -> bool {
        self.code == error_codes::PERMISSION_DENIED
    }
}

impl std::fmt::Display for AdminError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for AdminError {}

/// The success payload of an [`AdminPush::Response`].
///
/// Variant order is wire-load-bearing: append only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::large_enum_variant)] // `Status` mirrors the push; replies are rare and short-lived
pub enum AdminReply {
    /// Done, nothing to return (`Subscribe`, `CancelToken`, `Revoke`, `Rename`, `CancelJob`,
    /// `DeleteJob`, `DisconnectWorker`, `ApplySettings`, `Restart`).
    Done,
    /// The new pairing code (the answer to `IssueToken`). Secret: handle with care.
    Token(IssuedToken),
    /// A status snapshot (the answer to `Status`).
    Status(CoordinatorStatus),
    /// One piece of a job picture (the answer to `FetchResult`). Pieces arrive in order;
    /// the one with `last == true` ends the transfer.
    ResultChunk {
        /// The job the picture belongs to.
        job: JobId,
        /// Position of this piece, from 0.
        index: u32,
        /// The picture's total size in bytes.
        total_len: u64,
        /// At most [`RESULT_CHUNK_LEN`] bytes of the PNG.
        data: Vec<u8>,
        /// True for the final piece.
        last: bool,
    },
}

/// One message from the coordinator to a remote administrator.
///
/// Variant order is wire-load-bearing: append only.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[allow(clippy::large_enum_variant)] // `Status` is the contract's shape; one lives at a time
pub enum AdminPush {
    /// A status snapshot: at most once a second, and only when it changed.
    Status(CoordinatorStatus),
    /// Something happened.
    Event(CoordinatorEvent),
    /// A log line at or above the subscribed level.
    Log(LogLine),
    /// Sent every [`HEARTBEAT_INTERVAL`] on a subscribed connection.
    Heartbeat,
    /// The answer to the request named by `req_id`.
    Response {
        /// The `req_id` of the request.
        req_id: u64,
        /// What came back.
        result: Result<AdminReply, AdminError>,
    },
}

impl AdminPush {
    /// A successful response.
    #[must_use]
    pub const fn ok(req_id: u64, reply: AdminReply) -> Self {
        Self::Response {
            req_id,
            result: Ok(reply),
        }
    }

    /// A failed response.
    #[must_use]
    pub const fn err(req_id: u64, error: AdminError) -> Self {
        Self::Response {
            req_id,
            result: Err(error),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        admin::status::{CoordinatorState, RenderMode},
        messages::{read_control_message, write_message},
    };
    use std::time::SystemTime;

    fn job() -> JobId {
        JobId([9; 16])
    }

    /// One of every request variant.
    fn every_request() -> Vec<AdminRequest> {
        vec![
            AdminRequest::Subscribe {
                req_id: 1,
                status_ms: 1000,
                level: Some(LogLevel::Info),
            },
            AdminRequest::Subscribe {
                req_id: 2,
                status_ms: 2500,
                level: None,
            },
            AdminRequest::Status { req_id: 3 },
            AdminRequest::IssueToken {
                req_id: 4,
                role: PeerRole::Worker,
                name: "Attic PC".to_string(),
            },
            AdminRequest::CancelToken {
                req_id: 5,
                id: TokenId(7),
            },
            AdminRequest::Revoke {
                req_id: 6,
                fingerprint: "ab".repeat(32),
            },
            AdminRequest::Rename {
                req_id: 7,
                fingerprint: "cd".repeat(32),
                name: "Phone".to_string(),
            },
            AdminRequest::CancelJob {
                req_id: 8,
                id: job(),
            },
            AdminRequest::DeleteJob {
                req_id: 9,
                id: job(),
            },
            AdminRequest::DisconnectWorker {
                req_id: 10,
                id: WorkerId(4),
            },
            AdminRequest::ApplySettings {
                req_id: 11,
                patch: SettingsPatch {
                    job_retention_days: Some(14),
                    render_mode: Some(RenderMode::CpuOnly),
                    display_name: Some("Studio PC".to_string()),
                    ..SettingsPatch::default()
                },
            },
            AdminRequest::Restart { req_id: 12 },
            AdminRequest::FetchResult {
                req_id: 13,
                job: job(),
            },
        ]
    }

    fn every_push() -> Vec<AdminPush> {
        let status =
            CoordinatorStatus::minimal("Studio PC", "0.12.0", 45, CoordinatorState::Running);
        vec![
            AdminPush::Status(status.clone()),
            AdminPush::Event(CoordinatorEvent::AdminAction {
                who: "Laptop (ab12cd34)".to_string(),
                action: "revoke".to_string(),
                target: "S23".to_string(),
            }),
            AdminPush::Log(LogLine {
                time: 5,
                level: LogLevel::Warn,
                target: "serve".to_string(),
                message: "hello".to_string(),
            }),
            AdminPush::Heartbeat,
            AdminPush::ok(1, AdminReply::Done),
            AdminPush::ok(
                4,
                AdminReply::Token(IssuedToken {
                    id: TokenId(3),
                    token: "TOKEN".to_string(),
                    link: "pleochroma://pair?t=TOKEN".to_string(),
                    expires_at: SystemTime::UNIX_EPOCH + Duration::from_secs(600),
                }),
            ),
            AdminPush::ok(3, AdminReply::Status(status)),
            AdminPush::ok(
                13,
                AdminReply::ResultChunk {
                    job: job(),
                    index: 2,
                    total_len: 900_000,
                    data: vec![1, 2, 3, 4],
                    last: true,
                },
            ),
            AdminPush::err(6, AdminError::denied("You have monitor access.")),
            AdminPush::err(
                11,
                AdminError::invalid("Job memory must be between 64 and 1048576."),
            ),
        ]
    }

    #[test]
    fn every_request_round_trips_through_postcard() {
        for request in every_request() {
            let bytes = postcard::to_allocvec(&request).unwrap();
            let back: AdminRequest = postcard::from_bytes(&bytes).unwrap();
            assert_eq!(back, request);
        }
    }

    #[test]
    fn every_request_round_trips_through_a_frame() {
        for request in every_request() {
            let mut wire = Vec::new();
            write_message(&mut wire, &request).unwrap();
            let back: AdminRequest = read_control_message(&mut wire.as_slice()).unwrap();
            assert_eq!(back, request);
        }
    }

    #[test]
    fn every_push_round_trips_through_postcard_and_a_frame() {
        for push in every_push() {
            let bytes = postcard::to_allocvec(&push).unwrap();
            assert_eq!(postcard::from_bytes::<AdminPush>(&bytes).unwrap(), push);
            let mut wire = Vec::new();
            write_message(&mut wire, &push).unwrap();
            let back: AdminPush = read_control_message(&mut wire.as_slice()).unwrap();
            assert_eq!(back, push);
        }
    }

    #[test]
    fn requests_report_their_id_and_action() {
        let requests = every_request();
        let ids: Vec<u64> = requests.iter().map(AdminRequest::req_id).collect();
        assert_eq!(ids, (1..=13).collect::<Vec<u64>>());
        for request in &requests {
            assert!(!request.action().is_empty());
        }
        assert!(!requests[0].is_command());
        assert!(!requests[2].is_command());
        assert!(requests[3].is_command());
        assert!(requests[11].is_command());
        assert!(!requests[12].is_command());
    }

    #[test]
    fn a_full_chunk_fits_a_control_frame() {
        let push = AdminPush::ok(
            1,
            AdminReply::ResultChunk {
                job: job(),
                index: 0,
                total_len: u64::MAX,
                data: vec![0xAA; RESULT_CHUNK_LEN],
                last: false,
            },
        );
        let mut wire = Vec::new();
        write_message(&mut wire, &push).unwrap();
        assert!(wire.len() < crate::framing::MAX_CONTROL_FRAME_LEN as usize);
        assert_eq!(
            read_control_message::<_, AdminPush>(&mut wire.as_slice()).unwrap(),
            push
        );
    }

    #[test]
    fn errors_carry_the_documented_codes() {
        let denied = AdminError::denied("no");
        assert_eq!(denied.code, error_codes::PERMISSION_DENIED);
        assert!(denied.is_denied());
        assert_eq!(denied.to_string(), "no");
        let invalid = AdminError::invalid("bad");
        assert_eq!(invalid.code, error_codes::VALIDATION_FAILED);
        assert!(!invalid.is_denied());
    }

    #[test]
    fn heartbeat_constants_and_idle_exemption() {
        assert_eq!(HEARTBEAT_INTERVAL, Duration::from_secs(15));
        assert!(HEARTBEAT_TIMEOUT > HEARTBEAT_INTERVAL * 2);
        assert!(!idle_close_applies(true));
        assert!(idle_close_applies(false));
    }

    #[test]
    fn a_token_reply_does_not_leak_through_debug() {
        let push = AdminPush::ok(
            4,
            AdminReply::Token(IssuedToken {
                id: TokenId(3),
                token: "SECRET-TOKEN".to_string(),
                link: "pleochroma://pair?t=SECRET-TOKEN".to_string(),
                expires_at: SystemTime::UNIX_EPOCH,
            }),
        );
        assert!(!format!("{push:?}").contains("SECRET"));
    }
}
