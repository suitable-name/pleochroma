//! The `cert` sub-subcommands.

use crate::cli::{
    CertClaimArgs, CertInitArgs, CertIssueClientArgs, CertIssueServerArgs, CertIssueTokenArgs,
    Command, parse,
};
use pleochroma_net::messages::PeerRole;
use std::path::PathBuf;

#[test]
fn parses_cert_init() {
    let argv = ["cert", "init", "--dir", "pki"].map(String::from);
    assert_eq!(
        parse(&argv).unwrap(),
        Command::CertInit(CertInitArgs {
            dir: PathBuf::from("pki")
        })
    );
}

/// A bare `cert` names every sub-subcommand it accepts.
#[test]
fn bare_cert_lists_every_sub_subcommand() {
    let err = parse(&["cert".to_string()]).unwrap_err();
    for sub in [
        "init",
        "issue-server",
        "issue-client",
        "issue-token",
        "claim",
    ] {
        assert!(err.contains(&format!("\"{sub}\"")), "{sub}: {err}");
    }
}

#[test]
fn cert_init_requires_dir() {
    let argv = ["cert", "init"].map(String::from);
    assert!(parse(&argv).is_err());
}

#[test]
fn parses_cert_issue_server_with_repeated_host_and_ip() {
    let argv = [
        "cert",
        "issue-server",
        "--dir",
        "pki",
        "--host",
        "worker.lan",
        "--host",
        "worker2.lan",
        "--ip",
        "10.0.0.5",
    ]
    .map(String::from);
    assert_eq!(
        parse(&argv).unwrap(),
        Command::CertIssueServer(CertIssueServerArgs {
            dir: PathBuf::from("pki"),
            hosts: vec!["worker.lan".to_string(), "worker2.lan".to_string()],
            ips: vec!["10.0.0.5".parse().unwrap()],
        })
    );
}

#[test]
fn cert_issue_server_rejects_an_unparseable_ip() {
    let argv = ["cert", "issue-server", "--dir", "pki", "--ip", "not-an-ip"].map(String::from);
    assert!(parse(&argv).is_err());
}

#[test]
fn parses_cert_issue_client() {
    let argv = [
        "cert",
        "issue-client",
        "--dir",
        "pki",
        "--name",
        "laptop",
        "--out",
        "bundle",
    ]
    .map(String::from);
    assert_eq!(
        parse(&argv).unwrap(),
        Command::CertIssueClient(CertIssueClientArgs {
            dir: PathBuf::from("pki"),
            name: "laptop".to_string(),
            out: PathBuf::from("bundle"),
            role: PeerRole::Viewer,
        })
    );
}

#[test]
fn cert_issue_client_and_issue_token_accept_a_role() {
    let argv = [
        "cert",
        "issue-client",
        "--dir",
        "pki",
        "--name",
        "box",
        "--out",
        "b",
        "--role",
        "worker",
    ]
    .map(String::from);
    let Command::CertIssueClient(parsed) = parse(&argv).unwrap() else {
        panic!("expected CertIssueClient")
    };
    assert_eq!(parsed.role, PeerRole::Worker);

    let argv = [
        "cert",
        "issue-token",
        "--ca",
        "pki/ca.pem",
        "--admin-addr",
        "127.0.0.1:7881",
        "--name",
        "box",
        "--role",
        "worker",
    ]
    .map(String::from);
    let Command::CertIssueToken(parsed) = parse(&argv).unwrap() else {
        panic!("expected CertIssueToken")
    };
    assert_eq!(parsed.role, PeerRole::Worker);

    // `--role admin` is a real token request now; `issue-client` still refuses it.
    let argv = [
        "cert",
        "issue-token",
        "--ca",
        "pki/ca.pem",
        "--admin-addr",
        "127.0.0.1:7879",
        "--name",
        "box",
        "--role",
        "admin",
    ]
    .map(String::from);
    let Command::CertIssueToken(parsed) = parse(&argv).unwrap() else {
        panic!("expected CertIssueToken")
    };
    assert_eq!(parsed.role, PeerRole::Admin);

    let argv = [
        "cert",
        "issue-client",
        "--dir",
        "pki",
        "--name",
        "box",
        "--out",
        "b",
        "--role",
        "admin",
    ]
    .map(String::from);
    assert!(parse(&argv).unwrap_err().contains("--role"));

    let argv = ["cert", "issue-token", "--role", "bogus"].map(String::from);
    let err = parse(&argv).unwrap_err();
    assert!(err.contains("--role") && err.contains("monitor"), "{err}");
}

fn issue_token_argv(extra: &[&str]) -> Vec<String> {
    let mut argv: Vec<String> = [
        "cert",
        "issue-token",
        "--ca",
        "pki/ca.pem",
        "--admin-addr",
        "127.0.0.1:7879",
        "--name",
        "phone",
    ]
    .map(String::from)
    .to_vec();
    argv.extend(extra.iter().map(ToString::to_string));
    argv
}

fn parsed_token_args(extra: &[&str]) -> Result<CertIssueTokenArgs, String> {
    match parse(&issue_token_argv(extra))? {
        Command::CertIssueToken(args) => Ok(args),
        other => panic!("expected CertIssueToken, got {other:?}"),
    }
}

#[test]
fn cert_issue_token_role_monitor_parses() {
    let args = parsed_token_args(&["--role", "monitor"]).unwrap();
    assert_eq!(args.role, PeerRole::Monitor);
    assert!(args.addresses.is_empty());
    assert!(!args.no_qr);
}

#[test]
fn cert_issue_token_address_is_repeatable_up_to_three() {
    let args = parsed_token_args(&[
        "--address",
        "192.168.1.20:7878",
        "--address",
        "10.0.0.5:7878",
    ])
    .unwrap();
    assert_eq!(
        args.addresses,
        vec![
            "192.168.1.20:7878".parse().unwrap(),
            "10.0.0.5:7878".parse().unwrap()
        ]
    );

    let three = parsed_token_args(&[
        "--address",
        "10.0.0.1:7878",
        "--address",
        "10.0.0.2:7878",
        "--address",
        "10.0.0.3:7878",
    ])
    .unwrap();
    assert_eq!(three.addresses.len(), 3);

    let err = parsed_token_args(&[
        "--address",
        "10.0.0.1:7878",
        "--address",
        "10.0.0.2:7878",
        "--address",
        "10.0.0.3:7878",
        "--address",
        "10.0.0.4:7878",
    ])
    .unwrap_err();
    assert!(err.contains("--address") && err.contains('3'), "{err}");
}

#[test]
fn cert_issue_token_address_refuses_what_a_link_cannot_hold() {
    for bad in [
        "192.168.1.20",
        "192.168.1.20:0",
        "host.lan:7878",
        "[::1]:7878",
        "",
    ] {
        let err = parsed_token_args(&["--address", bad]).unwrap_err();
        assert!(err.contains("--address"), "{bad:?}: {err}");
    }
    let err = parsed_token_args(&["--address", "10.0.0.1:7878", "--address", "10.0.0.1:7878"])
        .unwrap_err();
    assert!(err.contains("twice"), "{err}");
    let err = parsed_token_args(&["--address"]).unwrap_err();
    assert!(err.contains("--address"), "{err}");
}

#[test]
fn cert_issue_token_no_qr_flag_turns_the_qr_off() {
    assert!(!parsed_token_args(&[]).unwrap().no_qr);
    assert!(parsed_token_args(&["--no-qr"]).unwrap().no_qr);
}

#[test]
fn cert_issue_client_requires_name_and_out() {
    let argv = ["cert", "issue-client", "--dir", "pki"].map(String::from);
    assert!(parse(&argv).is_err());
}

#[test]
fn parses_cert_issue_token() {
    let argv = [
        "cert",
        "issue-token",
        "--ca",
        "pki/ca.pem",
        "--admin-addr",
        "127.0.0.1:7879",
        "--name",
        "laptop",
    ]
    .map(String::from);
    assert_eq!(
        parse(&argv).unwrap(),
        Command::CertIssueToken(CertIssueTokenArgs {
            ca: PathBuf::from("pki/ca.pem"),
            admin_addr: "127.0.0.1:7879".to_string(),
            name: "laptop".to_string(),
            role: PeerRole::Viewer,
            addresses: Vec::new(),
            no_qr: false,
        })
    );
}

#[test]
fn cert_issue_token_requires_ca_admin_addr_and_name() {
    let argv = ["cert", "issue-token", "--ca", "pki/ca.pem"].map(String::from);
    assert!(parse(&argv).is_err());
}

#[test]
fn parses_cert_claim() {
    let argv = [
        "cert",
        "claim",
        "--token",
        "GW1-XXXXX",
        "--addr",
        "127.0.0.1:7879",
        "--out",
        "bundle",
    ]
    .map(String::from);
    assert_eq!(
        parse(&argv).unwrap(),
        Command::CertClaim(CertClaimArgs {
            token: "GW1-XXXXX".to_string(),
            addr: "127.0.0.1:7879".to_string(),
            out: PathBuf::from("bundle"),
        })
    );
}

#[test]
fn cert_claim_requires_token_addr_and_out() {
    let argv = ["cert", "claim", "--token", "GW1-XXXXX"].map(String::from);
    assert!(parse(&argv).is_err());
}

#[test]
fn unknown_cert_sub_subcommand_is_rejected() {
    let argv = ["cert", "frobnicate"].map(String::from);
    assert!(parse(&argv).is_err());
}

#[test]
fn bare_cert_with_no_sub_subcommand_is_rejected() {
    let argv = ["cert".to_string()];
    assert!(parse(&argv).is_err());
}
