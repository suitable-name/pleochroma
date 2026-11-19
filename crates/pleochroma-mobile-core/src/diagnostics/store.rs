//! The folder of diagnostic logs: kinds, listing, tail reads, deletion, retention and sample files.

use std::{
    collections::HashSet,
    fmt::Write as _,
    fs::{self, File},
    io::{self, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    time::{Duration, UNIX_EPOCH},
};

/// The most bytes all log files may use together after a prune.
const MAX_TOTAL_BYTES: u64 = 20 * 1024 * 1024;
/// The most tracer files kept after a prune.
const MAX_TRACER_FILES: usize = 10;
/// How much of a crash log is searched for the names of its trace files.
const CRASH_TEXT_BYTES: u64 = 64 * 1024;

/// What a file in the logs folder is, decided by its name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogKind {
    /// `tracer-<stamp>.log`, the tracer log of one app start.
    Tracer,
    /// `crash-<stamp>.log`, written by the start-up check after a crash.
    Crash,
    /// `interrupted-<stamp>.log`, written when the app was closed during a step.
    Interrupted,
    /// `probe-<stamp>.log`, the report of one GPU probe.
    Probe,
    /// `anr-<stamp>.txt`, the system's trace of an unresponsive app.
    Anr,
    /// `*.pb`, the binary tombstone; never text, never part of "Select all".
    CrashSnapshot,
}

/// One file of the logs folder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogEntry {
    /// The file name, without a folder.
    pub name: String,
    /// What the file is.
    pub kind: LogKind,
    /// The file size in bytes.
    pub bytes: u64,
    /// The last change, in milliseconds since the Unix epoch.
    pub modified_ms: i64,
}

/// The logs folder (`<files>/logs/`) and every operation on the files in it.
pub struct LogStore {
    dir: PathBuf,
}

impl LogStore {
    /// A store over `logs_dir`; the folder is created when something is written.
    #[must_use]
    pub const fn new(logs_dir: PathBuf) -> Self {
        Self { dir: logs_dir }
    }

    /// The logs folder.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Every log file, newest first, with the crash snapshots last.
    #[must_use]
    pub fn list(&self) -> Vec<LogEntry> {
        let Ok(read) = fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut entries: Vec<LogEntry> = read
            .filter_map(Result::ok)
            .filter_map(|item| {
                let name = item.file_name().into_string().ok()?;
                let kind = Self::kind_of(&name)?;
                let meta = item.metadata().ok()?;
                meta.is_file().then(|| LogEntry {
                    name,
                    kind,
                    bytes: meta.len(),
                    modified_ms: modified_ms(&meta),
                })
            })
            .collect();
        entries.sort_by(|a, b| {
            (a.kind == LogKind::CrashSnapshot)
                .cmp(&(b.kind == LogKind::CrashSnapshot))
                .then(b.modified_ms.cmp(&a.modified_ms))
                .then(b.name.cmp(&a.name))
        });
        entries
    }

    /// The kind of a file name: the extension `.pb` first, then the prefix.
    ///
    /// A name with a folder part, and every name of another shape, is `None`.
    #[must_use]
    pub fn kind_of(name: &str) -> Option<LogKind> {
        if !is_plain_name(name) {
            return None;
        }
        if name.ends_with(".pb") {
            return Some(LogKind::CrashSnapshot);
        }
        if name.starts_with("crash-snapshot-") {
            return None;
        }
        let text = |prefix: &str| name.starts_with(prefix) && name.ends_with(".log");
        if text("tracer-") {
            Some(LogKind::Tracer)
        } else if text("crash-") {
            Some(LogKind::Crash)
        } else if text("interrupted-") {
            Some(LogKind::Interrupted)
        } else if text("probe-") {
            Some(LogKind::Probe)
        } else if name.starts_with("anr-") && name.ends_with(".txt") {
            Some(LogKind::Anr)
        } else {
            None
        }
    }

    /// The last `max_bytes` of a text log as text, cut at a line start, and the file's whole size.
    ///
    /// # Errors
    ///
    /// Fails for a name that is not a text log (a crash snapshot is binary) and when the file
    /// cannot be read.
    pub fn read_tail(&self, name: &str, max_bytes: u64) -> io::Result<(String, u64)> {
        match Self::kind_of(name) {
            None => return Err(invalid_input("not a log file")),
            Some(LogKind::CrashSnapshot) => return Err(invalid_input("a binary file")),
            Some(_) => {}
        }
        let mut file = File::open(self.dir.join(name))?;
        let total = file.metadata()?.len();
        let start = total.saturating_sub(max_bytes);
        // One byte earlier tells whether `start` is already a line start.
        let from = start.saturating_sub(1);
        file.seek(SeekFrom::Start(from))?;
        let mut bytes = Vec::new();
        file.take(total - from).read_to_end(&mut bytes)?;
        if start > 0 {
            let cut = if bytes.first() == Some(&b'\n') {
                1
            } else {
                bytes
                    .iter()
                    .position(|byte| *byte == b'\n')
                    .map_or(1, |at| at + 1)
            };
            bytes.drain(..cut.min(bytes.len()));
        }
        Ok((String::from_utf8_lossy(&bytes).into_owned(), total))
    }

    /// Deletes the named log files except those in `keep` (files being written).
    ///
    /// Returns how many were removed. A name that is not a log file is ignored.
    pub fn delete(&self, names: &[String], keep: &[String]) -> usize {
        let mut seen: HashSet<&str> = HashSet::new();
        let mut removed = 0;
        for name in names {
            if keep.contains(name) || Self::kind_of(name).is_none() || !seen.insert(name) {
                continue;
            }
            if fs::remove_file(self.dir.join(name)).is_ok() {
                removed += 1;
            }
        }
        removed
    }

    /// Applies the retention rule: at most 10 tracer files and 20 MB in total.
    ///
    /// Tracer files go first, oldest first, then the other files, and the newest crash or
    /// interrupted entry with its trace files last.
    pub fn prune(&self) {
        let mut entries = self.list();
        let extra: Vec<String> = entries
            .iter()
            .filter(|entry| entry.kind == LogKind::Tracer)
            .skip(MAX_TRACER_FILES)
            .map(|entry| entry.name.clone())
            .collect();
        for name in &extra {
            let _ = fs::remove_file(self.dir.join(name));
        }
        entries.retain(|entry| !extra.contains(&entry.name));
        let mut total: u64 = entries.iter().map(|entry| entry.bytes).sum();
        if total <= MAX_TOTAL_BYTES {
            return;
        }
        let protected = self.protected_names(&entries);
        let rank = |entry: &LogEntry| {
            if protected.contains(&entry.name) {
                2_u8
            } else {
                u8::from(entry.kind != LogKind::Tracer)
            }
        };
        let mut order: Vec<&LogEntry> = entries.iter().collect();
        order.sort_by_key(|entry| (rank(entry), entry.modified_ms));
        for entry in order {
            if total <= MAX_TOTAL_BYTES {
                break;
            }
            if fs::remove_file(self.dir.join(&entry.name)).is_ok() {
                total = total.saturating_sub(entry.bytes);
            }
        }
    }

    /// The newest crash or interrupted entry and the trace files its text names.
    fn protected_names(&self, entries: &[LogEntry]) -> HashSet<String> {
        let mut names = HashSet::new();
        let Some(newest) = entries
            .iter()
            .find(|entry| matches!(entry.kind, LogKind::Crash | LogKind::Interrupted))
        else {
            return names;
        };
        names.insert(newest.name.clone());
        if let Ok((text, _)) = self.read_tail(&newest.name, CRASH_TEXT_BYTES) {
            names.extend(
                entries
                    .iter()
                    .filter(|entry| matches!(entry.kind, LogKind::Anr | LogKind::CrashSnapshot))
                    .filter(|entry| text.contains(&entry.name))
                    .map(|entry| entry.name.clone()),
            );
        }
        names
    }

    /// Writes the fixed sample files of the preview flag `--developer-sample-logs`.
    ///
    /// The names, sizes and times are fixed so screenshots repeat; one file is a
    /// `crash-snapshot-….pb`.
    ///
    /// # Errors
    ///
    /// Fails when the folder or a file cannot be written.
    pub fn write_sample_logs(&self) -> io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let day17 = 1_794_873_600_u64; // 2026-11-17 00:00 UTC
        let day16 = day17 - 86_400;
        let tracer_head =
            "tracer log, sample\napp version sample\nengine Ray-Traced backend Device GPU\n";
        let crash_head = "crash, sample\nstep building the Ray-Traced shader\nreason crash in native code\n\
                          trace file crash-snapshot-20261117-153100.pb (Android tombstone, protobuf)\n";
        let probe_head = "GPU probe, sample\n";
        let samples: [(&str, Vec<u8>, u64); 5] = [
            (
                "tracer-20261117-153200.log",
                filler(tracer_head, 14 * 1024),
                day17 + 55_920,
            ),
            (
                "crash-20261117-153100.log",
                filler(crash_head, 3 * 1024),
                day17 + 55_860,
            ),
            (
                "probe-20261117-154000.log",
                filler(probe_head, 6 * 1024),
                day17 + 56_400,
            ),
            (
                "tracer-20261116-091500.log",
                filler(tracer_head, 3_250_585),
                day16 + 33_300,
            ),
            (
                "crash-snapshot-20261117-153100.pb",
                binary_filler(410 * 1024),
                day17 + 55_860,
            ),
        ];
        for (name, content, stamp) in samples {
            let path = self.dir.join(name);
            fs::write(&path, content)?;
            File::options()
                .write(true)
                .open(&path)?
                .set_modified(UNIX_EPOCH + Duration::from_secs(stamp))?;
        }
        Ok(())
    }
}

/// True for a bare file name: no folder part, no parent step.
fn is_plain_name(name: &str) -> bool {
    !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\', '\0', ':'])
}

fn invalid_input(text: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, text)
}

fn modified_ms(meta: &fs::Metadata) -> i64 {
    meta.modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

const SAMPLE_EVENTS: [&str; 6] = [
    "scene built facets 57 material class isotropic bounces 8",
    "compile start kernel trace class isotropic",
    "compile done kernel trace class isotropic 412 ms",
    "range done samples 16 of 256 38 ms per sample",
    "first readback 52 ms",
    "range done samples 32 of 256 36 ms per sample",
];

/// ASCII text of exactly `size` bytes: `head`, then event lines, ending in a newline.
fn filler(head: &str, size: usize) -> Vec<u8> {
    let mut text = String::from(head);
    let mut n = 0_usize;
    while text.len() < size {
        let at = (n as f64).mul_add(0.137, 0.4);
        let event = SAMPLE_EVENTS[n % SAMPLE_EVENTS.len()];
        let _ = writeln!(text, "+{at:.3} s {event}");
        n += 1;
    }
    text.truncate(size);
    text.pop();
    text.push('\n');
    text.into_bytes()
}

/// `size` bytes of a repeating byte pattern, standing in for a tombstone.
fn binary_filler(size: usize) -> Vec<u8> {
    (0..size)
        .map(|at| u8::try_from((at * 7 + 3) % 251).unwrap_or(0))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform_mock::unique_temp_dir;

    fn temp_store(tag: &str) -> LogStore {
        let dir = unique_temp_dir(tag);
        fs::create_dir_all(&dir).expect("create the logs folder");
        LogStore::new(dir)
    }

    fn put(store: &LogStore, name: &str, bytes: &[u8], age_s: u64) {
        let path = store.dir().join(name);
        fs::write(&path, bytes).expect("write the file");
        let time = UNIX_EPOCH + Duration::from_secs(1_800_000_000 - age_s);
        File::options()
            .write(true)
            .open(&path)
            .expect("open the file")
            .set_modified(time)
            .expect("set the time");
    }

    fn names(store: &LogStore) -> Vec<String> {
        store.list().into_iter().map(|entry| entry.name).collect()
    }

    #[test]
    fn pb_files_are_crash_snapshots_even_with_a_crash_prefix() {
        assert_eq!(
            LogStore::kind_of("crash-snapshot-20261117-153100.pb"),
            Some(LogKind::CrashSnapshot)
        );
        assert_eq!(
            LogStore::kind_of("crash-20261117-153100.pb"),
            Some(LogKind::CrashSnapshot)
        );
        assert_eq!(
            LogStore::kind_of("crash-20261117-153100.log"),
            Some(LogKind::Crash)
        );
        assert_eq!(LogStore::kind_of("crash-snapshot-1.log"), None);
    }

    #[test]
    fn kinds_by_prefix_and_odd_names() {
        assert_eq!(
            LogStore::kind_of("tracer-20261117-1532-2.log"),
            Some(LogKind::Tracer)
        );
        assert_eq!(
            LogStore::kind_of("interrupted-1.log"),
            Some(LogKind::Interrupted)
        );
        assert_eq!(LogStore::kind_of("probe-1.log"), Some(LogKind::Probe));
        assert_eq!(LogStore::kind_of("anr-1.txt"), Some(LogKind::Anr));
        assert_eq!(LogStore::kind_of("gpu-steps"), None);
        assert_eq!(LogStore::kind_of("../tracer-1.log"), None);
        assert_eq!(LogStore::kind_of("logs/tracer-1.log"), None);
        assert_eq!(LogStore::kind_of(""), None);
    }

    #[test]
    fn list_is_newest_first_with_snapshots_last() {
        let store = temp_store("store-list");
        put(&store, "tracer-a.log", b"a", 300);
        put(&store, "crash-snapshot-b.pb", b"b", 0);
        put(&store, "probe-c.log", b"c", 100);
        put(&store, "gpu-steps", b"x", 0);
        assert_eq!(
            names(&store),
            ["probe-c.log", "tracer-a.log", "crash-snapshot-b.pb"]
        );
        let _ = fs::remove_dir_all(store.dir());
    }

    #[test]
    fn read_tail_cuts_at_a_line_start() {
        let store = temp_store("store-tail");
        put(&store, "tracer-a.log", b"one\ntwo\nthree\nfour\n", 0);
        // The last 9 bytes start inside "three": the partial line is dropped.
        let (text, total) = store.read_tail("tracer-a.log", 9).expect("read");
        assert_eq!(total, 19);
        assert_eq!(text, "four\n");
        // Exactly at a line start nothing is lost.
        let (text, _) = store.read_tail("tracer-a.log", 5).expect("read");
        assert_eq!(text, "four\n");
        let (text, _) = store.read_tail("tracer-a.log", 1000).expect("read");
        assert_eq!(text, "one\ntwo\nthree\nfour\n");
        assert!(store.read_tail("../x.log", 10).is_err());
        put(&store, "crash-snapshot-b.pb", b"\x00\x01", 0);
        assert!(store.read_tail("crash-snapshot-b.pb", 10).is_err());
        let _ = fs::remove_dir_all(store.dir());
    }

    #[test]
    fn delete_skips_the_keep_list_and_foreign_names() {
        let store = temp_store("store-delete");
        put(&store, "tracer-a.log", b"a", 0);
        put(&store, "tracer-b.log", b"b", 0);
        put(&store, "probe-c.log", b"c", 0);
        fs::write(store.dir().join("gpu-steps"), b"x").expect("write");
        let removed = store.delete(
            &[
                "tracer-a.log".to_owned(),
                "tracer-b.log".to_owned(),
                "tracer-b.log".to_owned(),
                "gpu-steps".to_owned(),
                "../tracer-a.log".to_owned(),
            ],
            &["tracer-a.log".to_owned()],
        );
        assert_eq!(removed, 1);
        assert!(store.dir().join("tracer-a.log").exists());
        assert!(!store.dir().join("tracer-b.log").exists());
        assert!(store.dir().join("gpu-steps").exists());
        let _ = fs::remove_dir_all(store.dir());
    }

    #[test]
    fn prune_keeps_the_ten_newest_tracer_files() {
        let store = temp_store("store-prune-count");
        for i in 0..13_u64 {
            put(&store, &format!("tracer-{i:02}.log"), b"x", 1000 - i * 10);
        }
        put(&store, "crash-1.log", b"x", 5000);
        store.prune();
        let kept = names(&store);
        assert_eq!(
            kept.iter()
                .filter(|name| name.starts_with("tracer-"))
                .count(),
            10
        );
        assert!(!kept.contains(&"tracer-00.log".to_owned()));
        assert!(kept.contains(&"tracer-12.log".to_owned()));
        assert!(kept.contains(&"crash-1.log".to_owned()));
        let _ = fs::remove_dir_all(store.dir());
    }

    #[test]
    fn prune_over_the_size_limit_removes_the_newest_crash_entry_last() {
        let store = temp_store("store-prune-size");
        let big = vec![b'x'; 11 * 1024 * 1024];
        put(&store, "tracer-old.log", &big, 900);
        put(&store, "probe-old.log", &big, 800);
        put(&store, "anr-new.txt", b"anr text", 100);
        put(
            &store,
            "crash-new.log",
            b"files saved: anr-new.txt and crash-snapshot-new.pb",
            90,
        );
        put(&store, "crash-snapshot-new.pb", &big, 80);
        // 33 MB in total: the old tracer file goes first, then the old probe file.
        store.prune();
        let kept = names(&store);
        assert!(!kept.contains(&"tracer-old.log".to_owned()));
        assert!(kept.contains(&"crash-new.log".to_owned()));
        assert!(kept.contains(&"crash-snapshot-new.pb".to_owned()));
        assert!(kept.contains(&"anr-new.txt".to_owned()));
        let total: u64 = store.list().iter().map(|entry| entry.bytes).sum();
        assert!(total <= MAX_TOTAL_BYTES);
        assert!(!kept.contains(&"probe-old.log".to_owned()));
        let _ = fs::remove_dir_all(store.dir());
    }

    #[test]
    fn sample_logs_have_fixed_names_and_sizes() {
        let store = temp_store("store-sample");
        store.write_sample_logs().expect("write the samples");
        let entries = store.list();
        let snapshot = entries
            .iter()
            .find(|entry| entry.kind == LogKind::CrashSnapshot)
            .expect("a crash snapshot");
        assert_eq!(snapshot.name, "crash-snapshot-20261117-153100.pb");
        assert_eq!(snapshot.bytes, 410 * 1024);
        assert_eq!(
            entries.last().map(|entry| entry.kind),
            Some(LogKind::CrashSnapshot)
        );
        let tracer = entries
            .iter()
            .find(|entry| entry.name == "tracer-20261117-153200.log")
            .expect("the tracer sample");
        assert_eq!(tracer.bytes, 14 * 1024);
        let _ = fs::remove_dir_all(store.dir());
    }
}
