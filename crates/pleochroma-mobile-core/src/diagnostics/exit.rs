//! The start-up check: what an earlier process left behind, and what the app does about it.
//!
//! At every start, before any GPU work, the breadcrumb file of an earlier process and the
//! system's exit information are read together. A confirmed crash or ANR in a GPU step marks
//! that engine (Automatic then uses the CPU); a confirmed crash in a shader cache step only
//! switches the cache off; anything else is an "interrupted" entry. The table is in the
//! blueprint, section 4.4.

use std::{
    fmt::Write as _,
    path::PathBuf,
    sync::{Arc, Mutex, PoisonError},
};

use pleochroma::diag::{Engine, StepKind};

use super::{
    breadcrumb::{self, GPU_STEPS_FILE, Leftover},
    session::{LOGS_DIR, app_version, now_ms, utc_display, utc_stamp},
    state::{DIAG_FILE, DiagFile},
};
use crate::platform::{DeviceFacts, ExitInfo, ExitReason, Platform};

/// The shader cache folder inside the app's cache directory.
pub const CACHE_FOLDER: &str = "gpu-pipelines";

/// The first Android release that reports why the previous process ended.
const FIRST_EXIT_INFO_SDK: u32 = 30;

/// What the last start found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LastStart {
    /// Nothing worth telling.
    Clean,
    /// A confirmed crash or ANR in a GPU step: the engine is marked.
    EngineCrash {
        /// The engine whose step was open.
        engine: Engine,
        /// The step in plain words, for example "building the Ray-Traced shader".
        step: String,
        /// When it happened, as a person reads it (UTC).
        when: String,
        /// The crash log's file name.
        log: String,
    },
    /// A confirmed crash in a cache step: the cache rule applied, no engine mark.
    CacheCrash {
        /// The step in plain words.
        step: String,
        /// When it happened (UTC).
        when: String,
        /// The crash log's file name.
        log: String,
    },
    /// The app was closed or killed, or a probe stage crashed: no engine is marked.
    Interrupted {
        /// The step in plain words.
        step: String,
        /// When it happened (UTC).
        when: String,
        /// The log's file name (an `interrupted-` or, for a probe crash, a `crash-` log).
        log: String,
    },
}

/// The result of the start-up check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartReport {
    /// What the check found.
    pub last_start: LastStart,
    /// The Last Start sheet is due (a crash entry made by this check, not shown yet).
    pub notice_due: bool,
}

/// Whether this start uses the persistent shader cache.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheDecision {
    /// The cache is used.
    On,
    /// Off for this start only (one confirmed cache crash; the folder was deleted).
    OffThisStart,
    /// Off for this app version and system (two confirmed cache crashes).
    OffOnThisDevice,
}

/// What this process decided at its first start-up check, and the report nobody has taken.
struct Process {
    decision: Option<CacheDecision>,
    pending: Option<StartReport>,
}

static PROCESS: Mutex<Process> = Mutex::new(Process {
    decision: None,
    pending: None,
});

fn process() -> std::sync::MutexGuard<'static, Process> {
    PROCESS.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Once per process, before any GPU work, by the entry point: runs the start-up check and
/// returns the shader cache folder to set, or `None` when the cache stays off. The report is
/// kept for the next `Session::new`.
#[must_use]
pub fn process_start(platform: &Arc<dyn Platform>) -> Option<PathBuf> {
    let (report, fresh) = run_check(platform);
    let decision = {
        let mut process = process();
        process.pending = Some(report);
        *process.decision.get_or_insert(fresh)
    };
    (decision == CacheDecision::On).then(|| platform.app_dirs().cache.join(CACHE_FOLDER))
}

/// The report and the cache decision for a new `Session`: the report `process_start` kept, or
/// a fresh check (a relaunch, or an entry point that did not call `process_start`).
pub(super) fn session_start(platform: &Arc<dyn Platform>) -> (StartReport, CacheDecision) {
    let pending = process().pending.take();
    if let Some(report) = pending {
        let decision = process().decision.unwrap_or(CacheDecision::On);
        return (report, decision);
    }
    let (report, fresh) = run_check(platform);
    let decision = *process().decision.get_or_insert(fresh);
    (report, decision)
}

/// The pure decision about the shader cache for one app version and fingerprint.
#[must_use]
pub fn cache_decision(state: &DiagFile, version: &str, fingerprint: &str) -> CacheDecision {
    if !state.matches(version, fingerprint) {
        CacheDecision::On
    } else if state.cache_crashes >= 2 {
        CacheDecision::OffOnThisDevice
    } else if state.cache_skip_next {
        CacheDecision::OffThisStart
    } else {
        CacheDecision::On
    }
}

/// Reads the leftovers and the exit information, writes the log entry, updates
/// `diagnostics.toml` and deletes the cache folder when the cache is off.
fn run_check(platform: &Arc<dyn Platform>) -> (StartReport, CacheDecision) {
    let dirs = platform.app_dirs();
    let facts = platform.device_facts();
    let version = app_version(&facts);
    let logs = dirs.files.join(LOGS_DIR);
    let state_path = dirs.files.join(DIAG_FILE);
    let mut file = DiagFile::load(&state_path);
    let leftovers = breadcrumb::take_foreign(&logs.join(GPU_STEPS_FILE), platform.process_id());
    let exit = platform.last_exit(file.last_exit_ms);
    let outcome = evaluate(
        &mut file,
        &leftovers,
        exit.as_ref(),
        &facts,
        &version,
        now_ms(),
    );
    if let Some((name, text)) = &outcome.log
        && std::fs::create_dir_all(&logs).is_ok()
        && let Err(error) = std::fs::write(logs.join(name), text)
    {
        tracing::warn!(%error, "the start-up check could not write its log entry");
    }
    let decision = cache_decision(&file, &version, &facts.fingerprint);
    if decision != CacheDecision::On {
        let _ = std::fs::remove_dir_all(dirs.cache.join(CACHE_FOLDER));
    }
    if decision == CacheDecision::OffThisStart {
        // The skip is for this start only; the crash count stays.
        file.cache_skip_next = false;
    }
    if let Err(error) = file.save(&state_path) {
        tracing::warn!(%error, "the diagnostics state could not be saved");
    }
    (outcome.report, decision)
}

/// What `evaluate` decided: the report and the log entry to write (name and text).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct Outcome {
    pub report: StartReport,
    pub log: Option<(String, String)>,
}

const fn is_crash(reason: ExitReason) -> bool {
    matches!(
        reason,
        ExitReason::CrashNative | ExitReason::Crash | ExitReason::Anr
    )
}

const fn engine_display(engine: Engine) -> &'static str {
    match engine {
        Engine::PathTraced => "Path-Traced",
        Engine::RayTraced => "Ray-Traced",
    }
}

/// The classification of blueprint section 4.4. Pure: it changes `file` and returns what to
/// write. `leftovers` are the steps of earlier processes in file order, the newest last.
pub(super) fn evaluate(
    file: &mut DiagFile,
    leftovers: &[Leftover],
    exit: Option<&ExitInfo>,
    facts: &DeviceFacts,
    version: &str,
    now: i64,
) -> Outcome {
    file.for_pair(version, &facts.fingerprint);
    if let Some(exit) = exit {
        file.last_exit_ms = file.last_exit_ms.max(exit.timestamp_ms);
    }
    let Some(step) = leftovers.last() else {
        file.previous_step.clear();
        return Outcome {
            report: StartReport {
                last_start: LastStart::Clean,
                notice_due: false,
            },
            log: None,
        };
    };
    let confirmed = confirm(file, step, exit, facts);
    let when_ms = exit
        .map(|exit| exit.timestamp_ms)
        .filter(|&ms| ms > 0)
        .or_else(|| Some(step.wall_ms).filter(|&ms| ms > 0))
        .unwrap_or(now);
    let words = step_words(step);
    let when = utc_display(when_ms);
    let stamp = utc_stamp(when_ms);
    let crash_name = format!("crash-{stamp}.log");
    let (last_start, name, result, notice_due) = if !confirmed {
        let name = format!("interrupted-{stamp}.log");
        let last_start = LastStart::Interrupted {
            step: words.clone(),
            when,
            log: name.clone(),
        };
        (last_start, name, String::new(), false)
    } else if step.kind.is_cache() {
        file.cache_crashes += 1;
        file.cache_skip_next = true;
        let result = if file.cache_crashes >= 2 {
            "The shader cache is off on this device for this app version.".to_owned()
        } else {
            "The shader cache is deleted and built again.".to_owned()
        };
        let last_start = LastStart::CacheCrash {
            step: words.clone(),
            when,
            log: crash_name.clone(),
        };
        (last_start, crash_name, result, true)
    } else if step.kind == StepKind::ProbeStage {
        let last_start = LastStart::Interrupted {
            step: words.clone(),
            when,
            log: crash_name.clone(),
        };
        let result = "No engine is marked: the probe is a test.".to_owned();
        (last_start, crash_name, result, false)
    } else {
        // `None` means the step belongs to the graphics card itself: both engines are marked and
        // the sheet names Ray-Traced, the default engine.
        let engine = step.engine.unwrap_or(Engine::RayTraced);
        match step.engine {
            Some(engine) => file.set_marked(engine, true),
            None => {
                file.set_marked(Engine::PathTraced, true);
                file.set_marked(Engine::RayTraced, true);
            }
        }
        let result = format!(
            "{} uses the CPU on this device for now.",
            engine_display(engine)
        );
        let last_start = LastStart::EngineCrash {
            engine,
            step: words.clone(),
            when,
            log: crash_name.clone(),
        };
        (last_start, crash_name, result, true)
    };
    let text = log_text(&LogEntry {
        confirmed,
        words: &words,
        step,
        exit,
        facts,
        version,
        when_ms,
        result: &result,
    });
    Outcome {
        report: StartReport {
            last_start,
            notice_due,
        },
        log: Some((name, text)),
    }
}

/// Whether the exit information (or, without it, a repeat) confirms a crash in `step`.
fn confirm(
    file: &mut DiagFile,
    step: &Leftover,
    exit: Option<&ExitInfo>,
    facts: &DeviceFacts,
) -> bool {
    if let Some(exit) = exit {
        file.previous_step.clear();
        return is_crash(exit.reason) && (exit.pid == 0 || exit.pid == step.pid);
    }
    if facts.sdk_int < FIRST_EXIT_INFO_SDK {
        // No exit information on this Android release: the same step open at two starts in a row.
        let key = step.key();
        if file.previous_step == key {
            file.previous_step.clear();
            return true;
        }
        file.previous_step = key;
        return false;
    }
    file.previous_step.clear();
    false
}

/// The step in plain words; a probe stage names itself.
fn step_words(step: &Leftover) -> String {
    let words = step.kind.phrase(step.engine);
    if step.kind == StepKind::ProbeStage && !step.label.is_empty() && !words.contains(&step.label) {
        format!("{words} ({})", step.label)
    } else {
        words
    }
}

struct LogEntry<'a> {
    confirmed: bool,
    words: &'a str,
    step: &'a Leftover,
    exit: Option<&'a ExitInfo>,
    facts: &'a DeviceFacts,
    version: &'a str,
    when_ms: i64,
    result: &'a str,
}

const fn reason_text(reason: ExitReason) -> &'static str {
    match reason {
        ExitReason::CrashNative => "native crash",
        ExitReason::Crash => "crash",
        ExitReason::Anr => "ANR (the app did not respond)",
        ExitReason::UserRequested => "closed by the user",
        ExitReason::LowMemory => "closed because the device ran low on memory",
        ExitReason::Other => "other",
    }
}

/// The text of a crash or interrupted log. It holds no design title, file name or path.
fn log_text(entry: &LogEntry<'_>) -> String {
    let LogEntry {
        confirmed,
        words,
        step,
        exit,
        facts,
        version,
        when_ms,
        result,
    } = *entry;
    let mut text = String::new();
    if confirmed {
        let _ = writeln!(text, "Pleochroma crash log");
        let _ = writeln!(text, "The app closed unexpectedly while {words}.");
    } else {
        let _ = writeln!(text, "Pleochroma interrupted log");
        let _ = writeln!(text, "The app was closed while {words}.");
    }
    let _ = writeln!(text, "Time: {}", utc_display(when_ms));
    match exit {
        Some(exit) => {
            let _ = writeln!(text, "Exit reason: {}", reason_text(exit.reason));
            let _ = writeln!(text, "Exit description: {}", exit.description);
        }
        None if confirmed => {
            let _ = writeln!(
                text,
                "Exit reason: unknown (this Android release gives none; the same step was open at two starts in a row)"
            );
        }
        None => {
            let _ = writeln!(text, "Exit reason: unknown");
        }
    }
    let _ = writeln!(text, "Step: {}", breadcrumb_kind(step.kind));
    if !step.label.is_empty() {
        let _ = writeln!(text, "Kernel or stage: {}", step.label);
    }
    if !step.class.is_empty() {
        let _ = writeln!(text, "Pass class: {}", step.class);
    }
    if let Some(engine) = step.engine {
        let _ = writeln!(text, "Engine: {}", engine_display(engine));
    }
    if !result.is_empty() {
        let _ = writeln!(text, "Result: {result}");
    }
    if !step.gpu.is_empty() {
        let _ = writeln!(text, "Graphics card: {} ({})", step.gpu, step.driver);
    }
    let _ = writeln!(text, "App version: {version}");
    let _ = writeln!(text, "Device: {}", device_summary(facts));
    if let Some(trace) = exit.and_then(|exit| exit.trace_file.as_deref()) {
        if trace.ends_with(".pb") {
            let _ = writeln!(text, "Trace file: {trace} (Android tombstone, protobuf)");
        } else {
            let _ = writeln!(text, "Trace file: {trace}");
        }
    }
    text
}

const fn breadcrumb_kind(kind: StepKind) -> &'static str {
    match kind {
        StepKind::AdapterRequest => "adapter request",
        StepKind::DeviceRequest => "device request",
        StepKind::CacheLoad => "shader cache load",
        StepKind::CacheCreate => "shader cache creation",
        StepKind::CacheSave => "shader cache save",
        StepKind::Compile => "shader compile",
        StepKind::FirstDispatch => "first dispatch",
        StepKind::FirstReadback => "first readback",
        StepKind::ProbeStage => "probe stage",
        StepKind::DebugCrash => "forced crash (debug build)",
    }
}

/// The one-line device summary of a log: model, Android release, ABI and memory. No id.
fn device_summary(facts: &DeviceFacts) -> String {
    let mut text = format!(
        "{} {}, Android {} (SDK {}), {}, {} MB RAM",
        facts.manufacturer,
        facts.model,
        facts.android_release,
        facts.sdk_int,
        facts.abi,
        facts.total_ram_mb
    );
    if let Some(soc) = &facts.soc_model {
        let _ = write!(text, ", SoC {soc}");
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        platform::AppDirs,
        platform_mock::{MockPlatform, unique_temp_dir},
    };

    const VERSION: &str = "0.12.0";

    fn facts(sdk_int: u32, fingerprint: &str) -> DeviceFacts {
        DeviceFacts {
            app_version: VERSION.to_owned(),
            manufacturer: "Evermyth".to_owned(),
            model: "KB1001".to_owned(),
            android_release: "16".to_owned(),
            sdk_int,
            fingerprint: fingerprint.to_owned(),
            ..DeviceFacts::default()
        }
    }

    fn open(kind: StepKind, engine: Option<Engine>) -> Leftover {
        Leftover {
            pid: 100,
            run_id: "abcd1234".to_owned(),
            thread: "render-0".to_owned(),
            kind,
            engine,
            label: "whitted_main".to_owned(),
            class: String::new(),
            wall_ms: 1_700_000_000_000,
            gpu: "Mali-G57".to_owned(),
            driver: "Mali v1".to_owned(),
        }
    }

    fn exit(reason: ExitReason) -> ExitInfo {
        ExitInfo {
            reason,
            description: "signal 11 (SIGSEGV)".to_owned(),
            timestamp_ms: 1_700_000_100_000,
            pid: 100,
            trace_file: Some("crash-snapshot-20231114-221500.pb".to_owned()),
        }
    }

    fn run(
        file: &mut DiagFile,
        leftover: Option<Leftover>,
        info: Option<&ExitInfo>,
        sdk: u32,
        fingerprint: &str,
    ) -> Outcome {
        let leftovers: Vec<Leftover> = leftover.into_iter().collect();
        evaluate(
            file,
            &leftovers,
            info,
            &facts(sdk, fingerprint),
            VERSION,
            1_700_000_200_000,
        )
    }

    #[test]
    fn no_leftover_is_a_clean_start_and_the_exit_time_is_remembered() {
        let mut file = DiagFile::default();
        let info = exit(ExitReason::CrashNative);
        let outcome = run(&mut file, None, Some(&info), 36, "fp");
        assert_eq!(outcome.report.last_start, LastStart::Clean);
        assert!(!outcome.report.notice_due);
        assert!(outcome.log.is_none());
        assert_eq!(file.last_exit_ms, info.timestamp_ms);
        assert!(!file.marked(Engine::RayTraced));
    }

    #[test]
    fn a_native_crash_in_a_gpu_step_marks_that_engine_and_writes_a_crash_log() {
        let mut file = DiagFile::default();
        let info = exit(ExitReason::CrashNative);
        let step = open(StepKind::Compile, Some(Engine::RayTraced));
        let outcome = run(&mut file, Some(step), Some(&info), 36, "fp");
        let LastStart::EngineCrash {
            engine, log, step, ..
        } = &outcome.report.last_start
        else {
            panic!(
                "expected an engine crash, got {:?}",
                outcome.report.last_start
            );
        };
        assert_eq!(*engine, Engine::RayTraced);
        assert!(log.starts_with("crash-2023"), "{log}");
        assert!(!step.is_empty());
        assert!(outcome.report.notice_due);
        assert!(file.marked(Engine::RayTraced));
        assert!(!file.marked(Engine::PathTraced));
        let (name, text) = outcome.log.expect("a log entry");
        assert_eq!(&name, log);
        assert!(text.contains("The app closed unexpectedly while"), "{text}");
        assert!(text.contains("Exit reason: native crash"), "{text}");
        assert!(text.contains("crash-snapshot-20231114-221500.pb (Android tombstone, protobuf)"));
        assert!(text.contains("Ray-Traced uses the CPU"), "{text}");
        assert!(text.contains("Evermyth KB1001"), "{text}");
    }

    #[test]
    fn an_anr_in_a_gpu_step_marks_the_engine_too() {
        let mut file = DiagFile::default();
        let mut info = exit(ExitReason::Anr);
        info.trace_file = Some("anr-20231114-221500.txt".to_owned());
        let step = open(StepKind::FirstDispatch, Some(Engine::PathTraced));
        let outcome = run(&mut file, Some(step), Some(&info), 36, "fp");
        assert!(matches!(
            outcome.report.last_start,
            LastStart::EngineCrash {
                engine: Engine::PathTraced,
                ..
            }
        ));
        assert!(file.marked(Engine::PathTraced));
        let (_, text) = outcome.log.expect("a log entry");
        assert!(
            text.contains("Trace file: anr-20231114-221500.txt\n"),
            "{text}"
        );
        assert!(!text.contains("tombstone"), "{text}");
    }

    #[test]
    fn a_crash_in_a_cache_step_sets_no_engine_mark_and_applies_the_cache_rule() {
        let mut file = DiagFile::default();
        let info = exit(ExitReason::CrashNative);
        for kind in [
            StepKind::CacheLoad,
            StepKind::CacheCreate,
            StepKind::CacheSave,
        ] {
            let mut file = DiagFile::default();
            let step = open(kind, Some(Engine::PathTraced));
            let outcome = run(&mut file, Some(step), Some(&info), 36, "fp");
            assert!(matches!(
                outcome.report.last_start,
                LastStart::CacheCrash { .. }
            ));
            assert!(outcome.report.notice_due);
            assert!(!file.marked(Engine::PathTraced), "{kind:?}");
            assert!(!file.marked(Engine::RayTraced), "{kind:?}");
        }
        let step = open(StepKind::CacheLoad, Some(Engine::RayTraced));
        let outcome = run(&mut file, Some(step), Some(&info), 36, "fp");
        let (name, text) = outcome.log.expect("a log entry");
        assert!(name.starts_with("crash-"), "{name}");
        assert!(text.contains("deleted and built again"), "{text}");
        assert_eq!(file.cache_crashes, 1);
        assert!(file.cache_skip_next);
    }

    #[test]
    fn the_cache_guard_skips_one_start_then_switches_off_then_a_new_fingerprint_clears_it() {
        let info = exit(ExitReason::CrashNative);
        let mut file = DiagFile::default();
        let step = open(StepKind::CacheLoad, None);
        let _ = run(&mut file, Some(step.clone()), Some(&info), 36, "fp");
        assert_eq!(
            cache_decision(&file, VERSION, "fp"),
            CacheDecision::OffThisStart
        );
        // The start that skipped the cache clears the skip; the count stays.
        file.cache_skip_next = false;
        assert_eq!(cache_decision(&file, VERSION, "fp"), CacheDecision::On);
        let _ = run(&mut file, Some(step), Some(&info), 36, "fp");
        assert_eq!(
            cache_decision(&file, VERSION, "fp"),
            CacheDecision::OffOnThisDevice
        );
        assert_eq!(cache_decision(&file, VERSION, "fp-new"), CacheDecision::On);
        assert_eq!(cache_decision(&file, "0.13.0", "fp"), CacheDecision::On);
        // A start under a new fingerprint drops the counts from the file.
        let _ = run(&mut file, None, None, 36, "fp-new");
        assert_eq!(file.cache_crashes, 0);
        assert_eq!(cache_decision(&file, VERSION, "fp-new"), CacheDecision::On);
    }

    #[test]
    fn closing_the_app_is_an_interrupted_entry_without_a_mark() {
        for reason in [
            ExitReason::UserRequested,
            ExitReason::LowMemory,
            ExitReason::Other,
        ] {
            let mut file = DiagFile::default();
            let info = exit(reason);
            let step = open(StepKind::Compile, Some(Engine::RayTraced));
            let outcome = run(&mut file, Some(step), Some(&info), 36, "fp");
            let LastStart::Interrupted { log, .. } = &outcome.report.last_start else {
                panic!("expected interrupted for {reason:?}");
            };
            assert!(log.starts_with("interrupted-"), "{log}");
            assert!(!outcome.report.notice_due);
            assert!(!file.marked(Engine::RayTraced));
            let (_, text) = outcome.log.expect("a log entry");
            assert!(text.contains("The app was closed while"), "{text}");
        }
    }

    #[test]
    fn an_exit_of_another_process_does_not_confirm_the_crash() {
        let mut file = DiagFile::default();
        let mut info = exit(ExitReason::CrashNative);
        info.pid = 555;
        let step = open(StepKind::Compile, Some(Engine::RayTraced));
        let outcome = run(&mut file, Some(step), Some(&info), 36, "fp");
        assert!(matches!(
            outcome.report.last_start,
            LastStart::Interrupted { .. }
        ));
        assert!(!file.marked(Engine::RayTraced));
    }

    #[test]
    fn without_exit_information_the_same_step_twice_in_a_row_is_the_crash() {
        let mut file = DiagFile::default();
        let step = open(StepKind::Compile, Some(Engine::RayTraced));
        let first = run(&mut file, Some(step.clone()), None, 28, "fp");
        assert!(matches!(
            first.report.last_start,
            LastStart::Interrupted { .. }
        ));
        assert!(!file.marked(Engine::RayTraced), "one start is not enough");
        let second = run(&mut file, Some(step), None, 28, "fp");
        assert!(matches!(
            second.report.last_start,
            LastStart::EngineCrash {
                engine: Engine::RayTraced,
                ..
            }
        ));
        assert!(file.marked(Engine::RayTraced));
        let (_, text) = second.log.expect("a log entry");
        assert!(text.contains("two starts in a row"), "{text}");
    }

    #[test]
    fn a_clean_start_or_another_step_between_breaks_the_row() {
        let mut file = DiagFile::default();
        let step = open(StepKind::Compile, Some(Engine::RayTraced));
        let _ = run(&mut file, Some(step.clone()), None, 29, "fp");
        let _ = run(&mut file, None, None, 29, "fp");
        let _ = run(&mut file, Some(step.clone()), None, 29, "fp");
        assert!(!file.marked(Engine::RayTraced));
        let other = open(StepKind::FirstDispatch, Some(Engine::RayTraced));
        let _ = run(&mut file, Some(other), None, 29, "fp");
        assert!(!file.marked(Engine::RayTraced));
    }

    #[test]
    fn a_cache_step_twice_in_a_row_without_exit_information_applies_the_cache_rule() {
        let mut file = DiagFile::default();
        let step = open(StepKind::CacheSave, Some(Engine::PathTraced));
        let first = run(&mut file, Some(step.clone()), None, 28, "fp");
        assert!(matches!(
            first.report.last_start,
            LastStart::Interrupted { .. }
        ));
        assert_eq!(file.cache_crashes, 0);
        let second = run(&mut file, Some(step), None, 28, "fp");
        assert!(matches!(
            second.report.last_start,
            LastStart::CacheCrash { .. }
        ));
        assert_eq!(file.cache_crashes, 1);
        assert!(!file.marked(Engine::PathTraced));
    }

    #[test]
    fn android_30_and_later_without_exit_information_never_confirms() {
        let mut file = DiagFile::default();
        let step = open(StepKind::Compile, Some(Engine::RayTraced));
        let _ = run(&mut file, Some(step.clone()), None, 36, "fp");
        let outcome = run(&mut file, Some(step), None, 36, "fp");
        assert!(matches!(
            outcome.report.last_start,
            LastStart::Interrupted { .. }
        ));
        assert!(!file.marked(Engine::RayTraced));
    }

    #[test]
    fn a_probe_stage_crash_names_the_stage_and_marks_nothing() {
        let mut file = DiagFile::default();
        let info = exit(ExitReason::CrashNative);
        let mut step = open(StepKind::ProbeStage, Some(Engine::PathTraced));
        step.label = "uniaxial reduce".to_owned();
        let outcome = run(&mut file, Some(step), Some(&info), 36, "fp");
        let LastStart::Interrupted { step, log, .. } = &outcome.report.last_start else {
            panic!("expected interrupted");
        };
        assert!(step.contains("uniaxial reduce"), "{step}");
        assert!(log.starts_with("crash-"), "{log}");
        assert!(!outcome.report.notice_due);
        assert!(!file.marked(Engine::PathTraced));
        assert_eq!(file.cache_crashes, 0);
    }

    #[test]
    fn a_forced_crash_marks_the_engine_it_names() {
        let mut file = DiagFile::default();
        let info = exit(ExitReason::CrashNative);
        let step = open(StepKind::DebugCrash, Some(Engine::PathTraced));
        let outcome = run(&mut file, Some(step), Some(&info), 36, "fp");
        assert!(matches!(
            outcome.report.last_start,
            LastStart::EngineCrash {
                engine: Engine::PathTraced,
                ..
            }
        ));
        assert!(file.marked(Engine::PathTraced));
    }

    #[test]
    fn a_crash_in_a_step_without_an_engine_marks_both_engines() {
        let mut file = DiagFile::default();
        let info = exit(ExitReason::CrashNative);
        let step = open(StepKind::AdapterRequest, None);
        let outcome = run(&mut file, Some(step), Some(&info), 36, "fp");
        assert!(matches!(
            outcome.report.last_start,
            LastStart::EngineCrash {
                engine: Engine::RayTraced,
                ..
            }
        ));
        assert!(file.marked(Engine::PathTraced) && file.marked(Engine::RayTraced));
    }

    #[test]
    fn the_newest_leftover_decides_when_two_were_open() {
        let mut file = DiagFile::default();
        let info = exit(ExitReason::CrashNative);
        let outer = open(StepKind::Compile, Some(Engine::RayTraced));
        let inner = open(StepKind::CacheCreate, Some(Engine::RayTraced));
        let leftovers = [outer, inner];
        let outcome = evaluate(
            &mut file,
            &leftovers,
            Some(&info),
            &facts(36, "fp"),
            VERSION,
            0,
        );
        assert!(matches!(
            outcome.report.last_start,
            LastStart::CacheCrash { .. }
        ));
        assert!(!file.marked(Engine::RayTraced));
    }

    #[test]
    fn a_new_fingerprint_forgets_the_engine_mark() {
        let mut file = DiagFile::default();
        let info = exit(ExitReason::CrashNative);
        let step = open(StepKind::Compile, Some(Engine::RayTraced));
        let _ = run(&mut file, Some(step), Some(&info), 36, "fp-a");
        assert!(file.marked(Engine::RayTraced));
        let _ = run(&mut file, None, None, 36, "fp-b");
        assert!(!file.marked(Engine::RayTraced));
    }

    #[test]
    fn the_log_text_holds_no_user_data() {
        let mut file = DiagFile::default();
        let info = exit(ExitReason::CrashNative);
        let step = open(StepKind::Compile, Some(Engine::RayTraced));
        let outcome = run(&mut file, Some(step), Some(&info), 36, "secret-fingerprint");
        let (_, text) = outcome.log.expect("a log entry");
        assert!(!text.contains("secret-fingerprint"), "{text}");
    }

    #[test]
    fn the_check_on_disk_writes_the_entry_saves_the_state_and_keeps_the_breadcrumb_rule() {
        let root = unique_temp_dir("exit-check");
        let dirs = AppDirs {
            files: root.join("files"),
            cache: root.join("cache"),
        };
        let logs = dirs.files.join(LOGS_DIR);
        std::fs::create_dir_all(&logs).expect("logs dir");
        std::fs::create_dir_all(dirs.cache.join(CACHE_FOLDER)).expect("cache dir");
        let foreign = std::process::id().wrapping_add(1);
        std::fs::write(
            logs.join(GPU_STEPS_FILE),
            format!(
                "pid={foreign}\trun=ab\tthread=render-0\tstep=CacheLoad\tengine=PathTraced\tlabel=\tclass=\twall_ms=1700000000000\tgpu=\tdriver=\n"
            ),
        )
        .expect("breadcrumb written");
        let mock = MockPlatform::new(dirs.clone());
        let platform: Arc<dyn Platform> = Arc::new(mock);
        // The mock reports no exit information and sdk_int 0: the first start is an
        // interrupted entry and the breadcrumb file is consumed.
        let (report, decision) = run_check(&platform);
        assert!(matches!(report.last_start, LastStart::Interrupted { .. }));
        assert_eq!(decision, CacheDecision::On);
        assert!(!logs.join(GPU_STEPS_FILE).exists());
        let entries: Vec<String> = std::fs::read_dir(&logs)
            .expect("logs")
            .filter_map(Result::ok)
            .map(|entry| entry.file_name().to_string_lossy().into_owned())
            .collect();
        assert!(
            entries.iter().any(|name| name.starts_with("interrupted-")),
            "{entries:?}"
        );
        let saved = DiagFile::load(&dirs.files.join(DIAG_FILE));
        assert!(
            saved.previous_step.starts_with("CacheLoad|PathTraced"),
            "{saved:?}"
        );
        assert!(
            dirs.cache.join(CACHE_FOLDER).exists(),
            "the cache stays while it is on"
        );
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn a_cache_skip_deletes_the_cache_folder_and_is_spent_after_one_start() {
        let root = unique_temp_dir("exit-skip");
        let dirs = AppDirs {
            files: root.join("files"),
            cache: root.join("cache"),
        };
        std::fs::create_dir_all(dirs.cache.join(CACHE_FOLDER)).expect("cache dir");
        std::fs::create_dir_all(&dirs.files).expect("files dir");
        let mock = MockPlatform::new(dirs.clone());
        let version = app_version(&mock.device_facts());
        let fingerprint = mock.device_facts().fingerprint;
        let mut seeded = DiagFile::default();
        seeded.for_pair(&version, &fingerprint);
        seeded.cache_crashes = 1;
        seeded.cache_skip_next = true;
        seeded.save(&dirs.files.join(DIAG_FILE)).expect("seeded");
        let platform: Arc<dyn Platform> = Arc::new(mock);
        let (_, decision) = run_check(&platform);
        assert_eq!(decision, CacheDecision::OffThisStart);
        assert!(!dirs.cache.join(CACHE_FOLDER).exists());
        let (_, again) = run_check(&platform);
        assert_eq!(again, CacheDecision::On, "the skip lasts one start");
        let _ = std::fs::remove_dir_all(root);
    }
}
