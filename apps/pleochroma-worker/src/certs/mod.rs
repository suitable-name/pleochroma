//! The coordinator's server certificate: names, hot swap and migration.
//!
//! Every server certificate carries the fixed name
//! [`COORDINATOR_TLS_NAME`](pleochroma_net::tls::COORDINATOR_TLS_NAME) as a DNS name, so
//! a pairing client verifies it whatever address it dialed. It also lists the current LAN
//! IPv4 addresses, `localhost`, `127.0.0.1` and any extra names (VPN or DNS) for older clients
//! and for `join`. [`certificate_names`] decides that list.
//!
//! - [`SwappableCert`] is the `rustls` certificate resolver of the server
//!   configs; a new certificate replaces the old one without restarting a listener.
//! - [`watch`] checks the LAN addresses every 10 seconds and, when they change, mints a new
//!   certificate with the same CA and swaps it in.
//! - [`migrate_server_certificate`] reissues, on the first start of 0.12.0, a server
//!   certificate that lacks the fixed name. The CA stays, so paired devices keep working, and
//!   every name the old certificate listed stays too.
//!
//! Issuing itself is [`crate::pki::issue_server`]; this module only chooses the names and
//! moves the result into the running listeners.

use crate::{
    cli::ServeArgs,
    pki::{self, PkiError},
};
use pleochroma_net::{netinfo::LanAddress, tls};
use rustls::sign::CertifiedKey;
use std::{
    net::{IpAddr, Ipv4Addr},
    path::{Path, PathBuf},
};

mod resolver;
#[cfg(test)]
mod tests;
pub mod watch;

pub use resolver::SwappableCert;
pub use watch::{AddressWatch, CertificateRefresh, CertificateSwap, SwapHook, announcing_hook};

/// The LAN addresses a certificate lists: the physical adapters' addresses, in the order
/// [`pleochroma_net::netinfo::lan_ipv4`] gave them.
///
/// Virtual adapters (Hyper-V, WSL, Docker, VPN tunnels) are left out unless nothing else exists.
#[must_use]
pub fn certificate_lan_addresses(lan: &[LanAddress]) -> Vec<Ipv4Addr> {
    let mut physical: Vec<Ipv4Addr> = lan
        .iter()
        .filter(|a| !a.virtual_adapter)
        .map(|a| a.addr)
        .collect();
    if physical.is_empty() {
        physical = lan.iter().map(|a| a.addr).collect();
    }
    physical.dedup();
    physical
}

/// The DNS names and IP addresses of a server certificate.
///
/// DNS names: the fixed name, `localhost`, then `extra_names` (blank entries and
/// duplicates dropped; an entry that is an IP address moves to the IP list). IP
/// addresses: `127.0.0.1`, then [`certificate_lan_addresses`].
#[must_use]
pub fn certificate_names(lan: &[LanAddress], extra_names: &[String]) -> (Vec<String>, Vec<IpAddr>) {
    let mut hosts: Vec<String> = vec![tls::COORDINATOR_TLS_NAME.to_string(), "localhost".into()];
    let mut ips: Vec<IpAddr> = vec![IpAddr::V4(Ipv4Addr::LOCALHOST)];

    for name in extra_names {
        let name = name.trim();
        if name.is_empty() {
            continue;
        }
        if let Ok(ip) = name.parse::<IpAddr>() {
            if !ips.contains(&ip) {
                ips.push(ip);
            }
        } else if !hosts.iter().any(|h| h.eq_ignore_ascii_case(name)) {
            hosts.push(name.to_string());
        }
    }
    for addr in certificate_lan_addresses(lan) {
        let ip = IpAddr::V4(addr);
        if !ips.contains(&ip) {
            ips.push(ip);
        }
    }
    (hosts, ips)
}

/// The most DNS names, and the most IP addresses, a reissued certificate carries over from
/// the one it replaces (stale LAN addresses would otherwise pile up without end).
const MAX_CARRIED_NAMES: usize = 32;

/// The files of a coordinator's server certificate.
///
/// A coordinator started with `--cert` and `--key` keeps using exactly those files when its
/// certificate is migrated or reissued; the CA that signs it is the one in the folder of
/// `--ca`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerFiles {
    /// The folder holding the CA (`ca.pem` and `ca.key`) that signs the certificate.
    pub ca_dir: PathBuf,
    /// The certificate file.
    pub cert: PathBuf,
    /// The private key file.
    pub key: PathBuf,
}

impl ServerFiles {
    /// The default layout: `server.pem` and `server.key` next to the CA in `dir`.
    #[must_use]
    pub fn in_dir(dir: &Path) -> Self {
        Self {
            ca_dir: dir.to_path_buf(),
            cert: dir.join(pki::SERVER_CERT_FILE),
            key: dir.join(pki::SERVER_KEY_FILE),
        }
    }

    /// The files a `serve` started with `args` uses: `--cert` and `--key` when given, else the
    /// defaults next to `--ca`. `None` without `--ca`.
    #[must_use]
    pub fn from_args(args: &ServeArgs) -> Option<Self> {
        let default = Self::in_dir(&pki::role::pki_dir_of(args.ca.as_deref()?));
        Some(Self {
            cert: args.cert.clone().unwrap_or(default.cert),
            key: args.key.clone().unwrap_or(default.key),
            ca_dir: default.ca_dir,
        })
    }
}

/// How many of the oldest carried names the cap never drops. A certificate lists its names in
/// the order they were added, so the first ones are the operator's own (`--host`, `--ip`, a VPN
/// address) and the first LAN addresses; later ones are the stale LAN addresses of networks the
/// computer has since left.
const PROTECTED_OLD_NAMES: usize = 8;

/// The DNS names and IP addresses of the certificate at `cert_path`, in the order the
/// certificate lists them; empty when it is missing or unreadable.
fn old_names(cert_path: &Path) -> (Vec<String>, Vec<IpAddr>) {
    let Ok(chain) = tls::load_certs(cert_path) else {
        return (Vec::new(), Vec::new());
    };
    chain
        .first()
        .and_then(|leaf| pki::role::subject_alt_names(leaf.as_ref()).ok())
        .unwrap_or_default()
}

/// The names a reissued certificate lists: `base` (the fixed names and the operator's extra
/// ones), then the names `old` carried over, then `current` (this network's addresses).
///
/// `base` and `current` are never dropped. The carried names fill what is left under
/// [`MAX_CARRIED_NAMES`]; when there are more than fit, the first [`PROTECTED_OLD_NAMES`] stay (the
/// operator's original names) and the rest keep the newest, so the oldest stale LAN addresses
/// are the ones that go.
fn carry_capped<T: Clone>(
    base: Vec<T>,
    old: Vec<T>,
    current: Vec<T>,
    same: fn(&T, &T) -> bool,
) -> Vec<T> {
    let mut carried: Vec<T> = Vec::new();
    for name in old {
        let known = base
            .iter()
            .chain(&current)
            .chain(&carried)
            .any(|other| same(other, &name));
        if !known {
            carried.push(name);
        }
    }
    let room = MAX_CARRIED_NAMES.saturating_sub(base.len() + current.len());
    if carried.len() > room {
        let head = PROTECTED_OLD_NAMES.min(room);
        let newest_from = carried.len() - (room - head);
        let mut kept: Vec<T> = carried[..head].to_vec();
        kept.extend_from_slice(&carried[newest_from..]);
        carried = kept;
    }
    let mut all = base;
    all.extend(carried);
    all.extend(current);
    all
}

/// Issues the coordinator's server certificate into `files`, signed by the CA in `files.ca_dir`.
///
/// It is issued for the names of [`certificate_names`] plus every name the certificate it
/// replaces lists (so an operator's `--host`/`--ip` names, VPN addresses and the like survive).
///
/// # Errors
///
/// Whatever [`crate::pki::issue_server_to`] returns: no CA in the folder, a signing failure or
/// an unwritable file.
pub fn issue_server_certificate(
    files: &ServerFiles,
    extra_names: &[String],
    lan: &[LanAddress],
) -> Result<(), PkiError> {
    let (hosts, ips) = certificate_names(lan, extra_names);
    let (old_hosts, old_ips) = old_names(&files.cert);
    let lan_ips: Vec<IpAddr> = certificate_lan_addresses(lan)
        .into_iter()
        .map(IpAddr::V4)
        .collect();
    let (current_ips, base_ips): (Vec<IpAddr>, Vec<IpAddr>) =
        ips.into_iter().partition(|ip| lan_ips.contains(ip));
    let hosts = carry_capped(hosts, old_hosts, Vec::new(), |a, b| {
        a.eq_ignore_ascii_case(b)
    });
    let ips = carry_capped(base_ips, old_ips, current_ips, |a, b| a == b);
    pki::issue_server_to(&files.ca_dir, &files.cert, &files.key, &hosts, &ips)
}

/// Whether the DER certificate lists the fixed coordinator name.
///
/// A DNS name sits in the certificate as plain ASCII, so a byte search is enough here; the
/// real name check is the TLS handshake.
#[must_use]
pub fn certificate_has_fixed_name(der: &[u8]) -> bool {
    contains_bytes(der, tls::COORDINATOR_TLS_NAME.as_bytes())
}

/// Whether the DER certificate lists every address in `ips` (a byte search, like
/// [`certificate_has_fixed_name`]).
#[must_use]
pub fn certificate_lists_addresses(der: &[u8], ips: &[Ipv4Addr]) -> bool {
    ips.iter().all(|ip| contains_bytes(der, &ip.octets()))
}

fn contains_bytes(haystack: &[u8], needle: &[u8]) -> bool {
    !needle.is_empty() && haystack.windows(needle.len()).any(|w| w == needle)
}

/// The first start of a version that knows the fixed name: reissues the server certificate in
/// `files` if it lacks that name.
///
/// The new certificate is signed by the same CA, so every paired device (which pins that
/// CA) keeps working; only the certificate and key files change. Every name the old
/// certificate listed stays in the new one (see [`issue_server_certificate`]), so a client
/// that dials `--host` or `--ip` names, or joins over a VPN, still verifies the certificate. A
/// missing certificate file is not a migration (the wizard or `cert issue-server` creates it)
/// and returns `false`. Returns `true` when it reissued, after logging one plain line.
///
/// # Errors
///
/// [`PkiError::Certificate`] if the certificate exists but cannot be read, otherwise
/// whatever [`issue_server_certificate`] returns.
pub fn migrate_server_certificate(
    files: &ServerFiles,
    extra_names: &[String],
    lan: &[LanAddress],
) -> Result<bool, PkiError> {
    if !files.cert.exists() {
        return Ok(false);
    }
    let chain = tls::load_certs(&files.cert).map_err(|source| PkiError::Certificate {
        path: files.cert.clone(),
        source,
    })?;
    if chain.iter().any(|c| certificate_has_fixed_name(c.as_ref())) {
        return Ok(false);
    }
    issue_server_certificate(files, extra_names, lan)?;
    tracing::info!(
        "pleochroma-worker: reissued the server certificate {} with the fixed name {} (same CA, the old names stay, paired devices keep working)",
        files.cert.display(),
        tls::COORDINATOR_TLS_NAME
    );
    Ok(true)
}

/// Loads a certificate chain and its private key from PEM files into a `rustls`
/// [`CertifiedKey`], ready for [`SwappableCert`].
///
/// # Errors
///
/// A human-readable message if either file is unreadable or the key does not match
/// `rustls`'s signing requirements.
pub fn load_certified_key(cert_path: &Path, key_path: &Path) -> Result<CertifiedKey, String> {
    let chain = tls::load_certs(cert_path).map_err(|e| format!("{}: {e}", cert_path.display()))?;
    let key =
        tls::load_private_key(key_path).map_err(|e| format!("{}: {e}", key_path.display()))?;
    CertifiedKey::from_der(chain, key, &rustls::crypto::ring::default_provider())
        .map_err(|e| format!("{}: {e}", key_path.display()))
}
