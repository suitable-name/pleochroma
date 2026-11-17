// Real throwaway CAs and certificates, real TLS 1.3 handshakes pumped in memory: nothing is
// mocked at the `rustls` layer.

use super::*;
use crate::serve::server_config_with_resolver;
use pleochroma_net::tls::{COORDINATOR_TLS_NAME, fingerprint};
use rustls::pki_types::ServerName;
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};

fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "pleochroma-worker-certs-test-{label}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn lan(addr: [u8; 4], virtual_adapter: bool) -> LanAddress {
    LanAddress {
        addr: Ipv4Addr::from(addr),
        interface: if virtual_adapter {
            "vEthernet (WSL)".into()
        } else {
            "Ethernet".into()
        },
        default_route: !virtual_adapter,
        virtual_adapter,
    }
}

// The tests below work on the default layout (`server.pem` and `server.key` next to the CA);
// these two keep their calls short. The custom-file tests use `ServerFiles` directly.
fn issue_server_certificate(
    dir: &Path,
    extra: &[String],
    lan: &[LanAddress],
) -> Result<(), PkiError> {
    super::issue_server_certificate(&ServerFiles::in_dir(dir), extra, lan)
}

fn migrate_server_certificate(
    dir: &Path,
    extra: &[String],
    lan: &[LanAddress],
) -> Result<bool, PkiError> {
    super::migrate_server_certificate(&ServerFiles::in_dir(dir), extra, lan)
}

fn server_der(pki_dir: &Path) -> Vec<u8> {
    tls::load_certs(&pki_dir.join(pki::SERVER_CERT_FILE)).unwrap()[0]
        .as_ref()
        .to_vec()
}

/// A CA in a fresh directory plus a client bundle in another.
fn pki_with_client(label: &str) -> (PathBuf, PathBuf) {
    let pki_dir = temp_dir(&format!("{label}-pki"));
    let bundle = temp_dir(&format!("{label}-bundle"));
    pki::init(&pki_dir).unwrap();
    pki::issue_client(&pki_dir, "test-client", &bundle).unwrap();
    (pki_dir, bundle)
}

fn server_config(pki_dir: &Path, resolver: &Arc<SwappableCert>) -> Arc<rustls::ServerConfig> {
    let ca = tls::load_ca(&pki_dir.join(pki::CA_CERT_FILE)).unwrap();
    server_config_with_resolver(ca, Arc::clone(resolver)).unwrap()
}

fn plain_client(pki_dir: &Path, bundle: &Path) -> Arc<rustls::ClientConfig> {
    let ca = tls::load_ca(&pki_dir.join(pki::CA_CERT_FILE)).unwrap();
    let chain = tls::load_certs(&bundle.join(pki::CLIENT_CERT_FILE)).unwrap();
    let key = tls::load_private_key(&bundle.join(pki::CLIENT_KEY_FILE)).unwrap();
    tls::client_config(ca, chain, key).unwrap()
}

fn fixed_name_client(pki_dir: &Path, bundle: &Path) -> Arc<rustls::ClientConfig> {
    let ca = tls::load_ca(&pki_dir.join(pki::CA_CERT_FILE)).unwrap();
    let chain = tls::load_certs(&bundle.join(pki::CLIENT_CERT_FILE)).unwrap();
    let key = tls::load_private_key(&bundle.join(pki::CLIENT_KEY_FILE)).unwrap();
    tls::client_config_fixed_name(ca, chain, key).unwrap()
}

/// Runs one TLS 1.3 handshake between a client dialing `dialed` and a server using
/// `server`, in memory. Returns the fingerprint of the certificate the server presented.
fn handshake(
    client: Arc<rustls::ClientConfig>,
    server: &Arc<rustls::ServerConfig>,
    dialed: ServerName<'static>,
) -> Result<[u8; 32], rustls::Error> {
    let mut client = rustls::ClientConnection::new(client, dialed).unwrap();
    let mut server = rustls::ServerConnection::new(Arc::clone(server)).unwrap();
    for _ in 0..20 {
        let mut to_server = Vec::new();
        while client.wants_write() {
            client.write_tls(&mut to_server).unwrap();
        }
        let mut reader = &to_server[..];
        while !reader.is_empty() {
            server.read_tls(&mut reader).unwrap();
            server.process_new_packets()?;
        }
        let mut to_client = Vec::new();
        while server.wants_write() {
            server.write_tls(&mut to_client).unwrap();
        }
        let mut reader = &to_client[..];
        while !reader.is_empty() {
            client.read_tls(&mut reader).unwrap();
            client.process_new_packets()?;
        }
        if !client.is_handshaking() && !server.is_handshaking() {
            let leaf = client.peer_certificates().unwrap()[0].clone();
            return Ok(fingerprint(&leaf));
        }
    }
    Err(rustls::Error::General("handshake did not finish".into()))
}

fn dial_ip(addr: [u8; 4]) -> ServerName<'static> {
    ServerName::from(IpAddr::V4(Ipv4Addr::from(addr)))
}

#[test]
fn certificate_names_carry_the_fixed_name_the_lan_ips_and_localhost() {
    let lan_list = [
        lan([192, 168, 1, 20], false),
        lan([172, 20, 0, 1], true),
        lan([10, 0, 0, 7], false),
    ];
    let (hosts, ips) = certificate_names(&lan_list, &["vpn.example".into(), "  ".into()]);
    assert_eq!(hosts[0], COORDINATOR_TLS_NAME);
    assert!(hosts.contains(&"localhost".to_string()));
    assert!(hosts.contains(&"vpn.example".to_string()));
    assert_eq!(hosts.len(), 3, "blank extra names are dropped: {hosts:?}");
    assert_eq!(ips[0], IpAddr::V4(Ipv4Addr::LOCALHOST));
    assert!(ips.contains(&IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20))));
    assert!(ips.contains(&IpAddr::V4(Ipv4Addr::new(10, 0, 0, 7))));
    assert!(
        !ips.contains(&IpAddr::V4(Ipv4Addr::new(172, 20, 0, 1))),
        "a virtual adapter is skipped while a physical one exists: {ips:?}"
    );
}

#[test]
fn virtual_adapters_are_used_when_nothing_else_exists() {
    let (_, ips) = certificate_names(&[lan([172, 20, 0, 1], true)], &[]);
    assert!(ips.contains(&IpAddr::V4(Ipv4Addr::new(172, 20, 0, 1))));
}

#[test]
fn an_extra_name_that_is_an_ip_address_joins_the_ip_list() {
    let (hosts, ips) = certificate_names(&[], &["100.64.1.9".into()]);
    assert!(!hosts.contains(&"100.64.1.9".to_string()));
    assert!(ips.contains(&"100.64.1.9".parse::<IpAddr>().unwrap()));
}

#[test]
fn an_issued_certificate_lists_the_fixed_name_the_ips_and_localhost() {
    let dir = temp_dir("sans");
    pki::init(&dir).unwrap();
    issue_server_certificate(&dir, &[], &[lan([10, 20, 30, 40], false)]).unwrap();

    let der = server_der(&dir);
    assert!(certificate_has_fixed_name(&der));
    assert!(contains_bytes(&der, b"localhost"));
    assert!(certificate_lists_addresses(
        &der,
        &[Ipv4Addr::new(10, 20, 30, 40), Ipv4Addr::LOCALHOST]
    ));
    assert!(!certificate_lists_addresses(
        &der,
        &[Ipv4Addr::new(10, 20, 30, 41)]
    ));

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn reissuing_keeps_the_ca_and_changes_the_server_key() {
    let dir = temp_dir("reissue");
    pki::init(&dir).unwrap();
    issue_server_certificate(&dir, &[], &[lan([10, 20, 30, 40], false)]).unwrap();
    let ca_before = std::fs::read(dir.join(pki::CA_CERT_FILE)).unwrap();
    let ca_key_before = std::fs::read(dir.join(pki::CA_KEY_FILE)).unwrap();
    let first = server_der(&dir);

    issue_server_certificate(&dir, &[], &[lan([10, 20, 30, 41], false)]).unwrap();

    assert_eq!(
        std::fs::read(dir.join(pki::CA_CERT_FILE)).unwrap(),
        ca_before
    );
    assert_eq!(
        std::fs::read(dir.join(pki::CA_KEY_FILE)).unwrap(),
        ca_key_before
    );
    let second = server_der(&dir);
    assert_ne!(first, second);
    assert!(certificate_lists_addresses(
        &second,
        &[Ipv4Addr::new(10, 20, 30, 41)]
    ));

    std::fs::remove_dir_all(&dir).ok();
}

/// Writes a server certificate the way 0.11.1 did: signed by the CA in `dir`, no fixed name,
/// listing exactly `names` (host names and addresses, like `--host` and `--ip`).
fn write_old_certificate(dir: &Path, cert_path: &Path, key_path: &Path, names: &[&str]) {
    let issuer = pki::load_ca(dir).unwrap();
    let names: Vec<String> = names.iter().map(|n| (*n).to_string()).collect();
    let mut params = rcgen::CertificateParams::new(names).unwrap();
    params.extended_key_usages = vec![rcgen::ExtendedKeyUsagePurpose::ServerAuth];
    let key_pair = rcgen::KeyPair::generate().unwrap();
    let cert = params.signed_by(&key_pair, &issuer).unwrap();
    std::fs::write(cert_path, cert.pem()).unwrap();
    std::fs::write(key_path, key_pair.serialize_pem()).unwrap();
}

/// [`write_old_certificate`] in the default layout, listing `worker.lan`.
fn issue_old_server_certificate(dir: &Path) {
    write_old_certificate(
        dir,
        &dir.join(pki::SERVER_CERT_FILE),
        &dir.join(pki::SERVER_KEY_FILE),
        &["worker.lan"],
    );
}

/// What the certificate in `path` lists: DNS names and addresses.
fn names_in(path: &Path) -> (Vec<String>, Vec<IpAddr>) {
    let chain = tls::load_certs(path).unwrap();
    pki::role::subject_alt_names(chain[0].as_ref()).unwrap()
}

/// A server config over the certificate in `cert`/`key`, trusting the CA of `dir`.
fn server_over(dir: &Path, cert: &Path, key: &Path) -> Arc<rustls::ServerConfig> {
    let resolver = Arc::new(SwappableCert::new(load_certified_key(cert, key).unwrap()));
    server_config(dir, &resolver)
}

fn dial_name(name: &str) -> ServerName<'static> {
    ServerName::try_from(name.to_string()).unwrap()
}

#[test]
fn migration_detects_an_old_certificate_and_reissues_it_once() {
    let dir = temp_dir("migrate");
    pki::init(&dir).unwrap();
    assert!(
        !migrate_server_certificate(&dir, &[], &[]).unwrap(),
        "no server.pem is not a migration"
    );

    issue_old_server_certificate(&dir);
    assert!(!certificate_has_fixed_name(&server_der(&dir)));
    let ca_before = std::fs::read(dir.join(pki::CA_CERT_FILE)).unwrap();

    assert!(migrate_server_certificate(&dir, &[], &[lan([10, 20, 30, 40], false)]).unwrap());
    assert!(certificate_has_fixed_name(&server_der(&dir)));
    assert_eq!(
        std::fs::read(dir.join(pki::CA_CERT_FILE)).unwrap(),
        ca_before
    );

    assert!(
        !migrate_server_certificate(&dir, &[], &[lan([10, 20, 30, 40], false)]).unwrap(),
        "a certificate with the fixed name is left alone"
    );

    std::fs::remove_dir_all(&dir).ok();
}

// ---- Upgrading a 0.11.1 setup: the operator's names, files and clients must keep working ----

#[test]
fn migration_keeps_the_names_of_an_old_certificate() {
    let (dir, bundle) = pki_with_client("migrate-names");
    // A 0.11.1 coordinator started with `--host render.lan --ip 100.64.1.9` (a VPN address).
    write_old_certificate(
        &dir,
        &dir.join(pki::SERVER_CERT_FILE),
        &dir.join(pki::SERVER_KEY_FILE),
        &["render.lan", "100.64.1.9"],
    );

    assert!(migrate_server_certificate(&dir, &[], &[lan([10, 20, 30, 40], false)]).unwrap());

    let (hosts, ips) = names_in(&dir.join(pki::SERVER_CERT_FILE));
    assert!(
        hosts.contains(&COORDINATOR_TLS_NAME.to_string()),
        "{hosts:?}"
    );
    assert!(hosts.contains(&"render.lan".to_string()), "{hosts:?}");
    assert!(
        ips.contains(&"100.64.1.9".parse::<IpAddr>().unwrap()),
        "{ips:?}"
    );
    assert!(
        ips.contains(&"10.20.30.40".parse::<IpAddr>().unwrap()),
        "{ips:?}"
    );

    // Both kinds of client still verify it: `join render.lan:7880` (the dialed host name, as
    // `join` now builds its config) and the VPN address, and a pairing client by the fixed name.
    let server = server_over(
        &dir,
        &dir.join(pki::SERVER_CERT_FILE),
        &dir.join(pki::SERVER_KEY_FILE),
    );
    handshake(
        fixed_name_client(&dir, &bundle),
        &server,
        dial_name("render.lan"),
    )
    .expect("join by host name");
    handshake(
        plain_client(&dir, &bundle),
        &server,
        dial_name("render.lan"),
    )
    .expect("an address-and-name client of 0.11.1");
    handshake(
        plain_client(&dir, &bundle),
        &server,
        dial_ip([100, 64, 1, 9]),
    )
    .expect("a client over the VPN address");
    handshake(
        fixed_name_client(&dir, &bundle),
        &server,
        dial_ip([192, 0, 2, 77]),
    )
    .expect("a pairing client whatever it dialed");

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&bundle).ok();
}

#[test]
fn a_join_client_still_verifies_a_coordinator_that_was_not_migrated_yet() {
    // The worker is upgraded before its coordinator: the old certificate has no fixed name,
    // so the fixed-name client falls back to the dialed name, as `join` did before.
    let (dir, bundle) = pki_with_client("join-old");
    issue_old_server_certificate(&dir);
    let server = server_over(
        &dir,
        &dir.join(pki::SERVER_CERT_FILE),
        &dir.join(pki::SERVER_KEY_FILE),
    );
    handshake(
        fixed_name_client(&dir, &bundle),
        &server,
        dial_name("worker.lan"),
    )
    .expect("the dialed name is in the old certificate");
    handshake(
        fixed_name_client(&dir, &bundle),
        &server,
        dial_name("other.lan"),
    )
    .expect_err("a name the certificate lacks still fails");

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&bundle).ok();
}

#[test]
fn migration_and_reissue_use_the_custom_cert_and_key_files() {
    let (dir, _bundle) = pki_with_client("custom-files");
    let elsewhere = temp_dir("custom-files-out");
    let files = ServerFiles {
        ca_dir: dir.clone(),
        cert: elsewhere.join("my-server.crt"),
        key: elsewhere.join("my-server.key"),
    };
    write_old_certificate(&dir, &files.cert, &files.key, &["custom.lan"]);
    let key_before = std::fs::read(&files.key).unwrap();

    assert!(
        super::migrate_server_certificate(&files, &[], &[lan([10, 20, 30, 40], false)]).unwrap()
    );

    let (hosts, _) = names_in(&files.cert);
    assert!(
        hosts.contains(&COORDINATOR_TLS_NAME.to_string()),
        "{hosts:?}"
    );
    assert!(hosts.contains(&"custom.lan".to_string()), "{hosts:?}");
    assert_ne!(std::fs::read(&files.key).unwrap(), key_before, "a new key");
    assert!(
        !dir.join(pki::SERVER_CERT_FILE).exists() && !dir.join(pki::SERVER_KEY_FILE).exists(),
        "no stray server files next to the CA"
    );
    assert!(
        !elsewhere.join("my-server.crt.tmp").exists(),
        "the temporary file is gone"
    );

    // The address watch rewrites the same files and keeps the operator's name.
    let resolver = Arc::new(SwappableCert::new(
        load_certified_key(&files.cert, &files.key).unwrap(),
    ));
    let mut refresh = CertificateRefresh::new(
        files.clone(),
        Vec::new(),
        Arc::clone(&resolver),
        Arc::new(|_: &CertificateSwap| {}),
    );
    assert!(refresh.check(&[lan([10, 20, 30, 99], false)]).unwrap());
    let (hosts, ips) = names_in(&files.cert);
    assert!(hosts.contains(&"custom.lan".to_string()), "{hosts:?}");
    assert!(
        ips.contains(&"10.20.30.99".parse::<IpAddr>().unwrap()),
        "{ips:?}"
    );
    assert!(!dir.join(pki::SERVER_CERT_FILE).exists());

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&elsewhere).ok();
}

#[test]
fn the_watch_does_not_drop_a_vpn_name_when_the_network_changes() {
    let dir = temp_dir("watch-keeps");
    pki::init(&dir).unwrap();
    issue_server_certificate(
        &dir,
        &["vpn.example".into(), "100.64.1.9".into()],
        &[lan([10, 20, 30, 40], false)],
    )
    .unwrap();
    let key = load_certified_key(
        &dir.join(pki::SERVER_CERT_FILE),
        &dir.join(pki::SERVER_KEY_FILE),
    )
    .unwrap();
    // A headless coordinator has no extra names configured, yet the names stay.
    let mut refresh = CertificateRefresh::new(
        ServerFiles::in_dir(&dir),
        Vec::new(),
        Arc::new(SwappableCert::new(key)),
        Arc::new(|_: &CertificateSwap| {}),
    );
    assert!(refresh.check(&[lan([192, 168, 7, 7], false)]).unwrap());
    let (hosts, ips) = names_in(&dir.join(pki::SERVER_CERT_FILE));
    assert!(hosts.contains(&"vpn.example".to_string()), "{hosts:?}");
    assert!(
        ips.contains(&"100.64.1.9".parse::<IpAddr>().unwrap()),
        "{ips:?}"
    );
    assert!(
        ips.contains(&"192.168.7.7".parse::<IpAddr>().unwrap()),
        "{ips:?}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn carried_names_are_capped_so_old_addresses_cannot_pile_up_forever() {
    let dir = temp_dir("cap");
    pki::init(&dir).unwrap();
    let rounds = u8::try_from(MAX_CARRIED_NAMES).unwrap() + 8;
    assert_eq!(rounds, 40, "the newest address below is 10.0.0.40");
    // The operator's own address is in the first certificate, before any LAN address.
    let operator: IpAddr = "100.64.1.9".parse().unwrap();
    issue_server_certificate(&dir, &["100.64.1.9".into()], &[lan([10, 0, 0, 1], false)]).unwrap();
    for n in 2..=rounds {
        issue_server_certificate(&dir, &[], &[lan([10, 0, 0, n], false)]).unwrap();
    }
    let (_, ips) = names_in(&dir.join(pki::SERVER_CERT_FILE));
    assert!(ips.len() <= MAX_CARRIED_NAMES, "{} addresses", ips.len());
    assert!(
        ips.contains(&"10.0.0.40".parse::<IpAddr>().unwrap()),
        "the newest address is always listed"
    );
    assert!(
        ips.contains(&operator),
        "the operator's original address survives the cap: {ips:?}"
    );
    assert!(
        !ips.contains(&"10.0.0.9".parse::<IpAddr>().unwrap()),
        "the oldest stale LAN addresses are the ones dropped: {ips:?}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_failed_reissue_is_reported_once_per_address_set() {
    // No CA in the folder: every reissue fails.
    let dir = temp_dir("reissue-fails");
    let (other, _bundle) = pki_with_client("reissue-fails-key");
    let key = {
        issue_server_certificate(&other, &[], &[lan([10, 20, 30, 40], false)]).unwrap();
        load_certified_key(
            &other.join(pki::SERVER_CERT_FILE),
            &other.join(pki::SERVER_KEY_FILE),
        )
        .unwrap()
    };
    let mut refresh = CertificateRefresh::new(
        ServerFiles::in_dir(&dir),
        Vec::new(),
        Arc::new(SwappableCert::new(key)),
        Arc::new(|_: &CertificateSwap| {}),
    );
    let first = [lan([192, 168, 5, 5], false)];
    assert!(
        refresh.check(&first).is_err(),
        "the first failure is reported"
    );
    assert!(
        !refresh.check(&first).unwrap(),
        "the same addresses are not retried or reported again"
    );
    assert!(!refresh.check(&first).unwrap());
    assert!(
        refresh.check(&[lan([192, 168, 6, 6], false)]).is_err(),
        "new addresses are tried (and reported) again"
    );

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&other).ok();
}

#[test]
fn a_swap_is_seen_by_the_next_handshake() {
    let (dir, bundle) = pki_with_client("swap");
    issue_server_certificate(&dir, &[], &[lan([10, 20, 30, 40], false)]).unwrap();
    let key_a = load_certified_key(
        &dir.join(pki::SERVER_CERT_FILE),
        &dir.join(pki::SERVER_KEY_FILE),
    )
    .unwrap();
    let resolver = Arc::new(SwappableCert::new(key_a));
    let server = server_config(&dir, &resolver);
    let loopback = [127, 0, 0, 1];

    let before = handshake(plain_client(&dir, &bundle), &server, dial_ip(loopback)).unwrap();

    issue_server_certificate(&dir, &[], &[lan([10, 20, 30, 41], false)]).unwrap();
    resolver.swap(
        load_certified_key(
            &dir.join(pki::SERVER_CERT_FILE),
            &dir.join(pki::SERVER_KEY_FILE),
        )
        .unwrap(),
    );
    let after = handshake(plain_client(&dir, &bundle), &server, dial_ip(loopback)).unwrap();

    assert_ne!(
        before, after,
        "the same server config presents the swapped certificate"
    );

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&bundle).ok();
}

#[test]
fn a_pairing_client_verifies_the_fixed_name_whatever_address_it_dialed() {
    let (dir, bundle) = pki_with_client("pairing");
    issue_server_certificate(&dir, &[], &[lan([10, 20, 30, 40], false)]).unwrap();
    let key = load_certified_key(
        &dir.join(pki::SERVER_CERT_FILE),
        &dir.join(pki::SERVER_KEY_FILE),
    )
    .unwrap();
    let resolver = Arc::new(SwappableCert::new(key));
    let server = server_config(&dir, &resolver);
    // Not in the certificate's addresses.
    let dialed = [192, 0, 2, 77];

    let err = handshake(plain_client(&dir, &bundle), &server, dial_ip(dialed))
        .expect_err("an address client must still fail for an address the certificate lacks");
    assert!(
        err.to_string()
            .to_lowercase()
            .contains("not valid for name"),
        "{err}"
    );

    handshake(fixed_name_client(&dir, &bundle), &server, dial_ip(dialed))
        .expect("a pairing client verifies the fixed name, not the dialed address");

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&bundle).ok();
}

#[test]
fn a_pairing_client_still_rejects_a_certificate_from_another_ca() {
    let (dir, bundle) = pki_with_client("pairing-ca");
    let (other_dir, _other_bundle) = pki_with_client("pairing-other");
    issue_server_certificate(&other_dir, &[], &[lan([10, 20, 30, 40], false)]).unwrap();
    let key = load_certified_key(
        &other_dir.join(pki::SERVER_CERT_FILE),
        &other_dir.join(pki::SERVER_KEY_FILE),
    )
    .unwrap();
    let resolver = Arc::new(SwappableCert::new(key));
    // The other server trusts the client's CA so only the server certificate is at issue.
    let server = server_config(&dir, &resolver);

    let err = handshake(
        fixed_name_client(&dir, &bundle),
        &server,
        dial_ip([127, 0, 0, 1]),
    )
    .expect_err("the CA pin must hold");
    // Both CAs carry the same subject name, so the verifier may match the pinned CA by name
    // and then fail its signature check, instead of finding no issuer at all.
    let message = err.to_string().to_lowercase();
    assert!(
        message.contains("unknownissuer") || message.contains("badsignature"),
        "{err}"
    );

    std::fs::remove_dir_all(&dir).ok();
    std::fs::remove_dir_all(&bundle).ok();
}

#[test]
fn the_address_check_swaps_only_when_the_lan_set_changes() {
    let dir = temp_dir("watch");
    pki::init(&dir).unwrap();
    let first_lan = [lan([10, 20, 30, 40], false)];
    issue_server_certificate(&dir, &[], &first_lan).unwrap();
    let key = load_certified_key(
        &dir.join(pki::SERVER_CERT_FILE),
        &dir.join(pki::SERVER_KEY_FILE),
    )
    .unwrap();
    let resolver = Arc::new(SwappableCert::new(key));
    let before = fingerprint(&resolver.current().cert[0]);

    let swaps = Arc::new(AtomicUsize::new(0));
    let hook_swaps = Arc::clone(&swaps);
    let mut refresh = CertificateRefresh::new(
        ServerFiles::in_dir(&dir),
        Vec::new(),
        Arc::clone(&resolver),
        Arc::new(move |swap: &CertificateSwap| {
            assert_eq!(swap.addresses, vec![Ipv4Addr::new(10, 20, 30, 41)]);
            hook_swaps.fetch_add(1, Ordering::SeqCst);
        }),
    );

    // The certificate on disk already lists this address: nothing to do.
    assert!(!refresh.check(&first_lan).unwrap());
    assert!(!refresh.check(&first_lan).unwrap());
    // No usable address keeps the certificate.
    assert!(!refresh.check(&[]).unwrap());
    assert_eq!(swaps.load(Ordering::SeqCst), 0);
    assert_eq!(fingerprint(&resolver.current().cert[0]), before);

    // A new address: reissue with the same CA and swap.
    let ca_before = std::fs::read(dir.join(pki::CA_CERT_FILE)).unwrap();
    assert!(refresh.check(&[lan([10, 20, 30, 41], false)]).unwrap());
    assert_eq!(swaps.load(Ordering::SeqCst), 1);
    assert_ne!(fingerprint(&resolver.current().cert[0]), before);
    assert_eq!(
        std::fs::read(dir.join(pki::CA_CERT_FILE)).unwrap(),
        ca_before
    );
    assert!(!refresh.check(&[lan([10, 20, 30, 41], false)]).unwrap());

    std::fs::remove_dir_all(&dir).ok();
}
