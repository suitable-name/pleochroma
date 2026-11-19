//! [`Session`]: the tracer log's switch and file, and everything else one app start owns.
//!
//! A session is created by `AppState::new`, so every app start (also a relaunch in the same
//! process) has its own: its own file, its own unlock, its own switch. The global sinks write to
//! the newest one through [`current`].

use std::{
    fmt::Write as _,
    fs::OpenOptions,
    io::Write,
    path::PathBuf,
    sync::{
        Arc, Mutex, PoisonError, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::{Instant, SystemTime, UNIX_EPOCH},
};

use pleochroma::diag::{AdapterFacts, BreadcrumbSink};

use super::{
    breadcrumb::{Breadcrumbs, GPU_STEPS_FILE},
    exit::{self, CacheDecision, LastStart, StartReport},
    state::DiagState,
    store::LogStore,
    ticker::Ticker,
    unlock::Unlock,
};
use crate::platform::{DeviceFacts, Platform};

/// The logs folder inside the app's private files folder.
pub const LOGS_DIR: &str = "logs";

/// A tracer file stops here and the switch turns off.
pub const MAX_FILE_BYTES: u64 = 8 * 1024 * 1024;

/// The last line of a tracer file that reached its limit.
const LIMIT_LINE: &str = "log size limit reached";

/// The session the global sinks write to.
static CURRENT: Mutex<Weak<Session>> = Mutex::new(Weak::new());

/// Month names for [`utc_display`].
const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// Milliseconds since the Unix epoch (0 when the clock is before it).
#[must_use]
pub fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| {
            i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX)
        })
}

/// The civil date and time (UTC) of `ms` as year, month, day, hour, minute, second.
fn civil(ms: i64) -> (i64, i64, i64, i64, i64, i64) {
    let seconds = ms.div_euclid(1000);
    let days = seconds.div_euclid(86_400);
    let of_day = seconds.rem_euclid(86_400);
    // Days since 0000-03-01, then the era arithmetic of the proleptic Gregorian calendar.
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let march_based = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * march_based + 2) / 5 + 1;
    let month = if march_based < 10 {
        march_based + 3
    } else {
        march_based - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (
        year,
        month,
        day,
        of_day / 3600,
        of_day % 3600 / 60,
        of_day % 60,
    )
}

/// `YYYYMMDD-HHMMSS` (UTC), the stamp in the log file names.
#[must_use]
pub fn utc_stamp(ms: i64) -> String {
    let (year, month, day, hour, minute, second) = civil(ms);
    format!("{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}")
}

/// "Nov 17, 15:31 UTC", the time a person reads. The platform gives no time zone.
#[must_use]
pub fn utc_display(ms: i64) -> String {
    let (_, month, day, hour, minute, _) = civil(ms);
    let name = usize::try_from(month - 1)
        .ok()
        .and_then(|index| MONTHS.get(index))
        .copied()
        .unwrap_or("?");
    format!("{name} {day}, {hour:02}:{minute:02} UTC")
}

/// The app version the marks are keyed by: the platform's, else this crate's.
#[must_use]
pub fn app_version(facts: &DeviceFacts) -> String {
    if facts.app_version.is_empty() {
        env!("CARGO_PKG_VERSION").to_owned()
    } else {
        facts.app_version.clone()
    }
}

/// Whether and how the tracer log is writing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TracerState {
    /// Nothing written in this session.
    Off,
    /// Writing `file`, which holds `bytes` so far.
    On {
        /// The file's name inside the logs folder.
        file: String,
        /// Its size so far.
        bytes: u64,
    },
    /// Switched off by hand; `file` holds what was written.
    Stopped {
        /// The file's name inside the logs folder.
        file: String,
        /// Its size.
        bytes: u64,
    },
    /// A write failed: the device storage is full.
    StoppedStorageFull,
    /// The file reached 8 MB.
    StoppedAtLimit,
}

/// What the header needs from the settings; built by the caller, no GPU access.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HeaderFacts {
    /// The render engine, as the settings name it.
    pub engine: String,
    /// The backend choice.
    pub backend: String,
    /// The quality preset.
    pub quality: String,
    /// The sample targets.
    pub sample_targets: String,
    /// The bounce limit.
    pub bounces: u32,
    /// Whether the denoiser is on.
    pub denoise: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Off,
    On,
    Stopped,
    Full,
    AtLimit,
}

/// The tracer file and what is known about it.
struct FileState {
    mode: Mode,
    name: Option<String>,
    writer: Option<Box<dyn Write + Send>>,
    bytes: u64,
    part: u32,
    stamp: String,
    limit: u64,
}

/// Everything one app start owns for diagnostics. See the module docs.
pub struct Session {
    logs_dir: PathBuf,
    facts: DeviceFacts,
    started: Instant,
    on: AtomicBool,
    storage_notice: AtomicBool,
    start_notice: AtomicBool,
    file: Mutex<FileState>,
    unlock: Unlock,
    report: StartReport,
    state: DiagState,
    store: LogStore,
    crumbs: Arc<Breadcrumbs>,
    ticker: Mutex<Option<Ticker>>,
}

impl Session {
    /// Runs or takes the start-up check, becomes the current session and installs the
    /// breadcrumb sink. Called by `AppState::new`.
    #[must_use]
    pub fn new(platform: &Arc<dyn Platform>) -> Arc<Self> {
        let facts = platform.device_facts();
        let files = platform.app_dirs().files.clone();
        let (report, decision) = exit::session_start(platform);
        let crumbs = Breadcrumbs::shared(
            files.join(LOGS_DIR).join(GPU_STEPS_FILE),
            platform.process_id(),
        );
        let session = Arc::new(Self::build(
            &files,
            facts,
            report,
            decision,
            Arc::clone(&crumbs),
        ));
        // The start-up check has read the leftovers; pruning may now remove old files.
        session.store.prune();
        let sink: Arc<dyn BreadcrumbSink> = crumbs;
        pleochroma::diag::set_breadcrumb_sink(Some(sink));
        *CURRENT.lock().unwrap_or_else(PoisonError::into_inner) = Arc::downgrade(&session);
        *session
            .ticker
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Some(Ticker::start(&session));
        session
    }

    /// For tests: its own folders and facts, no start-up check, no global touched.
    #[must_use]
    pub fn for_test(files_dir: &std::path::Path, facts: DeviceFacts) -> Arc<Self> {
        let report = StartReport {
            last_start: LastStart::Clean,
            notice_due: false,
        };
        let crumbs = Arc::new(Breadcrumbs::new(
            files_dir.join(LOGS_DIR).join(GPU_STEPS_FILE),
            std::process::id(),
        ));
        Arc::new(Self::build(
            files_dir,
            facts,
            report,
            CacheDecision::On,
            crumbs,
        ))
    }

    fn build(
        files_dir: &std::path::Path,
        facts: DeviceFacts,
        report: StartReport,
        decision: CacheDecision,
        crumbs: Arc<Breadcrumbs>,
    ) -> Self {
        let logs_dir = files_dir.join(LOGS_DIR);
        let state = DiagState::open(
            files_dir,
            &app_version(&facts),
            &facts.fingerprint,
            decision,
        );
        Self {
            store: LogStore::new(logs_dir.clone()),
            logs_dir,
            facts,
            started: Instant::now(),
            on: AtomicBool::new(false),
            storage_notice: AtomicBool::new(false),
            start_notice: AtomicBool::new(report.notice_due),
            file: Mutex::new(FileState {
                mode: Mode::Off,
                name: None,
                writer: None,
                bytes: 0,
                part: 1,
                stamp: String::new(),
                limit: MAX_FILE_BYTES,
            }),
            unlock: Unlock::new(),
            report,
            state,
            crumbs,
            ticker: Mutex::new(None),
        }
    }

    /// Whether the tracer log is switched on. One relaxed load.
    #[must_use]
    pub fn tracer_on(&self) -> bool {
        self.on.load(Ordering::Relaxed)
    }

    /// Switches the tracer log on or off.
    ///
    /// Switching on creates or reopens the file and writes the header when the file is new.
    /// If the file cannot be created the state becomes `StoppedStorageFull`.
    pub fn set_tracer(&self, on: bool, header: &HeaderFacts) {
        let mut file = self.lock_file();
        if on {
            if file.mode != Mode::On {
                self.open_file(&mut file, header);
            }
        } else if file.mode == Mode::On {
            self.on.store(false, Ordering::Relaxed);
            file.writer = None;
            file.mode = Mode::Stopped;
        }
    }

    /// What the tracer is doing.
    #[must_use]
    pub fn tracer_state(&self) -> TracerState {
        let file = self.lock_file();
        let name = file.name.clone().unwrap_or_default();
        match file.mode {
            Mode::Off => TracerState::Off,
            Mode::On => TracerState::On {
                file: name,
                bytes: file.bytes,
            },
            Mode::Stopped => TracerState::Stopped {
                file: name,
                bytes: file.bytes,
            },
            Mode::Full => TracerState::StoppedStorageFull,
            Mode::AtLimit => TracerState::StoppedAtLimit,
        }
    }

    /// Writes one whole line, with its "+12.345 s " time added, then flushes.
    ///
    /// Does nothing while the tracer is off. A failed write switches the tracer off and sets
    /// `StoppedStorageFull`; it never panics.
    pub fn write_line(&self, line: &str) {
        if !self.tracer_on() {
            return;
        }
        let elapsed = self.started.elapsed().as_millis();
        let line = line.replace(['\r', '\n'], " ");
        let text = format!("+{}.{:03} s {line}\n", elapsed / 1000, elapsed % 1000);
        let mut file = self.lock_file();
        if file.mode == Mode::On {
            self.append(&mut file, &text);
        }
    }

    /// True once after the storage-full stop, so the UI shows its toast once.
    #[must_use]
    pub fn take_storage_full_notice(&self) -> bool {
        self.storage_notice.swap(false, Ordering::Relaxed)
    }

    /// True once when the start-up check made a crash entry the user has not been told about.
    #[must_use]
    pub fn take_start_notice(&self) -> bool {
        self.start_notice.swap(false, Ordering::Relaxed)
    }

    /// The five-tap unlock of this session.
    #[must_use]
    pub const fn unlock(&self) -> &Unlock {
        &self.unlock
    }

    /// What the start-up check found at this start.
    #[must_use]
    pub const fn start_report(&self) -> &StartReport {
        &self.report
    }

    /// `diagnostics.toml`: the engine marks and the shader cache decision.
    #[must_use]
    pub const fn state(&self) -> &DiagState {
        &self.state
    }

    /// The logs folder's store.
    #[must_use]
    pub const fn store(&self) -> &LogStore {
        &self.store
    }

    /// The device facts of this start.
    #[must_use]
    pub const fn facts(&self) -> &DeviceFacts {
        &self.facts
    }

    /// The breadcrumb list this session reads and the sink writes.
    #[must_use]
    pub const fn breadcrumbs(&self) -> &Arc<Breadcrumbs> {
        &self.crumbs
    }

    /// The adapter recorded in this process, if any (for the e-mail subject and `device.txt`).
    #[must_use]
    pub fn adapter(&self) -> Option<AdapterFacts> {
        self.crumbs.recorded_adapter()
    }

    fn lock_file(&self) -> std::sync::MutexGuard<'_, FileState> {
        self.file.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Opens the tracer file (a new one after the size limit) and writes the header into a new
    /// file. On failure the state is `Full` and the notice is armed.
    fn open_file(&self, file: &mut FileState, header: &HeaderFacts) {
        if file.mode == Mode::AtLimit {
            file.part += 1;
            file.name = None;
            file.bytes = 0;
        }
        let fresh = file.name.is_none();
        if fresh {
            if file.stamp.is_empty() {
                file.stamp = utc_stamp(now_ms());
            }
            file.name = Some(if file.part <= 1 {
                format!("tracer-{}.log", file.stamp)
            } else {
                format!("tracer-{}-{}.log", file.stamp, file.part)
            });
        }
        let name = file.name.clone().unwrap_or_default();
        let opened = std::fs::create_dir_all(&self.logs_dir).and_then(|()| {
            OpenOptions::new()
                .create(true)
                .append(true)
                .open(self.logs_dir.join(&name))
        });
        match opened {
            Ok(handle) => {
                file.writer = Some(Box::new(handle));
                file.mode = Mode::On;
                self.on.store(true, Ordering::Relaxed);
                if fresh {
                    let text = self.header_text(header);
                    self.append(file, &text);
                }
            }
            Err(_) => {
                if fresh {
                    file.name = None;
                }
                self.storage_stopped(file);
            }
        }
    }

    /// Writes `text` (one or more whole lines) and flushes. Applies the 8 MB rule and the
    /// storage-full rule.
    fn append(&self, file: &mut FileState, text: &str) {
        let length = u64::try_from(text.len()).unwrap_or(u64::MAX);
        if file.bytes.saturating_add(length) > file.limit {
            let last = format!("{LIMIT_LINE}\n");
            let written = Self::put(file, &last);
            self.on.store(false, Ordering::Relaxed);
            file.writer = None;
            if written {
                file.mode = Mode::AtLimit;
            } else {
                self.storage_stopped(file);
            }
            return;
        }
        if !Self::put(file, text) {
            self.storage_stopped(file);
        }
    }

    /// One `write_all` and one `flush`; counts the bytes. False when either failed.
    fn put(file: &mut FileState, text: &str) -> bool {
        let Some(writer) = file.writer.as_mut() else {
            return false;
        };
        if writer.write_all(text.as_bytes()).is_err() || writer.flush().is_err() {
            return false;
        }
        file.bytes += u64::try_from(text.len()).unwrap_or(u64::MAX);
        true
    }

    fn storage_stopped(&self, file: &mut FileState) {
        self.on.store(false, Ordering::Relaxed);
        file.writer = None;
        file.mode = Mode::Full;
        self.storage_notice.store(true, Ordering::Relaxed);
    }

    /// The lines at the top of a new tracer file.
    fn header_text(&self, header: &HeaderFacts) -> String {
        let facts = &self.facts;
        let mut text = String::from("Pleochroma tracer log\n");
        let _ = writeln!(
            text,
            "app {} (pleochroma {}, build {})",
            app_version(facts),
            pleochroma::VERSION,
            pleochroma::BUILD_ID
        );
        let mut device = format!(
            "device {} {}, Android {} (SDK {}), {}, {} MB RAM",
            facts.manufacturer,
            facts.model,
            facts.android_release,
            facts.sdk_int,
            facts.abi,
            facts.total_ram_mb
        );
        if let Some(soc) = &facts.soc_model {
            let _ = write!(device, ", SoC {soc}");
        }
        if let Some(headroom) = facts.thermal_headroom {
            let _ = write!(device, ", thermal headroom {headroom:.2}");
        }
        let _ = writeln!(text, "{device}");
        let _ = writeln!(
            text,
            "settings: engine {}, backend {}, quality {}, sample targets {}, bounces {}, denoise {}",
            header.engine,
            header.backend,
            header.quality,
            header.sample_targets,
            header.bounces,
            if header.denoise { "on" } else { "off" }
        );
        let adapter = self.adapter();
        let _ = writeln!(
            text,
            "shader cache: {}",
            cache_state_text(self.state.cache(), adapter.as_ref())
        );
        if let Some(adapter) = &adapter {
            let _ = writeln!(text, "{}", adapter_line(adapter));
        }
        text
    }
}

/// The shader cache state in the words of Render Details (its four states).
#[must_use]
pub fn cache_state_text(decision: CacheDecision, adapter: Option<&AdapterFacts>) -> &'static str {
    match decision {
        CacheDecision::OffThisStart => {
            "off for this start (the app closed while loading it last time)"
        }
        CacheDecision::OffOnThisDevice => {
            "off on this device (the app closed twice while loading it)"
        }
        CacheDecision::On => {
            if adapter.is_some_and(|facts| !facts.pipeline_cache_offered) {
                "not offered by this graphics card"
            } else {
                "on"
            }
        }
    }
}

/// The adapter line of the header: name, backend, driver, device type, limits, cache support.
#[must_use]
pub fn adapter_line(adapter: &AdapterFacts) -> String {
    format!(
        "adapter {} · {} · driver {} {} · {} · {} · pipeline cache offered: {}",
        adapter.name,
        adapter.backend,
        adapter.driver,
        adapter.driver_info,
        adapter.device_type,
        adapter.limits,
        if adapter.pipeline_cache_offered {
            "yes"
        } else {
            "no"
        }
    )
}

/// The session the global sinks write to; replaced by every [`Session::new`].
#[must_use]
pub fn current() -> Option<Arc<Session>> {
    CURRENT
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .upgrade()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform_mock::unique_temp_dir;

    fn session(tag: &str) -> (Arc<Session>, PathBuf) {
        let dir = unique_temp_dir(tag);
        let facts = DeviceFacts {
            app_version: "0.12.0".to_owned(),
            manufacturer: "Evermyth".to_owned(),
            model: "KB1001".to_owned(),
            android_release: "16".to_owned(),
            sdk_int: 36,
            fingerprint: "fp".to_owned(),
            ..DeviceFacts::default()
        };
        (Session::for_test(&dir, facts), dir)
    }

    fn header() -> HeaderFacts {
        HeaderFacts {
            engine: "Ray-Traced".to_owned(),
            backend: "Automatic".to_owned(),
            quality: "Standard".to_owned(),
            sample_targets: "64".to_owned(),
            bounces: 8,
            denoise: true,
        }
    }

    fn tracer_files(dir: &std::path::Path) -> Vec<String> {
        let mut names: Vec<String> = std::fs::read_dir(dir.join(LOGS_DIR))
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|entry| entry.file_name().to_string_lossy().into_owned())
                    .filter(|name| name.starts_with("tracer-"))
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    struct FailingWriter;

    impl Write for FailingWriter {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("no space left on device"))
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn a_new_session_is_locked_and_off() {
        let (session, dir) = session("session-new");
        assert!(!session.unlock().unlocked());
        assert!(!session.tracer_on());
        assert_eq!(session.tracer_state(), TracerState::Off);
        assert_eq!(session.start_report().last_start, LastStart::Clean);
        assert!(!session.take_storage_full_notice());
        assert!(
            tracer_files(&dir).is_empty(),
            "nothing is written before the switch"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_line_written_while_off_goes_nowhere() {
        let (session, dir) = session("session-off");
        session.write_line("nothing");
        assert!(tracer_files(&dir).is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_first_switch_on_creates_one_file_with_a_header_and_later_ones_append() {
        let (session, dir) = session("session-append");
        session.set_tracer(true, &header());
        assert!(session.tracer_on());
        session.write_line("first");
        session.set_tracer(false, &header());
        assert!(!session.tracer_on());
        assert!(matches!(
            session.tracer_state(),
            TracerState::Stopped { .. }
        ));
        session.write_line("dropped while off");
        session.set_tracer(true, &header());
        session.write_line("second");
        let names = tracer_files(&dir);
        assert_eq!(names.len(), 1, "{names:?}");
        let text = std::fs::read_to_string(dir.join(LOGS_DIR).join(&names[0])).expect("file");
        assert_eq!(text.matches("Pleochroma tracer log").count(), 1, "{text}");
        assert!(text.contains("engine Ray-Traced"), "{text}");
        assert!(text.contains("Evermyth KB1001"), "{text}");
        assert!(text.contains("shader cache: on"), "{text}");
        assert!(text.contains(" s first\n"), "{text}");
        assert!(text.contains(" s second\n"), "{text}");
        assert!(!text.contains("dropped while off"), "{text}");
        assert!(text.lines().any(|line| line.starts_with('+')), "{text}");
        match session.tracer_state() {
            TracerState::On { file, bytes } => {
                assert_eq!(file, names[0]);
                assert_eq!(bytes, u64::try_from(text.len()).expect("fits"));
            }
            other => panic!("unexpected state {other:?}"),
        }
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_line_is_one_line() {
        let (session, dir) = session("session-oneline");
        session.set_tracer(true, &header());
        session.write_line("a\nb\r\nc");
        let names = tracer_files(&dir);
        let text = std::fs::read_to_string(dir.join(LOGS_DIR).join(&names[0])).expect("file");
        assert!(text.contains(" s a b  c\n"), "{text}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn eight_megabytes_write_the_final_line_and_a_second_file_follows() {
        let (session, dir) = session("session-limit");
        session.lock_file().limit = 900;
        session.set_tracer(true, &header());
        for index in 0..40 {
            session.write_line(&format!("line number {index} with some padding text"));
        }
        assert!(!session.tracer_on());
        assert_eq!(session.tracer_state(), TracerState::StoppedAtLimit);
        let first = tracer_files(&dir);
        assert_eq!(first.len(), 1);
        let text = std::fs::read_to_string(dir.join(LOGS_DIR).join(&first[0])).expect("file");
        assert_eq!(text.lines().last(), Some(LIMIT_LINE), "{text}");
        session.set_tracer(true, &header());
        session.write_line("continued");
        let names = tracer_files(&dir);
        assert_eq!(names.len(), 2, "{names:?}");
        let second = names
            .iter()
            .find(|name| name.ends_with("-2.log"))
            .expect("-2 file");
        let text = std::fs::read_to_string(dir.join(LOGS_DIR).join(second)).expect("file");
        assert!(text.contains(" s continued\n"), "{text}");
        assert!(text.contains("Pleochroma tracer log"), "{text}");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_failed_write_switches_off_sets_the_notice_once_and_does_not_panic() {
        let (session, dir) = session("session-full");
        session.set_tracer(true, &header());
        session.lock_file().writer = Some(Box::new(FailingWriter));
        session.write_line("this cannot be written");
        assert!(!session.tracer_on());
        assert_eq!(session.tracer_state(), TracerState::StoppedStorageFull);
        assert!(session.take_storage_full_notice());
        assert!(!session.take_storage_full_notice());
        session.write_line("ignored");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_file_that_cannot_be_created_is_the_storage_full_stop() {
        let (session, dir) = session("session-nocreate");
        // A plain file where the logs folder should be makes `create_dir_all` fail.
        std::fs::create_dir_all(&dir).expect("dir");
        std::fs::write(dir.join(LOGS_DIR), b"in the way").expect("blocker");
        session.set_tracer(true, &header());
        assert!(!session.tracer_on());
        assert_eq!(session.tracer_state(), TracerState::StoppedStorageFull);
        assert!(session.take_storage_full_notice());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_header_names_the_cache_state_and_a_known_adapter() {
        let (session, dir) = session("session-header");
        session.crumbs.adapter(&AdapterFacts {
            name: "Mali-G57".to_owned(),
            pipeline_cache_offered: false,
            ..AdapterFacts::default()
        });
        session.set_tracer(true, &header());
        let names = tracer_files(&dir);
        let text = std::fs::read_to_string(dir.join(LOGS_DIR).join(&names[0])).expect("file");
        assert!(
            text.contains("shader cache: not offered by this graphics card"),
            "{text}"
        );
        assert!(text.contains("adapter Mali-G57"), "{text}");
        assert_eq!(
            session.adapter().map(|a| a.name),
            Some("Mali-G57".to_owned())
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_cache_texts_are_the_four_states() {
        let offered = AdapterFacts {
            pipeline_cache_offered: true,
            ..AdapterFacts::default()
        };
        assert_eq!(cache_state_text(CacheDecision::On, Some(&offered)), "on");
        assert_eq!(cache_state_text(CacheDecision::On, None), "on");
        assert!(
            cache_state_text(CacheDecision::OffThisStart, None).starts_with("off for this start")
        );
        assert!(
            cache_state_text(CacheDecision::OffOnThisDevice, None)
                .starts_with("off on this device")
        );
    }

    #[test]
    fn the_clock_helpers_format_utc() {
        assert_eq!(utc_stamp(0), "19700101-000000");
        assert_eq!(utc_stamp(1_700_000_000_000), "20231114-221320");
        assert_eq!(utc_display(1_700_000_000_000), "Nov 14, 22:13 UTC");
        // 2024-02-29 12:00:00 UTC, a leap day.
        assert_eq!(utc_stamp(1_709_208_000_000), "20240229-120000");
        assert_eq!(utc_display(-1), "Dec 31, 23:59 UTC");
    }

    #[test]
    fn the_app_version_falls_back_to_the_crate_version() {
        assert_eq!(
            app_version(&DeviceFacts::default()),
            env!("CARGO_PKG_VERSION")
        );
        let facts = DeviceFacts {
            app_version: "9.9.9".to_owned(),
            ..DeviceFacts::default()
        };
        assert_eq!(app_version(&facts), "9.9.9");
    }
}
