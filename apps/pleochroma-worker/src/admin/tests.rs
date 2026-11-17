//! Remote administration over a real coordinator: real certificates, real mutual TLS on
//! loopback, a real `serve` with `--allow-remote-admin` on or off.
//!
//! The gate and the permission table are also covered by their own unit tests
//! (`crate::serve::handshake`, `pleochroma_net::admin::permissions`); these tests prove the
//! pieces work together: the refusal with the setting off, a status, a subscription that
//! pushes, a refused never-remote request, the audit event, the monitor's refusals, revoking a
//! device that is connected, and making, claiming and using a monitor or admin code.

use super::restart;
use crate::{
    cli::{self, Command},
    pki,
    serve::{self, ServeHandle},
};
use pleochroma_net::{
    admin::{
        protocol::{AdminError, AdminPush, AdminReply, AdminRequest},
        status::{
            CoordinatorEvent, CoordinatorStatus, JobId, LogLevel, SettingsPatch, TokenId, WorkerId,
        },
    },
    client::ClientError,
    handshake::UNKNOWN_BUILD_HASH,
    messages::{
        Hello, PROTOCOL_VERSION, PeerRole, error_codes, read_control_message, write_message,
    },
    tls::{fingerprint, fingerprint_to_hex},
};
use rustls::pki_types::ServerName;
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream},
    path::{Path, PathBuf},
    sync::{Arc, PoisonError},
    time::{Duration, Instant},
};

type Client = rustls::StreamOwned<rustls::ClientConnection, TcpStream>;

fn unique_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "pleochroma-worker-admin-test-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A coordinator with TLS on loopback and its certificate folder.
struct Fixture {
    root: PathBuf,
    pki: PathBuf,
    handle: ServeHandle,
}

impl Fixture {
    /// Starts `serve` with remote administration `remote_admin` and the enrollment listener
    /// when `enroll`.
    fn start(remote_admin: bool, enroll: bool) -> Self {
        let root = unique_dir("fixture");
        let pki = root.join("pki");
        pki::init(&pki).unwrap();
        pki::issue_server(
            &pki,
            &["localhost".to_string()],
            &["127.0.0.1".parse().unwrap()],
        )
        .unwrap();
        // The viewers' list must exist for `serve` to start; nobody is on it.
        std::fs::write(pki.join(pki::VIEWER_ALLOWLIST_FILE), "").unwrap();

        let ca = pki.join(pki::CA_CERT_FILE).display().to_string();
        let cert = pki.join(pki::SERVER_CERT_FILE).display().to_string();
        let key = pki.join(pki::SERVER_KEY_FILE).display().to_string();
        let db = root.join("absent.sqlite").display().to_string();
        let mut words: Vec<String> = [
            "serve",
            "--ca",
            ca.as_str(),
            "--cert",
            cert.as_str(),
            "--key",
            key.as_str(),
            "--bind",
            "127.0.0.1:0",
            "--no-workers",
            "--db",
            db.as_str(),
        ]
        .iter()
        .map(|word| (*word).to_owned())
        .collect();
        if !enroll {
            words.push("--no-enroll".to_owned());
        }
        if remote_admin {
            words.push("--allow-remote-admin".to_owned());
        }
        let Ok(Command::Serve(args)) = cli::parse(&words) else {
            panic!("the serve arguments parse");
        };
        let handle = serve::start(&args).expect("the coordinator starts");
        Self { root, pki, handle }
    }

    fn addr(&self) -> SocketAddr {
        self.handle.viewer_addr
    }

    /// A bundle folder for a new device of `role` (also listed on the role's allowlist).
    fn device(&self, name: &str, role: PeerRole) -> PathBuf {
        let bundle = self.root.join(format!("bundle-{name}"));
        pki::issue_client_with_role(&self.pki, name, &bundle, role).unwrap();
        bundle
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.handle.shutdown(Duration::from_secs(10));
        std::fs::remove_dir_all(&self.root).ok();
    }
}

fn client_config(bundle: &Path) -> Arc<rustls::ClientConfig> {
    let ca = pleochroma_net::tls::load_ca(&bundle.join(pki::CA_CERT_FILE)).unwrap();
    let certs = pleochroma_net::tls::load_certs(&bundle.join(pki::CLIENT_CERT_FILE)).unwrap();
    let key = pleochroma_net::tls::load_private_key(&bundle.join(pki::CLIENT_KEY_FILE)).unwrap();
    pleochroma_net::tls::client_config(ca, certs, key).unwrap()
}

/// The fingerprint (hex) of the device whose bundle is in `bundle`.
fn fingerprint_of(bundle: &Path) -> String {
    let certs = pleochroma_net::tls::load_certs(&bundle.join(pki::CLIENT_CERT_FILE)).unwrap();
    fingerprint_to_hex(&fingerprint(&certs[0]))
}

fn hello_as(role: PeerRole) -> Hello {
    Hello {
        role,
        ..Hello::viewer(PROTOCOL_VERSION, UNKNOWN_BUILD_HASH, UNKNOWN_BUILD_HASH)
    }
}

/// Connects with the certificate in `bundle` and sends a `HELLO` of `role`.
fn admit(addr: SocketAddr, bundle: &Path, role: PeerRole) -> Result<Client, ClientError> {
    let tcp = TcpStream::connect(addr).unwrap();
    tcp.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let conn = rustls::ClientConnection::new(
        client_config(bundle),
        ServerName::from(IpAddr::V4(Ipv4Addr::LOCALHOST)),
    )
    .unwrap();
    let mut client = rustls::StreamOwned::new(conn, tcp);
    pleochroma_net::client::handshake_with_hello(&mut client, &hello_as(role))?;
    Ok(client)
}

/// One push, or `None` when none arrives within `wait`.
fn read_push(client: &mut Client, wait: Duration) -> Option<AdminPush> {
    client.sock.set_read_timeout(Some(wait)).unwrap();
    read_control_message::<_, AdminPush>(client).ok()
}

/// Sends `request` and returns its answer; every other push that arrives first goes to `seen`.
fn ask(
    client: &mut Client,
    request: &AdminRequest,
    seen: &mut Vec<AdminPush>,
) -> Result<AdminReply, AdminError> {
    let id = request.req_id();
    write_message(&mut *client, request).unwrap();
    loop {
        match read_push(client, Duration::from_secs(10)).expect("an answer to the request") {
            AdminPush::Response { req_id, result } if req_id == id => return result,
            other => seen.push(other),
        }
    }
}

/// The first push matching `what`: from `seen` or from the connection (up to ten seconds).
fn wait_for(
    client: &mut Client,
    seen: &mut Vec<AdminPush>,
    what: impl Fn(&AdminPush) -> bool,
) -> AdminPush {
    if let Some(index) = seen.iter().position(&what) {
        return seen.remove(index);
    }
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Some(push) = read_push(client, Duration::from_millis(500)) {
            if what(&push) {
                return push;
            }
            seen.push(push);
        }
    }
    panic!("the expected push did not arrive");
}

fn status_of(client: &mut Client, seen: &mut Vec<AdminPush>) -> CoordinatorStatus {
    match ask(client, &AdminRequest::Status { req_id: 1 }, seen) {
        Ok(AdminReply::Status(status)) => status,
        other => panic!("expected a status, got {other:?}"),
    }
}

// ---- The handshake with the setting off and on ----------------------------------------

#[test]
fn with_remote_administration_off_a_monitor_and_an_admin_are_refused_in_plain_words() {
    let fx = Fixture::start(false, false);
    for role in [PeerRole::Monitor, PeerRole::Admin] {
        let bundle = fx.device("remote", role);
        let Err(ClientError::Refused(refusal)) = admit(fx.addr(), &bundle, role) else {
            panic!("{role:?} must be refused while remote administration is off");
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

#[test]
fn with_remote_administration_on_the_hello_must_match_the_certificate_and_the_list() {
    let fx = Fixture::start(true, false);
    let monitor = fx.device("wall", PeerRole::Monitor);
    let admin = fx.device("ops", PeerRole::Admin);

    // A monitor certificate cannot say it is an admin.
    let Err(ClientError::Refused(refusal)) = admit(fx.addr(), &monitor, PeerRole::Admin) else {
        panic!("a monitor certificate with an admin HELLO must be refused");
    };
    assert!(
        refusal.message.contains("an admin certificate"),
        "{}",
        refusal.message
    );
    // An admin certificate cannot say it is a viewer: the viewer handler refuses it.
    assert!(admit(fx.addr(), &admin, PeerRole::Viewer).is_err());

    // Matching roles are admitted.
    assert!(admit(fx.addr(), &monitor, PeerRole::Monitor).is_ok());
    assert!(admit(fx.addr(), &admin, PeerRole::Admin).is_ok());

    // A certificate taken off its list is refused at the next connection.
    std::fs::write(pki::admin_allowlist_in(&fx.pki), "").unwrap();
    let Err(ClientError::Refused(refusal)) = admit(fx.addr(), &admin, PeerRole::Admin) else {
        panic!("an unlisted admin certificate must be refused");
    };
    assert!(
        refusal.message.contains("not on the coordinator's list"),
        "{}",
        refusal.message
    );
}

// ---- A session: status, subscribe, denial, audit ----------------------------------------

#[test]
fn an_admin_session_answers_status_pushes_after_subscribe_and_denies_never_remote_requests() {
    let _hook = restart::TEST_LOCK
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    restart::clear_hook();
    let fx = Fixture::start(true, false);
    let bundle = fx.device("ops", PeerRole::Admin);
    let mine = fingerprint_of(&bundle);
    let mut client = admit(fx.addr(), &bundle, PeerRole::Admin).expect("an admin is admitted");
    let mut seen = Vec::new();

    // Status: the coordinator says remote administration is on, and lists this device.
    let status = status_of(&mut client, &mut seen);
    assert!(status.remote_admin_enabled);
    assert!(
        status
            .devices
            .iter()
            .any(|d| d.fingerprint == mine && d.role == PeerRole::Admin && d.online),
        "{:?}",
        status.devices
    );

    // Subscribe: Done, then a status push (and nothing is pushed twice while it is unchanged).
    let subscribed = ask(
        &mut client,
        &AdminRequest::Subscribe {
            req_id: 2,
            status_ms: 1000,
            level: Some(LogLevel::Info),
        },
        &mut seen,
    );
    assert_eq!(subscribed, Ok(AdminReply::Done));
    let pushed = wait_for(&mut client, &mut seen, |p| {
        matches!(p, AdminPush::Status(_))
    });
    assert!(matches!(pushed, AdminPush::Status(s) if s.remote_admin_enabled));

    // A never-remote request is denied in plain words, and nothing changes.
    let denied = ask(
        &mut client,
        &AdminRequest::IssueToken {
            req_id: 3,
            role: PeerRole::Admin,
            name: "sneaky".to_owned(),
        },
        &mut seen,
    )
    .unwrap_err();
    assert!(denied.is_denied(), "{denied:?}");
    assert!(
        denied.message.contains("own computer"),
        "{}",
        denied.message
    );

    // Out-of-range settings are a validation error, not a denial.
    let invalid = ask(
        &mut client,
        &AdminRequest::ApplySettings {
            req_id: 4,
            patch: SettingsPatch {
                max_job_memory_mib: Some(1),
                ..SettingsPatch::default()
            },
        },
        &mut seen,
    )
    .unwrap_err();
    assert_eq!(invalid.code, error_codes::VALIDATION_FAILED);

    // One cannot remove the device one is connected with.
    let own = ask(
        &mut client,
        &AdminRequest::Revoke {
            req_id: 5,
            fingerprint: mine,
        },
        &mut seen,
    )
    .unwrap_err();
    assert_eq!(own.code, error_codes::VALIDATION_FAILED);

    // A headless coordinator has nobody to restart it, and says so.
    let restart_error =
        ask(&mut client, &AdminRequest::Restart { req_id: 6 }, &mut seen).unwrap_err();
    assert!(
        restart_error.message.contains("cannot restart itself"),
        "{}",
        restart_error.message
    );
    assert!(!restart_error.is_denied());

    // The job calls refuse in plain words on a coordinator that keeps no jobs here.
    let job = JobId([5; 16]);
    for request in [
        AdminRequest::CancelJob { req_id: 7, id: job },
        AdminRequest::DeleteJob { req_id: 8, id: job },
        AdminRequest::DisconnectWorker {
            req_id: 9,
            id: WorkerId(3),
        },
        AdminRequest::FetchResult { req_id: 10, job },
    ] {
        let error = ask(&mut client, &request, &mut seen).unwrap_err();
        assert!(!error.is_denied(), "{error:?}");
        assert!(!error.message.is_empty());
    }
}

#[test]
fn every_command_is_audited_for_the_event_stream_and_the_subscriber() {
    let fx = Fixture::start(true, false);
    let events = fx.handle.events();
    let bundle = fx.device("ops", PeerRole::Admin);
    let mut client = admit(fx.addr(), &bundle, PeerRole::Admin).expect("an admin is admitted");
    let mut seen = Vec::new();

    // A viewer on the list, so a rename has something to rename.
    let viewer = fx.root.join("bundle-laptop");
    pki::issue_client(&fx.pki, "laptop", &viewer).unwrap();
    let viewer_fp = fingerprint_of(&viewer);

    assert_eq!(
        ask(
            &mut client,
            &AdminRequest::Subscribe {
                req_id: 1,
                status_ms: 1000,
                level: None,
            },
            &mut seen
        ),
        Ok(AdminReply::Done)
    );
    assert_eq!(
        ask(
            &mut client,
            &AdminRequest::Rename {
                req_id: 2,
                fingerprint: viewer_fp,
                name: "Studio laptop".to_owned(),
            },
            &mut seen
        ),
        Ok(AdminReply::Done)
    );
    assert_eq!(
        ask(
            &mut client,
            &AdminRequest::CancelToken {
                req_id: 3,
                id: TokenId(999),
            },
            &mut seen
        ),
        Ok(AdminReply::Done)
    );

    // The coordinator's own event stream names who did it and what.
    let mut audited = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(10);
    while audited.len() < 2 && Instant::now() < deadline {
        if let Ok(CoordinatorEvent::AdminAction {
            who,
            action,
            target,
        }) = events.recv_timeout(Duration::from_millis(200))
        {
            audited.push((who, action, target));
        }
    }
    assert_eq!(audited.len(), 2, "{audited:?}");
    for (who, _, _) in &audited {
        assert!(who.starts_with("ops ("), "{who}");
    }
    assert_eq!(
        (audited[0].1.as_str(), audited[0].2.as_str()),
        ("rename device", "laptop to Studio laptop")
    );
    assert_eq!(audited[1].1, "cancel pairing code");

    // The subscribed administrator hears the same events.
    let pushed = wait_for(
        &mut client,
        &mut seen,
        |p| matches!(p, AdminPush::Event(CoordinatorEvent::AdminAction { action, .. }) if action == "cancel pairing code"),
    );
    assert!(matches!(pushed, AdminPush::Event(_)));
}

// ---- The monitor ------------------------------------------------------------------------

#[test]
fn a_monitor_may_read_and_every_command_is_denied() {
    let fx = Fixture::start(true, false);
    let bundle = fx.device("wall", PeerRole::Monitor);
    let mut client = admit(fx.addr(), &bundle, PeerRole::Monitor).expect("a monitor is admitted");
    let mut seen = Vec::new();

    let status = status_of(&mut client, &mut seen);
    assert!(status.remote_admin_enabled);
    assert_eq!(
        ask(
            &mut client,
            &AdminRequest::Subscribe {
                req_id: 2,
                status_ms: 1000,
                level: None,
            },
            &mut seen
        ),
        Ok(AdminReply::Done)
    );

    let fp = "ab".repeat(32);
    let job = JobId([1; 16]);
    let commands = [
        AdminRequest::IssueToken {
            req_id: 10,
            role: PeerRole::Viewer,
            name: "Phone".to_owned(),
        },
        AdminRequest::CancelToken {
            req_id: 11,
            id: TokenId(1),
        },
        AdminRequest::Revoke {
            req_id: 12,
            fingerprint: fp.clone(),
        },
        AdminRequest::Rename {
            req_id: 13,
            fingerprint: fp,
            name: "x".to_owned(),
        },
        AdminRequest::CancelJob {
            req_id: 14,
            id: job,
        },
        AdminRequest::DeleteJob {
            req_id: 15,
            id: job,
        },
        AdminRequest::DisconnectWorker {
            req_id: 16,
            id: WorkerId(1),
        },
        AdminRequest::ApplySettings {
            req_id: 17,
            patch: SettingsPatch {
                job_retention_days: Some(30),
                ..SettingsPatch::default()
            },
        },
        AdminRequest::Restart { req_id: 18 },
    ];
    for command in commands {
        let action = command.action();
        let error = ask(&mut client, &command, &mut seen).unwrap_err();
        assert!(error.is_denied(), "{action}: {error:?}");
        assert!(
            error.message.contains("monitor access"),
            "{}",
            error.message
        );
    }

    // Reading a picture is allowed; this coordinator has none, and says so without denying.
    let fetch = ask(
        &mut client,
        &AdminRequest::FetchResult { req_id: 19, job },
        &mut seen,
    )
    .unwrap_err();
    assert!(!fetch.is_denied(), "{fetch:?}");
}

// ---- Settings on a coordinator without a settings file -----------------------------------

/// A headless coordinator takes the live knobs and refuses everything else whole.
#[cfg(feature = "worker")]
#[test]
fn a_headless_coordinator_applies_only_live_settings_and_announces_the_change() {
    let _hook = restart::TEST_LOCK
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    restart::clear_hook();
    if crate::settings::settings_file().is_some() {
        // Another test of this process loaded a settings file: this one is about its absence.
        return;
    }
    let fx = Fixture::start(true, false);
    let bundle = fx.device("ops", PeerRole::Admin);
    let mut client = admit(fx.addr(), &bundle, PeerRole::Admin).expect("an admin is admitted");
    let mut seen = Vec::new();
    assert_eq!(
        ask(
            &mut client,
            &AdminRequest::Subscribe {
                req_id: 1,
                status_ms: 1000,
                level: None,
            },
            &mut seen
        ),
        Ok(AdminReply::Done)
    );

    // A mixed patch is refused as a whole, in plain words; the live knob is not half applied.
    let mixed = ask(
        &mut client,
        &AdminRequest::ApplySettings {
            req_id: 2,
            patch: SettingsPatch {
                whole_image_seconds: Some(33),
                job_retention_days: Some(30),
                ..SettingsPatch::default()
            },
        },
        &mut seen,
    )
    .unwrap_err();
    assert!(
        mixed.message.contains("without a settings file"),
        "{}",
        mixed.message
    );
    assert!(
        fx.handle
            .coordinator
            .as_ref()
            .is_some_and(|c| c.job_config().whole_image_secs != 33.0),
        "a refused patch changes nothing"
    );

    // Only live fields: applied, and the activity feed hears about it.
    let live = SettingsPatch {
        whole_image_seconds: Some(33),
        ..SettingsPatch::default()
    };
    assert_eq!(
        ask(
            &mut client,
            &AdminRequest::ApplySettings {
                req_id: 3,
                patch: live.clone(),
            },
            &mut seen
        ),
        Ok(AdminReply::Done)
    );
    let pushed = wait_for(&mut client, &mut seen, |push| {
        matches!(
            push,
            AdminPush::Event(CoordinatorEvent::SettingsChanged { .. })
        )
    });
    assert!(matches!(pushed, AdminPush::Event(_)));

    // The same value again changes nothing and is still a success, not a refusal.
    assert_eq!(
        ask(
            &mut client,
            &AdminRequest::ApplySettings {
                req_id: 4,
                patch: live,
            },
            &mut seen
        ),
        Ok(AdminReply::Done)
    );
}

// ---- Revoking a connected administrator ---------------------------------------------------

#[test]
fn removing_an_administrator_closes_its_open_session() {
    let fx = Fixture::start(true, false);
    let first = fx.device("first", PeerRole::Admin);
    let second = fx.device("second", PeerRole::Admin);
    let mut gone = admit(fx.addr(), &first, PeerRole::Admin).expect("the first is admitted");
    let mut remover = admit(fx.addr(), &second, PeerRole::Admin).expect("the second is admitted");
    let mut seen = Vec::new();
    let mut other = Vec::new();
    status_of(&mut gone, &mut other);

    let revoke_reply = ask(
        &mut remover,
        &AdminRequest::Revoke {
            req_id: 1,
            fingerprint: fingerprint_of(&first),
        },
        &mut seen,
    );
    assert_eq!(revoke_reply, Ok(AdminReply::Done));

    // The first administrator's connection ends (it leaves the coordinator's table of live
    // connections); a new one is refused.
    let first_fp = pleochroma_net::tls::fingerprint_from_hex(&fingerprint_of(&first)).unwrap();
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline
        && !fx
            .handle
            .connections()
            .for_fingerprint(&first_fp)
            .is_empty()
    {
        std::thread::sleep(Duration::from_millis(50));
    }
    assert!(
        fx.handle
            .connections()
            .for_fingerprint(&first_fp)
            .is_empty(),
        "the removed administrator's connection must close"
    );
    assert!(admit(fx.addr(), &first, PeerRole::Admin).is_err());
    drop(gone);
}

// ---- Codes for monitors and administrators --------------------------------------------------

#[test]
fn monitor_and_admin_codes_are_made_locally_only_while_remote_administration_is_on() {
    let off = Fixture::start(false, true);
    for role in [PeerRole::Monitor, PeerRole::Admin] {
        let error = off.handle.issue_token(role, "Wall").unwrap_err();
        assert!(error.contains("switched off"), "{error}");
    }
    // Everything else still works as it did.
    assert!(off.handle.issue_token(PeerRole::Viewer, "Phone").is_ok());
    assert!(
        !pleochroma_net::admin::status::CoordinatorStatus::minimal(
            "x",
            "0",
            45,
            pleochroma_net::admin::status::CoordinatorState::Running,
        )
        .remote_admin_enabled
    );

    let on = Fixture::start(true, true);
    for role in [PeerRole::Monitor, PeerRole::Admin] {
        let issued = on.handle.issue_token(role, "Wall").expect("a code is made");
        let link = pleochroma_net::pairing::PairingLink::parse(&issued.link).unwrap();
        assert_eq!(link.role, role);
        assert_eq!(
            link.enroll_port,
            on.handle.viewer_enroll_addr.unwrap().port()
        );
        assert_eq!(link.addrs, vec![on.addr()]);
        assert!(!format!("{issued:?}").contains(&issued.token));
        on.handle.cancel_token(issued.id);
    }
}

#[test]
fn a_claimed_admin_code_opens_a_session_and_a_claimed_monitor_code_only_reads() {
    let fx = Fixture::start(true, true);
    let enroll_addr = fx
        .handle
        .viewer_enroll_addr
        .expect("the enrollment listener runs");

    for role in [PeerRole::Admin, PeerRole::Monitor] {
        let issued = fx
            .handle
            .issue_token(role, "Remote")
            .expect("a code is made");
        let outcome = pleochroma_net::enroll::claim_any(&issued.token, &[enroll_addr])
            .expect("the code claims");
        let bundle = fx
            .root
            .join(format!("claimed-{}", pki::role::role_name(role)));
        crate::enroll_client::write_bundle(&bundle, &outcome.bundle).unwrap();

        // The Common Name carries the role prefix and the certificate is on the role's list.
        let certs = pleochroma_net::tls::load_certs(&bundle.join(pki::CLIENT_CERT_FILE)).unwrap();
        assert_eq!(pki::role_of_certificate(&certs[0]).unwrap(), role);
        let list = pleochroma_net::tls::Allowlist::load(&pki::allowlist_in(&fx.pki, role)).unwrap();
        assert!(list.contains(&fingerprint(&certs[0])));

        let mut client = admit(fx.addr(), &bundle, role).expect("the claimed device is admitted");
        let mut seen = Vec::new();
        let status = status_of(&mut client, &mut seen);
        assert!(status.devices.iter().any(|d| d.role == role && d.online));

        let command = ask(
            &mut client,
            &AdminRequest::CancelToken {
                req_id: 2,
                id: TokenId(1),
            },
            &mut seen,
        );
        match role {
            PeerRole::Monitor => assert!(command.unwrap_err().is_denied()),
            _ => assert_eq!(command, Ok(AdminReply::Done)),
        }
    }
}
