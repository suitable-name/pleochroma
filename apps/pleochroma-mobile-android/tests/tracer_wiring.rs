//! The tracer log's wiring test: the file layer and the logcat stand-in see each event once.
//!
//! On a device the registry holds the logcat `fmt` layer and [`TracerFileLayer`], and the `log`
//! facade has a composite logger beside them. This binary rebuilds what the desktop can: the
//! same registry with a `fmt` layer (info) that stands in for logcat. It checks that
//!
//! - the tracer file gets an allowed event exactly once, from one thread or many, and nothing
//!   while the switch is off, nothing below info and nothing from a target outside the list;
//! - the stand-in still receives every info event (the file layer never swallows one);
//! - a panic writes exactly one panic line, though the hook was installed twice.
//!
//! The layer is the global default, so the binary holds one test. Needs the `desktop-preview`
//! feature.

use std::{
    collections::HashSet,
    fs,
    io::{self, Write},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicU32, Ordering},
    },
};

use pleochroma_mobile_android::tracer_layer::{TracerFileLayer, install_panic_line};
use pleochroma_mobile_core::{
    diagnostics::{Session, session::HeaderFacts},
    platform::Platform,
    platform_mock::MockPlatform,
};
use tracing_subscriber::{Layer as _, filter::LevelFilter, layer::SubscriberExt as _};

/// Threads that write at the same time.
const THREADS: usize = 4;
/// Events each of them writes.
const EVENTS_PER_THREAD: usize = 25;
/// The message of the panic; the layer must not write it (the location is not a GPU one).
const PANIC_MESSAGE: &str = "wiring-panic-0008";
/// The source line of the `panic!`, stored just before it runs.
static PANIC_AT: AtomicU32 = AtomicU32::new(0);

/// A shared byte buffer that stands in for logcat.
#[derive(Clone, Default)]
struct Capture(Arc<Mutex<Vec<u8>>>);

impl Capture {
    /// Everything written so far, as text.
    fn text(&self) -> String {
        let bytes = self.0.lock().unwrap_or_else(PoisonError::into_inner);
        String::from_utf8_lossy(&bytes).into_owned()
    }
}

impl Write for Capture {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

/// How many times `needle` occurs in `text`.
fn count(text: &str, needle: &str) -> usize {
    text.matches(needle).count()
}

/// A line without its "+12.345 s " time, so two writes of one event compare equal.
fn without_time(line: &str) -> &str {
    if line.starts_with('+') {
        line.split_once(" s ").map_or(line, |(_, rest)| rest)
    } else {
        line
    }
}

/// The events of the off phase, the on phase and the second off phase.
fn emit_events() {
    // Allowed and wanted.
    tracing::info!(target: "pleochroma::tracer", "wiring-a-0001");
    tracing::warn!(target: "pleochroma_mobile_core::render::pool", "wiring-b-0002");
    tracing::warn!(
        target: "pleochroma::renderer::gpu::context",
        path = "/never/written",
        "wiring-e-0005"
    );
    // Below info: neither the file nor the stand-in.
    tracing::debug!(target: "pleochroma::tracer", "wiring-c-0003");
    // Info outside the list: logcat only.
    tracing::info!(target: "wgpu_core", "wiring-d-0004");
    tracing::info!("wiring-f-0006");
}

/// Four threads write unique events at once.
fn emit_from_threads() {
    let handles: Vec<_> = (0..THREADS)
        .map(|thread| {
            std::thread::spawn(move || {
                for event in 0..EVENTS_PER_THREAD {
                    tracing::info!(target: "pleochroma::tracer", "wiring-t{thread}-{event:03}");
                }
            })
        })
        .collect();
    for handle in handles {
        handle.join().expect("the writer thread finished");
    }
}

/// Panics on a worker thread and waits for it.
fn panic_on_a_worker() {
    let handle = std::thread::Builder::new()
        .name("render-wiring-worker".to_owned())
        .spawn(|| {
            PANIC_AT.store(line!() + 1, Ordering::SeqCst);
            panic!("{PANIC_MESSAGE}");
        })
        .expect("a thread");
    assert!(handle.join().is_err(), "the worker panicked");
}

#[test]
fn the_file_layer_and_the_logcat_stand_in_see_each_event_once() {
    let logcat = Capture::default();
    let writer = logcat.clone();
    let subscriber = tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_writer(move || writer.clone())
                .with_filter(LevelFilter::INFO),
        )
        .with(TracerFileLayer);
    tracing::subscriber::set_global_default(subscriber)
        .expect("no other subscriber is installed in this test binary");
    // Twice on purpose: the second call must change nothing.
    install_panic_line();
    install_panic_line();

    // The layer reads the global current session, so it comes from `Session::new`.
    let platform: Arc<dyn Platform> = Arc::new(MockPlatform::default());
    let logs_dir = platform.app_dirs().files.join("logs");
    let session = Session::new(&platform);
    let header = HeaderFacts::default();

    // Off: nothing is written, the stand-in still hears it.
    tracing::info!(target: "pleochroma::tracer", "wiring-off-0007");
    assert!(!session.tracer_on());

    session.set_tracer(true, &header);
    assert!(session.tracer_on());
    emit_events();
    emit_from_threads();
    panic_on_a_worker();
    session.set_tracer(false, &header);

    // Off again.
    tracing::info!(target: "pleochroma::tracer", "wiring-off-0009");

    let mut tracer = String::new();
    for entry in fs::read_dir(&logs_dir)
        .expect("the logs folder exists")
        .flatten()
    {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("tracer-") {
            tracer.push_str(&fs::read_to_string(entry.path()).expect("a text file"));
        }
    }
    let logcat_text = logcat.text();

    // The file: wanted events once, the rest never.
    for token in ["wiring-a-0001", "wiring-b-0002", "wiring-e-0005"] {
        assert_eq!(count(&tracer, token), 1, "`{token}` in the file:\n{tracer}");
        assert_eq!(
            count(&logcat_text, token),
            1,
            "`{token}` in logcat:\n{logcat_text}"
        );
    }
    for token in [
        "wiring-c-0003",
        "wiring-d-0004",
        "wiring-f-0006",
        "wiring-off-0007",
        "wiring-off-0009",
        "/never/written",
        PANIC_MESSAGE,
    ] {
        assert_eq!(
            count(&tracer, token),
            0,
            "`{token}` must not be in the file:\n{tracer}"
        );
    }

    // Logcat still gets info (also the events the file refuses), and never debug.
    for token in [
        "wiring-d-0004",
        "wiring-f-0006",
        "wiring-off-0007",
        "wiring-off-0009",
    ] {
        assert_eq!(
            count(&logcat_text, token),
            1,
            "`{token}` in logcat:\n{logcat_text}"
        );
    }
    assert_eq!(count(&logcat_text, "wiring-c-0003"), 0);

    // Many threads: every event is in the file once and in the stand-in once.
    for thread in 0..THREADS {
        for event in 0..EVENTS_PER_THREAD {
            let token = format!("wiring-t{thread}-{event:03}");
            assert_eq!(count(&tracer, &token), 1, "`{token}` in the file");
            assert_eq!(count(&logcat_text, &token), 1, "`{token}` in logcat");
        }
    }

    // No line of the file was written twice.
    let mut seen = HashSet::new();
    for line in tracer.lines() {
        assert!(
            seen.insert(without_time(line)),
            "line written twice: {line}"
        );
    }

    // One panic, one line, with the location of this file and no message.
    assert_eq!(
        count(&tracer, "ERROR panic at "),
        1,
        "panic lines in:\n{tracer}"
    );
    let location = format!("tracer_wiring.rs:{}", PANIC_AT.load(Ordering::SeqCst));
    assert!(tracer.contains(&location), "no `{location}` in:\n{tracer}");

    drop(session);
    drop(platform);
}
