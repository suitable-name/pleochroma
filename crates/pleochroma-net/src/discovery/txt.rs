//! The pure parts of discovery: the TXT record, the CA filter and the DNS-safe labels.
//!
//! Nothing here touches the network, so the tests need no multicast. The TXT record carries
//! two public values: the protocol version (`v`) and a 16-hex-character tag of the CA
//! (`ca`). The tag is the first 8 bytes of SHA-256 over the CA fingerprint, so it lets a
//! browser skip other people's coordinators without revealing the fingerprint itself. It is
//! a filter, not a security check: the real check is still the TLS handshake against the
//! pinned CA. No secret ever goes into a TXT record.

use crate::{messages::PROTOCOL_VERSION, tls::Fingerprint};
use sha2::{Digest, Sha256};
use std::{collections::HashMap, fmt::Write};

/// The browse type: the service type plus the local domain.
pub const SERVICE_TYPE: &str = "_pleochroma._tcp.local.";

/// What follows the instance name in a full service name.
pub const SERVICE_SUFFIX: &str = "._pleochroma._tcp.local.";

/// The TXT key of the protocol version.
pub const KEY_VERSION: &str = "v";

/// The TXT key of the CA tag.
pub const KEY_CA: &str = "ca";

/// The longest instance name in bytes (a DNS label holds 63; this keeps some room).
const MAX_INSTANCE_BYTES: usize = 60;

/// The longest host label in bytes.
const MAX_HOST_BYTES: usize = 40;

/// The decoded TXT record of a coordinator announcement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TxtRecord {
    /// The announcer's wire protocol version.
    pub version: u16,
    /// The 16-hex-character CA tag (lowercase).
    pub ca_tag: String,
}

/// The CA tag of `ca_fingerprint`: 16 lowercase hex characters of its SHA-256 prefix.
#[must_use]
pub fn ca_tag(ca_fingerprint: &Fingerprint) -> String {
    let digest = Sha256::digest(ca_fingerprint);
    digest[..8]
        .iter()
        .fold(String::with_capacity(16), |mut tag, byte| {
            let _ = write!(tag, "{byte:02x}");
            tag
        })
}

/// The TXT properties an announcement for `ca_fingerprint` carries.
#[must_use]
pub fn encode_txt(ca_fingerprint: &Fingerprint) -> HashMap<String, String> {
    HashMap::from([
        (KEY_VERSION.to_owned(), PROTOCOL_VERSION.to_string()),
        (KEY_CA.to_owned(), ca_tag(ca_fingerprint)),
    ])
}

/// Decodes a TXT record from a key lookup. `None` when `v` or `ca` is missing or
/// malformed (the tag must be exactly 16 hex characters).
#[must_use]
pub fn decode_txt<'a>(get: impl Fn(&str) -> Option<&'a str>) -> Option<TxtRecord> {
    let version = get(KEY_VERSION)?.trim().parse::<u16>().ok()?;
    let tag = get(KEY_CA)?.trim();
    if tag.len() != 16 || !tag.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some(TxtRecord {
        version,
        ca_tag: tag.to_ascii_lowercase(),
    })
}

/// Whether `record` belongs to the CA `ca_fingerprint`.
#[must_use]
pub fn matches_ca(record: &TxtRecord, ca_fingerprint: &Fingerprint) -> bool {
    record.ca_tag == ca_tag(ca_fingerprint)
}

/// The instance name of a full service name (`"My PC._pleochroma._tcp.local."` gives
/// `"My PC"`). A name without the service suffix comes back unchanged.
#[must_use]
pub fn instance_name(fullname: &str) -> String {
    fullname
        .strip_suffix(SERVICE_SUFFIX)
        .unwrap_or(fullname)
        .to_owned()
}

/// The instance name to announce: `name` trimmed and cut to fit a DNS label at a
/// character boundary; `"Pleochroma"` when nothing is left.
#[must_use]
pub fn instance_label(name: &str) -> String {
    let mut label = String::new();
    for ch in name.trim().chars().filter(|c| !c.is_control() && *c != '.') {
        if label.len() + ch.len_utf8() > MAX_INSTANCE_BYTES {
            break;
        }
        label.push(ch);
    }
    let label = label.trim().to_owned();
    if label.is_empty() {
        "Pleochroma".to_owned()
    } else {
        label
    }
}

/// The mDNS host name to announce: `pleochroma-<name>.local.` with every character that
/// is not an ASCII letter or digit turned into a hyphen.
#[must_use]
pub fn host_name(name: &str) -> String {
    let mut body: String = name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .take(MAX_HOST_BYTES)
        .collect();
    body = body.trim_matches('-').to_owned();
    if body.is_empty() {
        body.push_str("host");
    }
    format!("pleochroma-{body}.local.")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fp(seed: u8) -> Fingerprint {
        let mut out = [0_u8; 32];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = seed.wrapping_add(u8::try_from(i).unwrap_or(0));
        }
        out
    }

    #[test]
    fn tag_is_16_lowercase_hex_and_stable() {
        let tag = ca_tag(&fp(1));
        assert_eq!(tag.len(), 16);
        assert!(
            tag.bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        );
        assert_eq!(tag, ca_tag(&fp(1)));
        assert_ne!(tag, ca_tag(&fp(2)));
    }

    #[test]
    fn tag_does_not_contain_the_fingerprint() {
        let print = fp(7);
        let hex = print.iter().take(8).fold(String::new(), |mut acc, b| {
            use std::fmt::Write as _;
            let _ = write!(acc, "{b:02x}");
            acc
        });
        assert_ne!(ca_tag(&print), hex);
    }

    #[test]
    fn encode_then_decode_round_trips() {
        let print = fp(3);
        let txt = encode_txt(&print);
        let record = decode_txt(|k| txt.get(k).map(String::as_str)).expect("decodes");
        assert_eq!(record.version, PROTOCOL_VERSION);
        assert_eq!(record.ca_tag, ca_tag(&print));
        assert!(matches_ca(&record, &print));
    }

    #[test]
    fn the_filter_rejects_another_ca() {
        let txt = encode_txt(&fp(3));
        let record = decode_txt(|k| txt.get(k).map(String::as_str)).expect("decodes");
        assert!(!matches_ca(&record, &fp(4)));
    }

    #[test]
    fn decode_rejects_missing_or_malformed_fields() {
        let good = encode_txt(&fp(5));
        let mut no_ca = good.clone();
        no_ca.remove(KEY_CA);
        assert!(decode_txt(|k| no_ca.get(k).map(String::as_str)).is_none());
        let mut no_version = good.clone();
        no_version.remove(KEY_VERSION);
        assert!(decode_txt(|k| no_version.get(k).map(String::as_str)).is_none());
        let mut bad_version = good.clone();
        bad_version.insert(KEY_VERSION.to_owned(), "x".to_owned());
        assert!(decode_txt(|k| bad_version.get(k).map(String::as_str)).is_none());
        let mut short_tag = good.clone();
        short_tag.insert(KEY_CA.to_owned(), "abcd".to_owned());
        assert!(decode_txt(|k| short_tag.get(k).map(String::as_str)).is_none());
        let mut not_hex = good;
        not_hex.insert(KEY_CA.to_owned(), "zzzzzzzzzzzzzzzz".to_owned());
        assert!(decode_txt(|k| not_hex.get(k).map(String::as_str)).is_none());
    }

    #[test]
    fn decode_accepts_an_uppercase_tag() {
        let tag = ca_tag(&fp(6)).to_ascii_uppercase();
        let txt = HashMap::from([
            (KEY_VERSION.to_owned(), "45".to_owned()),
            (KEY_CA.to_owned(), tag),
        ]);
        let record = decode_txt(|k| txt.get(k).map(String::as_str)).expect("decodes");
        assert!(matches_ca(&record, &fp(6)));
    }

    #[test]
    fn instance_name_strips_the_service_suffix() {
        assert_eq!(instance_name("My PC._pleochroma._tcp.local."), "My PC");
        assert_eq!(instance_name("plain"), "plain");
    }

    #[test]
    fn instance_label_is_trimmed_and_bounded() {
        assert_eq!(instance_label("  Studio PC "), "Studio PC");
        assert_eq!(instance_label(""), "Pleochroma");
        assert_eq!(instance_label("a.b"), "ab");
        let long = "é".repeat(100);
        let label = instance_label(&long);
        assert!(label.len() <= MAX_INSTANCE_BYTES);
        assert!(label.chars().all(|c| c == 'é'));
    }

    #[test]
    fn host_name_is_dns_safe() {
        assert_eq!(host_name("Studio PC"), "pleochroma-studio-pc.local.");
        assert_eq!(host_name("---"), "pleochroma-host.local.");
        assert!(host_name(&"x".repeat(200)).len() < 70);
    }
}
