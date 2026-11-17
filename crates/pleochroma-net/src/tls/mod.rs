//! Mutual-TLS configuration for `pleochroma-net`'s transport.
//!
//! Loading certs/keys from PEM, building `rustls` configs against a private CA, and the
//! client-certificate fingerprint allowlist that stands in for revocation. Pinned to
//! TLS 1.3 -- `rustls`'s `tls12` feature is left disabled entirely, not merely
//! unconfigured at runtime.
//!
//! **Socket-free**, consistent with the rest of this crate: every function here takes
//! bytes/paths in and returns a `rustls::ServerConfig`/`rustls::ClientConfig`/fingerprint
//! out, none of it touching a `TcpStream`. Wrapping an actual socket in
//! `rustls::StreamOwned` happens at the call site. Certificate *generation* (the CA and
//! leaf certs) is deliberately NOT here either -- that's `apps/pleochroma-worker`'s `pki`
//! module; this module is what loads an already-issued bundle, shared by worker and
//! viewer alike.
//!
//! # Trust model
//!
//! [`server_config`] and [`client_config`] both verify the peer's certificate chain
//! against a private CA using `rustls`'s own built-in `webpki`-based verifiers, never a
//! hand-written `ServerCertVerifier`/`ClientCertVerifier` impl (that's how validation
//! quietly gets disabled in production -- skip verification for local testing via
//! `pleochroma-worker serve --insecure-no-tls` instead, which is loopback-only and warns
//! on every accepted connection).
//!
//! [`server_config`] also REQUIRES a client certificate -- no anonymous-client path --
//! since that mutual check is what replaces a password in this design.
//!
//! CA-chain validity is necessary but not sufficient: anyone with a CA-signed
//! certificate can complete the handshake. The actual authorization decision is the
//! [`Allowlist`] of client-certificate SHA-256 fingerprints, checked by the caller after
//! the handshake against `rustls::ServerConnection::peer_certificates()`. Revoking a
//! client means deleting its line from the allowlist file; there is no CRL or OCSP.

use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fmt,
    io::{self, BufReader},
    path::{Path, PathBuf},
    sync::Arc,
};

#[cfg(feature = "pairing")]
mod fixed_name;
mod key_file;

pub use key_file::write_private_key_pem;

/// The fixed DNS name every coordinator certificate carries, whatever addresses it has.
///
/// A client built with `client_config_fixed_name` (the `pairing` feature) verifies the server against this name
/// instead of the dialed address, so a coordinator whose address changes keeps working. The
/// `.invalid` top-level domain is reserved (RFC 2606) and never resolves; the name is only
/// ever compared against the certificate, never looked up. Trust still rests on the CA pin.
pub const COORDINATOR_TLS_NAME: &str = "coordinator.pleochroma.invalid";

/// A SHA-256 client-certificate fingerprint, as stored in an [`Allowlist`].
pub type Fingerprint = [u8; 32];

/// The host part of a `host:port` address, as `rustls::pki_types::ServerName` accepts it.
///
/// Everything before the last `:`, with the square brackets of an IPv6 literal
/// (`[::1]:7879`) removed; `None` if `addr` has no `:`. Every dialer (enrollment, join,
/// desktop viewer) uses this one rule. Full validation still happens in
/// `ServerName::try_from`.
#[must_use]
pub fn host_for_server_name(addr: &str) -> Option<&str> {
    let (host, _port) = addr.rsplit_once(':')?;
    Some(
        host.strip_prefix('[')
            .and_then(|h| h.strip_suffix(']'))
            .unwrap_or(host),
    )
}

/// Everything that can go wrong loading certs/keys from disk or building a `rustls`
/// config from them.
///
/// Carries the offending path (and, for a `rustls` failure, the inner error's own
/// `Display` text) rather than collapsing to a generic "handshake failed", so an expired
/// certificate, a clock-skew rejection, and a CA mismatch stay distinguishable.
#[derive(Debug)]
pub enum TlsError {
    /// `path` couldn't be opened or read.
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    /// `path` was read but contained no PEM-encoded certificate.
    NoCertificates { path: PathBuf },
    /// `path` was read but contained no recognizable private key block.
    NoPrivateKey { path: PathBuf },
    /// `rustls` itself refused a certificate, key, or handshake (expired, clock skew,
    /// wrong CA, no matching SAN, or a malformed config) -- its own `Display` names which.
    Rustls(rustls::Error),
    /// A malformed line in an [`Allowlist`] file: not blank, not a `#` comment, and not
    /// 64 hex characters.
    MalformedAllowlistLine {
        path: PathBuf,
        line_number: usize,
        line: String,
    },
}

impl fmt::Display for TlsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => write!(f, "{}: {source}", path.display()),
            Self::NoCertificates { path } => {
                write!(f, "{}: no PEM-encoded certificate found", path.display())
            }
            Self::NoPrivateKey { path } => {
                write!(f, "{}: no PEM-encoded private key found", path.display())
            }
            // e.g. "invalid peer certificate: Expired" / "NotValidYet" / "UnknownIssuer".
            Self::Rustls(e) => write!(f, "TLS error: {e}"),
            Self::MalformedAllowlistLine {
                path,
                line_number,
                line,
            } => {
                write!(
                    f,
                    "{}:{line_number}: not a 64-character hex fingerprint: {line:?}",
                    path.display()
                )
            }
        }
    }
}

impl std::error::Error for TlsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Rustls(e) => Some(e),
            _ => None,
        }
    }
}

impl From<rustls::Error> for TlsError {
    fn from(e: rustls::Error) -> Self {
        Self::Rustls(e)
    }
}

fn io_err(path: &Path, source: std::io::Error) -> TlsError {
    TlsError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Loads every PEM-encoded certificate in `path`, in file order (a leaf certificate
/// followed by any intermediates, for a full chain).
///
/// # Errors
///
/// [`TlsError::Io`] if `path` can't be opened or read, [`TlsError::NoCertificates`] if
/// it contains no PEM `CERTIFICATE` block at all.
pub fn load_certs(path: &Path) -> Result<Vec<CertificateDer<'static>>, TlsError> {
    let file = std::fs::File::open(path).map_err(|e| io_err(path, e))?;
    let mut reader = BufReader::new(file);
    let certs: Vec<CertificateDer<'static>> = rustls_pemfile::certs(&mut reader)
        .collect::<Result<_, _>>()
        .map_err(|e| io_err(path, e))?;
    if certs.is_empty() {
        return Err(TlsError::NoCertificates {
            path: path.to_path_buf(),
        });
    }
    Ok(certs)
}

/// Loads the first PEM-encoded private key found in `path` (PKCS#8, PKCS#1, or SEC1).
///
/// # Errors
///
/// [`TlsError::Io`] if `path` can't be opened or read, [`TlsError::NoPrivateKey`] if it
/// contains no recognizable private key block.
pub fn load_private_key(path: &Path) -> Result<PrivateKeyDer<'static>, TlsError> {
    let file = std::fs::File::open(path).map_err(|e| io_err(path, e))?;
    let mut reader = BufReader::new(file);
    rustls_pemfile::private_key(&mut reader)
        .map_err(|e| io_err(path, e))?
        .ok_or_else(|| TlsError::NoPrivateKey {
            path: path.to_path_buf(),
        })
}

/// Loads a CA bundle from `path` into a `rustls::RootCertStore`.
///
/// Typically one self-signed certificate -- the trust anchor both [`server_config`]
/// (for verifying client certs) and [`client_config`] (for verifying the server cert)
/// are built with.
///
/// # Errors
///
/// Whatever [`load_certs`] returns, plus [`TlsError::Rustls`] if a loaded certificate
/// isn't a well-formed DER certificate `rustls` can add as a trust anchor.
pub fn load_ca(path: &Path) -> Result<rustls::RootCertStore, TlsError> {
    let certs = load_certs(path)?;
    let mut store = rustls::RootCertStore::empty();
    for cert in certs {
        store.add(cert)?;
    }
    Ok(store)
}

/// Builds a server-side mutual-TLS config, TLS 1.3 only.
///
/// Presents `cert_chain`/`key` to connecting peers, and REQUIRES each peer to present a
/// certificate signed by `ca` -- there is no anonymous-client path.
///
/// This is CA-chain validation only ("was this client certificate signed by a CA I
/// trust"), not "which specific signed client should be trusted" -- pair it with an
/// [`Allowlist`] check against the peer's certificate fingerprint after the handshake.
///
/// # Errors
///
/// [`TlsError::Rustls`] if `ca` is empty (nothing would ever verify) or `cert_chain`/
/// `key` don't form a valid, matching certificate and private key.
pub fn server_config(
    ca: rustls::RootCertStore,
    cert_chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
) -> Result<Arc<rustls::ServerConfig>, TlsError> {
    let verifier = rustls::server::WebPkiClientVerifier::builder(Arc::new(ca))
        .build()
        .map_err(|e| TlsError::Rustls(rustls::Error::General(e.to_string())))?;

    let config = rustls::ServerConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
        .with_client_cert_verifier(verifier)
        .with_single_cert(cert_chain, key)?;

    Ok(Arc::new(config))
}

/// Builds a client-side mutual-TLS config, TLS 1.3 only.
///
/// Presents `cert_chain`/`key` as its own client certificate, and verifies the server's
/// certificate against `ca` using `rustls`'s standard `webpki` server verifier (installed
/// by `with_root_certificates`, not a custom `ServerCertVerifier`).
///
/// # Errors
///
/// [`TlsError::Rustls`] if `cert_chain`/`key` don't form a valid, matching certificate
/// and private key.
pub fn client_config(
    ca: rustls::RootCertStore,
    cert_chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
) -> Result<Arc<rustls::ClientConfig>, TlsError> {
    let config = rustls::ClientConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
        .with_root_certificates(ca)
        .with_client_auth_cert(cert_chain, key)?;

    Ok(Arc::new(config))
}

/// [`client_config`] for a pairing client: the server is verified against
/// [`COORDINATOR_TLS_NAME`] whatever address was dialed.
///
/// Everything else matches [`client_config`] (TLS 1.3 only, the same CA, the same client
/// certificate), and the chain is still checked by `rustls`'s own `webpki` verifier. A server
/// certificate that lacks the fixed name (a coordinator older than 0.12.0) is checked against
/// the dialed name instead, as [`client_config`] does. Older clients and builds without the
/// `pairing` feature keep using [`client_config`].
///
/// # Errors
///
/// [`TlsError::Rustls`] if `ca` is empty or `cert_chain`/`key` don't form a valid, matching
/// certificate and private key.
#[cfg(feature = "pairing")]
pub fn client_config_fixed_name(
    ca: rustls::RootCertStore,
    cert_chain: Vec<CertificateDer<'static>>,
    key: PrivateKeyDer<'static>,
) -> Result<Arc<rustls::ClientConfig>, TlsError> {
    let verifier = Arc::new(fixed_name::FixedNameVerifier::new(ca)?);
    let config = rustls::ClientConfig::builder_with_protocol_versions(&[&rustls::version::TLS13])
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_client_auth_cert(cert_chain, key)?;

    Ok(Arc::new(config))
}

/// The SHA-256 fingerprint of a DER-encoded certificate -- the identity an
/// [`Allowlist`] entry names.
///
/// Computed over the whole DER certificate (not just the public key), so reissuing a
/// certificate for the same key produces a different fingerprint and needs a fresh
/// allowlist entry.
#[must_use]
pub fn fingerprint(cert: &CertificateDer<'_>) -> Fingerprint {
    let mut hasher = Sha256::new();
    hasher.update(cert.as_ref());
    hasher.finalize().into()
}

/// Formats a fingerprint as 64 lowercase hex characters -- the [`Allowlist`] file's
/// on-disk format.
#[must_use]
pub fn fingerprint_to_hex(fp: &Fingerprint) -> String {
    use std::fmt::Write as _;
    fp.iter()
        .fold(String::with_capacity(fp.len() * 2), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

/// Parses a 64-character hex string (whitespace-trimmed) back into a [`Fingerprint`].
/// Returns `None` for anything else -- wrong length, non-hex characters.
#[must_use]
pub fn fingerprint_from_hex(s: &str) -> Option<Fingerprint> {
    let s = s.trim();
    if s.len() != 64 || !s.is_ascii() {
        return None;
    }
    let mut out = [0u8; 32];
    for (i, byte_slot) in out.iter_mut().enumerate() {
        *byte_slot = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

/// An allowlist of trusted client-certificate SHA-256 fingerprints -- the whole
/// revocation story for this design (no CRL, no OCSP).
///
/// # File format
///
/// One entry per line: a 64-character hex fingerprint, optionally followed by
/// whitespace and a `#`-prefixed comment (conventionally the `--name` it was issued
/// under, e.g. `... # laptop`). Blank lines and `#`-comment lines are ignored. Revoking a
/// client is deleting its line -- meant to be hand-editable, not just machine-written.
/// The trailing comment is the device's name as the coordinator window shows it
/// ([`Allowlist::entries`]); a line without one is an unnamed device.
///
/// A line that isn't blank, a comment, or 64 hex characters is a load error rather than
/// silently skipped: a typo silently dropping an entry would fail open (nobody notices a
/// client is no longer allowed), the wrong direction for an allowlist to fail in.
#[derive(Debug, Default, Clone)]
pub struct Allowlist {
    fingerprints: HashSet<Fingerprint>,
    entries: Vec<AllowlistEntry>,
}

/// One allowlist line: the fingerprint and the name written after it, if any.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllowlistEntry {
    /// The client certificate's fingerprint.
    pub fingerprint: Fingerprint,
    /// The trailing `# name` comment, trimmed; `None` for a bare fingerprint (a line written
    /// before names were kept, or by hand).
    pub name: Option<String>,
}

impl AllowlistEntry {
    /// The name to show: the stored one, or "Unnamed device (middle dot) 3B9E..." with the first
    /// four hex characters of the fingerprint in upper case.
    #[must_use]
    pub fn display_name(&self) -> String {
        self.name.as_ref().map_or_else(
            || {
                let hex = fingerprint_to_hex(&self.fingerprint);
                format!("Unnamed device \u{b7} {}\u{2026}", hex[..4].to_uppercase())
            },
            Clone::clone,
        )
    }
}

impl Allowlist {
    /// Loads an allowlist from `path`. See the struct docs for the file format.
    ///
    /// # Errors
    ///
    /// [`TlsError::Io`] if `path` can't be opened or read, including if it doesn't
    /// exist -- fail-closed is deliberate; a caller wanting "no allowlist yet" to mean
    /// "trust nobody" should check [`Path::exists`] first.
    /// [`TlsError::MalformedAllowlistLine`] for the first line that isn't blank, a
    /// comment, or a valid fingerprint.
    pub fn load(path: &Path) -> Result<Self, TlsError> {
        let contents = std::fs::read_to_string(path).map_err(|e| io_err(path, e))?;
        let mut fingerprints = HashSet::new();
        let mut entries = Vec::new();
        for (i, raw_line) in contents.lines().enumerate() {
            let (code, comment) = raw_line.split_once('#').unwrap_or((raw_line, ""));
            let line = code.trim();
            if line.is_empty() {
                continue;
            }
            let fp =
                fingerprint_from_hex(line).ok_or_else(|| TlsError::MalformedAllowlistLine {
                    path: path.to_path_buf(),
                    line_number: i + 1,
                    line: raw_line.to_string(),
                })?;
            if fingerprints.insert(fp) {
                let name = comment.trim();
                entries.push(AllowlistEntry {
                    fingerprint: fp,
                    name: (!name.is_empty()).then(|| name.to_string()),
                });
            }
        }
        Ok(Self {
            fingerprints,
            entries,
        })
    }

    /// Every entry in file order, each fingerprint once (the first line wins).
    #[must_use]
    pub fn entries(&self) -> &[AllowlistEntry] {
        &self.entries
    }

    /// Removes every line of `fp` from the allowlist file at `path`.
    ///
    /// Comments, blank lines and all other entries stay. Returns whether a line was removed
    /// (`false` if `fp` was not listed or the file does not exist).
    ///
    /// The file is rewritten atomically: the new text goes to a temporary file next to it,
    /// which then replaces it. A running coordinator re-reads the file on its next check, so
    /// the device is refused from then on.
    ///
    /// # Errors
    ///
    /// The I/O error from reading, writing or replacing the file.
    pub fn remove(path: &Path, fp: &Fingerprint) -> io::Result<bool> {
        rewrite_allowlist(path, fp, &AllowlistEdit::Remove)
    }

    /// Sets the name of `fp` in the allowlist file at `path`.
    ///
    /// Its line becomes `fingerprint  # name` (a bare fingerprint when `name` is empty after
    /// trimming). The name goes through [`sanitize_allowlist_label`]. Comments, blank lines
    /// and the other entries stay as they are. Returns whether `fp` was found.
    ///
    /// The file is rewritten atomically, as [`Allowlist::remove`] does.
    ///
    /// # Errors
    ///
    /// The I/O error from reading, writing or replacing the file.
    pub fn rename(path: &Path, fp: &Fingerprint, name: &str) -> io::Result<bool> {
        rewrite_allowlist(path, fp, &AllowlistEdit::Rename(name))
    }

    /// Whether `fp` is in the allowlist.
    #[must_use]
    pub fn contains(&self, fp: &Fingerprint) -> bool {
        self.fingerprints.contains(fp)
    }

    /// Number of fingerprints in the allowlist.
    #[must_use]
    pub fn len(&self) -> usize {
        self.fingerprints.len()
    }

    /// Whether the allowlist holds no fingerprints.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.fingerprints.is_empty()
    }
}

/// What [`rewrite_allowlist`] does to the lines of one fingerprint.
enum AllowlistEdit<'a> {
    /// Drop the line.
    Remove,
    /// Rewrite the line with this name.
    Rename(&'a str),
}

/// Applies `edit` to every line of `fp` in the file at `path` and replaces the file
/// atomically (temporary file, then rename), under [`ALLOWLIST_APPEND_LOCK`]. Lines of other
/// fingerprints, comments, blank lines and each line's own ending are kept byte for byte.
fn rewrite_allowlist(path: &Path, fp: &Fingerprint, edit: &AllowlistEdit<'_>) -> io::Result<bool> {
    let _guard = ALLOWLIST_APPEND_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    let contents = match std::fs::read_to_string(path) {
        Ok(contents) => contents,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(false),
        Err(e) => return Err(e),
    };

    let mut found = false;
    let mut out = String::with_capacity(contents.len() + 32);
    for line in contents.split_inclusive('\n') {
        let body = line.trim_end_matches(['\r', '\n']);
        let ending = &line[body.len()..];
        let code = body.split('#').next().unwrap_or("").trim();
        if fingerprint_from_hex(code).as_ref() != Some(fp) {
            out.push_str(line);
            continue;
        }
        found = true;
        if let AllowlistEdit::Rename(name) = edit {
            out.push_str(&fingerprint_to_hex(fp));
            let name = sanitize_allowlist_label(name);
            let name = name.trim();
            if !name.is_empty() {
                out.push_str("  # ");
                out.push_str(name);
            }
            out.push_str(ending);
        }
    }
    if !found {
        return Ok(false);
    }

    let mut tmp_name = path.as_os_str().to_owned();
    tmp_name.push(".tmp");
    let tmp = PathBuf::from(tmp_name);
    let written = std::fs::write(&tmp, out.as_bytes()).and_then(|()| {
        if let Ok(meta) = std::fs::metadata(path) {
            // Best effort: the replacement keeps the original's permissions.
            let _ = std::fs::set_permissions(&tmp, meta.permissions());
        }
        std::fs::rename(&tmp, path)
    });
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(true)
}

/// Replaces `\r`, `\n`, and `#` in `label` with `_`.
///
/// So it can never inject a second allowlist line (`\r`/`\n`) or start a comment early
/// (`#`) in the `"{fingerprint}  # {label}"` line that [`append_to_allowlist`] writes.
/// `label` is operator-supplied (a `--name` or an enrollment name), so this guards against
/// typos and stray newlines rather than a hostile peer. It still matters: [`Allowlist::load`]
/// rejects the whole file on one malformed line, which would fail every client.
///
/// `pub` so `apps/pleochroma-worker`'s `EnrollRegistry::issue` applies the same scrub to the
/// enrollment name before it is minted into a certificate's Subject Common Name.
#[must_use]
pub fn sanitize_allowlist_label(label: &str) -> String {
    label.replace(['\r', '\n', '#'], "_")
}

/// Serializes every [`append_to_allowlist`] call within this process against every
/// other -- see that function's doc comment. Does not protect against a SECOND
/// process appending to the same file at the same time; enrollment is expected to run
/// from one `serve` process per allowlist file.
static ALLOWLIST_APPEND_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// Appends one `fingerprint  # label` line to the allowlist file at `path`, creating
/// the file (and its parent directory) if it doesn't exist yet.
///
/// Used by `pleochroma-worker cert issue-client` to enroll a freshly issued certificate
/// automatically; removing trust later is still the manual one-line file edit above.
/// `label` is sanitized via [`sanitize_allowlist_label`] before being written.
///
/// # Corruption this guards against
///
/// Three separate hazards, all closed here:
/// 1. **No trailing newline on the existing file's last line.** A hand-edited allowlist
///    that doesn't end in `\n` would otherwise glue the new fingerprint onto the
///    previous line's comment (that entry silently stops parsing as a bare
///    fingerprint) or directly onto a bare fingerprint (128 hex characters in a row,
///    which [`Allowlist::load`] rejects as [`TlsError::MalformedAllowlistLine`] --
///    failing every client, not just the new one, since `load` treats one bad line as a
///    hard parse error). Detected by reading the file's last byte first and prefixing a
///    `\n` onto the new line when it's missing (or the file is empty/new).
/// 2. **Interleaved writes.** The whole line is built in a `String` and written with one
///    `write_all` call. `writeln!` on a `File` would issue one write per piece of the
///    format string, which another thread's write could interleave with.
/// 3. **Concurrent claims in one process.** Two appends can still interleave at the OS
///    level, and the "check trailing newline, then write" pair must not race across
///    threads. [`ALLOWLIST_APPEND_LOCK`] holds the trailing-newline read, the write and the
///    post-write re-validation under one process-wide mutex.
///
/// Re-validates with [`Allowlist::load`] after writing (holding the same lock) so a
/// write that somehow still produced a malformed file is caught immediately, as an
/// error from THIS call, rather than silently failing every future connection's
/// allowlist check.
///
/// # Errors
///
/// [`TlsError::Io`] if the parent directory can't be created, the existing file can't be
/// read, or the file can't be opened for appending/written to.
/// [`TlsError::MalformedAllowlistLine`] if the post-write re-validation finds the file
/// malformed despite this call's own write.
pub fn append_to_allowlist(path: &Path, fp: &Fingerprint, label: &str) -> Result<(), TlsError> {
    use std::io::Write;

    let _guard = ALLOWLIST_APPEND_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent).map_err(|e| io_err(path, e))?;
    }

    // Whether the file already ends in a newline (or doesn't exist / is empty, in which
    // case no leading newline is needed either) -- see point 1 above.
    let needs_leading_newline = match std::fs::read(path) {
        Ok(bytes) => !bytes.is_empty() && bytes.last() != Some(&b'\n'),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => false,
        Err(e) => return Err(io_err(path, e)),
    };

    let mut line = String::new();
    if needs_leading_newline {
        line.push('\n');
    }
    line.push_str(&fingerprint_to_hex(fp));
    line.push_str("  # ");
    line.push_str(&sanitize_allowlist_label(label));
    line.push('\n');

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| io_err(path, e))?;
    file.write_all(line.as_bytes())
        .map_err(|e| io_err(path, e))?;
    drop(file);

    Allowlist::load(path).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fingerprint whose byte `i` is `i + shift`, so two shifts give distinct values.
    fn patterned_fingerprint(shift: u8) -> Fingerprint {
        std::array::from_fn(|i| {
            u8::try_from(i)
                .expect("a fingerprint has 32 bytes")
                .wrapping_add(shift)
        })
    }

    #[test]
    fn fingerprint_hex_round_trips() {
        let fp: Fingerprint = patterned_fingerprint(0);
        let hex = fingerprint_to_hex(&fp);
        assert_eq!(hex.len(), 64);
        assert_eq!(fingerprint_from_hex(&hex), Some(fp));
    }

    #[test]
    fn host_for_server_name_strips_ipv6_brackets() {
        assert_eq!(host_for_server_name("127.0.0.1:7879"), Some("127.0.0.1"));
        assert_eq!(host_for_server_name("worker.lan:7879"), Some("worker.lan"));
        assert_eq!(host_for_server_name("[::1]:7879"), Some("::1"));
        assert_eq!(host_for_server_name("worker.lan"), None);
    }

    #[test]
    fn fingerprint_from_hex_rejects_wrong_length_and_non_hex() {
        assert_eq!(fingerprint_from_hex("abcd"), None);
        assert_eq!(fingerprint_from_hex(&"zz".repeat(32)), None);
    }

    #[test]
    fn allowlist_parses_comments_and_blank_lines() {
        let dir = std::env::temp_dir().join(format!(
            "pleochroma-net-allowlist-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("allowlist.txt");
        let fp: Fingerprint = patterned_fingerprint(0);
        std::fs::write(
            &path,
            format!(
                "\n# a comment line\n{}  # laptop\n\n",
                fingerprint_to_hex(&fp)
            ),
        )
        .unwrap();

        let list = Allowlist::load(&path).unwrap();
        assert_eq!(list.len(), 1);
        assert!(list.contains(&fp));

        let other: Fingerprint = patterned_fingerprint(1);
        assert!(!list.contains(&other));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn allowlist_rejects_a_malformed_line() {
        let dir = std::env::temp_dir().join(format!(
            "pleochroma-net-allowlist-bad-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("allowlist.txt");
        std::fs::write(&path, "not-a-fingerprint\n").unwrap();

        let err = Allowlist::load(&path).unwrap_err();
        assert!(
            matches!(err, TlsError::MalformedAllowlistLine { .. }),
            "{err}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn sanitize_allowlist_label_replaces_line_breaking_and_comment_characters() {
        assert_eq!(sanitize_allowlist_label("laptop"), "laptop");
        assert_eq!(
            sanitize_allowlist_label("evil\r\ndeadbeef  # injected"),
            "evil__deadbeef  _ injected"
        );
        assert_eq!(sanitize_allowlist_label("a\nb"), "a_b");
        assert_eq!(sanitize_allowlist_label("a#b"), "a_b");
    }

    /// A label containing `\r\n` must not be able to inject a second,
    /// attacker-chosen allowlist line.
    #[test]
    fn append_to_allowlist_sanitizes_a_label_that_would_otherwise_inject_a_line() {
        let dir = std::env::temp_dir().join(format!(
            "pleochroma-net-allowlist-injection-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("allowlist.txt");
        let fp: Fingerprint = patterned_fingerprint(0);
        let evil_fp: Fingerprint = patterned_fingerprint(1);
        let label = format!(
            "laptop\r\n{}  # not really trusted",
            fingerprint_to_hex(&evil_fp)
        );

        append_to_allowlist(&path, &fp, &label).unwrap();

        // Loads clean (no MalformedAllowlistLine) and contains only the ONE fingerprint
        // actually passed to append_to_allowlist -- the embedded fingerprint-shaped text
        // inside the label never becomes its own trusted entry.
        let list = Allowlist::load(&path).unwrap();
        assert_eq!(list.len(), 1);
        assert!(list.contains(&fp));
        assert!(!list.contains(&evil_fp));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// Appending to a hand-edited allowlist whose last line has no trailing
    /// newline must not glue the new fingerprint onto the previous line -- both entries
    /// must parse as their own, separate lines.
    #[test]
    fn append_to_allowlist_prefixes_a_newline_when_the_file_lacks_a_trailing_one() {
        let dir = std::env::temp_dir().join(format!(
            "pleochroma-net-allowlist-no-trailing-newline-test-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("allowlist.txt");
        let existing: Fingerprint = patterned_fingerprint(0);
        // Deliberately no trailing `\n` -- exactly the hand-edited shape described above.
        std::fs::write(
            &path,
            format!("{}  # existing", fingerprint_to_hex(&existing)),
        )
        .unwrap();

        let new_fp: Fingerprint = patterned_fingerprint(1);
        append_to_allowlist(&path, &new_fp, "new-client").unwrap();

        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            contents.lines().count(),
            2,
            "the new entry must be its own line, not glued onto the previous one: {contents:?}"
        );

        let list = Allowlist::load(&path).unwrap();
        assert_eq!(list.len(), 2);
        assert!(list.contains(&existing));
        assert!(list.contains(&new_fp));

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn append_to_allowlist_creates_file_and_parent_dir() {
        let dir = std::env::temp_dir().join(format!(
            "pleochroma-net-allowlist-append-test-{}",
            std::process::id()
        ));
        let path = dir.join("nested").join("allowlist.txt");
        let fp: Fingerprint = patterned_fingerprint(0);

        append_to_allowlist(&path, &fp, "laptop").unwrap();
        let list = Allowlist::load(&path).unwrap();
        assert!(list.contains(&fp));

        std::fs::remove_dir_all(&dir).ok();
    }

    /// A fresh directory for one test, removed by the caller.
    fn test_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pleochroma-net-allowlist-{tag}-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn entries_carry_names_and_unnamed_devices_get_a_fallback() {
        let dir = test_dir("entries");
        let path = dir.join("allowlist.txt");
        let named = patterned_fingerprint(0);
        let bare = patterned_fingerprint(0x3b);
        std::fs::write(
            &path,
            format!(
                "# header\n{}  # Niklas's S23 \n{}\n{}  #\n",
                fingerprint_to_hex(&named),
                fingerprint_to_hex(&bare),
                fingerprint_to_hex(&named),
            ),
        )
        .unwrap();

        let list = Allowlist::load(&path).unwrap();
        let entries = list.entries();
        assert_eq!(entries.len(), 2, "a repeated fingerprint is listed once");
        assert_eq!(entries[0].fingerprint, named);
        assert_eq!(entries[0].name.as_deref(), Some("Niklas's S23"));
        assert_eq!(entries[0].display_name(), "Niklas's S23");
        assert_eq!(entries[1].fingerprint, bare);
        assert_eq!(entries[1].name, None);
        // Byte 0 of `bare` is 0x3b, byte 1 is 0x3c: "3b3c".
        assert_eq!(
            entries[1].display_name(),
            "Unnamed device \u{b7} 3B3C\u{2026}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn remove_drops_only_that_fingerprint_and_keeps_the_rest() {
        let dir = test_dir("remove");
        let path = dir.join("allowlist.txt");
        let a = patterned_fingerprint(0);
        let b = patterned_fingerprint(1);
        std::fs::write(
            &path,
            format!(
                "# keep me\r\n{}  # a\r\n\r\n{}  # b",
                fingerprint_to_hex(&a),
                fingerprint_to_hex(&b)
            ),
        )
        .unwrap();

        assert!(Allowlist::remove(&path, &a).unwrap());
        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            contents,
            format!("# keep me\r\n\r\n{}  # b", fingerprint_to_hex(&b))
        );
        let list = Allowlist::load(&path).unwrap();
        assert!(!list.contains(&a));
        assert!(list.contains(&b));
        assert!(!dir.join("allowlist.txt.tmp").exists());

        assert!(!Allowlist::remove(&path, &a).unwrap(), "already gone");
        assert!(!Allowlist::remove(&dir.join("missing.txt"), &a).unwrap());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn rename_writes_the_comment_and_keeps_other_lines() {
        let dir = test_dir("rename");
        let path = dir.join("allowlist.txt");
        let a = patterned_fingerprint(0);
        let b = patterned_fingerprint(1);
        std::fs::write(
            &path,
            format!(
                "# header\n{}\n{}  # old name\n",
                fingerprint_to_hex(&a),
                fingerprint_to_hex(&b)
            ),
        )
        .unwrap();

        assert!(Allowlist::rename(&path, &a, "Workshop laptop").unwrap());
        assert!(Allowlist::rename(&path, &b, "evil\n# x").unwrap());
        let contents = std::fs::read_to_string(&path).unwrap();
        assert_eq!(
            contents,
            format!(
                "# header\n{}  # Workshop laptop\n{}  # evil__ x\n",
                fingerprint_to_hex(&a),
                fingerprint_to_hex(&b)
            )
        );
        let list = Allowlist::load(&path).unwrap();
        assert_eq!(list.entries()[0].name.as_deref(), Some("Workshop laptop"));

        // An empty name clears it back to a bare fingerprint.
        assert!(Allowlist::rename(&path, &a, "  ").unwrap());
        let list = Allowlist::load(&path).unwrap();
        assert_eq!(list.entries()[0].name, None);
        assert!(!Allowlist::rename(&path, &patterned_fingerprint(9), "x").unwrap());

        std::fs::remove_dir_all(&dir).ok();
    }
}
