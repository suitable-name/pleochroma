//! The enrollment listener's per-connection handling.
//!
//! It is a TLS accept without a client-certificate requirement, then exactly one
//! [`pleochroma_net::enroll::EnrollRequest`]/
//! [`pleochroma_net::enroll::EnrollResponse`] exchange. See the `crate::enroll` module doc for
//! why this listener cannot reach `RenderRequest` handling.

use super::registry::{ClaimEvent, EnrollRegistry, ZeroizeLocal};
use pleochroma_net::{
    enroll::{EnrollRequest, EnrollResponse},
    messages::PeerRole,
};
use std::{
    io::{Read, Write},
    net::{SocketAddr, TcpStream},
    path::{Path, PathBuf},
    sync::Arc,
};

pub(super) type EnrollTlsStream = rustls::StreamOwned<rustls::ServerConnection, TcpStream>;

/// Hard cap on the encoded size of an [`EnrollRequest`], enforced by
/// [`handle_enroll_connection`]'s [`pleochroma_net::messages::read_message_bounded`]
/// call. An `EnrollRequest` is at most a `Claim`'s
/// [`pleochroma_net::token::SECRET_LEN`]-byte secret plus a handful of bytes of postcard
/// overhead -- comfortably under 4 KiB even with slack for a generously long `--name`.
const MAX_ENROLL_REQUEST_LEN: u32 = 4096;

/// Completes the TLS handshake for one just-accepted enrollment connection. No client
/// certificate is checked, unlike the render listener's `crate::serve::accept_tls`.
///
/// Sets `crate::serve::HANDSHAKE_TIMEOUT` on the raw socket before `complete_io`. Without it,
/// a peer that never finishes the handshake would pin this thread, since no client
/// certificate is required. The timeout is left in place afterwards:
/// [`handle_enroll_connection`] handles one request and returns, so no long-lived loop is
/// bounded by it.
pub(super) fn accept_enroll_tls(
    stream: TcpStream,
    config: &Arc<rustls::ServerConfig>,
    peer: Option<SocketAddr>,
) -> Option<EnrollTlsStream> {
    if let Err(e) = stream.set_read_timeout(Some(crate::serve::HANDSHAKE_TIMEOUT)) {
        tracing::debug!(
            "enrollment connection {peer:?}: failed to set handshake read timeout: {e}"
        );
    }
    if let Err(e) = stream.set_write_timeout(Some(crate::serve::HANDSHAKE_TIMEOUT)) {
        tracing::debug!(
            "enrollment connection {peer:?}: failed to set handshake write timeout: {e}"
        );
    }

    let conn = match rustls::ServerConnection::new(Arc::clone(config)) {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("enrollment connection {peer:?}: could not start TLS: {e}");
            return None;
        }
    };
    let mut tls_stream = rustls::StreamOwned::new(conn, stream);
    // One total deadline for the whole handshake, re-armed before every read (see
    // `crate::serve::DeadlineSocket`). This listener needs it at least as much as the render
    // listener does: it checks no client certificate, so the TLS handshake is its whole
    // authentication.
    let deadline = std::time::Instant::now() + crate::serve::HANDSHAKE_TIMEOUT;
    let handshake_result = {
        let mut io = crate::serve::DeadlineSocket::new(&tls_stream.sock, deadline);
        tls_stream.conn.complete_io(&mut io)
    };
    if let Err(e) = handshake_result {
        tracing::warn!("enrollment connection {peer:?}: TLS handshake failed: {e}");
        return None;
    }
    Some(tls_stream)
}

/// The refusal for a monitor or admin `Issue` while remote administration is off.
const REMOTE_ADMIN_OFF: &str = "monitor and admin codes need remote administration to be switched on: \
     start the coordinator with `serve --allow-remote-admin` (in the window: Settings, \
     Remote administration)";

/// The role and certificate label an `Issue { name }` asks for, checked against the
/// listener's role (see `super`'s module doc comment): a worker listener needs the
/// `worker:<label>` form `cert issue-token --role worker` sends and returns the bare
/// label; a viewer listener refuses that form. A viewer listener also honors
/// `monitor:<label>` and `admin:<label>`, but only when remote administration registered the
/// role's allowlist with the registry ([`EnrollRegistry::register_allowlist`]).
///
/// # Errors
///
/// The refusal reason for an `Issue` of the other role.
fn issue_target<'a>(
    name: &'a str,
    registry: &EnrollRegistry,
) -> Result<(PeerRole, &'a str), String> {
    let remote_admin = [
        (crate::pki::MONITOR_CN_PREFIX, PeerRole::Monitor),
        (crate::pki::ADMIN_CN_PREFIX, PeerRole::Admin),
    ]
    .into_iter()
    .find_map(|(prefix, role)| name.strip_prefix(prefix).map(|label| (role, label)));
    match (registry.role(), remote_admin) {
        (PeerRole::Viewer, Some((role, label))) => {
            if registry.allowlist_for(role).is_some() {
                Ok((role, label))
            } else {
                Err(REMOTE_ADMIN_OFF.to_string())
            }
        }
        (PeerRole::Worker, Some(_)) => Err(
            "this is the WORKER enrollment listener -- monitor and admin codes are issued on the \
             coordinator's VIEWER enrollment listener (default port 7879)"
                .to_string(),
        ),
        (PeerRole::Monitor | PeerRole::Admin, _) => Err(REMOTE_ADMIN_OFF.to_string()),
        (role, None) => match (role, name.strip_prefix(crate::pki::WORKER_CN_PREFIX)) {
            (PeerRole::Viewer, None) => Ok((PeerRole::Viewer, name)),
            (PeerRole::Worker, Some(label)) => Ok((PeerRole::Worker, label)),
            (PeerRole::Viewer, Some(_)) => Err(
                "this is the VIEWER enrollment listener -- a worker token must be issued on the coordinator's \
                 WORKER enrollment listener (default port 7881)"
                    .to_string(),
            ),
            (PeerRole::Worker, None) => Err(
                "this is the WORKER enrollment listener -- pass `cert issue-token --role worker` for a worker \
                 token, or use the viewer enrollment listener (default port 7879) for a viewer"
                    .to_string(),
            ),
            (PeerRole::Monitor | PeerRole::Admin, _) => Err(REMOTE_ADMIN_OFF.to_string()),
        },
    }
}

/// The reply to an `Issue { name }`: refused unless the peer is on loopback AND presented
/// the operator secret (`authorised`); otherwise a freshly minted token.
///
/// Loopback alone is not authority: any local user, or anything forwarding a port to the
/// loopback listener, appears as a loopback peer. The operator secret is readable only by
/// whoever can read the PKI directory, which is the same party that can read `ca.key`.
fn issue_response(
    name: &str,
    authorised: bool,
    registry: &EnrollRegistry,
    pki_dir: &Path,
    peer: Option<SocketAddr>,
) -> EnrollResponse {
    if !peer.is_some_and(|p| p.ip().is_loopback()) {
        tracing::warn!("enrollment connection {peer:?}: refused Issue from a non-loopback peer");
        return EnrollResponse::IssueRefused {
            reason: "issuing enrollment tokens is only permitted from loopback".to_string(),
        };
    }
    if !authorised {
        tracing::warn!("enrollment connection {peer:?}: refused Issue without the operator secret");
        return EnrollResponse::IssueRefused {
            reason: format!(
                "issuing enrollment tokens needs the operator secret ({} in the PKI directory)",
                pleochroma_net::enroll::OPERATOR_SECRET_FILE
            ),
        };
    }
    match issue_target(name, registry) {
        Ok((role, label)) => match registry.issue_for_role(pki_dir, label, role) {
            Ok(issued) => EnrollResponse::Issued {
                token: issued.token,
                expires_in_secs: issued.expires_in_secs,
            },
            // EnrollIssueError -> String at the wire boundary: `reason` travels to the
            // peer as plain text, never matched on.
            Err(reason) => EnrollResponse::IssueRefused {
                reason: reason.to_string(),
            },
        },
        Err(reason) => EnrollResponse::IssueRefused { reason },
    }
}

/// Handles exactly one [`EnrollRequest`]/[`EnrollResponse`] exchange on `stream`, then
/// returns -- not a loop, since both `Issue` and `Claim` are one-shot.
///
/// Generic over `Read + Write` so this crate's tests can drive it over an in-memory
/// duplex, like `crate::serve::handle_connection` does.
///
/// Reads the request via [`pleochroma_net::messages::read_message_bounded`] with
/// [`MAX_ENROLL_REQUEST_LEN`] rather than plain [`pleochroma_net::messages::read_message`]:
/// this listener accepts a connection from anyone (no client-certificate
/// requirement -- see the module doc comment), so a peer that never authenticates at all
/// must not be able to make this process attempt a
/// [`pleochroma_net::framing::MAX_FRAME_LEN`]-sized (512 MiB) allocation with a single
/// 4-byte length prefix.
///
/// # Errors
///
/// A human-readable message for a transport-level failure decoding the request or
/// encoding the response. An `Issue` refused for being non-loopback or lacking the
/// operator secret, or a `Claim` that doesn't match a pending enrollment, are NOT error
/// returns -- all are reported to the peer as a normal [`EnrollResponse`], since none
/// means the connection is broken.
pub(super) fn handle_enroll_connection<S: Read + Write>(
    mut stream: S,
    registry: &EnrollRegistry,
    pki_dir: &Path,
    allowlist_path: Option<&Path>,
    peer: Option<SocketAddr>,
) -> Result<(), String> {
    // A fresh `HANDSHAKE_TIMEOUT` budget for this phase, separate from the TLS handshake's.
    // `DeadlineIo` only needs `S: Read`, so this works for the real TLS stream and for the
    // in-memory duplexes of this module's tests alike.
    let deadline = std::time::Instant::now() + crate::serve::HANDSHAKE_TIMEOUT;
    let request: EnrollRequest = {
        let mut guarded = crate::serve::DeadlineIo::new(&mut stream, deadline);
        pleochroma_net::messages::read_message_bounded(&mut guarded, MAX_ENROLL_REQUEST_LEN)
            .map_err(|e| e.to_string())?
    };

    match request {
        EnrollRequest::Issue { .. } => {
            tracing::warn!(
                "enrollment connection {peer:?}: refused an Issue without the operator secret"
            );
            let response = EnrollResponse::IssueRefused {
                reason: "this listener requires the operator secret to issue a token -- update \
                         `pleochroma-worker cert issue-token`"
                    .to_string(),
            };
            pleochroma_net::messages::write_message(&mut stream, &response)
                .map_err(|e| e.to_string())
        }
        EnrollRequest::IssueAuthorized {
            name,
            mut operator_secret,
        } => {
            let authorised = registry.operator_secret_matches(&operator_secret);
            operator_secret.zeroize_local();
            let response = issue_response(&name, authorised, registry, pki_dir, peer);
            pleochroma_net::messages::write_message(&mut stream, &response)
                .map_err(|e| e.to_string())
        }
        EnrollRequest::Claim { mut secret } => {
            let claimed = registry.claim(&secret);
            secret.zeroize_local();

            let response = if let Some(claimed) = claimed {
                // The registry's own role uses the listener's allowlist; a token of another
                // role needs the path registered for that role, else it is refused.
                let target: Result<Option<PathBuf>, String> = if claimed.role == registry.role() {
                    Ok(allowlist_path.map(Path::to_path_buf))
                } else {
                    registry
                        .allowlist_for(claimed.role)
                        .map(Some)
                        .ok_or_else(|| {
                            "no allowlist is registered for this token's role".to_string()
                        })
                };
                let allowlisted = target.and_then(|path| {
                    path.map_or(Ok(()), |path| {
                        pleochroma_net::tls::append_to_allowlist(
                            &path,
                            &claimed.bundle.client_fingerprint,
                            &claimed.name,
                        )
                        .map_err(|e| e.to_string())
                    })
                });
                match allowlisted {
                    Ok(()) => {
                        registry.notify_claimed(&ClaimEvent {
                            id: claimed.id,
                            role: claimed.role,
                            name: claimed.name.clone(),
                            fingerprint: claimed.bundle.client_fingerprint,
                            peer: peer.unwrap_or_else(|| SocketAddr::from(([0, 0, 0, 0], 0))),
                        });
                        EnrollResponse::Claimed {
                            ca_pem: claimed.bundle.ca_pem,
                            client_cert_pem: claimed.bundle.client_cert_pem,
                            client_key_pem: claimed.bundle.client_key_pem.to_string(),
                        }
                    }
                    Err(e) => {
                        // The certificate must not be handed out without also being
                        // allowlisted -- an unlisted cert would just fail the render
                        // listener's check later, confusingly far from this cause.
                        tracing::error!(
                            "enrollment connection {peer:?}: claim succeeded but appending to the allowlist \
                             failed, so it is being reported as a failed claim instead: {e}"
                        );
                        EnrollResponse::ClaimFailed
                    }
                }
            } else {
                tracing::debug!(
                    "enrollment connection {peer:?}: claim did not match any pending enrollment (wrong \
                     secret, already claimed, or expired)"
                );
                EnrollResponse::ClaimFailed
            };
            pleochroma_net::messages::write_message(&mut stream, &response)
                .map_err(|e| e.to_string())
        }
    }
}

#[cfg(test)]
mod issue_authorisation_tests {
    use super::*;
    use pleochroma_net::messages::{read_message, write_message};
    use std::io::Cursor;
    use zeroize::Zeroizing;

    /// An in-memory connection: the request is pre-loaded, the reply collects in `output`.
    struct Duplex {
        input: Cursor<Vec<u8>>,
        output: Vec<u8>,
    }

    impl Read for Duplex {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            self.input.read(buf)
        }
    }

    impl Write for Duplex {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.output.write(buf)
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    /// Sends one loopback `Issue` carrying `secret` through `registry`'s handler and
    /// returns the reply.
    fn issue_from_loopback(registry: &EnrollRegistry, secret: [u8; 32]) -> EnrollResponse {
        let mut request = Vec::new();
        write_message(
            &mut request,
            &EnrollRequest::IssueAuthorized {
                name: "laptop".to_string(),
                operator_secret: secret,
            },
        )
        .unwrap();
        let mut duplex = Duplex {
            input: Cursor::new(request),
            output: Vec::new(),
        };
        let peer: SocketAddr = "127.0.0.1:50000".parse().unwrap();
        handle_enroll_connection(&mut duplex, registry, Path::new("."), None, Some(peer)).unwrap();
        read_message(&mut Cursor::new(duplex.output)).unwrap()
    }

    /// Loopback alone is not authority: without the operator secret the `Issue` is refused.
    #[test]
    fn an_issue_from_loopback_without_the_operator_secret_is_refused() {
        let registry = EnrollRegistry::new().with_operator_secret(Zeroizing::new([7u8; 32]));
        let reply = issue_from_loopback(&registry, [0u8; 32]);
        assert!(
            matches!(&reply, EnrollResponse::IssueRefused { reason } if reason.contains("operator secret")),
            "{reply:?}"
        );
    }

    /// A registry nobody configured a secret for issues nothing.
    #[test]
    fn an_issue_is_refused_when_no_operator_secret_is_configured() {
        let reply = issue_from_loopback(&EnrollRegistry::new(), [0u8; 32]);
        assert!(
            matches!(reply, EnrollResponse::IssueRefused { .. }),
            "{reply:?}"
        );
    }
}
