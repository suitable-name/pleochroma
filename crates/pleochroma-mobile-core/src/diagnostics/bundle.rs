//! The e-mail bundle: selected logs and `device.txt` as one zip in memory, and the mail fields.

use std::{
    collections::HashSet,
    fmt::Write as _,
    io::{Cursor, Write as _},
};

use pleochroma::diag::AdapterFacts;
use zip::{CompressionMethod, ZipWriter, write::SimpleFileOptions};

use super::store::{LogEntry, LogKind, LogStore};
use crate::platform::DeviceFacts;

/// The largest zip the e-mail path accepts.
pub const MAX_BUNDLE_BYTES: usize = 15 * 1024 * 1024;

/// The address the logs are sent to.
const MAIL_TO: &str = "pleochroma@gussek.eu";

/// Why a bundle was not built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BundleError {
    /// No file was selected.
    Empty,
    /// The name is not a file of the logs folder.
    NotALog(String),
    /// The zip is over [`MAX_BUNDLE_BYTES`].
    TooLarge,
    /// A file could not be read or the zip could not be written.
    Io(String),
}

/// The zip in memory: the named files of `store`, each under `logs/`, plus `device.txt`.
///
/// # Errors
///
/// [`BundleError::Empty`] for no names, [`BundleError::NotALog`] for a name that is not a log
/// file of `store` (a path, a parent step, another kind of file), [`BundleError::TooLarge`] when
/// the zip is over 15 MB and [`BundleError::Io`] when a file cannot be read.
pub fn build(store: &LogStore, names: &[String], device_txt: &str) -> Result<Vec<u8>, BundleError> {
    if names.is_empty() {
        return Err(BundleError::Empty);
    }
    if let Some(bad) = names.iter().find(|name| LogStore::kind_of(name).is_none()) {
        return Err(BundleError::NotALog(bad.clone()));
    }
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let mut seen: HashSet<&str> = HashSet::new();
    for name in names {
        if !seen.insert(name) {
            continue;
        }
        let bytes = std::fs::read(store.dir().join(name)).map_err(|e| io_error(&e))?;
        zip.start_file(format!("logs/{name}"), options)
            .map_err(|e| io_error(&e))?;
        zip.write_all(&bytes).map_err(|e| io_error(&e))?;
    }
    zip.start_file("device.txt", options)
        .map_err(|e| io_error(&e))?;
    zip.write_all(device_txt.as_bytes())
        .map_err(|e| io_error(&e))?;
    let bytes = zip.finish().map_err(|e| io_error(&e))?.into_inner();
    if bytes.len() > MAX_BUNDLE_BYTES {
        return Err(BundleError::TooLarge);
    }
    Ok(bytes)
}

fn io_error(error: &dyn std::fmt::Display) -> BundleError {
    BundleError::Io(error.to_string())
}

/// The names "Select all" ticks: every text log, never a crash snapshot.
#[must_use]
pub fn select_all_names(entries: &[LogEntry]) -> Vec<String> {
    entries
        .iter()
        .filter(|entry| entry.kind != LogKind::CrashSnapshot)
        .map(|entry| entry.name.clone())
        .collect()
}

/// The text of `device.txt`: model, SoC, GPU, driver, Android version and RAM, nothing else.
///
/// No account, advertising or device id, and no serial number.
#[must_use]
pub fn device_txt(facts: &DeviceFacts, adapter: Option<&AdapterFacts>) -> String {
    let mut text = String::new();
    let model = join_words(&[facts.manufacturer.as_str(), facts.model.as_str()]);
    let _ = writeln!(text, "Model: {}", or_unknown(&model));
    let _ = writeln!(
        text,
        "SoC: {}",
        or_unknown(facts.soc_model.as_deref().unwrap_or(""))
    );
    if let Some(adapter) = adapter.filter(|a| !a.name.trim().is_empty()) {
        let _ = writeln!(text, "GPU: {}", adapter.name.trim());
        let driver = join_words(&[adapter.driver.as_str(), adapter.driver_info.as_str()]);
        let _ = writeln!(text, "Driver: {}", or_unknown(&driver));
    } else {
        let _ = writeln!(text, "GPU: not started in this session");
    }
    let _ = writeln!(
        text,
        "Android: {} (API {})",
        or_unknown(&facts.android_release),
        facts.sdk_int
    );
    let _ = writeln!(text, "RAM: {} MB", facts.total_ram_mb);
    text
}

/// The (to, subject, body) of the e-mail.
///
/// The subject reads "Pleochroma logs · 0.12.0 · Evermyth KB1001 · Mali-G57"; the GPU part and
/// its separator are left out when no adapter was recorded in this session.
#[must_use]
pub fn mail_fields(
    facts: &DeviceFacts,
    adapter: Option<&AdapterFacts>,
) -> (String, String, String) {
    let mut subject = String::from("Pleochroma logs");
    let version = facts.app_version.trim();
    if !version.is_empty() {
        subject.push_str(" \u{b7} ");
        subject.push_str(version);
    }
    let model = join_words(&[facts.manufacturer.as_str(), facts.model.as_str()]);
    if !model.is_empty() {
        subject.push_str(" \u{b7} ");
        subject.push_str(&model);
    }
    if let Some(adapter) = adapter.filter(|a| !a.name.trim().is_empty()) {
        subject.push_str(" \u{b7} ");
        subject.push_str(adapter.name.trim());
    }
    let body = format!(
        "What did you do just before the problem?\n\n{}",
        device_txt(facts, adapter)
    );
    (MAIL_TO.to_owned(), subject, body)
}

/// The non-empty parts joined by one space.
fn join_words(parts: &[&str]) -> String {
    parts
        .iter()
        .copied()
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn or_unknown(text: &str) -> &str {
    if text.trim().is_empty() {
        "unknown"
    } else {
        text
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, io::Read as _};

    use super::*;
    use crate::platform_mock::unique_temp_dir;

    fn temp_store(tag: &str) -> LogStore {
        let dir = unique_temp_dir(tag);
        fs::create_dir_all(&dir).expect("create the logs folder");
        LogStore::new(dir)
    }

    fn facts() -> DeviceFacts {
        DeviceFacts {
            app_version: "0.12.0".to_owned(),
            android_release: "14".to_owned(),
            sdk_int: 34,
            manufacturer: "Evermyth".to_owned(),
            model: "KB1001".to_owned(),
            soc_model: Some("SM8550".to_owned()),
            abi: "arm64-v8a".to_owned(),
            total_ram_mb: 7_800,
            thermal_headroom: None,
            fingerprint: "evermyth/kb1001:14/user".to_owned(),
        }
    }

    fn adapter() -> AdapterFacts {
        AdapterFacts {
            name: "Mali-G57".to_owned(),
            backend: "Vulkan".to_owned(),
            driver: "Mali".to_owned(),
            driver_info: "v1.r40".to_owned(),
            ..AdapterFacts::default()
        }
    }

    fn read_entry(zip: &[u8], name: &str) -> Vec<u8> {
        let mut archive = zip::ZipArchive::new(Cursor::new(zip)).expect("a readable zip");
        let mut file = archive.by_name(name).expect("the entry");
        let mut out = Vec::new();
        file.read_to_end(&mut out).expect("read the entry");
        out
    }

    #[test]
    fn the_zip_reads_back() {
        let store = temp_store("bundle-readback");
        fs::write(
            store.dir().join("tracer-a.log"),
            "+0.1 s hello\n".repeat(200),
        )
        .expect("write");
        fs::write(store.dir().join("crash-snapshot-b.pb"), [0_u8, 1, 2, 255]).expect("write");
        let zip = build(
            &store,
            &["tracer-a.log".to_owned(), "crash-snapshot-b.pb".to_owned()],
            "Model: test\n",
        )
        .expect("a bundle");
        let archive = zip::ZipArchive::new(Cursor::new(zip.as_slice())).expect("a readable zip");
        assert_eq!(archive.len(), 3);
        assert_eq!(
            read_entry(&zip, "logs/tracer-a.log"),
            "+0.1 s hello\n".repeat(200).into_bytes()
        );
        assert_eq!(
            read_entry(&zip, "logs/crash-snapshot-b.pb"),
            [0_u8, 1, 2, 255]
        );
        assert_eq!(read_entry(&zip, "device.txt"), b"Model: test\n");
        assert!(zip.len() < 2_600, "the text is deflated");
        let _ = fs::remove_dir_all(store.dir());
    }

    #[test]
    fn files_outside_logs_are_rejected() {
        let store = temp_store("bundle-reject");
        fs::write(store.dir().join("tracer-a.log"), "x").expect("write");
        for bad in [
            "../tracer-a.log",
            "sub/tracer-a.log",
            "gpu-steps",
            "C:\\x.log",
            "",
        ] {
            assert_eq!(
                build(&store, &["tracer-a.log".to_owned(), bad.to_owned()], ""),
                Err(BundleError::NotALog(bad.to_owned())),
                "{bad}"
            );
        }
        assert_eq!(build(&store, &[], ""), Err(BundleError::Empty));
        let _ = fs::remove_dir_all(store.dir());
    }

    #[test]
    fn a_missing_file_is_an_io_error() {
        let store = temp_store("bundle-missing");
        assert!(matches!(
            build(&store, &["tracer-none.log".to_owned()], ""),
            Err(BundleError::Io(_))
        ));
        let _ = fs::remove_dir_all(store.dir());
    }

    #[test]
    fn over_fifteen_megabytes_is_refused() {
        let store = temp_store("bundle-large");
        // Pseudo-random bytes do not deflate, so the zip stays over the limit.
        let mut state = 0x9E37_79B9_7F4A_7C15_u64;
        let noise: Vec<u8> = (0..(MAX_BUNDLE_BYTES + 1024))
            .map(|_| {
                state ^= state << 13;
                state ^= state >> 7;
                state ^= state << 17;
                state.to_le_bytes()[0]
            })
            .collect();
        fs::write(store.dir().join("crash-snapshot-a.pb"), noise).expect("write");
        assert_eq!(
            build(&store, &["crash-snapshot-a.pb".to_owned()], ""),
            Err(BundleError::TooLarge)
        );
        let _ = fs::remove_dir_all(store.dir());
    }

    #[test]
    fn select_all_excludes_the_crash_snapshot() {
        let store = temp_store("bundle-selectall");
        fs::write(store.dir().join("tracer-a.log"), "x").expect("write");
        fs::write(store.dir().join("crash-a.log"), "x").expect("write");
        fs::write(store.dir().join("crash-snapshot-a.pb"), "x").expect("write");
        let mut names = select_all_names(&store.list());
        names.sort();
        assert_eq!(names, ["crash-a.log", "tracer-a.log"]);
        let _ = fs::remove_dir_all(store.dir());
    }

    #[test]
    fn device_txt_has_only_the_device_facts() {
        let text = device_txt(&facts(), Some(&adapter()));
        assert!(text.contains("Model: Evermyth KB1001"));
        assert!(text.contains("SoC: SM8550"));
        assert!(text.contains("GPU: Mali-G57"));
        assert!(text.contains("Driver: Mali v1.r40"));
        assert!(text.contains("Android: 14 (API 34)"));
        assert!(text.contains("RAM: 7800 MB"));
        assert!(!text.contains("evermyth/kb1001"), "no fingerprint");
        assert_eq!(text.lines().count(), 6);
        let none = device_txt(&facts(), None);
        assert!(none.contains("GPU: not started in this session"));
        assert!(!none.contains("Driver:"));
    }

    #[test]
    fn the_subject_omits_the_gpu_part_without_an_adapter() {
        let (to, subject, body) = mail_fields(&facts(), Some(&adapter()));
        assert_eq!(to, "pleochroma@gussek.eu");
        assert_eq!(
            subject,
            "Pleochroma logs \u{b7} 0.12.0 \u{b7} Evermyth KB1001 \u{b7} Mali-G57"
        );
        assert!(body.starts_with("What did you do just before the problem?\n\n"));
        assert!(body.contains("Model: Evermyth KB1001"));
        let (_, subject, _) = mail_fields(&facts(), None);
        assert_eq!(
            subject,
            "Pleochroma logs \u{b7} 0.12.0 \u{b7} Evermyth KB1001"
        );
        let (_, subject, _) = mail_fields(&facts(), Some(&AdapterFacts::default()));
        assert!(!subject.ends_with('\u{b7}') && !subject.contains("Mali"));
    }
}
