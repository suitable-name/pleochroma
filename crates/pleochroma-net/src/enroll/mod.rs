//! The enrollment wire protocol, and the claiming half of token-based enrollment.
//!
//! Lives here rather than in `apps/pleochroma-worker` because both it (`cert claim`) and
//! `apps/pleochroma-cut` need to encode/decode these messages and run the same
//! CA-fingerprint verification ([`PinnedCaVerifier`]) before ever sending the secret -- a
//! viewer must not depend on the server binary, and a security policy must not exist in
//! two copies that can drift apart. The server-side registry (minting, hashing,
//! constant-time compare, TTL sweep) has no viewer-side counterpart and stays in
//! `apps/pleochroma-worker/src/enroll/registry.rs`, which imports [`EnrollRequest`] and
//! [`EnrollResponse`] from here. [`crate::token`] is shared for the same reason. See
//! `apps/pleochroma-worker/src/enroll/mod.rs` for the full enrollment design: the token's
//! security properties, its lifecycle, and why issuing is loopback-only.

use crate::token;
use rustls::{
    CertificateError, DigitallySignedStruct, Error as RustlsError, RootCertStore, SignatureScheme,
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{CertificateDer, ServerName, UnixTime},
};
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    net::{SocketAddr, TcpStream, ToSocketAddrs},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};
use zeroize::{Zeroize, Zeroizing};

/// Connect timeout and per-read/per-write socket timeout of [`claim`]: an enrollment
/// exchange is a few small messages, so a listener that stalls longer is treated as dead.
const CLAIM_IO_TIMEOUT: Duration = Duration::from_secs(10);

/// Cap on the reply [`claim`] will read. A `Claimed` reply is a few PEM blocks (a few
/// KiB); nothing legitimate approaches this.
const MAX_CLAIM_REPLY_LEN: u32 = 64 * 1024;

/// Length in bytes of the operator secret that authorises [`EnrollRequest::IssueAuthorized`].
pub const OPERATOR_SECRET_LEN: usize = 32;

/// File name of the operator secret inside the PKI directory.
pub const OPERATOR_SECRET_FILE: &str = "issue.secret";

/// Path of the operator secret file inside `pki_dir`.
#[must_use]
pub fn operator_secret_path(pki_dir: &Path) -> PathBuf {
    pki_dir.join(OPERATOR_SECRET_FILE)
}

/// Reads the operator secret (hex, one line) from `<pki_dir>/issue.secret`.
///
/// `serve` creates the file on first start with owner-only permissions. `cert issue-token`
/// reads it and presents it in [`EnrollRequest::IssueAuthorized`]. The returned secret is
/// zeroized on drop.
///
/// # Errors
///
/// A human-readable message if the file can't be read or isn't 64 hex characters.
pub fn read_operator_secret(
    pki_dir: &Path,
) -> Result<Zeroizing<[u8; OPERATOR_SECRET_LEN]>, String> {
    let path = operator_secret_path(pki_dir);
    let text = Zeroizing::new(
        std::fs::read_to_string(&path)
            .map_err(|e| format!("could not read the operator secret {}: {e}", path.display()))?,
    );
    crate::tls::fingerprint_from_hex(text.trim())
        .map(Zeroizing::new)
        .ok_or_else(|| {
            format!(
                "{} is not a valid operator secret (expected {} hex characters)",
                path.display(),
                OPERATOR_SECRET_LEN * 2
            )
        })
}

/// Writes `secret` as hex to `<pki_dir>/issue.secret`, readable by the current user only
/// (the same protection as `ca.key`, see [`crate::tls::write_private_key_pem`]).
///
/// # Errors
///
/// A human-readable message if the file can't be created or its permissions restricted.
pub fn write_operator_secret(
    pki_dir: &Path,
    secret: &[u8; OPERATOR_SECRET_LEN],
) -> Result<(), String> {
    let mut text = Zeroizing::new(crate::tls::fingerprint_to_hex(secret));
    text.push('\n');
    crate::tls::write_private_key_pem(&operator_secret_path(pki_dir), &text)
}

/// One request on the enrollment wire protocol.
///
/// Deliberately a *different* message schema than [`crate::messages::ClientMessage`]
/// (the render protocol), not a variant of it, so bytes can never accidentally decode as
/// the wrong protocol if a wire got crossed.
///
/// `Debug` is written by hand: the claim secret and the operator secret print as
/// `<redacted>`.
#[derive(Serialize, Deserialize)]
pub enum EnrollRequest {
    /// Unauthenticated issue request, kept only to answer older clients. A listener refuses
    /// it with a reply that points at `cert issue-token`: loopback alone does not authorise
    /// issuing (see [`Self::IssueAuthorized`]). The variant stays so an older client gets
    /// that reply instead of a decode error.
    Issue { name: String },
    /// Attempt to claim a pending enrollment with this secret.
    Claim { secret: [u8; token::SECRET_LEN] },
    /// Mint a new token for a viewer (or, on the worker listener, a worker) labeled `name`
    /// once claimed. Honored only from a loopback peer that also presents the operator
    /// secret (`<pki_dir>/issue.secret`, see [`read_operator_secret`]), so a local user or
    /// a port forward without read access to the PKI directory cannot mint certificates.
    ///
    /// Protocol note: appended after `Claim` so the existing variants keep their postcard
    /// indices. The protocol has no version field, so a listener that predates this variant
    /// fails to decode it.
    IssueAuthorized {
        name: String,
        operator_secret: [u8; OPERATOR_SECRET_LEN],
    },
}

impl fmt::Debug for EnrollRequest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Issue { name } => f.debug_struct("Issue").field("name", name).finish(),
            Self::Claim { .. } => f
                .debug_struct("Claim")
                .field("secret", &format_args!("<redacted>"))
                .finish(),
            Self::IssueAuthorized { name, .. } => f
                .debug_struct("IssueAuthorized")
                .field("name", name)
                .field("operator_secret", &format_args!("<redacted>"))
                .finish(),
        }
    }
}

/// The reply to one [`EnrollRequest`] on the enrollment wire protocol.
///
/// `Debug` is written by hand: the token and the private key print as `<redacted>`.
#[derive(Serialize, Deserialize)]
pub enum EnrollResponse {
    Issued {
        /// The full `GW1-...` token string -- see [`crate::token`].
        token: String,
        expires_in_secs: u64,
    },
    /// Carries a reason -- loopback-only / registry-full, not attacker-facing. Contrast
    /// with [`EnrollResponse::ClaimFailed`], whose reason is always the same generic
    /// string.
    IssueRefused { reason: String },
    Claimed {
        ca_pem: String,
        client_cert_pem: String,
        client_key_pem: String,
    },
    /// Deliberately the same message for "wrong secret", "expired", "already claimed",
    /// and "internal error" -- distinguishing them would hand a remote prober an
    /// enumeration oracle. The real reason is still logged server-side
    /// (`RUST_LOG=debug`). [`ClaimError::Refused`] preserves this same ambiguity.
    ClaimFailed,
}

impl fmt::Debug for EnrollResponse {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Issued {
                expires_in_secs, ..
            } => f
                .debug_struct("Issued")
                .field("token", &format_args!("<redacted>"))
                .field("expires_in_secs", expires_in_secs)
                .finish(),
            Self::IssueRefused { reason } => f
                .debug_struct("IssueRefused")
                .field("reason", reason)
                .finish(),
            Self::Claimed { .. } => f
                .debug_struct("Claimed")
                .field("client_key_pem", &format_args!("<redacted>"))
                .finish_non_exhaustive(),
            Self::ClaimFailed => f.write_str("ClaimFailed"),
        }
    }
}

/// Verifies the enrollment listener's presented certificate chain against a CA whose
/// SHA-256 fingerprint matches [`Self::expected_ca_fingerprint`] -- never against a CA
/// file on disk, since a claiming client has none yet.
///
/// This is certificate PINNING, a legitimate pattern `rustls` exposes
/// `ClientConfig::dangerous()` specifically to support -- distinct from disabling
/// verification. All cryptographic work (chain construction, signature verification) is
/// still delegated to `rustls`/`rustls-webpki`/`ring`; this type only selects which
/// presented certificate counts as "the CA", using the token's fingerprint.
#[derive(Debug)]
struct PinnedCaVerifier {
    expected_ca_fingerprint: crate::tls::Fingerprint,
    provider: Arc<rustls::crypto::CryptoProvider>,
    /// Set when [`Self::verify_server_cert`] fails because no presented certificate
    /// matched the pinned fingerprint -- the attacker-in-the-middle-shaped failure,
    /// which [`claim`] reports distinctly from an ordinary handshake or connection
    /// failure. A flag, not a richer `rustls::Error` variant, because
    /// `verify_server_cert` must return a real `rustls::Error` with no generic
    /// `Other(Box<dyn Error>)` constructor; `Arc<AtomicBool>` because
    /// `ServerCertVerifier` requires `Send + Sync`.
    ca_mismatch: Arc<AtomicBool>,
}

impl PinnedCaVerifier {
    fn new(expected_ca_fingerprint: crate::tls::Fingerprint, ca_mismatch: Arc<AtomicBool>) -> Self {
        Self {
            expected_ca_fingerprint,
            provider: Arc::new(rustls::crypto::ring::default_provider()),
            ca_mismatch,
        }
    }
}

impl ServerCertVerifier for PinnedCaVerifier {
    /// Finds, among the certificates the server presented (`intermediates`), the one
    /// whose SHA-256 fingerprint matches the token's. If none matches, verification
    /// fails outright -- no fallback to "trust it anyway" -- and [`Self::ca_mismatch`] is
    /// set so [`claim`] can report this distinctly. Once found, a one-off
    /// `RootCertStore` containing just that certificate is built and the rest of
    /// chain/signature verification is delegated to `rustls`'s own `WebPkiServerVerifier`
    /// against it -- see this type's doc comment for why that keeps this "pinning", not
    /// "skipping verification".
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &ServerName<'_>,
        ocsp_response: &[u8],
        now: UnixTime,
    ) -> Result<ServerCertVerified, RustlsError> {
        let Some(pinned_ca) = intermediates
            .iter()
            .find(|c| crate::tls::fingerprint(c) == self.expected_ca_fingerprint)
        else {
            self.ca_mismatch.store(true, Ordering::SeqCst);
            return Err(RustlsError::General(
                "the server did not present a certificate chain containing the CA this \
                 enrollment token commits to -- refusing to trust it (this is either the \
                 wrong worker, or an attacker in the middle)"
                    .to_string(),
            ));
        };

        let mut roots = RootCertStore::empty();
        roots.add(pinned_ca.clone()).map_err(|e| {
            RustlsError::General(format!(
                "the pinned CA certificate is not usable as a trust anchor: {e}"
            ))
        })?;
        let verifier = rustls::client::WebPkiServerVerifier::builder(Arc::new(roots))
            .build()
            .map_err(|e| {
                RustlsError::General(format!("failed to build a verifier for the pinned CA: {e}"))
            })?;

        // The coordinator's fixed name first: a certificate issued since 0.12.0 carries it,
        // so an address change never breaks pairing. A certificate without it (not migrated
        // yet) is checked against the dialed name, as before.
        let fixed = ServerName::try_from(crate::tls::COORDINATOR_TLS_NAME)
            .map_err(|e| RustlsError::General(format!("invalid coordinator TLS name: {e}")))?
            .to_owned();
        match verifier.verify_server_cert(end_entity, intermediates, &fixed, ocsp_response, now) {
            Err(RustlsError::InvalidCertificate(
                CertificateError::NotValidForName | CertificateError::NotValidForNameContext { .. },
            )) => verifier.verify_server_cert(
                end_entity,
                intermediates,
                server_name,
                ocsp_response,
                now,
            ),
            other => other,
        }
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        Err(RustlsError::General(
            "TLS 1.2 is not supported by the enrollment listener".to_string(),
        ))
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, RustlsError> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

/// A freshly claimed client-certificate bundle, still in memory.
///
/// The same three PEM blocks `pleochroma-worker cert issue-client`/`cert claim` write to
/// `ca.pem`/`client.pem`/`client.key`. Writing to disk (with the private key's
/// permissions restricted -- see [`crate::tls::write_private_key_pem`]) is the caller's
/// job, since where the bundle belongs is caller-specific.
pub struct ClaimedBundle {
    /// PEM-encoded certificate authority that signs the server.
    pub ca_pem: String,
    /// PEM-encoded client certificate issued to the enrolling peer.
    pub client_cert_pem: String,
    /// Wrapped in [`Zeroizing`] since this is the client's private key, plaintext,
    /// having just crossed the wire. Overwritten as soon as the caller is done with it
    /// rather than lingering in a freed heap allocation.
    pub client_key_pem: Zeroizing<String>,
}

impl fmt::Debug for ClaimedBundle {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClaimedBundle")
            .field("ca_pem_len", &self.ca_pem.len())
            .field("client_cert_pem_len", &self.client_cert_pem.len())
            .field("client_key_pem", &format_args!("<redacted>"))
            .finish_non_exhaustive()
    }
}

/// Everything that can go wrong redeeming an enrollment token.
///
/// One type so a caller (a CLI's error formatting, or a GUI's toast) has a single match
/// to render instead of parsing a generic string. Every variant maps to genuinely
/// different advice for the user -- see [`fmt::Display`]'s impl below.
///
/// `Debug` is written by hand: a malformed token's error can name a character of the token,
/// so the token error is not printed.
pub enum ClaimError {
    /// `token` isn't a well-formed `GW1-...` string -- see [`token::TokenError`].
    InvalidToken(token::TokenError),
    /// `addr` isn't `host:port`, or its host half isn't valid for TLS server-name
    /// verification.
    InvalidAddr(String),
    /// The TCP connection itself could not be established -- most likely a wrong or
    /// unreachable address, not a security concern. Kept distinct from
    /// [`Self::CaFingerprintMismatch`] so a wrong address never gets alarming security
    /// wording, and a genuine impersonation never reads like "check your network".
    Connect {
        addr: String,
        source: std::io::Error,
    },
    /// The TLS handshake failed for a reason OTHER than the pinned CA fingerprint not
    /// matching (e.g. the pinned certificate itself failed ordinary `webpki`
    /// validation). Not conflated with [`Self::CaFingerprintMismatch`]: this is not
    /// evidence of impersonation.
    Handshake {
        addr: String,
        source: std::io::Error,
    },
    /// The security-relevant handshake failure: the server's certificate chain does not
    /// contain one matching the CA fingerprint this token commits to -- either the wrong
    /// worker or an attacker in the middle (see [`PinnedCaVerifier`]). Its own variant so
    /// callers must word this differently from an ordinary connection problem.
    CaFingerprintMismatch { addr: String },
    /// A transport-level failure sending the claim request or reading the response,
    /// after a successful, correctly-pinned handshake.
    Protocol(String),
    /// The server reported [`EnrollResponse::ClaimFailed`]: wrong, already-claimed, or
    /// expired (tokens live 180s) -- deliberately not distinguished (see that variant's
    /// doc comment), so this crate cannot honestly report more than the server revealed.
    Refused,
    /// The server responded with something other than `Claimed`/`ClaimFailed` to a
    /// `Claim` request -- a protocol/version mismatch rather than any of the above.
    UnexpectedResponse,
}

impl fmt::Debug for ClaimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidToken(_) => f.write_str("InvalidToken(<redacted>)"),
            Self::InvalidAddr(msg) => f.debug_tuple("InvalidAddr").field(msg).finish(),
            Self::Connect { addr, source } => f
                .debug_struct("Connect")
                .field("addr", addr)
                .field("source", source)
                .finish(),
            Self::Handshake { addr, source } => f
                .debug_struct("Handshake")
                .field("addr", addr)
                .field("source", source)
                .finish(),
            Self::CaFingerprintMismatch { addr } => f
                .debug_struct("CaFingerprintMismatch")
                .field("addr", addr)
                .finish(),
            Self::Protocol(msg) => f.debug_tuple("Protocol").field(msg).finish(),
            Self::Refused => f.write_str("Refused"),
            Self::UnexpectedResponse => f.write_str("UnexpectedResponse"),
        }
    }
}

impl fmt::Display for ClaimError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            // The error is not printed for a bad character: that would echo part of the token.
            Self::InvalidToken(token::TokenError::InvalidCharacter(_)) => write!(
                f,
                "not a valid enrollment token: it contains a character that is not allowed"
            ),
            Self::InvalidToken(e) => write!(f, "not a valid enrollment token: {e}"),
            // Already complete, self-describing messages -- pass through verbatim.
            Self::InvalidAddr(msg) | Self::Protocol(msg) => write!(f, "{msg}"),
            Self::Connect { addr, source } => write!(f, "could not connect to {addr}: {source}"),
            Self::Handshake { addr, source } => {
                write!(f, "TLS handshake with {addr} failed: {source}")
            }
            Self::CaFingerprintMismatch { addr } => write!(
                f,
                "TLS handshake with {addr} failed: the server did not present a certificate chain \
                 containing the CA this enrollment token commits to -- refusing to trust it (this is \
                 either the wrong worker, or an attacker in the middle)"
            ),
            Self::Refused => write!(
                f,
                "claim failed: the enrollment token was invalid, expired, or already used"
            ),
            Self::UnexpectedResponse => {
                write!(f, "unexpected response from the enrollment listener")
            }
        }
    }
}

impl std::error::Error for ClaimError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::InvalidToken(e) => Some(e),
            Self::Connect { source, .. } | Self::Handshake { source, .. } => Some(source),
            _ => None,
        }
    }
}

/// Redeems `token` against the enrollment listener at `addr` (`host:port`).
///
/// Decodes `token` (see [`crate::token`]), connects to `addr`, and verifies the listener
/// against the CA fingerprint the token carries -- via [`PinnedCaVerifier`] -- *before*
/// ever sending the secret. Zeroizes its own copy of the secret immediately after
/// sending it, and again when the decoded token is dropped. On success, returns the
/// bundle in memory; writing it to disk is the caller's job.
///
/// Never logs `token`, the decoded secret, or the claimed private key, on success or
/// failure -- no `tracing`/`println!` call anywhere in this function or in
/// [`PinnedCaVerifier`].
///
/// # Errors
///
/// See [`ClaimError`]'s variants -- one per distinguishable failure, so a caller can say
/// something different for "you typo'd the address" than for "something is
/// impersonating the worker".
pub fn claim(token: &str, addr: &str) -> Result<ClaimedBundle, ClaimError> {
    let decoded = token::decode(token).map_err(ClaimError::InvalidToken)?;

    let host = crate::tls::host_for_server_name(addr)
        .ok_or_else(|| ClaimError::InvalidAddr(format!("{addr:?} must be host:port")))?;
    let server_name = ServerName::try_from(host.to_string()).map_err(|e| {
        ClaimError::InvalidAddr(format!("{addr:?}: invalid host for TLS verification: {e}"))
    })?;

    let tcp = connect_with_timeout(addr).map_err(|e| ClaimError::Connect {
        addr: addr.to_string(),
        source: e,
    })?;
    claim_on_socket(tcp, server_name, &decoded, addr, None).map_err(|attempt| match attempt {
        Attempt::Failed(e) => e,
        // Only reachable with a gate, and `claim` passes none.
        Attempt::Superseded => ClaimError::UnexpectedResponse,
    })
}

/// Why one [`claim_on_socket`] attempt did not produce a bundle.
enum Attempt {
    /// A real failure, see [`ClaimError`].
    Failed(ClaimError),
    /// Another address passed the CA pin first ([`claim_any`]); this connection was dropped
    /// before the secret was sent.
    Superseded,
}

/// The pinned-CA handshake and the claim exchange over an already connected `tcp`.
///
/// `label` is the address text used in errors. With a `gate` (used by [`claim_any`]), the
/// first attempt to finish the pinned handshake flips it and goes on to send the secret;
/// every later one returns [`Attempt::Superseded`] without sending anything, so a token is
/// never spent on a connection that lost the race.
fn claim_on_socket(
    tcp: TcpStream,
    server_name: ServerName<'static>,
    decoded: &token::EnrollToken,
    label: &str,
    gate: Option<&AtomicBool>,
) -> Result<ClaimedBundle, Attempt> {
    let ca_mismatch = Arc::new(AtomicBool::new(false));
    let verifier = Arc::new(PinnedCaVerifier::new(
        decoded.ca_fingerprint,
        Arc::clone(&ca_mismatch),
    ));
    let client_config =
        rustls::ClientConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
            .dangerous()
            .with_custom_certificate_verifier(verifier)
            .with_no_client_auth();

    let conn =
        rustls::ClientConnection::new(Arc::new(client_config), server_name).map_err(|e| {
            Attempt::Failed(ClaimError::Handshake {
                addr: label.to_string(),
                source: std::io::Error::other(e),
            })
        })?;
    let mut stream = rustls::StreamOwned::new(conn, tcp);
    if let Err(e) = stream.conn.complete_io(&mut stream.sock) {
        return Err(Attempt::Failed(if ca_mismatch.load(Ordering::SeqCst) {
            ClaimError::CaFingerprintMismatch {
                addr: label.to_string(),
            }
        } else {
            ClaimError::Handshake {
                addr: label.to_string(),
                source: e,
            }
        }));
    }
    if gate.is_some_and(|g| g.swap(true, Ordering::SeqCst)) {
        return Err(Attempt::Superseded);
    }

    // Send the secret, then immediately zeroize the request's own copy -- it shouldn't
    // linger in memory longer than necessary. The caller's `decoded` wipes itself on drop.
    let mut request = EnrollRequest::Claim {
        secret: *decoded.secret,
    };
    let write_result = crate::messages::write_message(&mut stream, &request);
    if let EnrollRequest::Claim { secret } = &mut request {
        secret.zeroize();
    }
    write_result.map_err(|e| Attempt::Failed(ClaimError::Protocol(e.to_string())))?;

    let response: EnrollResponse =
        crate::messages::read_message_bounded(&mut stream, MAX_CLAIM_REPLY_LEN)
            .map_err(|e| Attempt::Failed(ClaimError::Protocol(e.to_string())))?;

    match response {
        EnrollResponse::Claimed {
            ca_pem,
            client_cert_pem,
            client_key_pem,
        } => Ok(ClaimedBundle {
            ca_pem,
            client_cert_pem,
            client_key_pem: Zeroizing::new(client_key_pem),
        }),
        EnrollResponse::ClaimFailed => Err(Attempt::Failed(ClaimError::Refused)),
        EnrollResponse::Issued { .. } | EnrollResponse::IssueRefused { .. } => {
            Err(Attempt::Failed(ClaimError::UnexpectedResponse))
        }
    }
}

/// Connect timeout of each address [`claim_any`] tries.
pub const CLAIM_ANY_CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// A successful [`claim_any`]: the bundle, and which address answered.
pub struct ClaimAnyOutcome {
    /// The claimed bundle.
    pub bundle: ClaimedBundle,
    /// The address whose handshake passed the CA pin first.
    pub addr: SocketAddr,
}

impl fmt::Debug for ClaimAnyOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClaimAnyOutcome")
            .field("bundle", &self.bundle)
            .field("addr", &self.addr)
            .finish()
    }
}

/// Why [`claim_any`] failed, in plain words for a phone or desktop screen.
///
/// When several addresses fail differently the most informative reason wins: a refused
/// code, then a wrong coordinator, then refused, unreachable and timed-out connections.
/// No variant carries the token or any part of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaimAnyError {
    /// The address list was empty.
    NoAddresses,
    /// The code is not a well-formed enrollment code.
    InvalidCode,
    /// Every address timed out.
    AllTimedOut,
    /// Every address actively refused the connection (nothing listens there).
    Refused,
    /// No address could be reached for another reason (no route, network down).
    Unreachable,
    /// A coordinator answered, but not the one this code was issued by (the CA pin
    /// failed).
    WrongCoordinator,
    /// The coordinator refused the code. The server deliberately does not say whether it
    /// expired or was already used, so this says both.
    CodeExpiredOrUsed,
    /// A secure connection could not be set up or the exchange broke; the text is a
    /// plain description without secrets.
    Failed(String),
}

impl fmt::Display for ClaimAnyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoAddresses => write!(f, "There is no address to connect to."),
            Self::InvalidCode => write!(f, "That is not a valid pairing code."),
            Self::AllTimedOut => write!(
                f,
                "The computer did not answer in time. Check that both are on the same Wi-Fi."
            ),
            Self::Refused => write!(
                f,
                "The computer refused the connection. Is Pleochroma running there with pairing open?"
            ),
            Self::Unreachable => write!(
                f,
                "The computer cannot be reached. Check that both are on the same network."
            ),
            Self::WrongCoordinator => write!(
                f,
                "A different computer answered than the one this code belongs to. Pairing was stopped."
            ),
            Self::CodeExpiredOrUsed => write!(
                f,
                "This code has expired or was already used. If you have not paired yet, someone \
                 else may have used it: ask on the computer."
            ),
            Self::Failed(msg) => write!(f, "Pairing failed: {msg}"),
        }
    }
}

impl std::error::Error for ClaimAnyError {}

impl ClaimAnyError {
    /// Higher wins when several addresses fail differently.
    const fn rank(&self) -> u8 {
        match self {
            Self::CodeExpiredOrUsed => 7,
            Self::WrongCoordinator => 6,
            Self::Failed(_) => 5,
            Self::Refused => 4,
            Self::Unreachable => 3,
            Self::AllTimedOut => 2,
            Self::InvalidCode | Self::NoAddresses => 1,
        }
    }

    /// The plain reason one failed attempt stands for.
    fn from_claim_error(err: &ClaimError) -> Self {
        use std::io::ErrorKind;
        match err {
            ClaimError::InvalidToken(_) => Self::InvalidCode,
            ClaimError::InvalidAddr(_) => Self::NoAddresses,
            ClaimError::Connect { source, .. } => match source.kind() {
                ErrorKind::TimedOut | ErrorKind::WouldBlock => Self::AllTimedOut,
                ErrorKind::ConnectionRefused => Self::Refused,
                _ => Self::Unreachable,
            },
            ClaimError::CaFingerprintMismatch { .. } => Self::WrongCoordinator,
            ClaimError::Refused => Self::CodeExpiredOrUsed,
            ClaimError::Handshake { .. }
            | ClaimError::Protocol(_)
            | ClaimError::UnexpectedResponse => Self::Failed(err.to_string()),
        }
    }
}

/// Redeems `token` against every address in `addrs` at once and keeps the first answer
/// whose TLS handshake passes the CA pin the token carries.
///
/// Each address gets [`CLAIM_ANY_CONNECT_TIMEOUT`] to connect. The first pinned handshake
/// wins and is the only connection that sends the secret; the others are dropped without
/// sending anything (a later address that finishes its handshake is dropped as well), so
/// the one-time code is never spent on a losing connection. Once the winner has the secret
/// out its outcome is final: a refusal from it is reported even if another address might
/// have answered differently.
///
/// Never logs or returns the token, the secret or the claimed key.
///
/// # Errors
///
/// See [`ClaimAnyError`]: one plain reason, the most informative among the addresses.
pub fn claim_any(token: &str, addrs: &[SocketAddr]) -> Result<ClaimAnyOutcome, ClaimAnyError> {
    if addrs.is_empty() {
        return Err(ClaimAnyError::NoAddresses);
    }
    let decoded = Arc::new(token::decode(token).map_err(|_| ClaimAnyError::InvalidCode)?);
    let gate = Arc::new(AtomicBool::new(false));
    let (tx, rx) = mpsc::channel::<Result<ClaimAnyOutcome, ClaimAnyError>>();

    for &addr in addrs {
        let decoded = Arc::clone(&decoded);
        let gate = Arc::clone(&gate);
        let tx = tx.clone();
        thread::spawn(move || {
            let label = addr.to_string();
            let result = TcpStream::connect_timeout(&addr, CLAIM_ANY_CONNECT_TIMEOUT)
                .map_err(|source| {
                    Attempt::Failed(ClaimError::Connect {
                        addr: label.clone(),
                        source,
                    })
                })
                .and_then(|tcp| {
                    tcp.set_read_timeout(Some(CLAIM_IO_TIMEOUT))
                        .and_then(|()| tcp.set_write_timeout(Some(CLAIM_IO_TIMEOUT)))
                        .map_err(|source| {
                            Attempt::Failed(ClaimError::Connect {
                                addr: label.clone(),
                                source,
                            })
                        })?;
                    let server_name = ServerName::IpAddress(addr.ip().into());
                    claim_on_socket(tcp, server_name, &decoded, &label, Some(&gate))
                });
            let message = match result {
                Ok(bundle) => Ok(ClaimAnyOutcome { bundle, addr }),
                // Dropped before the secret was sent: not a result.
                Err(Attempt::Superseded) => return,
                Err(Attempt::Failed(e)) => Err(ClaimAnyError::from_claim_error(&e)),
            };
            // The receiver is gone once a winner was returned.
            tx.send(message).ok();
        });
    }
    drop(tx);

    let mut worst: Option<ClaimAnyError> = None;
    // The channel closes when every attempt thread has finished, so this ends by itself.
    for message in rx {
        match message {
            Ok(outcome) => return Ok(outcome),
            Err(e) => {
                if worst.as_ref().is_none_or(|w| e.rank() > w.rank()) {
                    worst = Some(e);
                }
            }
        }
    }
    Err(worst.unwrap_or(ClaimAnyError::Unreachable))
}

/// Connects to `addr`, trying each resolved address with [`CLAIM_IO_TIMEOUT`], and arms
/// the same timeout as the socket's read and write deadline.
fn connect_with_timeout(addr: &str) -> std::io::Result<TcpStream> {
    let mut last_error = None;
    for candidate in addr.to_socket_addrs()? {
        match TcpStream::connect_timeout(&candidate, CLAIM_IO_TIMEOUT) {
            Ok(stream) => {
                stream.set_read_timeout(Some(CLAIM_IO_TIMEOUT))?;
                stream.set_write_timeout(Some(CLAIM_IO_TIMEOUT))?;
                return Ok(stream);
            }
            Err(e) => last_error = Some(e),
        }
    }
    Err(last_error.unwrap_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::AddrNotAvailable,
            "the address resolved to nothing",
        )
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operator_secret_round_trips_through_its_file() {
        let dir = std::env::temp_dir().join(format!(
            "pleochroma-net-operator-secret-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let secret: [u8; OPERATOR_SECRET_LEN] = std::array::from_fn(|i| u8::try_from(i).unwrap());
        write_operator_secret(&dir, &secret).unwrap();
        assert_eq!(*read_operator_secret(&dir).unwrap(), secret);

        std::fs::write(operator_secret_path(&dir), "not hex\n").unwrap();
        assert!(read_operator_secret(&dir).is_err());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn claim_rejects_an_invalid_token_before_ever_touching_the_network() {
        // No listener at this address: a connection-error result would mean `claim`
        // tried to connect before validating the token.
        let err = claim("not-a-token", "127.0.0.1:1").unwrap_err();
        assert!(matches!(err, ClaimError::InvalidToken(_)), "{err}");
    }

    #[test]
    fn claim_rejects_an_address_with_no_port() {
        let secret = [0u8; token::SECRET_LEN];
        let fp: crate::tls::Fingerprint = std::array::from_fn(|i| u8::try_from(i).unwrap());
        let token = token::encode(&secret, &fp);
        let err = claim(&token, "worker-with-no-port").unwrap_err();
        assert!(matches!(err, ClaimError::InvalidAddr(_)), "{err}");
    }

    /// A loopback address nothing listens on: bound, noted, closed again.
    fn dead_addr() -> SocketAddr {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap()
    }

    fn sample_token() -> String {
        let secret = [3u8; token::SECRET_LEN];
        let fp: crate::tls::Fingerprint = std::array::from_fn(|i| u8::try_from(i).unwrap());
        token::encode(&secret, &fp)
    }

    #[test]
    fn claim_any_with_no_addresses_or_a_bad_code_fails_before_connecting() {
        assert_eq!(
            claim_any(&sample_token(), &[]).unwrap_err(),
            ClaimAnyError::NoAddresses
        );
        assert_eq!(
            claim_any("not-a-token", &[dead_addr()]).unwrap_err(),
            ClaimAnyError::InvalidCode
        );
    }

    #[test]
    fn claim_any_with_only_dead_addresses_reports_refused_quickly() {
        let started = std::time::Instant::now();
        let err = claim_any(&sample_token(), &[dead_addr(), dead_addr()]).unwrap_err();
        assert_eq!(err, ClaimAnyError::Refused, "{err}");
        assert!(started.elapsed() < Duration::from_secs(4));
    }

    #[test]
    fn claim_any_errors_have_plain_words_and_the_most_informative_one_wins() {
        assert!(ClaimAnyError::CodeExpiredOrUsed.rank() > ClaimAnyError::WrongCoordinator.rank());
        assert!(ClaimAnyError::WrongCoordinator.rank() > ClaimAnyError::Refused.rank());
        assert!(ClaimAnyError::Refused.rank() > ClaimAnyError::AllTimedOut.rank());
        let used = ClaimAnyError::CodeExpiredOrUsed.to_string();
        assert!(used.contains("already used"), "{used}");
        assert!(used.contains("someone else"), "{used}");
        let timed_out = ClaimAnyError::AllTimedOut.to_string();
        assert!(timed_out.contains("did not answer"), "{timed_out}");
    }

    #[test]
    fn debug_output_never_shows_a_secret_or_the_key() {
        let request = EnrollRequest::Claim { secret: [0x5a; 32] };
        let text = format!("{request:?}");
        assert!(text.contains("<redacted>"), "{text}");
        assert!(!text.contains("90"), "{text}"); // 0x5a printed as a decimal byte
        let operator = format!(
            "{:?}",
            EnrollRequest::IssueAuthorized {
                name: "n".to_string(),
                operator_secret: [0x5a; 32],
            }
        );
        assert!(!operator.contains("90"), "{operator}");
        let issued = format!(
            "{:?}",
            EnrollResponse::Issued {
                token: "GW1-SECRET".to_string(),
                expires_in_secs: 180,
            }
        );
        assert!(!issued.contains("SECRET"), "{issued}");
        let claimed = format!(
            "{:?}",
            ClaimedBundle {
                ca_pem: "ca".to_string(),
                client_cert_pem: "cert".to_string(),
                client_key_pem: Zeroizing::new("PRIVATE-KEY".to_string()),
            }
        );
        assert!(!claimed.contains("PRIVATE-KEY"), "{claimed}");
        let bad = ClaimError::InvalidToken(token::TokenError::InvalidCharacter('Q'));
        assert!(!format!("{bad:?}").contains('Q'));
        assert!(!bad.to_string().contains('Q'));
    }

    #[test]
    fn claim_error_display_distinguishes_ca_mismatch_from_an_ordinary_handshake_failure() {
        let mismatch = ClaimError::CaFingerprintMismatch {
            addr: "worker.lan:7879".to_string(),
        }
        .to_string();
        let handshake = ClaimError::Handshake {
            addr: "worker.lan:7879".to_string(),
            source: std::io::Error::other("boom"),
        }
        .to_string();
        assert_ne!(mismatch, handshake);
        assert!(mismatch.contains("attacker in the middle"), "{mismatch}");
        assert!(!handshake.contains("attacker in the middle"), "{handshake}");
    }
}
