//! Backup and restore of a coordinator.
//!
//! The certificate folder (`pki/`: the authority's key, the server certificate, the allowlists of
//! paired devices) and the settings file, in one file sealed with a password the owner types.
//!
//! The file is `PLEOBAK1`, the key-derivation settings, a random salt and nonce, and the sealed
//! archive. The key comes from the password with Argon2id; the archive is sealed with
//! AES-256-GCM, and the header is authenticated too, so a changed header fails like a wrong
//! password. A backup holds the authority's private key: the window offers it only on this
//! computer, never to a remote administrator.

use super::{model::CoordinatorSettings, store};
use aes_gcm::{
    Aes256Gcm, KeyInit, Nonce,
    aead::{Aead, Payload},
};
use argon2::{Algorithm, Argon2, Params, Version};
use ring::rand::{SecureRandom, SystemRandom};
use std::path::{Path, PathBuf};
use zeroize::Zeroizing;

/// The first bytes of every backup file.
const MAGIC: &[u8; 8] = b"PLEOBAK1";

/// Salt length in bytes.
const SALT_LEN: usize = 16;

/// AES-GCM nonce length in bytes.
const NONCE_LEN: usize = 12;

/// Header length: the magic, three `u32` key-derivation settings, the salt and the nonce.
const HEADER_LEN: usize = 8 + 12 + SALT_LEN + NONCE_LEN;

/// The fewest characters a backup password may have.
pub const MIN_PASSWORD_CHARS: usize = 8;

/// The largest single file a backup takes (a certificate or a key is a few kilobytes).
const MAX_FILE_BYTES: u64 = 1 << 20;

/// The most files a backup takes.
const MAX_FILES: usize = 256;

/// The name of the settings file inside the archive.
const SETTINGS_ENTRY: &str = "coordinator.toml";

/// The prefix of a certificate-folder file inside the archive.
const PKI_PREFIX: &str = "pki/";

/// The Argon2id cost of a backup: memory in KiB, passes, lanes. The file stores the values it
/// was made with, so they can be raised later without breaking old backups.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Kdf {
    /// Memory in KiB.
    memory_kib: u32,
    /// Passes over the memory.
    passes: u32,
    /// Parallel lanes.
    lanes: u32,
}

impl Kdf {
    /// What a real backup uses: 64 MiB, 3 passes, one lane (well over the OWASP minimum).
    const STRONG: Self = Self {
        memory_kib: 65_536,
        passes: 3,
        lanes: 1,
    };

    /// Refuses costs a damaged or hostile file could use to exhaust memory or time.
    fn check(self) -> Result<Self, String> {
        let ok = (8..=1_048_576).contains(&self.memory_kib)
            && (1..=16).contains(&self.passes)
            && (1..=16).contains(&self.lanes);
        if ok {
            Ok(self)
        } else {
            Err("This backup file is damaged.".to_owned())
        }
    }
}

/// One file of the archive.
struct Entry {
    /// `coordinator.toml` or `pki/<file>`.
    name: String,
    /// The file's bytes (a private key among them): overwritten when the entry is dropped.
    data: Zeroizing<Vec<u8>>,
}

/// Writes a backup of this coordinator (the live settings file and the certificate folder of
/// the data folder in use) to `out`, sealed with `password`.
///
/// # Errors
///
/// A message in plain words: no settings loaded, no certificates to save, a short password, or
/// a file that cannot be read or written.
pub fn export_backup(out: &Path, password: &str) -> Result<(), String> {
    let (settings_file, data_dir) = live_paths()?;
    export_with(&settings_file, &data_dir, out, password, Kdf::STRONG)
}

/// Restores the backup at `file` into this coordinator's data folder and settings file, then
/// reloads the settings.
///
/// The certificate folder and the settings file that were there are kept next to their place as
/// `*.before-restore-<stamp>`. The coordinator must be restarted to use the restored certificates.
///
/// # Errors
///
/// A message in plain words: a wrong password or a file that is not a backup (nothing is
/// changed), or a file that cannot be written.
pub fn import_backup(file: &Path, password: &str) -> Result<(), String> {
    let (settings_file, data_dir) = live_paths()?;
    import_with(file, &settings_file, &data_dir, password)?;
    store::reload()?;
    Ok(())
}

/// The settings file and the data folder of the live coordinator.
fn live_paths() -> Result<(PathBuf, PathBuf), String> {
    let settings_file = store::settings_file().ok_or("The settings are not loaded yet.")?;
    let data_dir = store::data_dir().ok_or("The settings are not loaded yet.")?;
    Ok((settings_file, data_dir))
}

/// [`export_backup`] for explicit places and cost.
fn export_with(
    settings_file: &Path,
    data_dir: &Path,
    out: &Path,
    password: &str,
    kdf: Kdf,
) -> Result<(), String> {
    if password.chars().count() < MIN_PASSWORD_CHARS {
        return Err(format!(
            "The password needs at least {MIN_PASSWORD_CHARS} characters."
        ));
    }
    let entries = collect(settings_file, data_dir)?;
    let archive = encode(&entries)?;
    let sealed = seal(&archive, password, kdf)?;
    if let Some(parent) = out.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Could not create the folder for the backup: {e}"))?;
    }
    std::fs::write(out, sealed).map_err(|e| format!("Could not write the backup: {e}"))
}

/// [`import_backup`] for explicit places; it does not reload the live settings.
fn import_with(
    file: &Path,
    settings_file: &Path,
    data_dir: &Path,
    password: &str,
) -> Result<(), String> {
    let blob = std::fs::read(file).map_err(|e| format!("Could not read the backup file: {e}"))?;
    let archive = Zeroizing::new(open(&blob, password)?);
    let entries = decode(&archive)?;
    if !entries
        .iter()
        .any(|e| e.name == format!("{PKI_PREFIX}{}", crate::pki::CA_CERT_FILE))
    {
        return Err("This backup holds no certificates, so it cannot be restored.".to_owned());
    }
    let settings = if let Some(entry) = entries.iter().find(|e| e.name == SETTINGS_ENTRY) {
        let text = std::str::from_utf8(&entry.data)
            .map_err(|_| "This backup file is damaged.".to_owned())?;
        let mut settings: CoordinatorSettings =
            toml::from_str(text).map_err(|_| "This backup file is damaged.".to_owned())?;
        settings.validate()?;
        // The restored data lands in this computer's data folder.
        settings.data_folder = None;
        Some(settings)
    } else {
        None
    };

    let stamp = stamp();
    let aside = swap_in_certificates(data_dir, &entries, &stamp)?;
    if let Some(settings) = settings {
        let kept_note = aside.as_ref().map_or_else(String::new, |aside| {
            format!(" The previous certificates are in {}.", aside.display())
        });
        if settings_file.exists() {
            let mut aside = settings_file.file_name().unwrap_or_default().to_os_string();
            aside.push(format!(".before-restore-{stamp}"));
            std::fs::copy(settings_file, settings_file.with_file_name(aside)).map_err(|e| {
                format!(
                    "The certificates were restored, but the current settings could not be kept: {e}.{kept_note}"
                )
            })?;
        }
        settings.save(settings_file).map_err(|e| {
            format!("The certificates were restored, but the settings could not be written: {e}{kept_note}")
        })?;
    }
    Ok(())
}

/// Puts the certificate files of `entries` in `<data_dir>/pki`, all or nothing: they are written
/// to `pki.restore-tmp` first, the current `pki` is moved to `pki.before-restore-<stamp>`, and
/// the new folder is renamed into place. A failure at any step leaves the current certificates
/// where they were. Returns the folder the old certificates were moved to, if there were any.
fn swap_in_certificates(
    data_dir: &Path,
    entries: &[Entry],
    stamp: &str,
) -> Result<Option<PathBuf>, String> {
    let pki_dir = data_dir.join("pki");
    let staging = data_dir.join("pki.restore-tmp");
    // A leftover of an earlier failed restore.
    let _ = std::fs::remove_dir_all(&staging);
    std::fs::create_dir_all(&staging)
        .map_err(|e| format!("Could not create the certificate folder: {e}"))?;
    for entry in entries {
        if let Some(name) = entry.name.strip_prefix(PKI_PREFIX)
            && let Err(e) = write_private(&staging.join(name), &entry.data)
        {
            let _ = std::fs::remove_dir_all(&staging);
            return Err(e);
        }
    }
    let aside = pki_dir
        .exists()
        .then(|| data_dir.join(format!("pki.before-restore-{stamp}")));
    if let Some(aside) = &aside
        && let Err(e) = std::fs::rename(&pki_dir, aside)
    {
        let _ = std::fs::remove_dir_all(&staging);
        return Err(format!(
            "Could not set the current certificates aside: {e}. Nothing was changed."
        ));
    }
    if let Err(e) = std::fs::rename(&staging, &pki_dir) {
        let put_back = aside
            .as_ref()
            .is_none_or(|aside| std::fs::rename(aside, &pki_dir).is_ok());
        let _ = std::fs::remove_dir_all(&staging);
        return Err(if let (Some(aside), false) = (&aside, put_back) {
            format!(
                "Could not put the restored certificates in place: {e}. The current certificates are in {}.",
                aside.display()
            )
        } else {
            format!(
                "Could not put the restored certificates in place: {e}. The current certificates were kept."
            )
        });
    }
    Ok(aside)
}

/// Reads the files a backup holds: the settings file and the flat files of `pki/`.
fn collect(settings_file: &Path, data_dir: &Path) -> Result<Vec<Entry>, String> {
    let mut entries = Vec::new();
    if settings_file.is_file() {
        entries.push(Entry {
            name: SETTINGS_ENTRY.to_owned(),
            data: read_small(settings_file)?,
        });
    }
    let pki_dir = data_dir.join("pki");
    let read = std::fs::read_dir(&pki_dir)
        .map_err(|_| "There are no certificates to back up in the data folder.".to_owned())?;
    for item in read.flatten() {
        let path = item.path();
        let Some(name) = item.file_name().to_str().map(str::to_owned) else {
            continue;
        };
        if !path.is_file() || !safe_file_name(&name) {
            continue;
        }
        entries.push(Entry {
            name: format!("{PKI_PREFIX}{name}"),
            data: read_small(&path)?,
        });
    }
    let has_ca = entries
        .iter()
        .any(|e| e.name == format!("{PKI_PREFIX}{}", crate::pki::CA_CERT_FILE));
    if !has_ca {
        return Err("There are no certificates to back up in the data folder.".to_owned());
    }
    if entries.len() > MAX_FILES {
        return Err("The certificate folder holds too many files for a backup.".to_owned());
    }
    Ok(entries)
}

/// Reads a file of at most [`MAX_FILE_BYTES`].
fn read_small(path: &Path) -> Result<Zeroizing<Vec<u8>>, String> {
    let meta =
        std::fs::metadata(path).map_err(|e| format!("Could not read {}: {e}", path.display()))?;
    if meta.len() > MAX_FILE_BYTES {
        return Err(format!("{} is too large for a backup.", path.display()));
    }
    std::fs::read(path)
        .map(Zeroizing::new)
        .map_err(|e| format!("Could not read {}: {e}", path.display()))
}

/// A file name that stays inside the certificate folder: no separators, no `..`, not empty.
fn safe_file_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name != "."
        && name != ".."
        && !name.contains(['/', '\\', ':', '\0'])
}

/// Whether a certificate-folder file holds a secret: a private key or the operator secret.
fn is_secret_file(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    Path::new(&lower)
        .extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("key"))
        || lower.contains("secret")
}

/// Writes a certificate-folder file. A secret (a private key, the operator secret) is created
/// readable by its owner only where the system has such a notion, never with default
/// permissions first: the key goes through `write_private_key_pem` (a restricted temporary file,
/// then a rename), and on Unix any other secret is created with mode 0600.
fn write_private(path: &Path, data: &[u8]) -> Result<(), String> {
    let fail = |e: &dyn std::fmt::Display| format!("Could not write {}: {e}", path.display());
    let secret = path
        .file_name()
        .and_then(std::ffi::OsStr::to_str)
        .is_some_and(is_secret_file);
    if !secret {
        return std::fs::write(path, data).map_err(|e| fail(&e));
    }
    if let Ok(text) = std::str::from_utf8(data) {
        return pleochroma_net::tls::write_private_key_pem(path, text);
    }
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        options.mode(0o600);
    }
    let mut file = options.open(path).map_err(|e| fail(&e))?;
    std::io::Write::write_all(&mut file, data).map_err(|e| fail(&e))
}

/// Encodes the entries: a count, then per entry a `u16` name length, the name, a `u64` data
/// length and the data (all little-endian). The bytes hold the keys, so they are zeroized when
/// dropped.
fn encode(entries: &[Entry]) -> Result<Zeroizing<Vec<u8>>, String> {
    let mut out = Zeroizing::new(Vec::new());
    let count = u32::try_from(entries.len()).map_err(|_| "Too many files.".to_owned())?;
    out.extend_from_slice(&count.to_le_bytes());
    for entry in entries {
        let name = entry.name.as_bytes();
        let name_len =
            u16::try_from(name.len()).map_err(|_| "A file name is too long.".to_owned())?;
        out.extend_from_slice(&name_len.to_le_bytes());
        out.extend_from_slice(name);
        let data_len =
            u64::try_from(entry.data.len()).map_err(|_| "A file is too large.".to_owned())?;
        out.extend_from_slice(&data_len.to_le_bytes());
        out.extend_from_slice(&entry.data);
    }
    Ok(out)
}

/// A cursor over the archive bytes.
struct Reader<'a> {
    /// What is left to read.
    rest: &'a [u8],
}

impl<'a> Reader<'a> {
    /// The next `n` bytes, or the "damaged" error.
    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        if self.rest.len() < n {
            return Err(damaged());
        }
        let (head, tail) = self.rest.split_at(n);
        self.rest = tail;
        Ok(head)
    }
}

/// The "damaged file" sentence.
fn damaged() -> String {
    "This backup file is damaged.".to_owned()
}

/// Decodes [`encode`]'s bytes and checks every name: only `coordinator.toml` and flat files
/// under `pki/`, each once.
fn decode(bytes: &[u8]) -> Result<Vec<Entry>, String> {
    let mut reader = Reader { rest: bytes };
    let count = u32::from_le_bytes(reader.take(4)?.try_into().map_err(|_| damaged())?);
    let count = usize::try_from(count).map_err(|_| damaged())?;
    if count > MAX_FILES + 1 {
        return Err(damaged());
    }
    let mut entries: Vec<Entry> = Vec::with_capacity(count);
    for _ in 0..count {
        let name_len = u16::from_le_bytes(reader.take(2)?.try_into().map_err(|_| damaged())?);
        let name = std::str::from_utf8(reader.take(usize::from(name_len))?)
            .map_err(|_| damaged())?
            .to_owned();
        let data_len = u64::from_le_bytes(reader.take(8)?.try_into().map_err(|_| damaged())?);
        if data_len > MAX_FILE_BYTES {
            return Err(damaged());
        }
        let data = Zeroizing::new(
            reader
                .take(usize::try_from(data_len).map_err(|_| damaged())?)?
                .to_vec(),
        );
        let allowed =
            name == SETTINGS_ENTRY || name.strip_prefix(PKI_PREFIX).is_some_and(safe_file_name);
        if !allowed || entries.iter().any(|e| e.name == name) {
            return Err(damaged());
        }
        entries.push(Entry { name, data });
    }
    if !reader.rest.is_empty() {
        return Err(damaged());
    }
    Ok(entries)
}

/// The key for `password`, derived with Argon2id.
fn derive_key(password: &str, salt: &[u8], kdf: Kdf) -> Result<Zeroizing<[u8; 32]>, String> {
    let params =
        Params::new(kdf.memory_kib, kdf.passes, kdf.lanes, Some(32)).map_err(|_| damaged())?;
    let argon = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = Zeroizing::new([0_u8; 32]);
    argon
        .hash_password_into(password.as_bytes(), salt, &mut key[..])
        .map_err(|_| "Could not derive the key from the password.".to_owned())?;
    Ok(key)
}

/// Seals `plain` with `password`: the header, then the AES-256-GCM ciphertext.
fn seal(plain: &[u8], password: &str, kdf: Kdf) -> Result<Vec<u8>, String> {
    let kdf = kdf.check()?;
    let rng = SystemRandom::new();
    let mut salt = [0_u8; SALT_LEN];
    let mut nonce = [0_u8; NONCE_LEN];
    rng.fill(&mut salt)
        .and_then(|()| rng.fill(&mut nonce))
        .map_err(|_| "Could not get random bytes for the backup.".to_owned())?;

    let mut out = Vec::with_capacity(HEADER_LEN + plain.len() + 16);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&kdf.memory_kib.to_le_bytes());
    out.extend_from_slice(&kdf.passes.to_le_bytes());
    out.extend_from_slice(&kdf.lanes.to_le_bytes());
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce);

    let key = derive_key(password, &salt, kdf)?;
    let cipher = Aes256Gcm::new_from_slice(&key[..])
        .map_err(|_| "Could not start the encryption.".to_owned())?;
    let sealed = cipher
        .encrypt(
            Nonce::from_slice(&nonce),
            Payload {
                msg: plain,
                aad: &out,
            },
        )
        .map_err(|_| "Could not encrypt the backup.".to_owned())?;
    out.extend_from_slice(&sealed);
    Ok(out)
}

/// Opens what [`seal`] made.
fn open(blob: &[u8], password: &str) -> Result<Vec<u8>, String> {
    if blob.len() < HEADER_LEN + 16 || &blob[..8] != MAGIC {
        return Err("This file is not a Pleochroma backup.".to_owned());
    }
    let word = |at: usize| u32::from_le_bytes([blob[at], blob[at + 1], blob[at + 2], blob[at + 3]]);
    let kdf = Kdf {
        memory_kib: word(8),
        passes: word(12),
        lanes: word(16),
    }
    .check()?;
    let salt = &blob[20..20 + SALT_LEN];
    let nonce = &blob[20 + SALT_LEN..HEADER_LEN];
    let key = derive_key(password, salt, kdf)?;
    let cipher = Aes256Gcm::new_from_slice(&key[..])
        .map_err(|_| "Could not start the decryption.".to_owned())?;
    cipher
        .decrypt(
            Nonce::from_slice(nonce),
            Payload {
                msg: &blob[HEADER_LEN..],
                aad: &blob[..HEADER_LEN],
            },
        )
        .map_err(|_| "The password is wrong, or the backup file is damaged.".to_owned())
}

/// `YYYYMMDD-HHMMSS` in UTC, for the names of the things a restore sets aside.
fn stamp() -> String {
    let now = time::OffsetDateTime::now_utc();
    format!(
        "{:04}{:02}{:02}-{:02}{:02}{:02}",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A cost small enough for tests (the format stores it in the file).
    const TEST_KDF: Kdf = Kdf {
        memory_kib: 8,
        passes: 1,
        lanes: 1,
    };

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("pleochroma-backup-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("temp dir");
        dir
    }

    /// A data folder with a fake certificate folder and a settings file.
    fn fake_coordinator(dir: &Path) -> (PathBuf, PathBuf) {
        let data = dir.join("data");
        std::fs::create_dir_all(data.join("pki")).expect("pki");
        std::fs::write(data.join("pki").join("ca.pem"), b"CA CERT").expect("ca");
        std::fs::write(data.join("pki").join("ca.key"), b"CA PRIVATE KEY").expect("key");
        std::fs::write(data.join("pki").join("allowlist-viewers.txt"), b"abcd\n").expect("list");
        let settings = dir.join("coordinator.toml");
        CoordinatorSettings {
            name: "Studio PC".to_owned(),
            data_folder: Some(PathBuf::from("/somewhere/else")),
            ..CoordinatorSettings::default()
        }
        .save(&settings)
        .expect("settings");
        (settings, data)
    }

    #[test]
    fn a_backup_round_trips_and_a_wrong_password_fails() {
        let dir = temp_dir("round-trip");
        let (settings, data) = fake_coordinator(&dir);
        let file = dir.join("studio.pleobackup");
        export_with(&settings, &data, &file, "correct horse", TEST_KDF).expect("export");

        // The file is sealed: neither the key nor the name shows in it.
        let bytes = std::fs::read(&file).expect("read");
        assert!(bytes.starts_with(MAGIC));
        assert!(!bytes.windows(14).any(|w| w == b"CA PRIVATE KEY"));
        assert!(!bytes.windows(9).any(|w| w == b"Studio PC"));

        // A wrong password changes nothing.
        let other = dir.join("other");
        std::fs::create_dir_all(&other).expect("other");
        let other_settings = other.join("coordinator.toml");
        let err = import_with(
            &file,
            &other_settings,
            &other.join("data"),
            "wrong password",
        )
        .expect_err("wrong password");
        assert!(err.contains("password is wrong"));
        assert!(!other.join("data").exists());
        assert!(!other_settings.exists());

        // The right one restores the certificates and the settings.
        import_with(&file, &other_settings, &other.join("data"), "correct horse").expect("import");
        assert_eq!(
            std::fs::read(other.join("data/pki/ca.key")).expect("key"),
            b"CA PRIVATE KEY"
        );
        assert_eq!(
            std::fs::read(other.join("data/pki/allowlist-viewers.txt")).expect("list"),
            b"abcd\n"
        );
        let restored = CoordinatorSettings::load(&other_settings);
        assert_eq!(restored.notice, None);
        assert_eq!(restored.settings.name, "Studio PC");
        assert_eq!(restored.settings.data_folder, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_restore_sets_the_old_certificates_aside() {
        let dir = temp_dir("aside");
        let (settings, data) = fake_coordinator(&dir);
        let file = dir.join("b.pleobackup");
        export_with(&settings, &data, &file, "password one", TEST_KDF).expect("export");
        std::fs::write(data.join("pki").join("ca.key"), b"NEWER KEY").expect("change");
        import_with(&file, &settings, &data, "password one").expect("import");
        assert_eq!(
            std::fs::read(data.join("pki/ca.key")).expect("restored"),
            b"CA PRIVATE KEY"
        );
        let aside = std::fs::read_dir(&data)
            .expect("data")
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .find(|n| n.starts_with("pki.before-restore-"))
            .expect("set aside");
        assert_eq!(
            std::fs::read(data.join(aside).join("ca.key")).expect("old key"),
            b"NEWER KEY"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The new certificates are staged in `pki.restore-tmp` and renamed into place: nothing is
    /// left behind, and a failure before the swap leaves the current folder alone.
    #[test]
    fn a_restore_stages_the_certificates_and_leaves_no_staging_folder() {
        let dir = temp_dir("staging");
        let (settings, data) = fake_coordinator(&dir);
        let file = dir.join("s.pleobackup");
        export_with(&settings, &data, &file, "password one", TEST_KDF).expect("export");
        // A leftover of an earlier failed restore is cleared first.
        let staging = data.join("pki.restore-tmp");
        std::fs::create_dir_all(&staging).expect("leftover");
        std::fs::write(staging.join("stale.pem"), b"stale").expect("stale");
        import_with(&file, &settings, &data, "password one").expect("import");
        assert!(!staging.exists());
        assert!(!data.join("pki/stale.pem").exists());
        assert_eq!(
            std::fs::read(data.join("pki/ca.key")).expect("restored"),
            b"CA PRIVATE KEY"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn keys_and_secrets_are_recognized_by_name() {
        for secret in ["ca.key", "server.key", "SERVER.KEY", "issue.secret"] {
            assert!(is_secret_file(secret), "{secret}");
        }
        for plain in ["ca.pem", "server.pem", "allowlist-viewers.txt"] {
            assert!(!is_secret_file(plain), "{plain}");
        }
    }

    #[test]
    fn short_passwords_and_missing_certificates_are_refused() {
        let dir = temp_dir("refuse");
        let (settings, data) = fake_coordinator(&dir);
        let file = dir.join("c.pleobackup");
        assert!(export_with(&settings, &data, &file, "short", TEST_KDF).is_err());
        assert!(!file.exists());
        let empty = dir.join("empty-data");
        std::fs::create_dir_all(&empty).expect("empty");
        assert!(export_with(&settings, &empty, &file, "long enough", TEST_KDF).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_damaged_or_foreign_file_is_refused() {
        let dir = temp_dir("damaged");
        let (settings, data) = fake_coordinator(&dir);
        let file = dir.join("d.pleobackup");
        export_with(&settings, &data, &file, "long enough", TEST_KDF).expect("export");
        let mut bytes = std::fs::read(&file).expect("read");

        let last = bytes.len() - 1;
        bytes[last] ^= 0x01;
        assert!(open(&bytes, "long enough").is_err());
        bytes[last] ^= 0x01;
        // The header is authenticated: a changed salt fails like a wrong password.
        bytes[25] ^= 0x01;
        assert!(open(&bytes, "long enough").is_err());
        bytes[25] ^= 0x01;
        assert!(open(&bytes, "long enough").is_ok());

        assert!(open(b"not a backup at all, just some text bytes", "x").is_err());
        let mut hostile = bytes.clone();
        hostile[8..12].copy_from_slice(&u32::MAX.to_le_bytes());
        assert!(open(&hostile, "long enough").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_archive_refuses_names_outside_the_certificate_folder() {
        let good = vec![
            Entry {
                name: "pki/ca.pem".to_owned(),
                data: Zeroizing::new(vec![1, 2, 3]),
            },
            Entry {
                name: SETTINGS_ENTRY.to_owned(),
                data: Zeroizing::new(vec![]),
            },
        ];
        let bytes = encode(&good).expect("encode");
        assert_eq!(decode(&bytes).expect("decode").len(), 2);
        for bad in ["pki/../x", "../x", "pki/a/b", "other.txt", "pki/"] {
            let entries = vec![Entry {
                name: bad.to_owned(),
                data: Zeroizing::new(vec![]),
            }];
            assert!(decode(&encode(&entries).expect("encode")).is_err(), "{bad}");
        }
        assert!(decode(&bytes[..bytes.len() - 1]).is_err());
    }
}
