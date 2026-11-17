//! Tests for the pairing link: the real sample links, round trips, every error path,
//! secret redaction, and proptest fuzzing of both directions.

use std::net::{Ipv4Addr, SocketAddrV4};

use proptest::prelude::*;

use super::*;

/// The two real sample links from the coordinator mockup folder.
const SAMPLE_VIEWER: &str = "pleochroma://pair?v=1&r=viewer&n=Studio+PC&a=192.168.178.20:7878&a=10.0.0.5:7878&e=7879&t=GW1-DDSNM-0C0QF-VPG2E-VSX7X-DBE00-DSZAC-8FFS7-A5DWD-Z0T7B-MJVFR-R7X7K-RSEZF-304SE-1B1FM-MDQ3S-KRV70-K0247-R5V06-TYFM4-P3QH2-D6X";
const SAMPLE_WORKER: &str = "pleochroma://pair?v=1&r=worker&n=Studio+PC&a=192.168.178.20:7880&a=10.0.0.5:7880&e=7881&t=GW1-EP82C-ZFG5T-HPW6Q-ZP4N0-AK319-J3WR6-3RRNE-F9TAE-PZSJ8-PMFC3-WPJEJ-Q1VE1-E0AK1-3HTWN-CK78J-AEYTX-3G32H-V8SKY-TS9P4-QZ0YT-S06";

/// A token-shaped string that no real secret resembles.
const TOKEN: &str = "GW1-ABCDE-FGHJK-MNPQR-STVWX-YZ012";

fn v4(first: u8, second: u8, third: u8, fourth: u8, port: u16) -> SocketAddr {
    SocketAddr::V4(SocketAddrV4::new(
        Ipv4Addr::new(first, second, third, fourth),
        port,
    ))
}

fn link(name: &str) -> PairingLink {
    PairingLink::new(
        PeerRole::Viewer,
        name,
        vec![v4(192, 168, 1, 20, 7878)],
        7879,
        TOKEN,
    )
}

/// A minimal valid link text with `extra` appended to the query.
fn text_with(extra: &str) -> String {
    format!("{LINK_PREFIX}v=1&r=viewer&n=A&a=10.0.0.1:7878&e=7879&t={TOKEN}{extra}")
}

#[test]
fn sample_links_parse_and_rebuild_exactly() {
    let viewer = PairingLink::parse(SAMPLE_VIEWER).unwrap();
    assert_eq!(viewer.v, 1);
    assert_eq!(viewer.role, PeerRole::Viewer);
    assert_eq!(viewer.name, "Studio PC");
    assert_eq!(
        viewer.addrs,
        vec![v4(192, 168, 178, 20, 7878), v4(10, 0, 0, 5, 7878)]
    );
    assert_eq!(viewer.enroll_port, 7879);
    assert!(viewer.token().starts_with("GW1-"));
    assert_eq!(viewer.build().unwrap(), SAMPLE_VIEWER);

    let worker = PairingLink::parse(SAMPLE_WORKER).unwrap();
    assert_eq!(worker.role, PeerRole::Worker);
    assert_eq!(worker.enroll_port, 7881);
    assert_eq!(worker.build().unwrap(), SAMPLE_WORKER);
}

#[test]
fn sample_links_fit_a_version_11_qr_at_level_m() {
    // Version 11, error correction M holds 251 bytes in byte mode.
    for sample in [SAMPLE_VIEWER, SAMPLE_WORKER] {
        assert!(sample.len() <= 251, "{} bytes", sample.len());
        assert!(sample.len() >= 200, "{} bytes", sample.len());
        assert!(sample.len() <= MAX_LINK_LEN);
    }
    // The mockup links are about 217 characters.
    assert!((210..=225).contains(&SAMPLE_VIEWER.len()));
}

#[test]
fn every_role_round_trips() {
    for role in [
        PeerRole::Viewer,
        PeerRole::Worker,
        PeerRole::Monitor,
        PeerRole::Admin,
    ] {
        let mut original = link("Studio PC");
        original.role = role;
        let text = original.build().unwrap();
        assert_eq!(PairingLink::parse(&text).unwrap(), original);
    }
}

#[test]
fn awkward_names_round_trip() {
    for name in [
        "",
        "Studio PC",
        "a&b",
        "a=b",
        "a#b",
        "100% real",
        "plus+sign",
        "  two  spaces ",
        "Büro-Rechner",
        "Garage server \u{1F5A5}",
        "日本語",
        "a,b;c?d/e:f@g",
    ] {
        let original = link(name);
        let text = original.build().unwrap();
        assert!(!text.contains(' '), "{text}");
        assert!(!text[LINK_PREFIX.len()..].contains('#'), "{text}");
        assert_eq!(PairingLink::parse(&text).unwrap().name, name, "{name:?}");
    }
}

#[test]
fn name_encoding_is_plus_for_space_and_utf8_percent() {
    let text = link("Büro PC").build().unwrap();
    assert!(text.contains("n=B%C3%BCro+PC"), "{text}");
    let text = link("a+b&c=d#e").build().unwrap();
    assert!(text.contains("n=a%2Bb%26c%3Dd%23e"), "{text}");
}

#[test]
fn three_addresses_round_trip_in_order() {
    let mut original = link("x");
    original.addrs = vec![
        v4(10, 0, 0, 5, 7878),
        v4(192, 168, 178, 20, 7878),
        v4(172, 16, 0, 9, 7880),
    ];
    let parsed = PairingLink::parse(&original.build().unwrap()).unwrap();
    assert_eq!(parsed.addrs, original.addrs);
}

#[test]
fn comma_separated_addresses_are_accepted() {
    let text = format!(
        "{LINK_PREFIX}v=1&r=viewer&n=A&a=10.0.0.1:7878,10.0.0.2:7878&a=10.0.0.3:7878&e=7879&t={TOKEN}"
    );
    let parsed = PairingLink::parse(&text).unwrap();
    assert_eq!(parsed.addrs.len(), 3);
    assert_eq!(parsed.addrs[1], v4(10, 0, 0, 2, 7878));
}

#[test]
fn scheme_case_whitespace_fragment_and_unknown_keys_are_tolerated() {
    let text = format!(
        "  PLEOCHROMA://Pair?v=1&r=admin&x=future&n=A&a=10.0.0.1:7878&e=7879&t={TOKEN}#frag\n"
    );
    // The host part is matched without regard to case as well.
    let parsed = PairingLink::parse(&text).unwrap();
    assert_eq!(parsed.role, PeerRole::Admin);
    assert_eq!(parsed.token(), TOKEN);
}

#[test]
fn missing_name_reads_as_empty() {
    let text = format!("{LINK_PREFIX}v=1&r=viewer&a=10.0.0.1:7878&e=7879&t={TOKEN}");
    assert_eq!(PairingLink::parse(&text).unwrap().name, "");
}

#[test]
fn unknown_version_asks_for_an_update() {
    for v in ["2", "0", "99", "4294967296", "99999999999999999999"] {
        let text = format!("{LINK_PREFIX}v={v}&r=viewer&n=A&a=10.0.0.1:7878&e=7879&t={TOKEN}");
        let err = PairingLink::parse(&text).unwrap_err();
        assert_eq!(err, PairingLinkError::NeedsNewerVersion, "v={v}");
    }
    // Even a link whose other parts a newer app wrote differently.
    let err = PairingLink::parse(&format!("{LINK_PREFIX}v=2&r=viewer2&z=1")).unwrap_err();
    assert_eq!(err, PairingLinkError::NeedsNewerVersion);
    assert_eq!(
        PairingLinkError::NeedsNewerVersion.to_string(),
        "This link needs a newer Pleochroma; update the app."
    );
}

#[test]
fn building_a_link_with_another_version_fails() {
    let mut l = link("x");
    l.v = 2;
    assert_eq!(l.build().unwrap_err(), PairingLinkError::NeedsNewerVersion);
}

#[test]
fn not_a_pairing_link() {
    for text in [
        "",
        "hello",
        "https://example.com/pair?v=1",
        "pleochroma://other?v=1",
        "pleochroma://pair",
        "pleochroma:pair?v=1",
    ] {
        assert_eq!(
            PairingLink::parse(text).unwrap_err(),
            PairingLinkError::NotAPairingLink,
            "{text:?}"
        );
    }
}

#[test]
fn too_long_is_refused() {
    let text = text_with(&format!("&z={}", "a".repeat(MAX_LINK_LEN)));
    assert_eq!(
        PairingLink::parse(&text).unwrap_err(),
        PairingLinkError::TooLong
    );
}

#[test]
fn missing_parts() {
    let cases = [
        // no v
        format!("{LINK_PREFIX}r=viewer&n=A&a=10.0.0.1:7878&e=7879&t={TOKEN}"),
        // v is not a number
        format!("{LINK_PREFIX}v=x&r=viewer&n=A&a=10.0.0.1:7878&e=7879&t={TOKEN}"),
        // v empty
        format!("{LINK_PREFIX}v=&r=viewer&n=A&a=10.0.0.1:7878&e=7879&t={TOKEN}"),
        // no role
        format!("{LINK_PREFIX}v=1&n=A&a=10.0.0.1:7878&e=7879&t={TOKEN}"),
        // no enrollment port
        format!("{LINK_PREFIX}v=1&r=viewer&n=A&a=10.0.0.1:7878&t={TOKEN}"),
        // empty query
        LINK_PREFIX.to_string(),
    ];
    for text in cases {
        assert_eq!(
            PairingLink::parse(&text).unwrap_err(),
            PairingLinkError::MissingPart,
            "{text}"
        );
    }
}

#[test]
fn repeated_parts() {
    let again = format!("&t={TOKEN}");
    for extra in ["&v=1", "&r=worker", "&n=B", "&e=7880", again.as_str()] {
        assert_eq!(
            PairingLink::parse(&text_with(extra)).unwrap_err(),
            PairingLinkError::RepeatedPart,
            "{extra}"
        );
    }
}

#[test]
fn unknown_role() {
    for role in ["", "Viewer", "root", "viewer2"] {
        let text = format!("{LINK_PREFIX}v=1&r={role}&n=A&a=10.0.0.1:7878&e=7879&t={TOKEN}");
        assert_eq!(
            PairingLink::parse(&text).unwrap_err(),
            PairingLinkError::UnknownRole,
            "{role:?}"
        );
    }
}

#[test]
fn bad_names() {
    // Invalid UTF-8 after decoding.
    let text = format!("{LINK_PREFIX}v=1&r=viewer&n=%FF%FE&a=10.0.0.1:7878&e=7879&t={TOKEN}");
    assert_eq!(
        PairingLink::parse(&text).unwrap_err(),
        PairingLinkError::BadName
    );
    // A control character.
    let text = format!("{LINK_PREFIX}v=1&r=viewer&n=a%0Ab&a=10.0.0.1:7878&e=7879&t={TOKEN}");
    assert_eq!(
        PairingLink::parse(&text).unwrap_err(),
        PairingLinkError::BadName
    );
    // Too long, while parsing and while building.
    let long = "x".repeat(MAX_NAME_BYTES + 1);
    let text = format!("{LINK_PREFIX}v=1&r=viewer&n={long}&a=10.0.0.1:7878&e=7879&t={TOKEN}");
    assert_eq!(
        PairingLink::parse(&text).unwrap_err(),
        PairingLinkError::BadName
    );
    assert_eq!(link(&long).build().unwrap_err(), PairingLinkError::BadName);
    assert_eq!(link("a\tb").build().unwrap_err(), PairingLinkError::BadName);
    // The longest allowed name still works.
    let edge = "x".repeat(MAX_NAME_BYTES);
    assert!(link(&edge).build().is_ok());
}

#[test]
fn broken_percent_encoding() {
    for name in ["%", "%4", "%zz", "a%2", "%G1"] {
        let text = format!("{LINK_PREFIX}v=1&r=viewer&n={name}&a=10.0.0.1:7878&e=7879&t={TOKEN}");
        assert_eq!(
            PairingLink::parse(&text).unwrap_err(),
            PairingLinkError::BadEncoding,
            "{name}"
        );
    }
}

#[test]
fn bad_addresses() {
    for addr in [
        "10.0.0.1",
        "10.0.0.1:",
        "10.0.0.1:99999",
        "host:7878",
        "10.0.0:7878",
        ":7878",
        "x",
    ] {
        let text = format!("{LINK_PREFIX}v=1&r=viewer&n=A&a={addr}&e=7879&t={TOKEN}");
        assert_eq!(
            PairingLink::parse(&text).unwrap_err(),
            PairingLinkError::BadAddress,
            "{addr}"
        );
    }
    // Port 0 and a repeated address.
    for addrs in ["10.0.0.1:0", "10.0.0.1:7878&a=10.0.0.1:7878"] {
        let text = format!("{LINK_PREFIX}v=1&r=viewer&n=A&a={addrs}&e=7879&t={TOKEN}");
        assert_eq!(
            PairingLink::parse(&text).unwrap_err(),
            PairingLinkError::BadAddress,
            "{addrs}"
        );
    }
}

#[test]
fn ipv6_addresses_are_refused() {
    for addr in ["[::1]:7878", "[fe80::1]:7878", "[2001:db8::1]:7878"] {
        let text = format!("{LINK_PREFIX}v=1&r=viewer&n=A&a={addr}&e=7879&t={TOKEN}");
        assert_eq!(
            PairingLink::parse(&text).unwrap_err(),
            PairingLinkError::NotIpv4,
            "{addr}"
        );
    }
    let mut l = link("x");
    l.addrs = vec!["[::1]:7878".parse().unwrap()];
    assert_eq!(l.build().unwrap_err(), PairingLinkError::NotIpv4);
}

#[test]
fn address_count_limits() {
    let text = format!("{LINK_PREFIX}v=1&r=viewer&n=A&e=7879&t={TOKEN}");
    assert_eq!(
        PairingLink::parse(&text).unwrap_err(),
        PairingLinkError::NoAddress
    );
    let four = format!(
        "{LINK_PREFIX}v=1&r=viewer&n=A&a=10.0.0.1:1&a=10.0.0.2:1&a=10.0.0.3:1&a=10.0.0.4:1&e=7879&t={TOKEN}"
    );
    assert_eq!(
        PairingLink::parse(&four).unwrap_err(),
        PairingLinkError::TooManyAddresses
    );
    let mut l = link("x");
    l.addrs = Vec::new();
    assert_eq!(l.build().unwrap_err(), PairingLinkError::NoAddress);
    l.addrs = (1..=4).map(|i| v4(10, 0, 0, i, 7878)).collect();
    assert_eq!(l.build().unwrap_err(), PairingLinkError::TooManyAddresses);
}

#[test]
fn bad_enrollment_port() {
    for port in ["0", "65536", "-1", "x", ""] {
        let text = format!("{LINK_PREFIX}v=1&r=viewer&n=A&a=10.0.0.1:7878&e={port}&t={TOKEN}");
        assert_eq!(
            PairingLink::parse(&text).unwrap_err(),
            PairingLinkError::BadPort,
            "{port:?}"
        );
    }
    let mut l = link("x");
    l.enroll_port = 0;
    assert_eq!(l.build().unwrap_err(), PairingLinkError::BadPort);
}

#[test]
fn missing_and_bad_tokens() {
    let none = format!("{LINK_PREFIX}v=1&r=viewer&n=A&a=10.0.0.1:7878&e=7879");
    assert_eq!(
        PairingLink::parse(&none).unwrap_err(),
        PairingLinkError::MissingToken
    );
    let empty = format!("{LINK_PREFIX}v=1&r=viewer&n=A&a=10.0.0.1:7878&e=7879&t=");
    assert_eq!(
        PairingLink::parse(&empty).unwrap_err(),
        PairingLinkError::MissingToken
    );
    for token in [
        "ABC",
        "GW2-ABCDE",
        "GW1-",
        "gw1-ABCDE",
        "GW1-AB CD",
        "GW1-AB%20CD",
        "GW1-AB.CD",
    ] {
        let text = format!("{LINK_PREFIX}v=1&r=viewer&n=A&a=10.0.0.1:7878&e=7879&t={token}");
        assert_eq!(
            PairingLink::parse(&text).unwrap_err(),
            PairingLinkError::BadToken,
            "{token}"
        );
    }
    let long = format!("GW1-{}", "A".repeat(MAX_TOKEN_LEN));
    let text = format!("{LINK_PREFIX}v=1&r=viewer&n=A&a=10.0.0.1:7878&e=7879&t={long}");
    assert_eq!(
        PairingLink::parse(&text).unwrap_err(),
        PairingLinkError::BadToken
    );
    let mut l = link("x");
    l.token = Zeroizing::new(String::new());
    assert_eq!(l.build().unwrap_err(), PairingLinkError::MissingToken);
    l.token = Zeroizing::new("not-a-token".to_owned());
    assert_eq!(l.build().unwrap_err(), PairingLinkError::BadToken);
}

#[test]
fn debug_redacts_the_token() {
    let l = link("Studio PC");
    let shown = format!("{l:?}");
    assert!(!shown.contains("GW1"), "{shown}");
    assert!(!shown.contains("ABCDE"), "{shown}");
    assert!(shown.contains("<redacted>"), "{shown}");
    assert!(shown.contains("Studio PC"), "{shown}");
    let pretty = format!("{l:#?}");
    assert!(!pretty.contains("GW1"), "{pretty}");
}

#[test]
fn errors_never_show_link_text() {
    use PairingLinkError as E;
    for err in [
        E::NotAPairingLink,
        E::TooLong,
        E::NeedsNewerVersion,
        E::MissingPart,
        E::RepeatedPart,
        E::UnknownRole,
        E::BadName,
        E::BadAddress,
        E::NotIpv4,
        E::NoAddress,
        E::TooManyAddresses,
        E::BadPort,
        E::MissingToken,
        E::BadToken,
        E::BadEncoding,
    ] {
        let shown = format!("{err} {err:?}");
        assert!(!shown.contains("GW1"), "{shown}");
        assert!(!shown.contains("pleochroma://"), "{shown}");
    }
    // And a failing parse of a link that holds a token reports nothing of it.
    let err = PairingLink::parse(&text_with("&r=worker")).unwrap_err();
    assert!(!format!("{err} {err:?}").contains("ABCDE"));
}

#[test]
fn parsed_garbage_prefixed_links_do_not_panic() {
    for text in [
        "pleochroma://pair?&&&&",
        "pleochroma://pair?=",
        "pleochroma://pair?===&&&",
        "pleochroma://pair?v",
        "pleochroma://pair?a=%",
        "pleochroma://pair?v=1&a=,,,&r=viewer",
        "pleochroma://pair?v=1&a=%00&r=viewer",
        "pleochroma://pairé?v=1",
        "pleochroma://pai\u{301}",
        "\u{1F5A5}\u{1F5A5}\u{1F5A5}\u{1F5A5}\u{1F5A5}\u{1F5A5}\u{1F5A5}\u{1F5A5}\u{1F5A5}\u{1F5A5}\u{1F5A5}\u{1F5A5}",
    ] {
        let _ = PairingLink::parse(text);
    }
}

fn arb_role() -> impl Strategy<Value = PeerRole> {
    prop_oneof![
        Just(PeerRole::Viewer),
        Just(PeerRole::Worker),
        Just(PeerRole::Monitor),
        Just(PeerRole::Admin),
    ]
}

fn arb_addrs() -> impl Strategy<Value = Vec<SocketAddr>> {
    proptest::collection::vec((any::<[u8; 4]>(), 1u16..=u16::MAX), 1..=MAX_ADDRS).prop_map(|raw| {
        let mut seen = HashSet::new();
        raw.into_iter()
            .map(|(o, port)| v4(o[0], o[1], o[2], o[3], port))
            .filter(|a| seen.insert(*a))
            .collect()
    })
}

fn arb_link() -> impl Strategy<Value = PairingLink> {
    (
        arb_role(),
        "\\PC{0,16}",
        arb_addrs(),
        1u16..=u16::MAX,
        "GW1-[A-Z0-9]{1,5}(-[A-Z0-9]{5}){0,24}",
    )
        .prop_map(|(role, name, addrs, port, token)| {
            PairingLink::new(role, name, addrs, port, token)
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn parse_of_build_is_identity(original in arb_link()) {
        let text = original.build().unwrap();
        prop_assert!(text.len() <= MAX_LINK_LEN);
        prop_assert!(text.starts_with(LINK_PREFIX));
        let parsed = PairingLink::parse(&text).unwrap();
        prop_assert_eq!(&parsed, &original);
        // Building what was parsed gives the same text back.
        prop_assert_eq!(parsed.build().unwrap(), text);
    }

    #[test]
    fn parse_never_panics_on_arbitrary_text(text in "\\PC{0,300}") {
        let _ = PairingLink::parse(&text);
    }

    #[test]
    fn parse_never_panics_on_arbitrary_bytes_as_text(bytes in proptest::collection::vec(any::<u8>(), 0..400)) {
        let text = String::from_utf8_lossy(&bytes);
        let _ = PairingLink::parse(&text);
    }

    #[test]
    fn parse_never_panics_after_the_prefix(tail in "[ -~%+&=#,:\\[\\]é日]{0,300}") {
        let _ = PairingLink::parse(&format!("{LINK_PREFIX}{tail}"));
    }

    #[test]
    fn mutated_valid_links_never_panic(
        original in arb_link(),
        cut in 0usize..400,
        junk in "[ -~]{0,4}",
    ) {
        let text = original.build().unwrap();
        let at = cut.min(text.len());
        // Cut on a char boundary: links are ASCII, so every index is one.
        let mutated = format!("{}{}{}", &text[..at], junk, &text[at..]);
        let _ = PairingLink::parse(&mutated);
        let _ = PairingLink::parse(&text[..at]);
    }
}
