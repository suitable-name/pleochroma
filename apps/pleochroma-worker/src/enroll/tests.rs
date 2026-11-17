use super::{
    MAX_PENDING, TOKEN_TTL_SECS, connection::handle_enroll_connection, registry::EnrollRegistry,
};
use crate::pki;
use pleochroma_net::{
    enroll::{EnrollRequest, EnrollResponse},
    token,
};
use std::{
    io::{Cursor, Read, Write},
    net::SocketAddr,
    path::PathBuf,
    time::Duration,
};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

/// The operator secret every registry built by [`authorised_registry`] accepts.
const OPERATOR_SECRET: [u8; 32] = [7u8; 32];

/// A viewer registry that authorises `IssueAuthorized` requests carrying
/// [`OPERATOR_SECRET`].
fn authorised_registry() -> EnrollRegistry {
    EnrollRegistry::new().with_operator_secret(Zeroizing::new(OPERATOR_SECRET))
}

fn unique_temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "pleochroma-worker-enroll-test-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn loopback_peer() -> SocketAddr {
    "127.0.0.1:54321".parse().unwrap()
}

fn remote_peer() -> SocketAddr {
    "203.0.113.7:54321".parse().unwrap()
}

// ---- EnrollRegistry: pure in-memory, no network -----------------------------

#[test]
fn a_valid_token_claims_exactly_once() {
    let dir = unique_temp_dir("claim-once");
    pki::init(&dir).unwrap();
    let registry = EnrollRegistry::new();

    let (encoded, _ttl) = registry
        .issue_with_ttl(&dir, "laptop", Duration::from_secs(60))
        .unwrap();
    let decoded = token::decode(&encoded).unwrap();

    let first = registry.claim(&decoded.secret);
    assert!(first.is_some(), "a fresh token must claim successfully");
    assert_eq!(first.unwrap().name, "laptop");

    let second = registry.claim(&decoded.secret);
    assert!(second.is_none(), "the same secret must not claim twice");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_expired_token_fails_and_the_pending_record_is_gone() {
    let dir = unique_temp_dir("expired");
    pki::init(&dir).unwrap();
    let registry = EnrollRegistry::new();

    let (encoded, _ttl) = registry
        .issue_with_ttl(&dir, "laptop", Duration::from_millis(20))
        .unwrap();
    let decoded = token::decode(&encoded).unwrap();

    std::thread::sleep(Duration::from_millis(60));

    assert!(registry.claim(&decoded.secret).is_none());
    assert_eq!(
        registry.pending.lock().unwrap().len(),
        0,
        "the expired record must have been swept, not merely failed to match"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_wrong_token_fails_and_does_not_consume_the_pending_record() {
    let dir = unique_temp_dir("wrong-token");
    pki::init(&dir).unwrap();
    let registry = EnrollRegistry::new();

    let (encoded, _ttl) = registry
        .issue_with_ttl(&dir, "laptop", Duration::from_secs(60))
        .unwrap();
    let real = token::decode(&encoded).unwrap();

    let mut wrong_secret = *real.secret;
    wrong_secret[0] ^= 0xFF;
    assert!(registry.claim(&wrong_secret).is_none());
    assert_eq!(
        registry.pending.lock().unwrap().len(),
        1,
        "a wrong secret must not consume the real pending record"
    );

    // The real secret still claims successfully afterward.
    assert!(registry.claim(&real.secret).is_some());

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn issue_refuses_once_the_pending_cap_is_reached() {
    let dir = unique_temp_dir("cap");
    pki::init(&dir).unwrap();
    let registry = EnrollRegistry::new();

    for i in 0..MAX_PENDING {
        registry
            .issue_with_ttl(&dir, &format!("viewer-{i}"), Duration::from_secs(60))
            .unwrap();
    }
    let err = registry
        .issue_with_ttl(&dir, "one-too-many", Duration::from_secs(60))
        .unwrap_err()
        .to_string();
    assert!(err.contains("already pending"), "{err}");

    std::fs::remove_dir_all(&dir).ok();
}

/// `EnrollRegistry::issue` must sanitize `name` the same way
/// `pleochroma_net::tls::append_to_allowlist` sanitizes an allowlist label -- an
/// operator-supplied `--name`/enrollment `name` containing `\r`/`\n`/`#` must not be
/// able to inject a stray allowlist line once this enrollment is claimed.
#[test]
fn issue_sanitizes_a_name_containing_line_breaks_and_a_comment_marker() {
    let dir = unique_temp_dir("issue-sanitizes-name");
    pki::init(&dir).unwrap();
    let registry = EnrollRegistry::new();

    let evil_name =
        "laptop\r\ndeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeefdeadbeef  # not trusted";
    let (encoded, _ttl) = registry
        .issue_with_ttl(&dir, evil_name, Duration::from_secs(60))
        .unwrap();
    let decoded = token::decode(&encoded).unwrap();

    let claimed = registry
        .claim(&decoded.secret)
        .expect("a fresh token must claim successfully");
    assert!(!claimed.name.contains('\r'));
    assert!(!claimed.name.contains('\n'));
    assert!(!claimed.name.contains('#'));
    assert_eq!(
        claimed.name,
        pleochroma_net::tls::sanitize_allowlist_label(evil_name)
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn token_comparison_uses_constant_time_equality() {
    // Timing itself isn't observable in a unit test; this just pins down that
    // `EnrollRegistry::claim`'s `ct_eq` mechanism agrees with `==` on match/mismatch.
    let a = [7u8; 32];
    let b = [7u8; 32];
    let c = {
        let mut x = [7u8; 32];
        x[31] ^= 1;
        x
    };
    assert!(bool::from(a.ct_eq(&b)));
    assert!(!bool::from(a.ct_eq(&c)));
}

// ---- allowlist ordering -------------------------------------------------------

#[test]
fn allowlist_gains_the_fingerprint_only_after_a_successful_claim() {
    let dir = unique_temp_dir("allowlist-order");
    pki::init(&dir).unwrap();
    let allowlist_path = dir.join(pki::VIEWER_ALLOWLIST_FILE);
    let registry = EnrollRegistry::new();

    let (encoded, _ttl) = registry
        .issue_with_ttl(&dir, "laptop", Duration::from_secs(60))
        .unwrap();

    // Absent beforehand: issuing never touches the allowlist.
    assert!(
        !allowlist_path.exists(),
        "issuing a token must not create/append the allowlist"
    );

    // Drive the real connection handler, not just the registry in isolation.
    let mut input = Vec::new();
    let decoded = token::decode(&encoded).unwrap();
    pleochroma_net::messages::write_message(
        &mut input,
        &EnrollRequest::Claim {
            secret: *decoded.secret,
        },
    )
    .unwrap();
    let mut duplex = DuplexHalf::new(input);
    handle_enroll_connection(
        &mut duplex,
        &registry,
        &dir,
        Some(&allowlist_path),
        Some(loopback_peer()),
    )
    .unwrap();

    let mut out = Cursor::new(duplex.out);
    let response: EnrollResponse = pleochroma_net::messages::read_message(&mut out).unwrap();
    assert!(
        matches!(response, EnrollResponse::Claimed { .. }),
        "{response:?}"
    );

    let allowlist = pleochroma_net::tls::Allowlist::load(&allowlist_path).unwrap();
    assert_eq!(
        allowlist.len(),
        1,
        "exactly the claimed certificate, appended once"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_failed_claim_never_touches_the_allowlist() {
    let dir = unique_temp_dir("allowlist-no-touch");
    pki::init(&dir).unwrap();
    let allowlist_path = dir.join(pki::VIEWER_ALLOWLIST_FILE);
    let registry = EnrollRegistry::new();

    registry
        .issue_with_ttl(&dir, "laptop", Duration::from_secs(60))
        .unwrap();

    let mut input = Vec::new();
    pleochroma_net::messages::write_message(
        &mut input,
        &EnrollRequest::Claim {
            secret: [0u8; token::SECRET_LEN], // essentially certainly wrong
        },
    )
    .unwrap();
    let mut duplex = DuplexHalf::new(input);
    handle_enroll_connection(
        &mut duplex,
        &registry,
        &dir,
        Some(&allowlist_path),
        Some(loopback_peer()),
    )
    .unwrap();

    let mut out = Cursor::new(duplex.out);
    let response: EnrollResponse = pleochroma_net::messages::read_message(&mut out).unwrap();
    assert!(
        matches!(response, EnrollResponse::ClaimFailed),
        "{response:?}"
    );
    assert!(!allowlist_path.exists());

    std::fs::remove_dir_all(&dir).ok();
}

// ---- Bounded read ----------------------------------------------------------------

/// A malicious/corrupt length prefix declaring far more than
/// `connection::MAX_ENROLL_REQUEST_LEN` must be rejected from the 4-byte prefix alone,
/// before `handle_enroll_connection` ever attempts to allocate a buffer for it -- this
/// listener accepts a connection from anyone (no client certificate required), so an
/// unauthenticated peer must not be able to make it commit to a huge allocation.
#[test]
fn handle_enroll_connection_rejects_an_oversized_length_prefix_without_allocating() {
    let dir = unique_temp_dir("oversized-prefix");
    pki::init(&dir).unwrap();
    let registry = EnrollRegistry::new();

    let mut input = Vec::new();
    input.extend_from_slice(&0x1FFF_FFFFu32.to_le_bytes());
    // Deliberately no payload bytes follow: if the bound weren't enforced before
    // allocating, this would need to actually supply ~512 MiB (or the read would hang
    // waiting for bytes that never arrive) instead of failing immediately.
    let mut duplex = DuplexHalf::new(input);

    let err = handle_enroll_connection(&mut duplex, &registry, &dir, None, Some(loopback_peer()))
        .unwrap_err();
    assert!(err.contains("exceeds"), "{err}");
    assert!(
        duplex.out.is_empty(),
        "a request rejected before decoding gets no reply written"
    );

    std::fs::remove_dir_all(&dir).ok();
}

// ---- Issue is loopback-only -----------------------------------------------------

#[test]
fn issue_is_refused_from_a_non_loopback_peer() {
    let dir = unique_temp_dir("issue-remote-refused");
    pki::init(&dir).unwrap();
    let registry = authorised_registry();

    let mut input = Vec::new();
    pleochroma_net::messages::write_message(
        &mut input,
        &EnrollRequest::IssueAuthorized {
            name: "intruder".to_string(),
            operator_secret: OPERATOR_SECRET,
        },
    )
    .unwrap();
    let mut duplex = DuplexHalf::new(input);
    handle_enroll_connection(&mut duplex, &registry, &dir, None, Some(remote_peer())).unwrap();

    let mut out = Cursor::new(duplex.out);
    let response: EnrollResponse = pleochroma_net::messages::read_message(&mut out).unwrap();
    assert!(
        matches!(response, EnrollResponse::IssueRefused { .. }),
        "{response:?}"
    );
    assert_eq!(registry.pending.lock().unwrap().len(), 0);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn issue_succeeds_from_a_loopback_peer() {
    let dir = unique_temp_dir("issue-loopback-ok");
    pki::init(&dir).unwrap();
    let registry = authorised_registry();

    let mut input = Vec::new();
    pleochroma_net::messages::write_message(
        &mut input,
        &EnrollRequest::IssueAuthorized {
            name: "laptop".to_string(),
            operator_secret: OPERATOR_SECRET,
        },
    )
    .unwrap();
    let mut duplex = DuplexHalf::new(input);
    handle_enroll_connection(&mut duplex, &registry, &dir, None, Some(loopback_peer())).unwrap();

    let mut out = Cursor::new(duplex.out);
    let response: EnrollResponse = pleochroma_net::messages::read_message(&mut out).unwrap();
    let EnrollResponse::Issued {
        token,
        expires_in_secs,
    } = response
    else {
        panic!("expected Issued, got {response:?}");
    };
    assert!(token.starts_with(pleochroma_net::token::TOKEN_PREFIX));
    assert_eq!(expires_in_secs, TOKEN_TTL_SECS);

    std::fs::remove_dir_all(&dir).ok();
}

/// Sends one `IssueAuthorized { name }` carrying [`OPERATOR_SECRET`] from loopback
/// through `registry`'s connection handler.
fn issue_via_handler(
    registry: &EnrollRegistry,
    dir: &std::path::Path,
    name: &str,
) -> EnrollResponse {
    let mut input = Vec::new();
    pleochroma_net::messages::write_message(
        &mut input,
        &EnrollRequest::IssueAuthorized {
            name: name.to_string(),
            operator_secret: OPERATOR_SECRET,
        },
    )
    .unwrap();
    let mut duplex = DuplexHalf::new(input);
    handle_enroll_connection(&mut duplex, registry, dir, None, Some(loopback_peer())).unwrap();
    pleochroma_net::messages::read_message(&mut Cursor::new(duplex.out)).unwrap()
}

/// A worker enrollment registry mints `worker:<name>` certificates and appends them to
/// the worker allowlist; each listener refuses an `Issue` for the other role.
#[test]
fn a_worker_listener_mints_worker_certificates_and_refuses_viewer_issues() {
    use pleochroma_net::messages::PeerRole;
    let dir = unique_temp_dir("worker-role");
    pki::init(&dir).unwrap();
    let workers = EnrollRegistry::for_role(PeerRole::Worker)
        .with_operator_secret(Zeroizing::new(OPERATOR_SECRET));
    let viewers = authorised_registry();

    assert!(matches!(
        issue_via_handler(&workers, &dir, "laptop"),
        EnrollResponse::IssueRefused { reason } if reason.contains("--role worker")
    ));
    assert!(matches!(
        issue_via_handler(&viewers, &dir, "worker:box"),
        EnrollResponse::IssueRefused { reason } if reason.contains("WORKER enrollment")
    ));
    let EnrollResponse::Issued { token, .. } = issue_via_handler(&workers, &dir, "worker:box")
    else {
        panic!("a worker issue on the worker listener must succeed");
    };

    let decoded = token::decode(&token).unwrap();
    let mut input = Vec::new();
    pleochroma_net::messages::write_message(
        &mut input,
        &EnrollRequest::Claim {
            secret: *decoded.secret,
        },
    )
    .unwrap();
    let worker_allowlist = dir.join(pki::WORKER_ALLOWLIST_FILE);
    let mut duplex = DuplexHalf::new(input);
    handle_enroll_connection(
        &mut duplex,
        &workers,
        &dir,
        Some(&worker_allowlist),
        Some(loopback_peer()),
    )
    .unwrap();
    let response: EnrollResponse =
        pleochroma_net::messages::read_message(&mut Cursor::new(duplex.out)).unwrap();
    let EnrollResponse::Claimed {
        client_cert_pem, ..
    } = response
    else {
        panic!("expected Claimed, got {response:?}");
    };
    let der = rustls_pemfile_cert(&client_cert_pem);
    assert_eq!(pki::role_of_certificate(&der).unwrap(), PeerRole::Worker);
    let allowlist = pleochroma_net::tls::Allowlist::load(&worker_allowlist).unwrap();
    assert!(allowlist.contains(&pleochroma_net::tls::fingerprint(&der.into())));

    std::fs::remove_dir_all(&dir).ok();
}

/// The DER bytes of the one certificate in `pem`.
fn rustls_pemfile_cert(pem: &str) -> Vec<u8> {
    let path = unique_temp_dir("pem").join("cert.pem");
    std::fs::write(&path, pem).unwrap();
    pleochroma_net::tls::load_certs(&path).unwrap()[0].to_vec()
}

// ---- A claim connection cannot issue a render request --------------------------

// Only compilable on a `worker` build -- `RenderRequest`/`SceneState` don't exist
// without it, and a library-only build has no render protocol to reach anyway.
#[cfg(feature = "worker")]
#[test]
fn a_claim_connection_cannot_issue_a_render_request() {
    use pleochroma::{
        geometry::cuts::StandardGemCuts,
        optics::{materials::GemMaterial, raytracer::LightingPreset},
    };
    use pleochroma_net::{
        SceneState,
        messages::{ClientMessage, RenderRequest, StreamConfig, TransferMode},
    };

    let dir = unique_temp_dir("no-render-on-enroll");
    pki::init(&dir).unwrap();
    let registry = EnrollRegistry::new();

    // A real render-protocol RenderRequest -- what the render listener would accept.
    let scene = SceneState {
        engine: pleochroma::renderer::engine::EngineSpec::default(),
        width: 4,
        height: 4,
        yaw: 0.4,
        camera_roll: 0.0,
        stone_shadow: false,
        tray_reflection: 0.0,
        hdr_orientation: pleochroma::renderer::env_map::HdrOrientation::default(),
        estimator: pleochroma::optics::raytracer::Estimator::Branching,
        pitch: 0.3,
        distance: 3.0,
        light_yaw: 0.85,
        light_pitch: 0.95,
        exposure: 1.0,
        max_bounces: 4,
        lighting_preset: LightingPreset::Daylight,
        material: GemMaterial::diamond(),
        planes: StandardGemCuts::standard_round_brilliant(),
        girdle_frosted: false,
        backdrop: 0.0,
        environment: pleochroma_net::scene::SceneEnvironment::Studio,
        surface_glare: 1.0,
        tools: Vec::new(),
        fluorescence: pleochroma::optics::fluorescence::Fluorescence::default(),
        head_shadow_deg: 16.0,
        frosted_planes: Vec::new(),
        effects: pleochroma_net::scene::effects::SceneEffects::default(),
        camera_pan: [0.0, 0.0],
        stone_pose: [0.0, 0.0, 0.0],
    };
    let request = RenderRequest {
        intent: pleochroma_net::messages::RequestIntent::Batch,
        request_id: 1,
        scene,
        first_sample: 0,
        samples: 4,
        stream: StreamConfig {
            transfer_mode: TransferMode::FinalOnly,
            cadence_ms: 0,
            preview: None,
        },
    };
    let mut input = Vec::new();
    pleochroma_net::messages::write_message(
        &mut input,
        &ClientMessage::RenderRequest(Box::new(request)),
    )
    .unwrap();

    let mut duplex = DuplexHalf::new(input);
    // `handle_enroll_connection` has no reference to `stream_emit` and writes only one
    // `EnrollResponse`, so there's no code path to rendering. postcard's enum encoding
    // is a bare variant index, not self-describing, so the RenderRequest bytes might
    // decode as a garbage `EnrollRequest` variant instead of failing outright -- either
    // way, whatever gets written back must decode as an `EnrollResponse` and nothing else.
    let _ = handle_enroll_connection(&mut duplex, &registry, &dir, None, Some(loopback_peer()));
    if !duplex.out.is_empty() {
        let mut out = Cursor::new(duplex.out);
        let response: EnrollResponse = pleochroma_net::messages::read_message(&mut out)
            .expect("the only thing this listener ever writes is an EnrollResponse");
        assert!(
            matches!(
                response,
                EnrollResponse::Issued { .. }
                    | EnrollResponse::IssueRefused { .. }
                    | EnrollResponse::Claimed { .. }
                    | EnrollResponse::ClaimFailed
            ),
            "{response:?}"
        );
        assert_eq!(
            out.position(),
            out.get_ref().len() as u64,
            "nothing beyond one EnrollResponse may ever be written back on this listener"
        );
    }

    std::fs::remove_dir_all(&dir).ok();
}

// ---- Per-token role, remove, claim events, stoppable accept loop ---------------------

/// Runs one `Claim { secret }` through the connection handler and returns the reply.
fn claim_via_handler(
    registry: &EnrollRegistry,
    dir: &std::path::Path,
    allowlist: Option<&std::path::Path>,
    token_text: &str,
    peer: SocketAddr,
) -> EnrollResponse {
    let decoded = token::decode(token_text).unwrap();
    let mut input = Vec::new();
    pleochroma_net::messages::write_message(
        &mut input,
        &EnrollRequest::Claim {
            secret: *decoded.secret,
        },
    )
    .unwrap();
    let mut duplex = DuplexHalf::new(input);
    handle_enroll_connection(&mut duplex, registry, dir, allowlist, Some(peer)).unwrap();
    pleochroma_net::messages::read_message(&mut Cursor::new(duplex.out)).unwrap()
}

#[test]
fn every_token_carries_the_role_it_was_issued_with() {
    use pleochroma_net::messages::PeerRole;
    let dir = unique_temp_dir("per-token-role");
    pki::init(&dir).unwrap();
    let registry = EnrollRegistry::new(); // a viewer registry

    let viewer = registry
        .issue_for_role(&dir, "phone", PeerRole::Viewer)
        .unwrap();
    let worker = registry
        .issue_for_role(&dir, "box", PeerRole::Worker)
        .unwrap();
    assert_ne!(viewer.id, worker.id);
    assert_eq!(worker.role, PeerRole::Worker);

    let worker_claim = registry
        .claim(&token::decode(&worker.token).unwrap().secret)
        .unwrap();
    assert_eq!(worker_claim.role, PeerRole::Worker);
    assert_eq!(worker_claim.id, worker.id);
    assert!(worker_claim.name.starts_with(pki::WORKER_CN_PREFIX));
    let viewer_claim = registry
        .claim(&token::decode(&viewer.token).unwrap().secret)
        .unwrap();
    assert_eq!(viewer_claim.role, PeerRole::Viewer);
    assert_eq!(viewer_claim.name, "phone");

    // Monitor and admin certificates enroll only once remote administration registered
    // their allowlists (see `monitor_and_admin_codes_need_their_registered_allowlists`).
    for role in [PeerRole::Monitor, PeerRole::Admin] {
        let err = registry.issue_for_role(&dir, "remote", role).unwrap_err();
        assert!(err.to_string().contains("switched on"), "{err}");
    }
    assert_eq!(registry.pending.lock().unwrap().len(), 0);

    std::fs::remove_dir_all(&dir).ok();
}

/// Remote administration on: the viewer registry issues monitor and admin codes, a claim
/// gives the certificate the role prefix and lists it in that role's allowlist, and the
/// claim event carries the role. Off (no registered allowlist): nothing is issued.
#[test]
fn monitor_and_admin_codes_need_their_registered_allowlists() {
    use pleochroma_net::messages::PeerRole;
    let dir = unique_temp_dir("remote-admin-codes");
    pki::init(&dir).unwrap();
    let registry = authorised_registry();
    let events = registry.subscribe();

    // Off: refused both ways, nothing left pending.
    assert!(matches!(
        issue_via_handler(&registry, &dir, "monitor:wall"),
        EnrollResponse::IssueRefused { reason } if reason.contains("remote administration")
    ));
    assert!(matches!(
        issue_via_handler(&registry, &dir, "admin:ops"),
        EnrollResponse::IssueRefused { reason } if reason.contains("--allow-remote-admin")
    ));
    assert!(
        registry
            .issue_for_role(&dir, "wall", PeerRole::Monitor)
            .is_err()
    );
    assert_eq!(registry.pending.lock().unwrap().len(), 0);

    // On: the two allowlists are registered.
    let monitors = pki::monitor_allowlist_in(&dir);
    let admins = pki::admin_allowlist_in(&dir);
    registry.register_allowlist(PeerRole::Monitor, monitors.clone());
    registry.register_allowlist(PeerRole::Admin, admins.clone());

    for (name, role, list) in [
        ("monitor:wall", PeerRole::Monitor, &monitors),
        ("admin:ops", PeerRole::Admin, &admins),
    ] {
        let EnrollResponse::Issued { token, .. } = issue_via_handler(&registry, &dir, name) else {
            panic!("{name} must be issued while remote administration is on");
        };
        let response = claim_via_handler(&registry, &dir, None, &token, remote_peer());
        let EnrollResponse::Claimed {
            client_cert_pem, ..
        } = response
        else {
            panic!("expected Claimed, got {response:?}");
        };
        let der = rustls_pemfile_cert(&client_cert_pem);
        assert_eq!(pki::role_of_certificate(&der).unwrap(), role);
        let cn = pki::role::subject_common_name(&der).unwrap().unwrap();
        assert!(cn.starts_with(name.split(':').next().unwrap()), "{cn}");
        let allowlist = pleochroma_net::tls::Allowlist::load(list).unwrap();
        assert!(allowlist.contains(&pleochroma_net::tls::fingerprint(&der.into())));
        let claim = events.try_recv().expect("a claim event");
        assert_eq!(claim.role, role);
        assert_eq!(claim.name, cn);
    }
    // The viewer allowlist is not touched by these claims.
    assert!(!pki::viewer_allowlist_in(&dir).exists());

    // A worker listener never issues them, even with the allowlists registered.
    let workers = EnrollRegistry::for_role(PeerRole::Worker)
        .with_operator_secret(Zeroizing::new(OPERATOR_SECRET));
    workers.register_allowlist(PeerRole::Monitor, monitors);
    assert!(matches!(
        issue_via_handler(&workers, &dir, "monitor:wall"),
        EnrollResponse::IssueRefused { reason } if reason.contains("VIEWER enrollment")
    ));

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn remove_cancels_a_pending_token_once() {
    use pleochroma_net::messages::PeerRole;
    let dir = unique_temp_dir("remove");
    pki::init(&dir).unwrap();
    let registry = EnrollRegistry::new();

    let keep = registry
        .issue_for_role(&dir, "keep", PeerRole::Viewer)
        .unwrap();
    let cancel = registry
        .issue_for_role(&dir, "cancel", PeerRole::Viewer)
        .unwrap();

    assert!(registry.remove(cancel.id));
    assert!(!registry.remove(cancel.id), "already removed");
    assert!(
        registry
            .claim(&token::decode(&cancel.token).unwrap().secret)
            .is_none(),
        "a cancelled token must not claim"
    );
    assert!(
        registry
            .claim(&token::decode(&keep.token).unwrap().secret)
            .is_some(),
        "the other token is untouched"
    );
    assert!(!registry.remove(keep.id), "claimed tokens are gone too");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_claim_emits_role_fingerprint_and_peer_to_every_subscriber() {
    use pleochroma_net::messages::PeerRole;
    let dir = unique_temp_dir("claim-event");
    pki::init(&dir).unwrap();
    let worker_allowlist = dir.join(pki::WORKER_ALLOWLIST_FILE);
    let viewer_allowlist = dir.join(pki::VIEWER_ALLOWLIST_FILE);
    let registry = EnrollRegistry::new();
    registry.register_allowlist(PeerRole::Worker, worker_allowlist.clone());
    let first = registry.subscribe();
    let second = registry.subscribe();

    let issued = registry
        .issue_for_role(&dir, "box", PeerRole::Worker)
        .unwrap();
    let before = first.try_recv();
    assert!(before.is_err(), "issuing alone sends no event");

    let peer: SocketAddr = "192.168.1.50:41234".parse().unwrap();
    let reply = claim_via_handler(
        &registry,
        &dir,
        Some(&viewer_allowlist),
        &issued.token,
        peer,
    );
    assert!(matches!(reply, EnrollResponse::Claimed { .. }), "{reply:?}");

    for receiver in [&first, &second] {
        let event = receiver.try_recv().expect("one claim event per subscriber");
        assert_eq!(event.id, issued.id);
        assert_eq!(event.role, PeerRole::Worker);
        assert_eq!(event.peer, peer);
        assert!(event.name.starts_with(pki::WORKER_CN_PREFIX));
        let allowlist = pleochroma_net::tls::Allowlist::load(&worker_allowlist).unwrap();
        assert!(allowlist.contains(&event.fingerprint));
        assert!(receiver.try_recv().is_err(), "exactly one event");
    }
    assert!(!viewer_allowlist.exists(), "the viewer list is not touched");

    // The event holds no secret.
    let shown = format!("{:?}", first.try_recv());
    assert!(!shown.contains("GW1"), "{shown}");
    let issued_debug = format!("{issued:?}");
    assert!(!issued_debug.contains(&issued.token), "{issued_debug}");

    // A dropped receiver is forgotten without disturbing the others.
    drop(second);
    let again = registry
        .issue_for_role(&dir, "box2", PeerRole::Worker)
        .unwrap();
    let reply = claim_via_handler(&registry, &dir, None, &again.token, peer);
    assert!(matches!(reply, EnrollResponse::Claimed { .. }), "{reply:?}");
    assert_eq!(first.try_recv().unwrap().id, again.id);

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_claim_of_another_role_without_a_registered_allowlist_is_refused() {
    use pleochroma_net::messages::PeerRole;
    let dir = unique_temp_dir("claim-no-allowlist");
    pki::init(&dir).unwrap();
    let registry = EnrollRegistry::new();
    let events = registry.subscribe();
    let issued = registry
        .issue_for_role(&dir, "box", PeerRole::Worker)
        .unwrap();

    let reply = claim_via_handler(&registry, &dir, None, &issued.token, loopback_peer());
    assert!(matches!(reply, EnrollResponse::ClaimFailed), "{reply:?}");
    assert!(events.try_recv().is_err(), "a refused claim sends no event");

    std::fs::remove_dir_all(&dir).ok();
}

/// A real CA, server certificate and enrollment config on an ephemeral loopback port.
fn real_config(label: &str) -> (PathBuf, super::EnrollConfig) {
    let dir = unique_temp_dir(label);
    pki::init(&dir).unwrap();
    pki::issue_server(&dir, &[], &["127.0.0.1".parse().unwrap()]).unwrap();
    let config = super::EnrollConfig::build(
        "127.0.0.1:0".parse().unwrap(),
        &dir.join(pki::CA_CERT_FILE),
        &dir.join(pki::SERVER_CERT_FILE),
        &dir.join(pki::SERVER_KEY_FILE),
        Some(dir.join(pki::VIEWER_ALLOWLIST_FILE)),
        pleochroma_net::messages::PeerRole::Viewer,
    )
    .unwrap();
    (dir, config)
}

#[test]
fn the_accept_loop_exits_when_its_stop_flag_is_set_and_woken() {
    use crate::control::shutdown::ListenerStop;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    let (dir, config) = real_config("stop-loop");
    let bound = super::bind_enroll_listener(config).unwrap();
    let addr = bound.local_addr();
    let flag = Arc::new(AtomicBool::new(false));
    bound.spawn(ListenerStop {
        flag: Arc::clone(&flag),
        wake_addr: addr,
    });

    // Accepting: a plain TCP connection is taken.
    std::net::TcpStream::connect(addr).expect("the listener must accept before the stop");

    // Stop: set the flag, then wake the blocked accept with one connection.
    flag.store(true, Ordering::SeqCst);
    std::net::TcpStream::connect(addr).ok();

    // The loop returned and dropped its listener, so the port soon refuses connections.
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        if std::net::TcpStream::connect(addr).is_err() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the listener kept accepting after the stop flag"
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    std::fs::remove_dir_all(&dir).ok();
}

/// A `Read + Write` double over two independent in-memory buffers -- reimplemented
/// rather than shared with `crate::serve::tests::DuplexHalf` (private to that module),
/// and this module needs none of its `TimeoutRead` behavior.
struct DuplexHalf {
    in_: Cursor<Vec<u8>>,
    out: Vec<u8>,
}

impl DuplexHalf {
    fn new(input: Vec<u8>) -> Self {
        Self {
            in_: Cursor::new(input),
            out: Vec::new(),
        }
    }
}

impl Read for DuplexHalf {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.in_.read(buf)
    }
}

impl Write for DuplexHalf {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.out.write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
