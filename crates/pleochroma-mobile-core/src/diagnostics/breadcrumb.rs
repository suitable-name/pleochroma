//! The breadcrumb file `logs/gpu-steps`: one line per GPU step in flight, always on.
//!
//! A native crash or an ANR cannot write a message, so the step is on disk before the driver is
//! called. The next start reads what an earlier process left behind (see `exit`).

use std::{
    collections::hash_map::RandomState,
    fmt::Write as _,
    fs::File,
    hash::BuildHasher,
    io::Write as _,
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicU64, Ordering},
    },
};

use pleochroma::diag::{AdapterFacts, BreadcrumbSink, Engine, Step, StepId, StepKind};

use super::session::now_ms;

/// The breadcrumb file's name inside the logs folder.
pub const GPU_STEPS_FILE: &str = "gpu-steps";

/// The breadcrumb object of this process, shared by every session of it.
static SHARED: Mutex<Option<Arc<Breadcrumbs>>> = Mutex::new(None);

/// One step a process had open when it died.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leftover {
    /// The process that wrote the line.
    pub pid: u32,
    /// That process's random run id.
    pub run_id: String,
    /// The thread that entered the step.
    pub thread: String,
    /// What kind of step it was.
    pub kind: StepKind,
    /// The engine it belonged to, if any.
    pub engine: Option<Engine>,
    /// The kernel, entry point or probe stage, or "".
    pub label: String,
    /// The pass class, or "".
    pub class: String,
    /// When the step began, in milliseconds since the Unix epoch.
    pub wall_ms: i64,
    /// The graphics card's name when it was already recorded, or "".
    pub gpu: String,
    /// The driver strings when they were already recorded, or "".
    pub driver: String,
}

impl Leftover {
    /// The step as a key that compares two starts ("the same step twice in a row").
    #[must_use]
    pub fn key(&self) -> String {
        format!(
            "{}|{}|{}|{}",
            kind_name(self.kind),
            self.engine.map_or("", engine_name),
            self.label,
            self.class
        )
    }
}

struct OpenStep {
    id: u64,
    thread: String,
    kind: StepKind,
    engine: Option<Engine>,
    label: String,
    class: String,
    wall_ms: i64,
}

/// The open steps of this process and the file that mirrors them.
pub struct Breadcrumbs {
    path: PathBuf,
    pid: u32,
    run_id: String,
    next_id: AtomicU64,
    open: Mutex<Vec<OpenStep>>,
    adapter: Mutex<Option<AdapterFacts>>,
}

impl Breadcrumbs {
    /// A breadcrumb list writing `path`, for the process `pid`.
    #[must_use]
    pub fn new(path: PathBuf, pid: u32) -> Self {
        let run_id = format!("{:08x}", RandomState::new().hash_one(pid) >> 32);
        Self {
            path,
            pid,
            run_id,
            next_id: AtomicU64::new(1),
            open: Mutex::new(Vec::new()),
            adapter: Mutex::new(None),
        }
    }

    /// The object every session of this process shares, so a step still open after a relaunch
    /// stays on disk. A different path or process id starts a new one.
    #[must_use]
    pub fn shared(path: PathBuf, pid: u32) -> Arc<Self> {
        let mut shared = SHARED.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(existing) = shared.as_ref()
            && existing.path == path
            && existing.pid == pid
        {
            return Arc::clone(existing);
        }
        let created = Arc::new(Self::new(path, pid));
        *shared = Some(Arc::clone(&created));
        created
    }

    /// The adapter recorded so far in this process.
    #[must_use]
    pub fn recorded_adapter(&self) -> Option<AdapterFacts> {
        self.adapter
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// How many steps are open now.
    #[must_use]
    pub fn open_count(&self) -> usize {
        self.open
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }

    fn adapter_strings(&self) -> (String, String) {
        self.recorded_adapter()
            .map_or_else(Default::default, |facts| {
                let driver = format!("{} {}", facts.driver, facts.driver_info);
                (clean(&facts.name), clean(driver.trim()))
            })
    }

    fn render(&self, open: &[OpenStep]) -> String {
        let (gpu, driver) = self.adapter_strings();
        let mut text = String::new();
        for step in open {
            let engine = step.engine.map_or("", engine_name);
            let _ = writeln!(
                text,
                "pid={}\trun={}\tthread={}\tstep={}\tengine={}\tlabel={}\tclass={}\twall_ms={}\tgpu={}\tdriver={}",
                self.pid,
                self.run_id,
                step.thread,
                kind_name(step.kind),
                engine,
                step.label,
                step.class,
                step.wall_ms,
                gpu,
                driver,
            );
        }
        text
    }

    /// Rewrites the file from `open`; an empty list removes it. Only `sync` pays for
    /// `sync_data`. A write that fails is ignored: the breadcrumb is a help, never a risk.
    fn rewrite(&self, open: &[OpenStep], sync: bool) {
        if open.is_empty() {
            let _ = std::fs::remove_file(&self.path);
            return;
        }
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let mut temporary = self.path.as_os_str().to_owned();
        temporary.push(".tmp");
        let temporary = PathBuf::from(temporary);
        let text = self.render(open);
        let written = File::create(&temporary).and_then(|mut file| {
            file.write_all(text.as_bytes())?;
            if sync {
                file.sync_data()?;
            }
            Ok(())
        });
        if written.is_ok() {
            let _ = std::fs::rename(&temporary, &self.path);
        }
    }
}

impl BreadcrumbSink for Breadcrumbs {
    fn enter(&self, step: &Step<'_>) -> StepId {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let thread = std::thread::current();
        let entry = OpenStep {
            id,
            thread: clean(thread.name().unwrap_or("unnamed")),
            kind: step.kind,
            engine: step.engine,
            label: clean(step.label),
            class: clean(step.class),
            wall_ms: now_ms(),
        };
        let mut open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        open.push(entry);
        self.rewrite(&open, true);
        StepId(id)
    }

    fn leave(&self, id: StepId) {
        let mut open = self.open.lock().unwrap_or_else(PoisonError::into_inner);
        let before = open.len();
        open.retain(|step| step.id != id.0);
        if open.len() != before {
            self.rewrite(&open, false);
        }
    }

    fn adapter(&self, facts: &AdapterFacts) {
        *self.adapter.lock().unwrap_or_else(PoisonError::into_inner) = Some(facts.clone());
    }
}

/// Reads every step line of `text`; a line that does not parse is skipped.
#[must_use]
pub fn parse(text: &str) -> Vec<Leftover> {
    text.lines().filter_map(parse_line).collect()
}

/// Reads the steps an earlier process left in `path`, and removes their lines. Lines of this
/// process (`own_pid`) stay: that step still runs after a relaunch.
pub fn take_foreign(path: &Path, own_pid: u32) -> Vec<Leftover> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut foreign = Vec::new();
    let mut own = String::new();
    for line in text.lines() {
        match parse_line(line) {
            Some(step) if step.pid == own_pid => {
                own.push_str(line);
                own.push('\n');
            }
            Some(step) => foreign.push(step),
            None => {}
        }
    }
    if text.trim().is_empty() || (foreign.is_empty() && !own.is_empty()) {
        return foreign;
    }
    if own.is_empty() {
        let _ = std::fs::remove_file(path);
    } else {
        let _ = std::fs::write(path, own);
    }
    foreign
}

fn parse_line(line: &str) -> Option<Leftover> {
    let mut step = Leftover {
        pid: 0,
        run_id: String::new(),
        thread: String::new(),
        kind: StepKind::DebugCrash,
        engine: None,
        label: String::new(),
        class: String::new(),
        wall_ms: 0,
        gpu: String::new(),
        driver: String::new(),
    };
    let mut have_pid = false;
    let mut have_kind = false;
    for field in line.split('\t') {
        let (key, value) = field.split_once('=')?;
        match key {
            "pid" => {
                step.pid = value.parse().ok()?;
                have_pid = true;
            }
            "run" => value.clone_into(&mut step.run_id),
            "thread" => value.clone_into(&mut step.thread),
            "step" => {
                step.kind = kind_from_name(value)?;
                have_kind = true;
            }
            "engine" => step.engine = engine_from_name(value),
            "label" => value.clone_into(&mut step.label),
            "class" => value.clone_into(&mut step.class),
            "wall_ms" => step.wall_ms = value.parse().unwrap_or(0),
            "gpu" => value.clone_into(&mut step.gpu),
            "driver" => value.clone_into(&mut step.driver),
            _ => {}
        }
    }
    (have_pid && have_kind).then_some(step)
}

/// A value without the characters that separate fields and lines.
fn clean(value: &str) -> String {
    value
        .chars()
        .map(|c| {
            if matches!(c, '\t' | '\n' | '\r') {
                ' '
            } else {
                c
            }
        })
        .collect()
}

const fn kind_name(kind: StepKind) -> &'static str {
    match kind {
        StepKind::AdapterRequest => "AdapterRequest",
        StepKind::DeviceRequest => "DeviceRequest",
        StepKind::CacheLoad => "CacheLoad",
        StepKind::CacheCreate => "CacheCreate",
        StepKind::CacheSave => "CacheSave",
        StepKind::Compile => "Compile",
        StepKind::FirstDispatch => "FirstDispatch",
        StepKind::FirstReadback => "FirstReadback",
        StepKind::ProbeStage => "ProbeStage",
        StepKind::DebugCrash => "DebugCrash",
    }
}

fn kind_from_name(name: &str) -> Option<StepKind> {
    Some(match name {
        "AdapterRequest" => StepKind::AdapterRequest,
        "DeviceRequest" => StepKind::DeviceRequest,
        "CacheLoad" => StepKind::CacheLoad,
        "CacheCreate" => StepKind::CacheCreate,
        "CacheSave" => StepKind::CacheSave,
        "Compile" => StepKind::Compile,
        "FirstDispatch" => StepKind::FirstDispatch,
        "FirstReadback" => StepKind::FirstReadback,
        "ProbeStage" => StepKind::ProbeStage,
        "DebugCrash" => StepKind::DebugCrash,
        _ => return None,
    })
}

const fn engine_name(engine: Engine) -> &'static str {
    match engine {
        Engine::PathTraced => "PathTraced",
        Engine::RayTraced => "RayTraced",
    }
}

fn engine_from_name(name: &str) -> Option<Engine> {
    match name {
        "PathTraced" => Some(Engine::PathTraced),
        "RayTraced" => Some(Engine::RayTraced),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::platform_mock::unique_temp_dir;

    fn step<'a>(kind: StepKind, engine: Option<Engine>, label: &'a str) -> Step<'a> {
        Step {
            kind,
            engine,
            label,
            class: "isotropic",
        }
    }

    fn crumbs(tag: &str, pid: u32) -> (Breadcrumbs, PathBuf) {
        let dir = unique_temp_dir(tag);
        let path = dir.join("logs").join(GPU_STEPS_FILE);
        (Breadcrumbs::new(path, pid), dir)
    }

    #[test]
    fn a_step_is_on_disk_between_enter_and_leave() {
        let (crumbs, dir) = crumbs("crumb-basic", 4242);
        let id = crumbs.enter(&step(StepKind::Compile, Some(Engine::PathTraced), "reduce"));
        let text = std::fs::read_to_string(&crumbs.path).expect("file exists while open");
        let steps = parse(&text);
        assert_eq!(steps.len(), 1);
        assert_eq!(steps[0].pid, 4242);
        assert_eq!(steps[0].kind, StepKind::Compile);
        assert_eq!(steps[0].engine, Some(Engine::PathTraced));
        assert_eq!(steps[0].label, "reduce");
        assert_eq!(steps[0].class, "isotropic");
        assert_eq!(steps[0].run_id.len(), 8);
        crumbs.leave(id);
        assert!(!crumbs.path.exists(), "an empty list removes the file");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn two_threads_with_overlapping_steps_both_survive() {
        let (crumbs, dir) = crumbs("crumb-threads", 7);
        let crumbs = Arc::new(crumbs);
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
        let other = {
            let crumbs = Arc::clone(&crumbs);
            std::thread::Builder::new()
                .name("render-1".to_owned())
                .spawn(move || {
                    let id = crumbs.enter(&step(StepKind::Compile, Some(Engine::PathTraced), "a"));
                    entered_tx.send(()).expect("main is waiting");
                    go_rx.recv().expect("main sends");
                    crumbs.leave(id);
                })
                .expect("thread starts")
        };
        entered_rx.recv().expect("the thread entered");
        let mine = crumbs.enter(&step(StepKind::CacheSave, Some(Engine::RayTraced), "b"));
        let text = std::fs::read_to_string(&crumbs.path).expect("file");
        let steps = parse(&text);
        assert_eq!(steps.len(), 2);
        assert!(
            steps
                .iter()
                .any(|s| s.thread == "render-1" && s.label == "a")
        );
        assert!(
            steps
                .iter()
                .any(|s| s.kind == StepKind::CacheSave && s.label == "b")
        );
        go_tx.send(()).expect("thread waits");
        other.join().expect("thread ends");
        let text = std::fs::read_to_string(&crumbs.path).expect("one step left");
        assert_eq!(parse(&text).len(), 1);
        crumbs.leave(mine);
        assert_eq!(crumbs.open_count(), 0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn lines_of_this_process_are_ignored_and_kept() {
        let (crumbs, dir) = crumbs("crumb-own", 99);
        let _id = crumbs.enter(&step(StepKind::Compile, Some(Engine::RayTraced), "x"));
        assert!(take_foreign(&crumbs.path, 99).is_empty());
        assert!(crumbs.path.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn lines_of_an_earlier_process_are_returned_and_removed() {
        let (crumbs, dir) = crumbs("crumb-foreign", 99);
        let _id = crumbs.enter(&step(StepKind::Compile, Some(Engine::RayTraced), "x"));
        let left = take_foreign(&crumbs.path, 100);
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].pid, 99);
        assert_eq!(left[0].key(), "Compile|RayTraced|x|isotropic");
        assert!(!crumbs.path.exists());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_mixed_file_keeps_only_the_own_lines() {
        let (crumbs, dir) = crumbs("crumb-mixed", 5);
        let _id = crumbs.enter(&step(
            StepKind::FirstDispatch,
            Some(Engine::PathTraced),
            "k",
        ));
        let mut text = std::fs::read_to_string(&crumbs.path).expect("file");
        text.push_str("pid=3\trun=ab\tthread=t\tstep=Compile\tengine=\tlabel=z\tclass=\twall_ms=1\tgpu=\tdriver=\n");
        std::fs::write(&crumbs.path, text).expect("written");
        let left = take_foreign(&crumbs.path, 5);
        assert_eq!(left.len(), 1);
        assert_eq!(left[0].pid, 3);
        assert_eq!(left[0].engine, None);
        let kept = std::fs::read_to_string(&crumbs.path).expect("own line stays");
        assert_eq!(parse(&kept).len(), 1);
        assert_eq!(parse(&kept)[0].pid, 5);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn the_adapter_strings_reach_the_lines() {
        let (crumbs, dir) = crumbs("crumb-adapter", 1);
        crumbs.adapter(&AdapterFacts {
            name: "Mali-G57".to_owned(),
            driver: "Mali".to_owned(),
            driver_info: "v1.r44".to_owned(),
            ..AdapterFacts::default()
        });
        let _id = crumbs.enter(&step(StepKind::Compile, None, "k"));
        let left = take_foreign(&crumbs.path, 2);
        assert_eq!(left[0].gpu, "Mali-G57");
        assert_eq!(left[0].driver, "Mali v1.r44");
        assert_eq!(
            crumbs.recorded_adapter().map(|a| a.name),
            Some("Mali-G57".to_owned())
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn garbage_lines_are_skipped() {
        assert!(parse("nonsense\n\npid=x\tstep=Compile\n").is_empty());
    }
}
