//! The two CLI-facing halves of token-based enrollment that run OUTSIDE the `serve`
//! process.
//!
//! `cert issue-token` (run by the operator, against a running `serve`) and `cert claim`
//! (run on the machine being enrolled). The design and lifecycle are in the `crate::enroll`
//! module doc.
//!
//! The security-critical pinned-CA TLS handshake and wire exchange live in
//! [`pleochroma_net::enroll::claim`], shared with `apps/pleochroma-cut`. [`claim`] below is a
//! thin wrapper: it converts the [`pleochroma_net::enroll::ClaimError`] into this CLI's
//! `--token`/`--addr`-flavored message and writes the returned bundle to `--out`.

mod link;

use crate::{
    cli::{CertClaimArgs, CertIssueTokenArgs},
    pki,
};
use pleochroma_net::enroll::{ClaimError, EnrollRequest, EnrollResponse};
use rustls::{ClientConfig, ClientConnection, StreamOwned, pki_types::ServerName};
use std::{
    net::{TcpStream, ToSocketAddrs},
    path::Path,
    sync::Arc,
    time::Duration,
};
use zeroize::Zeroize;

/// Connect timeout and per-read/per-write socket timeout of `cert issue-token`: the
/// exchange is one small request and reply, so a listener that stalls longer is dead.
const ISSUE_IO_TIMEOUT: Duration = Duration::from_secs(10);

/// Cap on the reply `cert issue-token` will read. An `Issued` or `IssueRefused` reply is
/// a few hundred bytes; nothing legitimate approaches this.
const MAX_ISSUE_REPLY_LEN: u32 = 64 * 1024;

/// Splits `addr` (`host:port`) into its host part, for building a TLS [`ServerName`],
/// via [`pleochroma_net::tls::host_for_server_name`] (which strips the brackets of an
/// IPv6 literal; the result is further validated by `ServerName::try_from`).
///
/// # Errors
///
/// A human-readable message if `addr` has no `:` at all.
fn host_of<'a>(addr: &'a str, flag: &str) -> Result<&'a str, String> {
    pleochroma_net::tls::host_for_server_name(addr)
        .ok_or_else(|| format!("{flag} {addr:?} must be host:port"))
}

/// Turns a [`ClaimError`] into this CLI's own `--token`/`--addr`-flavored message.
/// `pleochroma_net::enroll::claim` knows nothing of CLI flag names (it's shared with
/// `apps/pleochroma-cut`), so that framing is added here.
fn claim_error_to_string(err: ClaimError, addr: &str) -> String {
    match err {
        // `ClaimError`'s own text, which never echoes a character of the token.
        err @ ClaimError::InvalidToken(_) => format!("--token: {err}"),
        ClaimError::InvalidAddr(msg) => format!("--addr {msg}"),
        ClaimError::Connect { source, .. } => format!("could not connect to {addr}: {source}"),
        ClaimError::Handshake { source, .. } => format!(
            "TLS handshake with {addr} failed: {source} -- if this names the pinned CA, double-check the \
             token was transcribed correctly (see `pleochroma-worker cert claim --help`)",
        ),
        ClaimError::CaFingerprintMismatch { .. } => format!(
            "TLS handshake with {addr} failed: the server did not present a certificate chain containing the \
             CA this enrollment token commits to -- refusing to trust it (this is either the wrong worker, or \
             an attacker in the middle) -- if this names the pinned CA, double-check the token was transcribed \
             correctly (see `pleochroma-worker cert claim --help`)",
        ),
        ClaimError::Protocol(msg) => msg,
        ClaimError::Refused => {
            "claim failed: the enrollment token was invalid, expired, or already used".to_string()
        }
        ClaimError::UnexpectedResponse => {
            format!("unexpected response from the enrollment listener at {addr}")
        }
    }
}

/// `pleochroma-worker cert claim --token <token> --addr <host:port> --out <bundle-dir>`.
///
/// Redeems `args.token` against `args.addr` via [`pleochroma_net::enroll::claim`], which
/// verifies the enrollment listener against the CA fingerprint the token carries before
/// ever sending the token's secret, then writes the same three-file bundle (`ca.pem`,
/// `client.pem`, `client.key`) `cert issue-client` would have, ACL-restricting the key
/// file the same way ([`pleochroma_net::tls::write_private_key_pem`]).
///
/// # Errors
///
/// A human-readable message if the token doesn't decode, `args.addr` isn't `host:port`,
/// the connection or TLS handshake fails (including the pinned-CA verification failing),
/// the wire exchange fails, the server reports the claim failed (wrong/expired/
/// already-used token), or the bundle can't be written to `args.out`.
pub fn claim(args: &CertClaimArgs) -> Result<(), String> {
    let bundle = pleochroma_net::enroll::claim(&args.token, &args.addr)
        .map_err(|e| claim_error_to_string(e, &args.addr))?;
    write_bundle(&args.out, &bundle)?;
    // `bundle.client_key_pem` is `Zeroizing<String>`, overwritten in place on drop.

    tracing::info!(
        "pleochroma-worker cert claim: wrote bundle to {} ({}, {}, {})",
        args.out.display(),
        pki::CA_CERT_FILE,
        pki::CLIENT_CERT_FILE,
        pki::CLIENT_KEY_FILE,
    );
    Ok(())
}

/// Writes a claimed bundle to `out` as `ca.pem`, `client.pem` and `client.key`.
///
/// The key is ACL-restricted via [`pleochroma_net::tls::write_private_key_pem`]; the
/// layout is what `cert issue-client` writes, shared by `cert claim` and `join --token`.
///
/// # Errors
///
/// A human-readable message if `out` or a file in it can't be written.
pub fn write_bundle(
    out: &Path,
    bundle: &pleochroma_net::enroll::ClaimedBundle,
) -> Result<(), String> {
    std::fs::create_dir_all(out).map_err(|e| format!("could not create {}: {e}", out.display()))?;
    let out_ca_path = out.join(pki::CA_CERT_FILE);
    let out_cert_path = out.join(pki::CLIENT_CERT_FILE);
    let out_key_path = out.join(pki::CLIENT_KEY_FILE);
    std::fs::write(&out_ca_path, &bundle.ca_pem)
        .map_err(|e| format!("could not write {}: {e}", out_ca_path.display()))?;
    std::fs::write(&out_cert_path, &bundle.client_cert_pem)
        .map_err(|e| format!("could not write {}: {e}", out_cert_path.display()))?;
    pleochroma_net::tls::write_private_key_pem(&out_key_path, &bundle.client_key_pem)
}

/// `pleochroma-worker cert issue-token --ca <ca.pem> --admin-addr <host:port> --name <label>`.
///
/// Connects to a running `serve` process's enrollment listener at `args.admin_addr`,
/// verifying it with ordinary (non-pinned) TLS against `args.ca`. The operator already has
/// that file, so no pinned CA is needed here as it is for [`claim`]. Asks the listener to
/// mint a token for `args.name` and prints the result.
///
/// The request is authorised by the operator secret in `issue.secret` next to `args.ca`
/// (created by `serve` on first start, readable only by its owner): this command must run
/// as a user that can read the PKI directory.
///
/// The token is printed via `println!`, deliberately not `tracing::info!`: it is a
/// bearer secret, and a tracing-formatted line is exactly what ends up captured in a
/// log file or forwarded to an aggregator.
///
/// # Errors
///
/// A human-readable message if `args.ca` or the `issue.secret` beside it can't be loaded,
/// `args.admin_addr` isn't `host:port`, the connection or TLS handshake fails, the wire
/// exchange fails, or the server refuses to issue a token (not a loopback connection, or too
/// many enrollments already pending; see `crate::enroll::EnrollRegistry::issue`).
pub fn run_issue_token(args: &CertIssueTokenArgs) -> Result<(), String> {
    // Everything the link needs is worked out before a code is minted, so a missing LAN
    // address never leaves a spent code behind.
    let enroll_port = link::enroll_port(&args.admin_addr)?;
    let lan: Vec<std::net::Ipv4Addr> = if args.addresses.is_empty() {
        pleochroma_net::netinfo::lan_ipv4()
            .into_iter()
            .map(|lan| lan.addr)
            .collect()
    } else {
        Vec::new()
    };
    let addrs = link::link_addresses(
        &args.addresses,
        &lan,
        link::coordinator_port(enroll_port, args.role),
    )?;

    let response = issue_token_over_tls(&args.ca, &args.admin_addr, &issue_name(args))?;

    match_issue_response(response, args, addrs, enroll_port)
}

/// The plain explanation of a refused monitor or admin code, or `None` for other roles.
fn remote_admin_hint(role: pleochroma_net::messages::PeerRole) -> Option<&'static str> {
    use pleochroma_net::messages::PeerRole;
    matches!(role, PeerRole::Monitor | PeerRole::Admin).then_some(
        "monitor and admin codes need the coordinator to run with `serve --allow-remote-admin` \
         (in the window: Settings, Remote administration)",
    )
}

/// The `Issue` name for `args`: the bare `--name` for a viewer, `worker:<name>` for
/// `--role worker`. The enrollment listener reads the requested role from this prefix (see
/// the `crate::enroll` module doc).
fn issue_name(args: &CertIssueTokenArgs) -> String {
    match args.role {
        pleochroma_net::messages::PeerRole::Viewer => args.name.clone(),
        pleochroma_net::messages::PeerRole::Worker => {
            format!("{}{}", pki::WORKER_CN_PREFIX, args.name)
        }
        // The coordinator only honors these with `serve --allow-remote-admin`; the prefix
        // tells it the requested role either way.
        pleochroma_net::messages::PeerRole::Monitor => {
            format!("{}{}", pki::MONITOR_CN_PREFIX, args.name)
        }
        pleochroma_net::messages::PeerRole::Admin => {
            format!("{}{}", pki::ADMIN_CN_PREFIX, args.name)
        }
    }
}

/// The actual TLS connect-and-exchange behind [`run_issue_token`], factored out so this
/// crate's tests can drive a real loopback round trip and inspect the raw
/// [`EnrollResponse`] (`run_issue_token` only ever prints it).
///
/// # Errors
///
/// A human-readable message if `ca_path` or the `issue.secret` in its directory can't be
/// loaded, `admin_addr` isn't `host:port`, or the connection, TLS handshake, or wire
/// exchange fails.
pub(crate) fn issue_token_over_tls(
    ca_path: &Path,
    admin_addr: &str,
    name: &str,
) -> Result<EnrollResponse, String> {
    let ca = pleochroma_net::tls::load_ca(ca_path)
        .map_err(|e| format!("--ca {}: {e}", ca_path.display()))?;
    let secret = pleochroma_net::enroll::read_operator_secret(&pki::role::pki_dir_of(ca_path))?;
    let client_config = ClientConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
        .with_root_certificates(ca)
        .with_no_client_auth();

    let host = host_of(admin_addr, "--admin-addr")?;
    let server_name = ServerName::try_from(host.to_string()).map_err(|e| {
        format!("--admin-addr {admin_addr:?}: invalid host for TLS verification: {e}")
    })?;

    let tcp = connect_with_timeout(admin_addr)
        .map_err(|e| format!("could not connect to {admin_addr}: {e}"))?;
    let conn = ClientConnection::new(Arc::new(client_config), server_name)
        .map_err(|e| format!("failed to start a TLS session: {e}"))?;
    let mut stream = StreamOwned::new(conn, tcp);
    stream
        .conn
        .complete_io(&mut stream.sock)
        .map_err(|e| format!("TLS handshake with {admin_addr} failed: {e}"))?;

    // The request's copy of the operator secret is overwritten as soon as it is sent.
    let mut request = EnrollRequest::IssueAuthorized {
        name: name.to_string(),
        operator_secret: *secret,
    };
    let write_result = pleochroma_net::messages::write_message(&mut stream, &request);
    if let EnrollRequest::IssueAuthorized {
        operator_secret, ..
    } = &mut request
    {
        operator_secret.zeroize();
    }
    write_result.map_err(|e| e.to_string())?;
    pleochroma_net::messages::read_message_bounded(&mut stream, MAX_ISSUE_REPLY_LEN)
        .map_err(|e| e.to_string())
}

/// Connects to `addr`, trying each resolved address with [`ISSUE_IO_TIMEOUT`], and arms
/// the same timeout as the socket's read and write deadline so a stalled listener fails
/// the command instead of hanging it.
fn connect_with_timeout(addr: &str) -> std::io::Result<TcpStream> {
    let mut last_error = None;
    for candidate in addr.to_socket_addrs()? {
        match TcpStream::connect_timeout(&candidate, ISSUE_IO_TIMEOUT) {
            Ok(stream) => {
                stream.set_read_timeout(Some(ISSUE_IO_TIMEOUT))?;
                stream.set_write_timeout(Some(ISSUE_IO_TIMEOUT))?;
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

fn match_issue_response(
    response: EnrollResponse,
    args: &CertIssueTokenArgs,
    addrs: Vec<std::net::SocketAddr>,
    enroll_port: u16,
) -> Result<(), String> {
    let name = args.name.as_str();
    match response {
        EnrollResponse::Issued {
            token,
            expires_in_secs,
        } => {
            // The log line names the label and lifetime only, never the code or the link.
            tracing::info!(
                "pleochroma-worker cert issue-token: issued a token for {name:?}, valid {expires_in_secs}s"
            );
            println!(
                "Enrollment token for {name:?} (valid {expires_in_secs}s), for one use only:\n"
            );
            println!("  {token}\n");
            println!(
                "Read or send this to whoever is enrolling -- it carries a one-time secret AND this coordinator's \
                 CA fingerprint, so `cert claim` (or `join --token`) can verify it's really talking to this \
                 coordinator before it ever sends anything back. It works once and expires in {expires_in_secs} \
                 seconds."
            );
            print_link_and_qr(args, addrs, enroll_port, &token)
        }
        EnrollResponse::IssueRefused { reason } => Err(remote_admin_hint(args.role).map_or_else(
            || format!("token issue refused: {reason}"),
            |hint| format!("token issue refused: {reason} -- {hint}"),
        )),
        other => Err(format!(
            "unexpected response from the enrollment listener: {other:?}"
        )),
    }
}

/// Prints the pairing link and, unless `--no-qr`, its QR code, to stdout only: both carry
/// the code, so neither goes through `tracing`.
fn print_link_and_qr(
    args: &CertIssueTokenArgs,
    addrs: Vec<std::net::SocketAddr>,
    enroll_port: u16,
    token: &str,
) -> Result<(), String> {
    let text = link::build_link(
        args.role,
        &link::coordinator_name(),
        addrs,
        enroll_port,
        token,
    )?;
    println!("\nPairing link (open it in Pleochroma on the device being paired):\n");
    println!("  {text}\n");
    if args.no_qr {
        return Ok(());
    }
    // A terminal gets black-on-white so any theme scans; a pipe or a file gets dark glyphs on
    // a light page; a console without escape sequences (the classic Windows console, which is
    // dark) gets the light modules, so the code is not inverted.
    // The drawing happens inside the call, because a Windows console gets its escape-sequence
    // mode back as soon as the call returns.
    link::with_terminal_escapes(|style| {
        let Some(qr) = link::half_block_qr(&text, style) else {
            println!("(The link is too long for a QR code; use the link or the code above.)");
            let _ = std::io::Write::flush(&mut std::io::stdout());
            return;
        };
        println!("Or scan this QR code with the Pleochroma app:\n");
        print!("{qr}");
        println!();
        if style == link::QrStyle::LightOnDark {
            println!(
                "(Drawn for a dark console. If it does not scan, use the link or the code above.)"
            );
        }
        let _ = std::io::Write::flush(&mut std::io::stdout());
    });
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_of_splits_on_the_last_colon() {
        assert_eq!(host_of("127.0.0.1:7879", "--addr").unwrap(), "127.0.0.1");
        assert_eq!(host_of("worker.lan:7879", "--addr").unwrap(), "worker.lan");
    }

    #[test]
    fn host_of_strips_the_brackets_of_an_ipv6_literal() {
        assert_eq!(host_of("[::1]:7879", "--addr").unwrap(), "::1");
    }

    #[test]
    fn host_of_rejects_a_missing_port() {
        let err = host_of("worker.lan", "--addr").unwrap_err();
        assert!(err.contains("--addr"), "{err}");
    }

    // ---- Real end-to-end round trips over a real loopback TLS connection ----------
    //
    // Unlike `crate::enroll`'s tests (in-memory duplex, never exercises TLS), these spin
    // up a real `crate::enroll::spawn_enroll_listener` on an ephemeral port and drive
    // `issue_token_over_tls`/`claim` against it, to prove the `PinnedCaVerifier`
    // handshake actually works, not just that it type-checks.

    fn unique_temp_dir(label: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pleochroma-worker-enroll-client-test-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Sets up a real CA + server certificate and a real (ephemeral-port) enrollment
    /// listener. Returns the pki dir, the listener's bound address, and the allowlist
    /// path it was configured with.
    fn start_real_enroll_listener() -> (std::path::PathBuf, std::net::SocketAddr, std::path::PathBuf)
    {
        let dir = unique_temp_dir("listener");
        crate::pki::init(&dir).unwrap();
        crate::pki::issue_server(&dir, &[], &["127.0.0.1".parse().unwrap()]).unwrap();

        let ca_path = dir.join(crate::pki::CA_CERT_FILE);
        let cert_path = dir.join(crate::pki::SERVER_CERT_FILE);
        let key_path = dir.join(crate::pki::SERVER_KEY_FILE);
        let allowlist_path = dir.join(crate::pki::VIEWER_ALLOWLIST_FILE);

        let config = crate::enroll::EnrollConfig::build(
            "127.0.0.1:0".parse().unwrap(), // ephemeral port
            &ca_path,
            &cert_path,
            &key_path,
            Some(allowlist_path.clone()),
            pleochroma_net::messages::PeerRole::Viewer,
        )
        .unwrap();
        // The port is only known after the bind, so the stop's wake address is set then.
        // These tests never stop the listener: the flag stays clear.
        let bound = crate::enroll::bind_enroll_listener(config).unwrap();
        let addr = bound.local_addr();
        bound.spawn(crate::control::shutdown::ListenerStop {
            flag: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            wake_addr: addr,
        });

        (dir, addr, allowlist_path)
    }

    /// A loopback address nothing listens on.
    fn dead_addr() -> std::net::SocketAddr {
        std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
    }

    /// Issues one viewer token over the real connection.
    fn issue_real_token(dir: &std::path::Path, addr: std::net::SocketAddr) -> String {
        let response =
            issue_token_over_tls(&dir.join(pki::CA_CERT_FILE), &addr.to_string(), "any-addr")
                .unwrap();
        let EnrollResponse::Issued { token, .. } = response else {
            panic!("expected Issued, got {response:?}");
        };
        token
    }

    #[test]
    fn claim_any_skips_a_dead_address_and_claims_on_the_live_one() {
        let (dir, live, allowlist_path) = start_real_enroll_listener();
        let token = issue_real_token(&dir, live);

        let started = std::time::Instant::now();
        let outcome =
            pleochroma_net::enroll::claim_any(&token, &[dead_addr(), live, dead_addr()]).unwrap();
        assert_eq!(outcome.addr, live);
        assert!(outcome.bundle.client_cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(started.elapsed() < Duration::from_secs(4));
        let allowlist = pleochroma_net::tls::Allowlist::load(&allowlist_path).unwrap();
        assert_eq!(allowlist.len(), 1);

        // The code is single use: the same code again says so in plain words.
        let err = pleochroma_net::enroll::claim_any(&token, &[live]).unwrap_err();
        assert_eq!(
            err,
            pleochroma_net::enroll::ClaimAnyError::CodeExpiredOrUsed
        );
        assert!(err.to_string().contains("already used"), "{err}");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn claim_any_with_two_addresses_of_the_same_coordinator_spends_the_code_once() {
        let (dir, live, _allowlist) = start_real_enroll_listener();
        let token = issue_real_token(&dir, live);

        let outcome = pleochroma_net::enroll::claim_any(&token, &[live, live]).unwrap();
        assert_eq!(outcome.addr, live);

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn claim_any_with_only_dead_addresses_fails_in_plain_words_under_four_seconds() {
        let (dir, live, _allowlist) = start_real_enroll_listener();
        let token = issue_real_token(&dir, live);

        let started = std::time::Instant::now();
        let err =
            pleochroma_net::enroll::claim_any(&token, &[dead_addr(), dead_addr()]).unwrap_err();
        assert!(started.elapsed() < Duration::from_secs(4));
        assert_eq!(err, pleochroma_net::enroll::ClaimAnyError::Refused);
        assert!(err.to_string().contains("refused"), "{err}");

        // The code was not spent by the failed attempt.
        assert!(pleochroma_net::enroll::claim_any(&token, &[live]).is_ok());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn claim_any_against_the_wrong_coordinator_says_so_and_keeps_the_code() {
        let (dir, live, _allowlist) = start_real_enroll_listener();
        let token = issue_real_token(&dir, live);
        let real = pleochroma_net::token::decode(&token).unwrap();
        let wrong_fingerprint: pleochroma_net::tls::Fingerprint = std::array::from_fn(|i| {
            u8::try_from(i)
                .expect("a fingerprint index fits u8")
                .wrapping_add(1)
        });
        let bad_token = pleochroma_net::token::encode(&real.secret, &wrong_fingerprint);

        let err = pleochroma_net::enroll::claim_any(&bad_token, &[dead_addr(), live]).unwrap_err();
        assert_eq!(err, pleochroma_net::enroll::ClaimAnyError::WrongCoordinator);
        assert!(pleochroma_net::enroll::claim_any(&token, &[live]).is_ok());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn issue_then_claim_end_to_end_over_a_real_tls_connection() {
        let (dir, addr, allowlist_path) = start_real_enroll_listener();
        let ca_path = dir.join(crate::pki::CA_CERT_FILE);

        assert!(
            !allowlist_path.exists(),
            "issuing must not touch the allowlist before any claim"
        );

        let response = issue_token_over_tls(&ca_path, &addr.to_string(), "real-viewer").unwrap();
        let EnrollResponse::Issued {
            token: token_str, ..
        } = response
        else {
            panic!("expected Issued, got {response:?}");
        };

        let out = unique_temp_dir("bundle");
        let claim_args = CertClaimArgs {
            token: token_str,
            addr: addr.to_string(),
            out: out.clone(),
        };
        claim(&claim_args).unwrap();

        assert!(out.join(pki::CA_CERT_FILE).exists());
        assert!(out.join(pki::CLIENT_CERT_FILE).exists());
        assert!(out.join(pki::CLIENT_KEY_FILE).exists());

        let allowlist = pleochroma_net::tls::Allowlist::load(&allowlist_path).unwrap();
        assert_eq!(
            allowlist.len(),
            1,
            "the claimed certificate's fingerprint, and only it"
        );

        // Single-use, over the real connection too.
        let err = claim(&claim_args).unwrap_err();
        assert!(err.contains("invalid"), "{err}");

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&out).ok();
    }

    #[test]
    fn claim_refuses_a_token_whose_ca_fingerprint_does_not_match_the_real_server() {
        let (dir, addr, allowlist_path) = start_real_enroll_listener();
        let ca_path = dir.join(crate::pki::CA_CERT_FILE);

        let response = issue_token_over_tls(&ca_path, &addr.to_string(), "real-viewer").unwrap();
        let EnrollResponse::Issued {
            token: token_str, ..
        } = response
        else {
            panic!("expected Issued, got {response:?}");
        };
        let real = pleochroma_net::token::decode(&token_str).unwrap();

        // Same secret, wrong CA fingerprint -- simulates an attacker or a transcription
        // error. `claim` must refuse to send the secret at all.
        let wrong_fingerprint: pleochroma_net::tls::Fingerprint = std::array::from_fn(|i| {
            u8::try_from(i)
                .expect("a fingerprint index fits u8")
                .wrapping_add(1)
        });
        assert_ne!(wrong_fingerprint, real.ca_fingerprint);
        let bad_token = pleochroma_net::token::encode(&real.secret, &wrong_fingerprint);

        let out = unique_temp_dir("bundle-wrong-ca");
        let claim_args = CertClaimArgs {
            token: bad_token,
            addr: addr.to_string(),
            out: out.clone(),
        };
        let err = claim(&claim_args).unwrap_err();
        assert!(
            err.to_lowercase().contains("handshake") || err.to_lowercase().contains("pinned"),
            "{err}"
        );

        // The real token still works afterward: the failed attempt never touched the registry.
        let claim_args_real = CertClaimArgs {
            token: token_str,
            addr: addr.to_string(),
            out: out.clone(),
        };
        claim(&claim_args_real).unwrap();
        let allowlist = pleochroma_net::tls::Allowlist::load(&allowlist_path).unwrap();
        assert_eq!(allowlist.len(), 1);

        std::fs::remove_dir_all(&dir).ok();
        std::fs::remove_dir_all(&out).ok();
    }
}
