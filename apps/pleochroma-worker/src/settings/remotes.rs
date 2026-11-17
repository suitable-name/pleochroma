//! The coordinators this computer manages remotely.
//!
//! `remotes.toml` and the credentials in `remotes/<id>/{ca.pem, client.pem, client.key}`, both in
//! the per-user settings folder (the folder `coordinator.toml` is in).
//!
//! A coordinator is added by redeeming a monitor or administrator pairing code
//! ([`add_from_link`], or [`add_from_code`] for a bare `GW1-` code and a typed address). The
//! claimed certificate bundle is stored with the private key written owner-only, and one entry
//! records the name, the addresses, the access level and the fingerprint of the coordinator's
//! certificate authority. [`remove`] deletes both. Forgetting a coordinator here does not
//! revoke its certificate on the coordinator; that takes "Remove access" there.
//!
//! No function in this module logs, prints or returns a code, a pairing link or a key.

use pleochroma_net::{
    admin::status::AccessRole,
    enroll::{ClaimedBundle, claim_any},
    messages::PeerRole,
    pairing::PairingLink,
    tls,
};
use serde::{Deserialize, Serialize};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    sync::{Mutex, PoisonError},
    time::{SystemTime, UNIX_EPOCH},
};

/// The file that lists the remote coordinators.
pub const REMOTES_FILE: &str = "remotes.toml";

/// The folder (inside the settings folder) that holds one credentials folder per coordinator.
pub const REMOTES_DIR: &str = "remotes";

/// The port a coordinator serves its viewers on when the typed address names none.
pub const DEFAULT_VIEWER_PORT: u16 = 7878;

/// The longest id, in characters.
const MAX_ID_LEN: usize = 40;

/// Serializes every read and write of `remotes.toml` and the credentials folders.
static FILES: Mutex<()> = Mutex::new(());

/// A stable short name for one saved coordinator; it names its credentials folder.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct RemoteId(pub String);

impl std::fmt::Display for RemoteId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// One saved coordinator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEntry {
    /// The stable id (also the credentials folder name).
    pub id: RemoteId,
    /// The coordinator's display name.
    pub name: String,
    /// Where it listens for viewers, as `ip:port`, the address that answered last first.
    pub addresses: Vec<String>,
    /// What this computer may do there: [`AccessRole::Monitor`] or [`AccessRole::Admin`].
    pub role: AccessRole,
    /// The fingerprint of the coordinator's certificate authority, 64 lowercase hex characters.
    pub ca_fingerprint: String,
    /// When this computer last had a session with it, in seconds since 1970 (UTC).
    pub last_seen: Option<u64>,
}

/// `remotes.toml` as written to disk.
#[derive(Debug, Default, Serialize, Deserialize)]
struct DiskFile {
    /// One table per coordinator.
    #[serde(default)]
    remote: Vec<DiskEntry>,
}

/// One `[[remote]]` table.
#[derive(Debug, Serialize, Deserialize)]
struct DiskEntry {
    id: String,
    name: String,
    #[serde(default)]
    addresses: Vec<String>,
    role: String,
    ca_fingerprint: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_seen: Option<u64>,
}

impl DiskEntry {
    /// The entry as the program uses it, or `None` for an unusable id or role.
    fn into_entry(self) -> Option<RemoteEntry> {
        let role = match self.role.as_str() {
            "monitor" => AccessRole::Monitor,
            "admin" => AccessRole::Admin,
            _ => return None,
        };
        valid_id(&self.id).then_some(RemoteEntry {
            id: RemoteId(self.id),
            name: self.name,
            addresses: self.addresses,
            role,
            ca_fingerprint: self.ca_fingerprint,
            last_seen: self.last_seen,
        })
    }
}

impl From<&RemoteEntry> for DiskEntry {
    fn from(entry: &RemoteEntry) -> Self {
        Self {
            id: entry.id.0.clone(),
            name: entry.name.clone(),
            addresses: entry.addresses.clone(),
            role: match entry.role {
                AccessRole::Admin => "admin",
                // `Local` is never stored; reading it back as a monitor is the safe side.
                AccessRole::Monitor | AccessRole::Local => "monitor",
            }
            .to_owned(),
            ca_fingerprint: entry.ca_fingerprint.clone(),
            last_seen: entry.last_seen,
        }
    }
}

/// True for an id this module would make: lower-case letters, digits and dashes.
#[must_use]
pub fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= MAX_ID_LEN
        && !id.starts_with('-')
        && id
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// The per-user settings folder, or `None` before the settings are loaded.
#[must_use]
pub fn base_dir() -> Option<PathBuf> {
    let file = super::settings_file()?;
    Some(
        file.parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf),
    )
}

/// Seconds since 1970 now.
#[must_use]
pub fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| elapsed.as_secs())
}

/// The folder that holds one coordinator's `ca.pem`, `client.pem` and `client.key`.
#[must_use]
pub fn cert_dir_in(base: &Path, id: &RemoteId) -> PathBuf {
    base.join(REMOTES_DIR).join(&id.0)
}

fn lock() -> std::sync::MutexGuard<'static, ()> {
    FILES.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Every saved coordinator, in the order they were added; empty before the settings load.
#[must_use]
pub fn list() -> Vec<RemoteEntry> {
    base_dir().map(|dir| list_in(&dir)).unwrap_or_default()
}

/// Redeems a monitor or administrator pairing link and saves the coordinator.
///
/// # Errors
///
/// Plain words: the text is not a pairing link, the code is for a phone, a viewer or a
/// render computer, the coordinator cannot be reached or refuses the code, or the credentials
/// cannot be saved.
pub fn add_from_link(link: &str) -> Result<RemoteId, String> {
    let parsed = PairingLink::parse(link).map_err(|e| e.to_string())?;
    check_link_role(parsed.role)?;
    let base = base_dir().ok_or_else(settings_not_ready)?;
    let ca_fingerprint = tls::fingerprint_to_hex(
        &pleochroma_net::token::decode(parsed.token())
            .map_err(|_| "That is not a valid pairing code.".to_owned())?
            .ca_fingerprint,
    );
    let enroll_addrs: Vec<SocketAddr> = parsed
        .addrs
        .iter()
        .map(|a| SocketAddr::new(a.ip(), parsed.enroll_port))
        .collect();
    let outcome = claim_any(parsed.token(), &enroll_addrs).map_err(|e| e.to_string())?;

    // The address that answered goes first.
    let mut addresses: Vec<String> = Vec::with_capacity(parsed.addrs.len());
    for addr in parsed
        .addrs
        .iter()
        .filter(|a| a.ip() == outcome.addr.ip())
        .chain(parsed.addrs.iter().filter(|a| a.ip() != outcome.addr.ip()))
    {
        addresses.push(addr.to_string());
    }
    let name = if parsed.name.trim().is_empty() {
        "Coordinator".to_owned()
    } else {
        parsed.name
    };
    add_in(&base, &name, addresses, &ca_fingerprint, &outcome.bundle)
}

/// Redeems a bare `GW1-` code against the computer at `address` and saves it.
///
/// `address` is the coordinator's viewer address, `ip` or `ip:port` (port 7878 when left out);
/// its enrollment listener is the next port up. The access level comes from the certificate the
/// coordinator issues.
///
/// # Errors
///
/// As [`add_from_link`], plus an address that is not an IPv4 address.
pub fn add_from_code(code: &str, address: &str) -> Result<RemoteId, String> {
    let viewer = parse_viewer_address(address)?;
    let enroll_port = viewer
        .port()
        .checked_add(1)
        .ok_or_else(|| "That address uses a port that cannot be paired on.".to_owned())?;
    let decoded = pleochroma_net::token::decode(code.trim())
        .map_err(|_| "That is not a valid pairing code.".to_owned())?;
    let ca_fingerprint = tls::fingerprint_to_hex(&decoded.ca_fingerprint);
    drop(decoded);
    let base = base_dir().ok_or_else(settings_not_ready)?;
    let outcome = claim_any(code.trim(), &[SocketAddr::new(viewer.ip(), enroll_port)])
        .map_err(|e| e.to_string())?;
    let name = format!("Computer at {}", viewer.ip());
    add_in(
        &base,
        &name,
        vec![viewer.to_string()],
        &ca_fingerprint,
        &outcome.bundle,
    )
}

/// Forgets a coordinator on this computer: deletes its credentials folder and its entry. It
/// cannot revoke the certificate on the coordinator. An unknown id is ignored.
pub fn remove(id: &RemoteId) {
    if let Some(base) = base_dir() {
        remove_in(&base, id);
    }
}

/// The saved coordinator with `id`.
#[must_use]
pub fn find_in(base: &Path, id: &RemoteId) -> Option<RemoteEntry> {
    list_in(base).into_iter().find(|e| &e.id == id)
}

/// Changes one saved entry and writes the file. Returns false when the id is unknown or the
/// file cannot be written.
pub fn update_in(base: &Path, id: &RemoteId, change: impl FnOnce(&mut RemoteEntry)) -> bool {
    let _guard = lock();
    let mut entries = load_unlocked(base);
    let Some(entry) = entries.iter_mut().find(|e| &e.id == id) else {
        return false;
    };
    change(entry);
    save_unlocked(base, &entries).is_ok()
}

/// Every entry in `base/remotes.toml`.
#[must_use]
pub fn list_in(base: &Path) -> Vec<RemoteEntry> {
    let _guard = lock();
    load_unlocked(base)
}

/// Writes `entries` as `base/remotes.toml`.
///
/// # Errors
///
/// Plain words when the file cannot be written.
pub fn save_in(base: &Path, entries: &[RemoteEntry]) -> Result<(), String> {
    let _guard = lock();
    save_unlocked(base, entries)
}

/// [`remove`] for the settings folder `base`.
pub fn remove_in(base: &Path, id: &RemoteId) {
    if !valid_id(&id.0) {
        return;
    }
    let _guard = lock();
    let mut entries = load_unlocked(base);
    let before = entries.len();
    entries.retain(|e| &e.id != id);
    if entries.len() != before {
        // A failed write leaves the entry; the folder is removed either way so no key stays.
        let _ = save_unlocked(base, &entries);
    }
    let _ = std::fs::remove_dir_all(cert_dir_in(base, id));
}

/// Stores a claimed bundle and its entry under `base`. A coordinator that is already saved
/// (same authority) keeps its id and gets the new credentials.
///
/// # Errors
///
/// Plain words: the certificate is not a monitor or administrator certificate, or the
/// credentials or the file cannot be written.
pub fn add_in(
    base: &Path,
    name: &str,
    addresses: Vec<String>,
    ca_fingerprint: &str,
    bundle: &ClaimedBundle,
) -> Result<RemoteId, String> {
    let _guard = lock();
    let mut entries = load_unlocked(base);
    let existing = entries
        .iter()
        .position(|e| e.ca_fingerprint == ca_fingerprint);
    let id = existing.map_or_else(
        || RemoteId(unique_id(name, &entries)),
        |i| entries[i].id.clone(),
    );

    // Write into a staging folder first, so a refused bundle leaves nothing behind and a
    // re-pairing never loses the old credentials to a failure.
    let remotes = base.join(REMOTES_DIR);
    let staging = remotes.join(format!(".staging-{}", id.0));
    let _ = std::fs::remove_dir_all(&staging);
    let role = stage_bundle(&staging, bundle).inspect_err(|_| {
        let _ = std::fs::remove_dir_all(&staging);
    })?;

    let target = remotes.join(&id.0);
    let _ = std::fs::remove_dir_all(&target);
    std::fs::rename(&staging, &target).map_err(|e| {
        let _ = std::fs::remove_dir_all(&staging);
        format!("Could not save the access for {name}: {e}")
    })?;

    let entry = RemoteEntry {
        id: id.clone(),
        name: name.to_owned(),
        addresses,
        role,
        ca_fingerprint: ca_fingerprint.to_owned(),
        last_seen: Some(unix_now()),
    };
    if let Some(i) = existing {
        entries[i] = entry;
    } else {
        entries.push(entry);
    }
    save_unlocked(base, &entries)?;
    Ok(id)
}

/// Writes the bundle into `dir` and returns the access level of its certificate.
fn stage_bundle(dir: &Path, bundle: &ClaimedBundle) -> Result<AccessRole, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("Could not create the access folder: {e}"))?;
    std::fs::write(dir.join(crate::pki::CA_CERT_FILE), &bundle.ca_pem)
        .map_err(|e| format!("Could not save the access: {e}"))?;
    std::fs::write(
        dir.join(crate::pki::CLIENT_CERT_FILE),
        &bundle.client_cert_pem,
    )
    .map_err(|e| format!("Could not save the access: {e}"))?;
    tls::write_private_key_pem(
        &dir.join(crate::pki::CLIENT_KEY_FILE),
        &bundle.client_key_pem,
    )
    .map_err(|e| format!("Could not save the access safely: {e}"))?;

    let certs = tls::load_certs(&dir.join(crate::pki::CLIENT_CERT_FILE))
        .map_err(|_| "The coordinator sent a certificate this computer cannot read.".to_owned())?;
    let first = certs
        .first()
        .ok_or_else(|| "The coordinator sent no certificate.".to_owned())?;
    let role = crate::pki::role_of_certificate(first)
        .map_err(|_| "The coordinator sent a certificate this computer cannot read.".to_owned())?;
    match role {
        PeerRole::Monitor => Ok(AccessRole::Monitor),
        PeerRole::Admin => Ok(AccessRole::Admin),
        PeerRole::Viewer | PeerRole::Worker => Err(wrong_role_text(role).to_owned()),
    }
}

/// Refuses a link whose code is not for remote management.
fn check_link_role(role: PeerRole) -> Result<(), String> {
    match role {
        PeerRole::Monitor | PeerRole::Admin => Ok(()),
        PeerRole::Viewer | PeerRole::Worker => Err(wrong_role_text(role).to_owned()),
    }
}

/// The plain refusal for a code of the wrong kind.
const fn wrong_role_text(role: PeerRole) -> &'static str {
    if matches!(role, PeerRole::Worker) {
        "This code is for a render computer, not for managing a coordinator. On the \
         coordinator, make a monitor or admin code instead."
    } else {
        "This code is for a phone or viewer, not for managing a coordinator. On the \
         coordinator, make a monitor or admin code instead."
    }
}

fn settings_not_ready() -> String {
    "The settings folder is not ready yet. Try again in a moment.".to_owned()
}

/// `ip` or `ip:port` as a viewer address.
fn parse_viewer_address(text: &str) -> Result<SocketAddr, String> {
    let text = text.trim();
    let bad =
        || "Type the coordinator's address like 192.168.1.20 or 192.168.1.20:7878.".to_owned();
    if let Ok(addr) = text.parse::<SocketAddr>() {
        return match addr.ip() {
            IpAddr::V4(ip) if !ip.is_unspecified() && addr.port() != 0 => {
                Ok(SocketAddr::new(IpAddr::V4(ip), addr.port()))
            }
            _ => Err(bad()),
        };
    }
    match text.parse::<Ipv4Addr>() {
        Ok(ip) if !ip.is_unspecified() => Ok(SocketAddr::new(IpAddr::V4(ip), DEFAULT_VIEWER_PORT)),
        _ => Err(bad()),
    }
}

/// A short id from `name` that no entry uses yet.
fn unique_id(name: &str, entries: &[RemoteEntry]) -> String {
    let base = slug(name);
    let taken = |candidate: &str| entries.iter().any(|e| e.id.0 == candidate);
    if !taken(&base) {
        return base;
    }
    (2_u32..=u32::MAX)
        .map(|n| format!("{base}-{n}"))
        .find(|candidate| !taken(candidate))
        .unwrap_or_else(|| base.clone())
}

/// Lower-case letters and digits of `name`, joined by single dashes.
fn slug(name: &str) -> String {
    let mut out = String::new();
    let mut pending_dash = false;
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            if pending_dash && !out.is_empty() {
                out.push('-');
            }
            pending_dash = false;
            out.push(c.to_ascii_lowercase());
        } else {
            pending_dash = true;
        }
    }
    out.truncate(MAX_ID_LEN - 6);
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        "coordinator".to_owned()
    } else {
        out
    }
}

/// Reads `base/remotes.toml`. A file that cannot be parsed is set aside as
/// `remotes.toml.broken` and the list starts empty.
fn load_unlocked(base: &Path) -> Vec<RemoteEntry> {
    let path = base.join(REMOTES_FILE);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    toml::from_str::<DiskFile>(&text).map_or_else(
        |_| {
            let _ = std::fs::rename(&path, base.join(format!("{REMOTES_FILE}.broken")));
            Vec::new()
        },
        |file| {
            file.remote
                .into_iter()
                .filter_map(DiskEntry::into_entry)
                .collect()
        },
    )
}

/// Writes `entries` to a temporary file and renames it over `base/remotes.toml`.
fn save_unlocked(base: &Path, entries: &[RemoteEntry]) -> Result<(), String> {
    let file = DiskFile {
        remote: entries.iter().map(DiskEntry::from).collect(),
    };
    let text = toml::to_string_pretty(&file)
        .map_err(|e| format!("Could not write the list of coordinators: {e}"))?;
    std::fs::create_dir_all(base)
        .map_err(|e| format!("Could not create the settings folder: {e}"))?;
    let path = base.join(REMOTES_FILE);
    let tmp = base.join(format!("{REMOTES_FILE}.tmp"));
    std::fs::write(&tmp, text)
        .map_err(|e| format!("Could not write the list of coordinators: {e}"))?;
    std::fs::rename(&tmp, &path)
        .map_err(|e| format!("Could not write the list of coordinators: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// A fresh empty folder under the system temp folder.
    fn temp_base(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pleochroma-remotes-test-{tag}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or(Duration::ZERO)
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn entry(id: &str, role: AccessRole) -> RemoteEntry {
        RemoteEntry {
            id: RemoteId(id.to_owned()),
            name: format!("Name of {id}"),
            addresses: vec!["192.168.1.20:7878".to_owned(), "10.0.0.5:7878".to_owned()],
            role,
            ca_fingerprint: "ab".repeat(32),
            last_seen: Some(1_700_000_000),
        }
    }

    /// A pairing link for `role` with a well-formed token.
    fn link_for(role: PeerRole) -> String {
        let token = pleochroma_net::token::encode(&[3_u8; 32], &[7_u8; 32]);
        PairingLink::new(
            role,
            "Garage server",
            vec!["127.0.0.1:7878".parse().unwrap()],
            7879,
            token,
        )
        .build()
        .unwrap()
    }

    /// A claimed bundle for `role`, made by a throw-away certificate authority.
    fn bundle_for(base: &Path, role: PeerRole) -> ClaimedBundle {
        let pki_dir = base.join("pki");
        crate::pki::init(&pki_dir).unwrap();
        let issued = crate::pki::issue_client_in_memory(&pki_dir, "Laptop", role).unwrap();
        ClaimedBundle {
            ca_pem: issued.ca_pem,
            client_cert_pem: issued.client_cert_pem,
            client_key_pem: zeroize::Zeroizing::new(issued.client_key_pem),
        }
    }

    #[test]
    fn remotes_toml_round_trips() {
        let base = temp_base("roundtrip");
        assert!(list_in(&base).is_empty());
        let entries = vec![
            entry("garage-server", AccessRole::Admin),
            RemoteEntry {
                last_seen: None,
                ..entry("attic", AccessRole::Monitor)
            },
        ];
        save_in(&base, &entries).unwrap();
        assert_eq!(list_in(&base), entries);
        let text = std::fs::read_to_string(base.join(REMOTES_FILE)).unwrap();
        assert!(text.contains("[[remote]]"), "{text}");
        assert!(text.contains("role = \"admin\""), "{text}");
        assert!(!base.join(format!("{REMOTES_FILE}.tmp")).exists());
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn unusable_entries_are_skipped_and_a_broken_file_is_set_aside() {
        let base = temp_base("skip");
        std::fs::write(
            base.join(REMOTES_FILE),
            "[[remote]]\nid = \"../evil\"\nname = \"x\"\nrole = \"admin\"\nca_fingerprint = \"\"\n\
             [[remote]]\nid = \"good\"\nname = \"x\"\nrole = \"local\"\nca_fingerprint = \"\"\n\
             [[remote]]\nid = \"fine\"\nname = \"x\"\nrole = \"monitor\"\nca_fingerprint = \"\"\n",
        )
        .unwrap();
        let listed = list_in(&base);
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].id, RemoteId("fine".to_owned()));
        assert_eq!(listed[0].role, AccessRole::Monitor);

        std::fs::write(base.join(REMOTES_FILE), "this is [not toml").unwrap();
        assert!(list_in(&base).is_empty());
        assert!(base.join(format!("{REMOTES_FILE}.broken")).exists());
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn ids_are_short_safe_and_unique() {
        assert_eq!(slug("Garage server"), "garage-server");
        assert_eq!(slug("  Büro-PC (2) "), "b-ro-pc-2");
        assert_eq!(slug("!!!"), "coordinator");
        assert!(valid_id(&slug(&"x".repeat(200))));
        assert!(!valid_id(""));
        assert!(!valid_id("../x"));
        assert!(!valid_id("A"));
        assert!(!valid_id(".staging-x"));
        let taken = vec![entry("garage-server", AccessRole::Admin)];
        assert_eq!(unique_id("Garage server", &taken), "garage-server-2");
        assert_eq!(unique_id("Attic", &taken), "attic");
    }

    #[test]
    fn a_viewer_link_is_refused_in_plain_words() {
        let err = add_from_link(&link_for(PeerRole::Viewer)).unwrap_err();
        assert!(
            err.starts_with("This code is for a phone or viewer"),
            "{err}"
        );
        assert!(!err.contains("GW1"), "{err}");
    }

    #[test]
    fn a_worker_link_is_refused_in_plain_words() {
        let err = add_from_link(&link_for(PeerRole::Worker)).unwrap_err();
        assert!(
            err.starts_with("This code is for a render computer"),
            "{err}"
        );
    }

    #[test]
    fn text_that_is_not_a_link_is_refused() {
        let err = add_from_link("hello there").unwrap_err();
        assert!(err.contains("pairing link"), "{err}");
        assert!(add_from_code("not-a-code", "192.168.1.20").is_err());
        assert!(
            add_from_code(
                &pleochroma_net::token::encode(&[1; 32], &[2; 32]),
                "not an address"
            )
            .unwrap_err()
            .contains("address")
        );
    }

    #[test]
    fn viewer_addresses_default_their_port() {
        assert_eq!(
            parse_viewer_address("192.168.1.20").unwrap(),
            "192.168.1.20:7878".parse().unwrap()
        );
        assert_eq!(
            parse_viewer_address(" 10.0.0.5:9000 ").unwrap(),
            "10.0.0.5:9000".parse().unwrap()
        );
        assert!(parse_viewer_address("0.0.0.0").is_err());
        assert!(parse_viewer_address("::1").is_err());
        assert!(parse_viewer_address("host.local").is_err());
    }

    #[test]
    fn a_monitor_bundle_is_stored_and_removed() {
        let base = temp_base("store");
        let bundle = bundle_for(&base, PeerRole::Monitor);
        let id = add_in(
            &base,
            "Garage server",
            vec!["192.168.1.20:7878".to_owned()],
            &"cd".repeat(32),
            &bundle,
        )
        .unwrap();
        assert_eq!(id, RemoteId("garage-server".to_owned()));
        let dir = cert_dir_in(&base, &id);
        for file in ["ca.pem", "client.pem", "client.key"] {
            assert!(dir.join(file).is_file(), "{file}");
        }
        let saved = find_in(&base, &id).unwrap();
        assert_eq!(saved.role, AccessRole::Monitor);
        assert_eq!(saved.addresses, vec!["192.168.1.20:7878".to_owned()]);
        assert!(saved.last_seen.is_some());
        let toml_text = std::fs::read_to_string(base.join(REMOTES_FILE)).unwrap();
        assert!(!toml_text.contains("PRIVATE"), "{toml_text}");

        // The same authority again keeps the id and replaces the access.
        let admin_bundle = bundle_for(&temp_base("store-admin"), PeerRole::Admin);
        let again = add_in(
            &base,
            "Garage server",
            vec!["192.168.1.21:7878".to_owned()],
            &"cd".repeat(32),
            &admin_bundle,
        )
        .unwrap();
        assert_eq!(again, id);
        assert_eq!(list_in(&base).len(), 1);
        assert_eq!(find_in(&base, &id).unwrap().role, AccessRole::Admin);

        assert!(update_in(&base, &id, |e| e.last_seen = Some(5)));
        assert_eq!(find_in(&base, &id).unwrap().last_seen, Some(5));
        assert!(!update_in(&base, &RemoteId("nobody".to_owned()), |_| {}));

        remove_in(&base, &id);
        assert!(list_in(&base).is_empty());
        assert!(!dir.exists());
        remove_in(&base, &RemoteId("../pki".to_owned()));
        assert!(base.join("pki").exists(), "an unsafe id deletes nothing");
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn a_viewer_certificate_is_refused_after_the_claim_and_leaves_nothing() {
        let base = temp_base("viewer-cert");
        let bundle = bundle_for(&base, PeerRole::Viewer);
        let err = add_in(&base, "Garage", vec![], &"ef".repeat(32), &bundle).unwrap_err();
        assert!(
            err.starts_with("This code is for a phone or viewer"),
            "{err}"
        );
        assert!(list_in(&base).is_empty());
        assert!(!cert_dir_in(&base, &RemoteId("garage".to_owned())).exists());
        assert!(!base.join(REMOTES_DIR).join(".staging-garage").exists());
        std::fs::remove_dir_all(&base).ok();
    }
}
