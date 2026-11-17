//! The `HELLO` gate shared by both build modes and both listener ports.
//!
//! Reads the peer's `HELLO` version first (so an older or newer peer gets a refusal
//! naming both protocol versions instead of a decode error), then refuses any role this
//! PORT does not serve (viewers on the viewer port, joining workers on the worker
//! port), checking both the role the `HELLO` claims and the role of the
//! client certificate the peer authenticated with (`crate::pki::role`). Every mismatch
//! is refused with `error_codes::ROLE_REFUSED` and a message naming the mix-up.

use crate::pki::role::role_name;
use pleochroma_net::{
    handshake::{self, HelloReadError},
    messages::{ErrorMsg, Hello, NetError, PeerRole, error_codes},
};
use std::io::{Read, Write};

/// The refusal text for a Monitor or Admin `HELLO` while no port serves remote
/// administration.
const REMOTE_ADMIN_REFUSAL: &str = "refusing to pair: remote administration is not available";

/// What [`read_and_check_hello`] decided about the peer's `HELLO`.
pub enum HelloCheck {
    /// A same-version `HELLO` of the port's role: continue with the build check and
    /// `WELCOME`.
    Accepted(Hello),
    /// Refuse the pairing: send this `ErrorMsg` in place of `WELCOME` (see
    /// [`send_refusal`]) and end the connection.
    Refused(ErrorMsg),
}

/// Reads the peer's `HELLO` and classifies it for a listener serving `port`.
///
/// A different protocol version, or a role (claimed or certified) this port does not serve, becomes
/// [`HelloCheck::Refused`] with a message naming what both sides have; everything else is
/// [`HelloCheck::Accepted`] for the caller's own build-compatibility gate.
///
/// `cert_role` is the role of the peer's client certificate (`None` only for
/// `--insecure-no-tls`, which has no certificate to check).
///
/// # Errors
///
/// [`NetError`] if the frame cannot be read, or a same-version `HELLO` does not decode.
pub fn read_and_check_hello<S: Read>(
    stream: &mut S,
    port: PeerRole,
    cert_role: Option<PeerRole>,
) -> Result<HelloCheck, NetError> {
    read_hello_then(stream, |hello| check_role(hello, port, cert_role))
}

/// [`read_and_check_hello`] for the VIEWER port when remote administration may be on: a Monitor or
/// Admin `HELLO` is judged by [`check_role_with_admin`] with `policy`.
///
/// Everything else is judged exactly as [`read_and_check_hello`] does.
///
/// # Errors
///
/// See [`read_and_check_hello`].
pub fn read_and_check_hello_admin<S: Read>(
    stream: &mut S,
    cert_role: Option<PeerRole>,
    policy: AdminPolicy,
) -> Result<HelloCheck, NetError> {
    read_hello_then(stream, |hello| {
        check_role_with_admin(hello, PeerRole::Viewer, cert_role, policy)
    })
}

/// Reads the peer's `HELLO` under the total deadline and hands it to `decide`; a version
/// mismatch is answered with a refusal before `decide` is asked.
fn read_hello_then<S: Read>(
    stream: &mut S,
    decide: impl FnOnce(Hello) -> HelloCheck,
) -> Result<HelloCheck, NetError> {
    // A total deadline for the whole read, on top of the per-read idle timeout: a peer that
    // trickles one byte at a time inside that window must still be cut off (see
    // `DeadlineIo`). It needs only `S: Read`, so a `TcpStream`, a TLS stream or a test
    // cursor all work.
    let deadline = std::time::Instant::now() + crate::serve::HANDSHAKE_TIMEOUT;
    let mut guarded = crate::serve::socket::DeadlineIo::new(stream, deadline);
    let hello = match handshake::read_hello(&mut guarded) {
        Ok(hello) => hello,
        Err(HelloReadError::Net(e)) => return Err(e),
        Err(HelloReadError::Incompatible(incompatible)) => {
            return Ok(HelloCheck::Refused(ErrorMsg {
                code: error_codes::BUILD_MISMATCH,
                message: format!("refusing to pair: {incompatible}"),
                // No request exists yet at the HELLO phase.
                request_id: None,
            }));
        }
    };
    Ok(decide(hello))
}

/// What the viewer port knows about a remote administration peer before it reads its `HELLO`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AdminPolicy {
    /// Remote administration is switched on (`--allow-remote-admin`, with TLS).
    pub enabled: bool,
    /// The peer certificate's fingerprint is on the allowlist of its role
    /// (`allowlist-monitors.txt` or `allowlist-admins.txt`).
    pub listed: bool,
}

/// The role half of [`read_and_check_hello_admin`].
///
/// [`check_role`], except that on the viewer port a consistent Monitor or Admin `HELLO` passes when
/// remote administration is on, the certificate is of the same role, and its fingerprint is on that
/// role's allowlist.
///
/// Every other case, including every case with remote administration off and every case on
/// the worker port, is exactly [`check_role`]'s answer.
#[must_use]
pub fn check_role_with_admin(
    hello: Hello,
    port: PeerRole,
    cert_role: Option<PeerRole>,
    policy: AdminPolicy,
) -> HelloCheck {
    let admin_hello = matches!(hello.role, PeerRole::Monitor | PeerRole::Admin);
    if port != PeerRole::Viewer || !admin_hello || !policy.enabled || !hello.role_is_consistent() {
        return check_role(hello, port, cert_role);
    }
    let refuse = |message: String| {
        HelloCheck::Refused(ErrorMsg {
            code: error_codes::ROLE_REFUSED,
            message,
            request_id: None,
        })
    };
    if cert_role != Some(hello.role) {
        return refuse(format!(
            "refusing to pair: this {} connection needs {}, not {} -- pair this device with {} \
             from the coordinator",
            role_name(hello.role),
            certificate_phrase(Some(hello.role)),
            certificate_phrase(cert_role),
            if hello.role == PeerRole::Admin {
                "an admin code"
            } else {
                "a monitor code"
            }
        ));
    }
    if !policy.listed {
        return refuse(format!(
            "refusing to pair: this {} certificate is not on the coordinator's list of trusted \
             {}s (it may have been removed)",
            role_name(hello.role),
            role_name(hello.role)
        ));
    }
    HelloCheck::Accepted(hello)
}

/// "a viewer certificate", "an admin certificate", ... or "no certificate", for refusals.
const fn certificate_phrase(role: Option<PeerRole>) -> &'static str {
    match role {
        None => "no certificate",
        Some(PeerRole::Viewer) => "a viewer certificate",
        Some(PeerRole::Worker) => "a worker certificate",
        Some(PeerRole::Monitor) => "a monitor certificate",
        Some(PeerRole::Admin) => "an admin certificate",
    }
}

/// The role half of [`read_and_check_hello`]: only a consistent `HELLO` whose role -- and whose
/// certificate's role -- is the port's own passes.
///
/// A Monitor or Admin `HELLO` is always refused here; the viewer port's remote administration goes
/// through [`check_role_with_admin`].
#[must_use]
pub fn check_role(hello: Hello, port: PeerRole, cert_role: Option<PeerRole>) -> HelloCheck {
    let refuse = |message: String| {
        HelloCheck::Refused(ErrorMsg {
            code: error_codes::ROLE_REFUSED,
            message,
            request_id: None,
        })
    };
    if !hello.role_is_consistent() {
        return refuse(format!(
            "refusing to pair: HELLO role {:?} disagrees with its capability ({}) -- a worker must \
             report its render capability, a viewer must not",
            hello.role,
            if hello.capability.is_some() {
                "present"
            } else {
                "absent"
            }
        ));
    }
    // Remote administration roles: with remote administration off (or on the worker port) a
    // Monitor or Admin HELLO is refused whatever certificate it carries.
    if matches!(hello.role, PeerRole::Monitor | PeerRole::Admin) {
        return refuse(REMOTE_ADMIN_REFUSAL.to_string());
    }
    if let Some(cert) = cert_role
        && cert != port
    {
        return refuse(format!(
            "refusing to pair: a {} certificate cannot be used on this {} port -- {}",
            role_name(cert),
            role_name(port),
            match port {
                PeerRole::Viewer => "viewers need a viewer certificate (`cert issue-client`)",
                PeerRole::Worker =>
                    "joining workers need a worker certificate (`cert issue-client --role worker` or \
                     `cert issue-token --role worker`)",
                PeerRole::Monitor | PeerRole::Admin => "remote administration is not available",
            }
        ));
    }
    match (port, hello.role) {
        (PeerRole::Viewer, PeerRole::Viewer) | (PeerRole::Worker, PeerRole::Worker) => {
            HelloCheck::Accepted(hello)
        }
        (PeerRole::Monitor | PeerRole::Admin, _) | (_, PeerRole::Monitor | PeerRole::Admin) => {
            refuse(REMOTE_ADMIN_REFUSAL.to_string())
        }
        (PeerRole::Viewer, PeerRole::Worker) => refuse(
            "refusing to pair: this is a VIEWER port -- `pleochroma-worker join` must dial the \
             coordinator's worker port (default 7880; `serve --worker-bind`) of a build with the \
             `worker` feature"
                .to_string(),
        ),
        (PeerRole::Worker, PeerRole::Viewer) => refuse(
            "refusing to pair: this is a coordinator's WORKER port -- viewers connect to the viewer \
             port (default 7878)"
                .to_string(),
        ),
    }
}

/// Logs and writes `refusal` in place of `WELCOME`.
///
/// Best-effort, like `super::connection::refuse_for_capacity`: the peer may already have hung up,
/// and the warning logged here is the meaningful outcome either way.
pub fn send_refusal<S: Write>(stream: &mut S, refusal: &ErrorMsg) {
    tracing::warn!("{} (code {})", refusal.message, refusal.code);
    let _ = pleochroma_net::messages::write_message(stream, refusal);
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_net::messages::{Backend, PROTOCOL_VERSION, RenderCapability};

    fn framed(hello: &Hello) -> std::io::Cursor<Vec<u8>> {
        let mut buf = Vec::new();
        pleochroma_net::messages::write_message(&mut buf, hello).unwrap();
        std::io::Cursor::new(buf)
    }

    fn worker_hello() -> Hello {
        Hello {
            role: PeerRole::Worker,
            capability: Some(RenderCapability {
                backend: Backend::Cpu { threads: 4 },
                max_pixels: 1,
                min_cadence_ms: 100,
                hdr: false,
            }),
            ..Hello::viewer(PROTOCOL_VERSION, [1; 8], [2; 8])
        }
    }

    const fn refused_code(check: &HelloCheck) -> Option<u32> {
        match check {
            HelloCheck::Accepted(_) => None,
            HelloCheck::Refused(e) => Some(e.code),
        }
    }

    /// An older peer's three-field `HELLO` (`protocol_version`, `build_hash`, `source_hash`;
    /// postcard encodes a tuple exactly like that struct) is refused with a message naming
    /// both versions.
    #[test]
    fn a_v13_hello_is_refused_naming_both_versions() {
        let mut buf = Vec::new();
        pleochroma_net::messages::write_message(&mut buf, &(13_u16, [1_u8; 8], [2_u8; 8])).unwrap();
        let check =
            read_and_check_hello(&mut std::io::Cursor::new(buf), PeerRole::Viewer, None).unwrap();
        let HelloCheck::Refused(refusal) = check else {
            panic!("a v13 HELLO must be refused");
        };
        assert_eq!(refusal.code, error_codes::BUILD_MISMATCH);
        let local = format!("v{PROTOCOL_VERSION}");
        assert!(
            refusal.message.contains("v13") && refusal.message.contains(&local),
            "{}",
            refusal.message
        );
    }

    #[test]
    fn a_worker_hello_on_the_viewer_port_is_refused_pointing_at_the_worker_port() {
        let check =
            read_and_check_hello(&mut framed(&worker_hello()), PeerRole::Viewer, None).unwrap();
        let HelloCheck::Refused(refusal) = check else {
            panic!("a worker HELLO must be refused on the viewer port");
        };
        assert_eq!(refusal.code, error_codes::ROLE_REFUSED);
        assert!(refusal.message.contains("join"), "{}", refusal.message);
        assert!(refusal.message.contains("7880"), "{}", refusal.message);
    }

    /// Every combination of port, claimed role and certificate role: only the port's own
    /// role on both counts passes.
    #[test]
    fn the_role_matrix_accepts_only_matching_hello_and_certificate_roles() {
        let viewer = Hello::viewer(PROTOCOL_VERSION, [1; 8], [2; 8]);
        let ports = [PeerRole::Viewer, PeerRole::Worker];
        let roles = [
            PeerRole::Viewer,
            PeerRole::Worker,
            PeerRole::Monitor,
            PeerRole::Admin,
        ];
        for port in ports {
            for claimed in roles {
                for cert in [None, Some(PeerRole::Viewer), Some(PeerRole::Worker)]
                    .into_iter()
                    .chain(roles.into_iter().skip(2).map(Some))
                {
                    let hello = match claimed {
                        PeerRole::Viewer => viewer.clone(),
                        PeerRole::Worker => worker_hello(),
                        PeerRole::Monitor | PeerRole::Admin => Hello {
                            role: claimed,
                            ..viewer.clone()
                        },
                    };
                    let check = check_role(hello, port, cert);
                    let ok = claimed == port && cert.is_none_or(|c| c == port);
                    let expected = (!ok).then_some(error_codes::ROLE_REFUSED);
                    assert_eq!(
                        refused_code(&check),
                        expected,
                        "port={port:?} claimed={claimed:?} cert={cert:?}"
                    );
                }
            }
        }
    }

    /// A Monitor or Admin HELLO is refused with `ROLE_REFUSED` on both ports, with or
    /// without a certificate, and the text says remote administration is not available.
    #[test]
    fn a_monitor_or_admin_hello_is_refused_as_not_available() {
        for role in [PeerRole::Monitor, PeerRole::Admin] {
            for port in [PeerRole::Viewer, PeerRole::Worker] {
                for cert in [None, Some(role), Some(PeerRole::Viewer)] {
                    let hello = Hello {
                        role,
                        ..Hello::viewer(PROTOCOL_VERSION, [1; 8], [2; 8])
                    };
                    let HelloCheck::Refused(refusal) = check_role(hello, port, cert) else {
                        panic!("{role:?} on {port:?} with {cert:?} must be refused");
                    };
                    assert_eq!(refusal.code, error_codes::ROLE_REFUSED);
                    assert!(
                        refusal
                            .message
                            .contains("remote administration is not available"),
                        "{}",
                        refusal.message
                    );
                }
            }
        }
    }

    fn admin_hello(role: PeerRole) -> Hello {
        Hello {
            role,
            ..Hello::viewer(PROTOCOL_VERSION, [1; 8], [2; 8])
        }
    }

    /// The viewer port's role matrix with remote administration on and off, the certificate
    /// on its allowlist or not: a Monitor or Admin `HELLO` passes only when the setting is
    /// on, the certificate has the same role and it is listed. Everything else is
    /// `check_role`'s answer.
    #[test]
    fn the_admin_matrix_with_remote_administration_on_and_off() {
        let certs = [
            None,
            Some(PeerRole::Viewer),
            Some(PeerRole::Worker),
            Some(PeerRole::Monitor),
            Some(PeerRole::Admin),
        ];
        for enabled in [false, true] {
            for listed in [false, true] {
                for claimed in [
                    PeerRole::Viewer,
                    PeerRole::Worker,
                    PeerRole::Monitor,
                    PeerRole::Admin,
                ] {
                    for cert in certs {
                        let hello = match claimed {
                            PeerRole::Viewer => Hello::viewer(PROTOCOL_VERSION, [1; 8], [2; 8]),
                            PeerRole::Worker => worker_hello(),
                            PeerRole::Monitor | PeerRole::Admin => admin_hello(claimed),
                        };
                        let policy = AdminPolicy { enabled, listed };
                        let check =
                            check_role_with_admin(hello.clone(), PeerRole::Viewer, cert, policy);
                        let admin_claim = matches!(claimed, PeerRole::Monitor | PeerRole::Admin);
                        let ok = if admin_claim {
                            enabled && listed && cert == Some(claimed)
                        } else {
                            claimed == PeerRole::Viewer
                                && cert.is_none_or(|c| c == PeerRole::Viewer)
                        };
                        assert_eq!(
                            refused_code(&check),
                            (!ok).then_some(error_codes::ROLE_REFUSED),
                            "on={enabled} listed={listed} claimed={claimed:?} cert={cert:?}"
                        );
                        if !(admin_claim && enabled) {
                            assert_eq!(
                                refused_code(&check),
                                refused_code(&check_role(hello, PeerRole::Viewer, cert)),
                                "outside an enabled admin claim the answer is check_role's"
                            );
                        }
                    }
                }
            }
        }
    }

    /// The refusal texts of the admin gate say what is wrong in plain words.
    #[test]
    fn the_admin_refusals_say_what_is_wrong() {
        let on = AdminPolicy {
            enabled: true,
            listed: true,
        };
        let text = |hello: Hello, cert: Option<PeerRole>, policy: AdminPolicy| {
            let HelloCheck::Refused(refusal) =
                check_role_with_admin(hello, PeerRole::Viewer, cert, policy)
            else {
                panic!("must be refused");
            };
            refusal.message
        };
        let off = text(
            admin_hello(PeerRole::Admin),
            Some(PeerRole::Admin),
            AdminPolicy {
                enabled: false,
                listed: true,
            },
        );
        assert!(
            off.contains("remote administration is not available"),
            "{off}"
        );
        let wrong = text(admin_hello(PeerRole::Admin), Some(PeerRole::Viewer), on);
        assert!(
            wrong.contains("an admin certificate") && wrong.contains("a viewer certificate"),
            "{wrong}"
        );
        let monitor_as_admin = text(admin_hello(PeerRole::Admin), Some(PeerRole::Monitor), on);
        assert!(
            monitor_as_admin.contains("a monitor certificate"),
            "{monitor_as_admin}"
        );
        let unlisted = text(
            admin_hello(PeerRole::Monitor),
            Some(PeerRole::Monitor),
            AdminPolicy {
                enabled: true,
                listed: false,
            },
        );
        assert!(
            unlisted.contains("not on the coordinator's list"),
            "{unlisted}"
        );
    }

    /// The worker port never serves administration, the setting notwithstanding; a passing
    /// administration `HELLO` is read through `read_and_check_hello_admin`.
    #[test]
    fn the_worker_port_refuses_admin_and_the_reader_applies_the_policy() {
        let on = AdminPolicy {
            enabled: true,
            listed: true,
        };
        for role in [PeerRole::Monitor, PeerRole::Admin] {
            assert_eq!(
                refused_code(&check_role_with_admin(
                    admin_hello(role),
                    PeerRole::Worker,
                    Some(role),
                    on
                )),
                Some(error_codes::ROLE_REFUSED)
            );
            let accepted =
                read_and_check_hello_admin(&mut framed(&admin_hello(role)), Some(role), on)
                    .unwrap();
            assert!(
                matches!(&accepted, HelloCheck::Accepted(hello) if hello.role == role),
                "{role:?}"
            );
            let refused = read_and_check_hello_admin(
                &mut framed(&admin_hello(role)),
                Some(role),
                AdminPolicy {
                    enabled: false,
                    listed: true,
                },
            )
            .unwrap();
            assert_eq!(refused_code(&refused), Some(error_codes::ROLE_REFUSED));
        }
    }

    #[test]
    fn a_certificate_mix_up_names_both_roles() {
        let HelloCheck::Refused(refusal) = check_role(
            Hello::viewer(PROTOCOL_VERSION, [1; 8], [2; 8]),
            PeerRole::Viewer,
            Some(PeerRole::Worker),
        ) else {
            panic!("a worker certificate must be refused on the viewer port");
        };
        assert!(
            refusal.message.contains("worker certificate")
                && refusal.message.contains("viewer port"),
            "{}",
            refusal.message
        );
    }

    #[test]
    fn an_inconsistent_role_is_refused_and_a_viewer_is_accepted() {
        let viewer = Hello::viewer(PROTOCOL_VERSION, [1; 8], [2; 8]);
        let mut odd = viewer.clone();
        odd.capability = Some(RenderCapability {
            backend: Backend::Cpu { threads: 4 },
            max_pixels: 1,
            min_cadence_ms: 100,
            hdr: false,
        });
        assert_eq!(
            refused_code(&check_role(odd, PeerRole::Viewer, None)),
            Some(error_codes::ROLE_REFUSED)
        );
        assert!(matches!(
            check_role(viewer, PeerRole::Viewer, Some(PeerRole::Viewer)),
            HelloCheck::Accepted(_)
        ));
        assert!(matches!(
            check_role(worker_hello(), PeerRole::Worker, Some(PeerRole::Worker)),
            HelloCheck::Accepted(_)
        ));
    }
}
