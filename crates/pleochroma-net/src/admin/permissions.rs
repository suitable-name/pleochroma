//! Who may send which [`AdminRequest`]: the permission table.
//!
//! The coordinator calls [`allowed`] before it acts on any request from an administration
//! connection, and answers a refusal with `PERMISSION_DENIED` and the reason text. The GUI
//! greys out actions from [`super::status::Capabilities`]; the tests here pin the two to
//! each other, so a stale flag is harmless but never wrong.
//!
//! | Request | Monitor | Admin | Local |
//! |---|---|---|---|
//! | `Subscribe`, `Status`, `FetchResult` | yes | yes | yes |
//! | `IssueToken` for Viewer or Worker | no | yes | yes |
//! | `IssueToken` for Monitor or Admin | no | **no** | yes |
//! | every other request | no | yes | yes |
//!
//! # Never remote
//!
//! These exist only on the coordinator's own computer; no request carries them, and
//! [`never_remote_denial`] gives the refusal a server returns if a peer asks for one by
//! another route: network, port and bind changes, the remote-administration switch,
//! resetting pairings, rotating the issuing secret, exporting a backup, stopping the
//! coordinator, and making monitor or administrator codes.

use super::{
    protocol::{AdminError, AdminRequest},
    status::AccessRole,
};
use crate::messages::PeerRole;

/// An action that can only be done on the coordinator's own computer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NeverRemote {
    /// Changing ports, the bind address or who can connect.
    Network,
    /// Switching remote administration on or off.
    RemoteAdminSwitch,
    /// Resetting the certificate authority (all pairings).
    ResetPairings,
    /// Rotating the issuing secret.
    RotateSecret,
    /// Exporting a backup.
    ExportBackup,
    /// Stopping the coordinator.
    Stop,
    /// Making a monitor or administrator code.
    PrivilegedToken,
}

impl NeverRemote {
    /// Every never-remote action.
    pub const ALL: [Self; 7] = [
        Self::Network,
        Self::RemoteAdminSwitch,
        Self::ResetPairings,
        Self::RotateSecret,
        Self::ExportBackup,
        Self::Stop,
        Self::PrivilegedToken,
    ];

    /// The refusal text the GUI shows, in plain words.
    #[must_use]
    pub const fn reason(self) -> &'static str {
        match self {
            Self::Network => {
                "Network settings (ports and addresses) can only be changed on the \
                 coordinator's own computer."
            }
            Self::RemoteAdminSwitch => {
                "Remote administration can only be switched on or off on the \
                 coordinator's own computer."
            }
            Self::ResetPairings => {
                "Resetting all pairings can only be done on the coordinator's own computer."
            }
            Self::RotateSecret => {
                "Rotating the issuing secret can only be done on the coordinator's own computer."
            }
            Self::ExportBackup => {
                "A backup can only be exported on the coordinator's own computer."
            }
            Self::Stop => "The coordinator can only be stopped on its own computer.",
            Self::PrivilegedToken => {
                "Monitor and administrator codes can only be made on the coordinator's own computer."
            }
        }
    }
}

/// The `PERMISSION_DENIED` error for a never-remote action.
#[must_use]
pub fn never_remote_denial(action: NeverRemote) -> AdminError {
    AdminError::denied(action.reason())
}

/// The refusal for a monitor trying to change something.
fn monitor_denial(req: &AdminRequest) -> AdminError {
    AdminError::denied(format!(
        "You have monitor access, so you cannot {}. Ask for full administration to change this.",
        req.action()
    ))
}

/// Checks `req` against the permission table for a caller with `role`.
///
/// # Errors
///
/// `PERMISSION_DENIED` with a plain reason when the role may not send the request.
pub fn allowed(role: AccessRole, req: &AdminRequest) -> Result<(), AdminError> {
    match role {
        AccessRole::Local => Ok(()),
        AccessRole::Monitor => match req {
            AdminRequest::Subscribe { .. }
            | AdminRequest::Status { .. }
            | AdminRequest::FetchResult { .. } => Ok(()),
            AdminRequest::IssueToken { .. }
            | AdminRequest::CancelToken { .. }
            | AdminRequest::Revoke { .. }
            | AdminRequest::Rename { .. }
            | AdminRequest::CancelJob { .. }
            | AdminRequest::DeleteJob { .. }
            | AdminRequest::DisconnectWorker { .. }
            | AdminRequest::ApplySettings { .. }
            | AdminRequest::Restart { .. } => Err(monitor_denial(req)),
        },
        AccessRole::Admin => match req {
            AdminRequest::IssueToken {
                role: PeerRole::Monitor | PeerRole::Admin,
                ..
            } => Err(never_remote_denial(NeverRemote::PrivilegedToken)),
            AdminRequest::Subscribe { .. }
            | AdminRequest::Status { .. }
            | AdminRequest::IssueToken { .. }
            | AdminRequest::CancelToken { .. }
            | AdminRequest::Revoke { .. }
            | AdminRequest::Rename { .. }
            | AdminRequest::CancelJob { .. }
            | AdminRequest::DeleteJob { .. }
            | AdminRequest::DisconnectWorker { .. }
            | AdminRequest::ApplySettings { .. }
            | AdminRequest::Restart { .. }
            | AdminRequest::FetchResult { .. } => Ok(()),
        },
    }
}

/// The access role of a remote peer, from its HELLO role. `None` for Viewer and Worker,
/// which are not administration roles.
#[must_use]
pub const fn access_role_for(role: PeerRole) -> Option<AccessRole> {
    match role {
        PeerRole::Monitor => Some(AccessRole::Monitor),
        PeerRole::Admin => Some(AccessRole::Admin),
        PeerRole::Viewer | PeerRole::Worker => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::admin::status::{Capabilities, JobId, SettingsPatch, TokenId, WorkerId};

    fn job() -> JobId {
        JobId([1; 16])
    }

    /// Every request variant; `IssueToken` once per role.
    fn matrix() -> Vec<AdminRequest> {
        let mut all = vec![
            AdminRequest::Subscribe {
                req_id: 1,
                status_ms: 1000,
                level: None,
            },
            AdminRequest::Status { req_id: 2 },
            AdminRequest::CancelToken {
                req_id: 4,
                id: TokenId(1),
            },
            AdminRequest::Revoke {
                req_id: 5,
                fingerprint: "ab".repeat(32),
            },
            AdminRequest::Rename {
                req_id: 6,
                fingerprint: "ab".repeat(32),
                name: "x".to_string(),
            },
            AdminRequest::CancelJob {
                req_id: 7,
                id: job(),
            },
            AdminRequest::DeleteJob {
                req_id: 8,
                id: job(),
            },
            AdminRequest::DisconnectWorker {
                req_id: 9,
                id: WorkerId(1),
            },
            AdminRequest::ApplySettings {
                req_id: 10,
                patch: SettingsPatch::default(),
            },
            AdminRequest::Restart { req_id: 11 },
            AdminRequest::FetchResult {
                req_id: 12,
                job: job(),
            },
        ];
        for role in [
            PeerRole::Viewer,
            PeerRole::Worker,
            PeerRole::Monitor,
            PeerRole::Admin,
        ] {
            all.push(AdminRequest::IssueToken {
                req_id: 3,
                role,
                name: "n".to_string(),
            });
        }
        all
    }

    fn is_read(req: &AdminRequest) -> bool {
        matches!(
            req,
            AdminRequest::Subscribe { .. }
                | AdminRequest::Status { .. }
                | AdminRequest::FetchResult { .. }
        )
    }

    fn is_privileged_token(req: &AdminRequest) -> bool {
        matches!(
            req,
            AdminRequest::IssueToken {
                role: PeerRole::Monitor | PeerRole::Admin,
                ..
            }
        )
    }

    #[test]
    fn local_may_do_everything() {
        for req in matrix() {
            assert_eq!(allowed(AccessRole::Local, &req), Ok(()), "{req:?}");
        }
    }

    #[test]
    fn monitor_may_only_read() {
        for req in matrix() {
            let result = allowed(AccessRole::Monitor, &req);
            if is_read(&req) {
                assert_eq!(result, Ok(()), "{req:?}");
            } else {
                let err = result.expect_err("a monitor cannot change anything");
                assert!(err.is_denied(), "{req:?}");
                assert!(err.message.contains("monitor access"), "{req:?}");
                assert!(err.message.contains(req.action()), "{req:?}");
            }
        }
    }

    #[test]
    fn admin_may_do_everything_but_privileged_tokens() {
        for req in matrix() {
            let result = allowed(AccessRole::Admin, &req);
            if is_privileged_token(&req) {
                let err = result.expect_err("a remote admin cannot make admin codes");
                assert!(err.is_denied(), "{req:?}");
                assert_eq!(err.message, NeverRemote::PrivilegedToken.reason());
            } else {
                assert_eq!(result, Ok(()), "{req:?}");
            }
        }
    }

    #[test]
    fn full_matrix_counts() {
        let all = matrix();
        assert_eq!(all.len(), 15);
        let ok = |role| all.iter().filter(|r| allowed(role, r).is_ok()).count();
        assert_eq!(ok(AccessRole::Local), 15);
        assert_eq!(ok(AccessRole::Admin), 13);
        assert_eq!(ok(AccessRole::Monitor), 3);
    }

    #[test]
    fn the_table_agrees_with_the_capability_flags() {
        for (role, caps) in [
            (AccessRole::Local, Capabilities::local()),
            (AccessRole::Admin, Capabilities::admin()),
            (AccessRole::Monitor, Capabilities::monitor()),
        ] {
            for req in matrix() {
                let flag = match &req {
                    AdminRequest::Subscribe { .. } | AdminRequest::Status { .. } => caps.view,
                    AdminRequest::FetchResult { .. } => caps.fetch_results,
                    AdminRequest::IssueToken { role, .. } => caps.can_issue(*role),
                    AdminRequest::CancelToken { .. } => caps.cancel_tokens,
                    AdminRequest::Revoke { .. } => caps.revoke,
                    AdminRequest::Rename { .. } => caps.rename,
                    AdminRequest::CancelJob { .. } => caps.cancel_jobs,
                    AdminRequest::DeleteJob { .. } => caps.delete_jobs,
                    AdminRequest::DisconnectWorker { .. } => caps.disconnect_workers,
                    AdminRequest::ApplySettings { .. } => caps.apply_settings,
                    AdminRequest::Restart { .. } => caps.restart,
                };
                assert_eq!(allowed(role, &req).is_ok(), flag, "{role:?} {req:?}");
            }
        }
    }

    #[test]
    fn never_remote_actions_are_refused_with_a_reason() {
        let mut reasons = Vec::new();
        for action in NeverRemote::ALL {
            let err = never_remote_denial(action);
            assert!(err.is_denied());
            assert!(err.message.contains("own computer"), "{action:?}");
            assert!(
                !reasons.contains(&err.message),
                "{action:?} repeats a reason"
            );
            reasons.push(err.message);
        }
        assert_eq!(reasons.len(), 7);
    }

    #[test]
    fn privileged_tokens_are_refused_for_every_remote_role() {
        for role in [PeerRole::Monitor, PeerRole::Admin] {
            let req = AdminRequest::IssueToken {
                req_id: 1,
                role,
                name: "Laptop".to_string(),
            };
            for who in [AccessRole::Admin, AccessRole::Monitor] {
                assert!(allowed(who, &req).is_err(), "{who:?} {role:?}");
            }
            assert!(allowed(AccessRole::Local, &req).is_ok());
        }
    }

    #[test]
    fn hello_roles_map_to_access_roles() {
        assert_eq!(
            access_role_for(PeerRole::Monitor),
            Some(AccessRole::Monitor)
        );
        assert_eq!(access_role_for(PeerRole::Admin), Some(AccessRole::Admin));
        assert_eq!(access_role_for(PeerRole::Viewer), None);
        assert_eq!(access_role_for(PeerRole::Worker), None);
    }
}
