//! The tracer log's privacy test: no sentinel string reaches a text file under `logs/`.
//!
//! The binary installs the real [`TracerFileLayer`] as the global default (beside a `fmt`
//! layer that stands in for logcat), so it holds one test only: a second install in the same
//! process would be refused. The test runs a render and an export of a design whose title,
//! designer, file name and HDR file name are sentinel strings, a remote attempt to a sentinel
//! address, and a panic on a worker thread whose message is a sentinel. No sentinel may appear
//! in any file under `logs/`; the panic's location must.
//!
//! Needs the `desktop-preview` feature (the layer exists only there or on Android).

use std::{
    collections::BTreeSet,
    fs,
    io::{self, Write},
    path::Path,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU32, Ordering},
    },
    time::{Duration, Instant},
};

use pleochroma_cut_core::templates::TEMPLATES;
use pleochroma_editor::EditorSession;
use pleochroma_mobile_android::tracer_layer::{TracerFileLayer, install_panic_line};
use pleochroma_mobile_core::{
    app_state::AppState,
    design_state::{Installed, begin_solve, install, run_solve},
    diagnostics::session::HeaderFacts,
    document::DocumentOrigin,
    hdr,
    platform::{PickedFile, Platform},
    platform_mock::MockPlatform,
    render::{ExportColorSpace, ExportRequest, RenderStatus, RenderView, export_job, start},
    settings::RenderBackend,
};
use tracing_subscriber::{Layer as _, filter::LevelFilter, layer::SubscriberExt as _};

/// The part every sentinel shares, so one scan catches them all.
const STEM: &str = "zq7f3a";
/// The design's title line.
const TITLE: &str = "zq7f3a-design-title";
/// The designer line of the design.
const DESIGNER: &str = "zq7f3a-designer";
/// A path of the design file, as a picker could report it.
const PATH_TEXT: &str = "/storage/emulated/0/Download/zq7f3a-folder/zq7f3a-design.asc";
/// The file name of the HDR map.
const HDR_NAME: &str = "zq7f3a-studio-sky.hdr";
/// The name of the remote worker.
const REMOTE_NAME: &str = "zq7f3a-remote-name";
/// The address of the remote worker.
const REMOTE_ADDRESS: &str = "zq7f3a-remote-host.invalid:4711";
/// The message of the worker thread's panic.
const PANIC_MESSAGE: &str = "zq7f3a-panic-message";

/// The traced raster of the run.
const SMALL: RenderView = RenderView {
    width: 32,
    height: 24,
};

/// The longest the test waits for the render.
const WAIT: Duration = Duration::from_secs(120);

/// The thread name the panic runs under (no sentinel: the name is written to the log).
const PANIC_THREAD: &str = "render-test-worker";

/// The source line of the worker's `panic!`, stored just before it runs.
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

/// Installs the registry of the app: the logcat stand-in (info) and the tracer file layer.
fn install_logging(logcat: &Capture) {
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
    install_panic_line();
}

/// Every file below `dir` with its content as text (invalid UTF-8 is replaced, not skipped).
fn read_tree(dir: &Path, found: &mut Vec<(String, String)>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            read_tree(&path, found);
        } else if let Ok(bytes) = fs::read(&path) {
            found.push((
                path.display().to_string(),
                String::from_utf8_lossy(&bytes).into_owned(),
            ));
        }
    }
}

/// A flat (uncompressed) Radiance file: the pixels avoid the run-length markers.
fn radiance_bytes(width: usize, height: usize) -> Vec<u8> {
    let mut bytes =
        format!("#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y {height} +X {width}\n").into_bytes();
    for y in 0..height {
        for x in 0..width {
            bytes.push(100 + u8::try_from(x * 3 % 100).expect("below 100"));
            bytes.push(100 + u8::try_from(y * 2 % 100).expect("below 100"));
            bytes.push(120);
            bytes.push(128 + u8::try_from(x % 3).expect("below 3"));
        }
    }
    bytes
}

/// A state over the round brilliant template, solved, on the CPU backend, with the sentinels
/// in the title, the designer, the file name and the remote settings.
fn state_with_sentinels(platform: Arc<dyn Platform>) -> AppState {
    let mut state = AppState::new(platform);
    let template = &TEMPLATES[0];
    state.session = EditorSession::from_template(
        template.fresh_spec(template.default_material_selection()),
        1,
    );
    state.session.design.meta.headers = vec![format!("by {DESIGNER}"), TITLE.to_owned()];
    state.document.origin = DocumentOrigin::Document {
        display_name: PATH_TEXT.to_owned(),
    };
    state.document.display_name = Some(PATH_TEXT.to_owned());
    state.document.asc_filename = Some(PATH_TEXT.to_owned());
    state.settings.render.engine = 0;
    state.settings.render.live_spp_compact = 8;
    state.settings.render.denoise = false;
    state.settings.render.backend = RenderBackend::DeviceCpu;
    state.settings.remote.name = REMOTE_NAME.to_owned();
    state.settings.remote.address = REMOTE_ADDRESS.to_owned();

    let job = begin_solve(&mut state).expect("a design with no answer gets a job");
    let response = run_solve(&job, &AtomicBool::new(false));
    let installed = install(
        &mut state,
        job.generation,
        response,
        Duration::from_millis(5),
    );
    assert_eq!(installed, Installed::Applied);
    state
}

/// Polls the status until `ready` holds; a failed render or the time limit fails the test.
fn wait_status(state: &AppState, what: &str, ready: impl Fn(&RenderStatus) -> bool) {
    let deadline = Instant::now() + WAIT;
    loop {
        let status = state.render.status();
        if ready(&status) {
            return;
        }
        assert!(
            !matches!(status, RenderStatus::Failed(_)),
            "the render failed while waiting for {what}: {status:?}"
        );
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {what}; status {status:?}"
        );
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Events that carry the sentinels in the fields and the text the layer must not write.
fn emit_probe_events() {
    // An allowed target with denied fields: the line stays, the fields go.
    tracing::info!(
        target: "pleochroma::tracer",
        title = TITLE,
        designer = DESIGNER,
        path = PATH_TEXT,
        file_name = HDR_NAME,
        address = REMOTE_ADDRESS,
        url = REMOTE_ADDRESS,
        facets = 8_u32,
        "tracer probe event"
    );
    // An allowed tree at warn level: it keeps its target; the path field goes.
    tracing::warn!(
        target: "pleochroma::renderer::gpu::context",
        path = PATH_TEXT,
        bytes = 7_u32,
        "gpu probe warning"
    );
    // A target outside the five trees: its text can carry anything and is never written.
    tracing::warn!(
        target: "pleochroma_mobile_core::document::autosave",
        "autosave failed for {PATH_TEXT} of {TITLE}"
    );
    tracing::info!(
        target: "pleochroma_mobile_core::remote::rendering",
        "dialing {REMOTE_ADDRESS} for {REMOTE_NAME}"
    );
}

/// Runs a live render and then a picture export of the design.
fn render_and_export(state: &mut AppState) {
    start(state, SMALL).expect("the render starts");
    wait_status(state, "the render to finish", |status| {
        matches!(status, RenderStatus::Done { .. })
    });
    let request = ExportRequest {
        long_edge_px: 32,
        spp: 16,
        color_space: ExportColorSpace::Srgb,
    };
    let job = export_job(state, &request).expect("an export job");
    job.run(&|_| {}, &AtomicBool::new(false))
        .expect("the export succeeds");
}

/// Asks for a remote render at the sentinel address. No credentials are stored, so nothing
/// is dialed; the attempt still goes through the render path that names the backend.
fn remote_attempt(state: &mut AppState) {
    state.settings.render.backend = RenderBackend::Remote;
    // The answer is a sentence for the user (enroll the phone first); only the log matters.
    let _ = start(state, SMALL);
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && !matches!(state.render.status(), RenderStatus::Failed(_)) {
        std::thread::sleep(Duration::from_millis(10));
    }
    state.render.stop();
    state.settings.render.backend = RenderBackend::DeviceCpu;
}

/// Panics on a worker thread with the sentinel message and waits for the thread.
fn panic_on_a_worker() {
    let handle = std::thread::Builder::new()
        .name(PANIC_THREAD.to_owned())
        .spawn(|| {
            PANIC_AT.store(line!() + 1, Ordering::SeqCst);
            panic!("{PANIC_MESSAGE}");
        })
        .expect("a thread");
    assert!(handle.join().is_err(), "the worker panicked");
}

#[test]
fn no_sentinel_reaches_a_text_log_and_the_panic_location_does() {
    let logcat = Capture::default();
    install_logging(&logcat);

    // The mock removes its folders when the last handle drops: `platform` stays until the end.
    let platform: Arc<dyn Platform> = Arc::new(MockPlatform::default());
    let logs_dir = platform.app_dirs().files.join("logs");
    let mut state = state_with_sentinels(Arc::clone(&platform));
    let session = Arc::clone(&state.diagnostics);
    let header = HeaderFacts {
        engine: "Path Traced".to_owned(),
        backend: "Device CPU".to_owned(),
        ..HeaderFacts::default()
    };
    session.set_tracer(true, &header);
    assert!(session.tracer_on(), "the tracer log is switched on");

    tracing::info!("starting the privacy run");
    emit_probe_events();
    let sky = PickedFile {
        display_name: HDR_NAME.to_owned(),
        bytes: radiance_bytes(16, 8),
    };
    hdr::import(&mut state, &sky).expect("the HDR map is imported");
    render_and_export(&mut state);
    remote_attempt(&mut state);
    panic_on_a_worker();

    // Dropping the state joins the render threads: every line is written after this.
    drop(state);
    session.set_tracer(false, &header);

    let mut files = Vec::new();
    read_tree(&logs_dir, &mut files);
    assert!(!files.is_empty(), "the logs folder holds the tracer file");
    let tracer: String = files
        .iter()
        .filter(|(name, _)| name.contains("tracer-"))
        .map(|(_, text)| text.as_str())
        .collect();

    // The log is not empty by accident: the render path wrote its lines.
    for wanted in ["scene built", "backend chosen", "tracer probe event"] {
        assert!(tracer.contains(wanted), "no `{wanted}` line in:\n{tracer}");
    }
    // Denied fields are dropped, other targets' text never arrives, an allowed warning stays.
    assert!(tracer.contains("pleochroma::renderer::gpu::context gpu probe warning bytes=7"));
    assert!(!tracer.contains("autosave failed"));
    assert!(!tracer.contains("dialing"));

    // The panic's location is there, without the message and with the right line.
    let line_no = PANIC_AT.load(Ordering::SeqCst);
    let location = format!("tracer_privacy.rs:{line_no} on {PANIC_THREAD}");
    assert!(
        tracer
            .lines()
            .any(|line| line.contains("ERROR panic at ") && line.contains(&location)),
        "no panic line with `{location}` in:\n{tracer}"
    );

    // No sentinel in any file under logs/ (every file, not only the tracer log).
    let sentinels = [
        ("title", TITLE),
        ("designer", DESIGNER),
        ("path", PATH_TEXT),
        ("HDR file name", HDR_NAME),
        ("remote name", REMOTE_NAME),
        ("remote address", REMOTE_ADDRESS),
        ("panic message", PANIC_MESSAGE),
        ("shared stem", STEM),
    ];
    let mut leaks = BTreeSet::new();
    for (name, text) in &files {
        let lowered = text.to_lowercase();
        for (what, sentinel) in sentinels {
            if lowered.contains(&sentinel.to_lowercase()) {
                leaks.insert(format!("{what} in {name}"));
            }
        }
    }
    assert!(leaks.is_empty(), "sentinels reached the logs: {leaks:?}");

    // Logcat's stand-in still received the info line.
    assert!(logcat.text().contains("starting the privacy run"));
    drop(platform);
}
