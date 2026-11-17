//! Finding the computer again when it has a new address (the pure part).
//!
//! A pairing keeps the computer's addresses, but a router can hand it a new one. While the
//! saved addresses do not answer, the Android app browses `_pleochroma._tcp` (Kotlin
//! `NsdManager`, with a multicast lock held only while browsing) and passes every resolved
//! announcement to [`read_announcement`]. This module decides, with no network and no
//! platform, whether an announcement is the computer this phone is paired with
//! ([`belongs_to`]), how the saved addresses change ([`apply`]) and what the user is told
//! ([`moved_message`]).
//!
//! The TXT tag is a filter, not a check: anyone on the network can copy it. The app
//! therefore connects to the announced addresses with the saved credentials first, and
//! calls [`apply`] only when that handshake succeeded. TLS verifies the computer by its
//! fixed certificate name, so a new address still verifies.

use crate::settings::RemoteSettings;
use pleochroma_net::discovery::txt;
use std::net::{IpAddr, SocketAddr};

/// How many addresses are kept, as in a pairing link.
pub const MAX_ADDRESSES: usize = 3;

/// One computer announced on the local network, as the phone's browser resolved it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Announcement {
    /// The name the computer announced (its display name).
    pub name: String,
    /// The announced wire protocol version.
    pub version: u16,
    /// The 16-hex-character tag of its CA (lowercase).
    pub ca_tag: String,
    /// Its rendering addresses (`ip:port`), IPv4 only, without duplicates; never empty.
    pub addresses: Vec<String>,
}

/// What changed when the saved addresses were replaced.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Moved {
    /// The name to show: the saved name, else the announced one.
    pub name: String,
    /// The new main address without its port (what the user recognizes).
    pub host: String,
}

/// Reads one resolved service.
///
/// `version` and `ca` are the TXT values, `addresses` is a comma-separated list of `ip:port`
/// texts. `None` when the TXT record is malformed or no usable address is left: an address
/// must be IPv4 with a port, and not loopback, unspecified, multicast or broadcast.
#[must_use]
pub fn read_announcement(
    name: &str,
    version: &str,
    ca: &str,
    addresses: &str,
) -> Option<Announcement> {
    let record = txt::decode_txt(|key| match key {
        txt::KEY_VERSION => Some(version),
        txt::KEY_CA => Some(ca),
        _ => None,
    })?;
    let mut usable: Vec<String> = Vec::new();
    for text in addresses.split(',') {
        let Ok(address) = text.trim().parse::<SocketAddr>() else {
            continue;
        };
        let IpAddr::V4(ip) = address.ip() else {
            continue;
        };
        if address.port() == 0
            || ip.is_loopback()
            || ip.is_unspecified()
            || ip.is_multicast()
            || ip.is_broadcast()
        {
            continue;
        }
        let text = address.to_string();
        if !usable.contains(&text) {
            usable.push(text);
        }
        if usable.len() == MAX_ADDRESSES {
            break;
        }
    }
    if usable.is_empty() {
        return None;
    }
    Some(Announcement {
        name: name.trim().to_owned(),
        version: record.version,
        ca_tag: record.ca_tag,
        addresses: usable,
    })
}

/// Whether the announcement belongs to the CA this phone is paired with. `saved_tags` are
/// the tags of the saved CA certificates (`remote::saved_ca_tags`).
#[must_use]
pub fn belongs_to(announcement: &Announcement, saved_tags: &[String]) -> bool {
    saved_tags
        .iter()
        .any(|tag| tag.eq_ignore_ascii_case(&announcement.ca_tag))
}

/// Writes the announced addresses into the saved list (first address into `address`, the
/// one the Remote page shows). Call it only after the computer answered at them.
///
/// Returns what changed, or `None` when the saved addresses already are the announced ones
/// (nothing moved; the earlier failure was something else).
pub fn apply(remote: &mut RemoteSettings, announcement: &Announcement) -> Option<Moved> {
    let saved = remote.endpoint_addresses();
    let unchanged = saved.len() == announcement.addresses.len()
        && announcement
            .addresses
            .iter()
            .all(|address| saved.contains(address));
    if unchanged {
        return None;
    }
    let first = announcement.addresses.first()?;
    first.clone_into(&mut remote.address);
    remote.addresses.clone_from(&announcement.addresses);
    let host = host_of(first);
    let saved_name = remote.name.trim();
    let name = if saved_name.is_empty() {
        announcement.name.clone()
    } else {
        saved_name.to_owned()
    };
    Some(Moved { name, host })
}

/// The `host` of an `ip:port` text (without brackets or port).
fn host_of(address: &str) -> String {
    address
        .parse::<SocketAddr>()
        .map_or_else(|_| address.to_owned(), |parsed| parsed.ip().to_string())
}

/// The sentence shown after the computer was found at a new address.
#[must_use]
pub fn moved_message(moved: &Moved) -> String {
    let name = moved.name.trim();
    let name = if name.is_empty() {
        "Your computer"
    } else {
        name
    };
    format!("{name} moved to {}.", moved.host)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_net::tls::Fingerprint;

    fn fp(seed: u8) -> Fingerprint {
        let mut out = [0_u8; 32];
        for (i, byte) in out.iter_mut().enumerate() {
            *byte = seed.wrapping_add(u8::try_from(i).unwrap_or(0));
        }
        out
    }

    fn announced(seed: u8, addresses: &str) -> Option<Announcement> {
        read_announcement("Studio PC", "45", &txt::ca_tag(&fp(seed)), addresses)
    }

    fn saved(name: &str, addresses: &[&str]) -> RemoteSettings {
        RemoteSettings {
            name: name.to_owned(),
            address: addresses.first().map_or(String::new(), |a| (*a).to_owned()),
            addresses: addresses.iter().map(|a| (*a).to_owned()).collect(),
            enrolled: true,
            ..RemoteSettings::default()
        }
    }

    #[test]
    fn a_good_announcement_is_read() {
        let found = announced(1, "192.168.1.20:7879, 10.0.0.4:7879").expect("reads");
        assert_eq!(found.name, "Studio PC");
        assert_eq!(found.version, 45);
        assert_eq!(found.addresses, ["192.168.1.20:7879", "10.0.0.4:7879"]);
    }

    #[test]
    fn unusable_addresses_are_dropped() {
        let found = announced(
            1,
            "127.0.0.1:7879,0.0.0.0:7879,224.0.0.1:7879,192.168.1.20:0,[fe80::1]:7879,nonsense,192.168.1.20:7879,192.168.1.20:7879",
        )
        .expect("one address is left");
        assert_eq!(found.addresses, ["192.168.1.20:7879"]);
        assert!(announced(1, "127.0.0.1:7879,nonsense").is_none());
        assert!(announced(1, "").is_none());
    }

    #[test]
    fn at_most_three_addresses_are_kept() {
        let found = announced(1, "10.0.0.1:1,10.0.0.2:1,10.0.0.3:1,10.0.0.4:1").expect("reads");
        assert_eq!(found.addresses.len(), MAX_ADDRESSES);
    }

    #[test]
    fn a_malformed_txt_record_is_refused() {
        assert!(read_announcement("x", "45", "abcd", "10.0.0.1:1").is_none());
        assert!(read_announcement("x", "no", &txt::ca_tag(&fp(1)), "10.0.0.1:1").is_none());
    }

    #[test]
    fn only_the_paired_ca_matches() {
        let found = announced(1, "10.0.0.1:7879").expect("reads");
        assert!(belongs_to(&found, &[txt::ca_tag(&fp(1))]));
        assert!(belongs_to(
            &found,
            &[
                txt::ca_tag(&fp(9)),
                txt::ca_tag(&fp(1)).to_ascii_uppercase()
            ]
        ));
        assert!(!belongs_to(&found, &[txt::ca_tag(&fp(2))]));
        assert!(!belongs_to(&found, &[]));
    }

    #[test]
    fn a_new_address_replaces_the_saved_list() {
        let mut remote = saved("Studio PC", &["192.168.1.5:7879", "10.0.0.5:7879"]);
        let found = announced(1, "192.168.1.20:7879").expect("reads");
        let moved = apply(&mut remote, &found).expect("moved");
        assert_eq!(remote.address, "192.168.1.20:7879");
        assert_eq!(remote.addresses, ["192.168.1.20:7879"]);
        assert_eq!(moved.host, "192.168.1.20");
        assert_eq!(moved_message(&moved), "Studio PC moved to 192.168.1.20.");
        assert!(remote.enrolled);
    }

    #[test]
    fn a_phone_set_up_by_hand_has_its_single_address_replaced() {
        let mut remote = saved("", &[]);
        remote.address = "192.168.1.5:7879".to_owned();
        let found = announced(1, "192.168.1.20:7879").expect("reads");
        let moved = apply(&mut remote, &found).expect("moved");
        assert_eq!(remote.address, "192.168.1.20:7879");
        assert_eq!(remote.addresses, ["192.168.1.20:7879"]);
        // No saved name: the announced one is used.
        assert_eq!(moved.name, "Studio PC");
    }

    #[test]
    fn the_same_addresses_change_nothing() {
        let mut remote = saved("Studio PC", &["192.168.1.20:7879"]);
        let before = remote.clone();
        let found = announced(1, "192.168.1.20:7879").expect("reads");
        assert!(apply(&mut remote, &found).is_none());
        assert_eq!(remote, before);
    }

    #[test]
    fn the_message_has_a_fallback_name() {
        let moved = Moved {
            name: "  ".to_owned(),
            host: "10.0.0.9".to_owned(),
        };
        assert_eq!(moved_message(&moved), "Your computer moved to 10.0.0.9.");
    }
}
