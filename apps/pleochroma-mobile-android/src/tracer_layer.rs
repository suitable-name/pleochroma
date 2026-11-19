//! The tracer log's `tracing` layer and the panic line, shared by the Android app and the desktop preview.
//!
//! [`TracerFileLayer`] sits beside the logcat layer in one `tracing_subscriber::registry()`.
//! It writes the events of the render and GPU path to the current diagnostics session while
//! the developer page has the tracer log switched on, and does nothing otherwise.
//!
//! The layer decides in `on_event`, never in `enabled`: the switch changes at any moment, and
//! an answer from `enabled` is cached per call site. It returns `Interest::sometimes()` from
//! `register_callsite` for the same reason. The target allow-list names five module trees,
//! not a prefix, so warnings of other crates (which can carry paths) never reach the file.

use std::{
    cell::Cell,
    fmt::{self, Write as _},
    panic::{self, AssertUnwindSafe, PanicHookInfo},
    sync::Once,
};

use pleochroma_mobile_core::diagnostics;
use tracing::{
    Event, Level, Metadata, Subscriber,
    field::{Field, Visit},
    level_filters::LevelFilter,
    subscriber::Interest,
};
use tracing_subscriber::{Layer, layer::Context};

/// The module trees whose `tracing` events reach the tracer file (besides `TRACER_TARGET`).
const ALLOWED_TREES: [&str; 4] = [
    "pleochroma::renderer::gpu",
    "pleochroma::renderer::gpu_backend",
    "pleochroma_mobile_core::render",
    "pleochroma_mobile_core::diagnostics",
];

/// Field names that never reach a text log, whatever target writes them.
const DENIED_FIELDS: [&str; 9] = [
    "path",
    "file",
    "file_name",
    "filename",
    "title",
    "designer",
    "address",
    "addr",
    "url",
];

/// Path parts that mark a panic location in the GPU code, where the message may be written.
const GPU_LOCATION_MARKS: [&str; 3] = ["wgpu", "naga", "renderer/gpu"];

thread_local! {
    /// True while this thread writes a line, so an event raised by the writer is dropped.
    static WRITING: Cell<bool> = const { Cell::new(false) };
}

/// Clears [`WRITING`] when dropped, also when the writer unwinds.
struct WritingGuard;

impl WritingGuard {
    /// Marks this thread as writing, or `None` when it already is.
    fn enter() -> Option<Self> {
        WRITING.with(|writing| (!writing.replace(true)).then_some(Self))
    }
}

impl Drop for WritingGuard {
    fn drop(&mut self) {
        WRITING.with(|writing| writing.set(false));
    }
}

/// Writes the allow-listed events to the current diagnostics session.
#[derive(Debug, Default, Clone, Copy)]
pub struct TracerFileLayer;

impl<S: Subscriber> Layer<S> for TracerFileLayer {
    fn register_callsite(&self, _metadata: &'static Metadata<'static>) -> Interest {
        Interest::sometimes()
    }

    fn max_level_hint(&self) -> Option<LevelFilter> {
        Some(LevelFilter::INFO)
    }

    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let metadata = event.metadata();
        if *metadata.level() > Level::INFO || !target_allowed(metadata.target()) {
            return;
        }
        let Some(session) = diagnostics::current() else {
            return;
        };
        if !session.tracer_on() {
            return;
        }
        let Some(_guard) = WritingGuard::enter() else {
            return;
        };
        session.write_line(&format_event(event));
    }
}

/// True when `target` is the tracer target or inside one of the four allow-listed trees.
#[must_use]
pub fn target_allowed(target: &str) -> bool {
    target == pleochroma::diag::TRACER_TARGET
        || ALLOWED_TREES.iter().any(|tree| in_tree(target, tree))
}

/// True when `target` is `tree` or a module below it (the next characters are `::`).
fn in_tree(target: &str, tree: &str) -> bool {
    target
        .strip_prefix(tree)
        .is_some_and(|rest| rest.is_empty() || rest.starts_with("::"))
}

/// One event as a single line: the level, the target (unless it is the tracer target), the
/// message, and the fields as `name=value`.
#[must_use]
pub fn format_event(event: &Event<'_>) -> String {
    let metadata = event.metadata();
    let mut visitor = LineVisitor::default();
    event.record(&mut visitor);
    let mut line = String::new();
    let _ = write!(line, "{}", metadata.level());
    if metadata.target() != pleochroma::diag::TRACER_TARGET {
        let _ = write!(line, " {}", metadata.target());
    }
    if !visitor.message.is_empty() {
        let _ = write!(line, " {}", visitor.message);
    }
    line.push_str(&visitor.fields);
    one_line(&line)
}

/// Replaces line breaks with spaces: a log line is written whole.
fn one_line(text: &str) -> String {
    text.replace(['\n', '\r'], " ")
}

/// Collects the message and the `name=value` fields of an event.
#[derive(Default)]
struct LineVisitor {
    message: String,
    fields: String,
}

impl LineVisitor {
    /// Appends one field; the denied names are dropped.
    fn push_field(&mut self, name: &str, value: &str) {
        if DENIED_FIELDS.contains(&name) {
            return;
        }
        if value.contains(char::is_whitespace) {
            let _ = write!(self.fields, " {name}={value:?}");
        } else {
            let _ = write!(self.fields, " {name}={value}");
        }
    }
}

impl Visit for LineVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "message" {
            self.message.push_str(value);
        } else {
            self.push_field(field.name(), value);
        }
    }

    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        if field.name() == "message" {
            let _ = write!(self.message, "{value:?}");
        } else {
            let mut text = String::new();
            let _ = write!(text, "{value:?}");
            self.push_field(field.name(), &text);
        }
    }
}

/// Installs the panic line: a hook that writes the panic's location and thread to the
/// current session while the tracer log is on, then calls the hook that was there before.
///
/// Call it after `log_panics` has installed its hook, so logcat keeps the panic record.
/// Only the first call has an effect.
pub fn install_panic_line() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            // A panic inside a panic hook aborts the process: never let the writer unwind.
            let _ = panic::catch_unwind(AssertUnwindSafe(|| write_panic_line(info)));
            previous(info);
        }));
    });
}

/// Writes the panic line of `info` to the current session when the tracer log is on.
fn write_panic_line(info: &PanicHookInfo<'_>) {
    let Some(session) = diagnostics::current() else {
        return;
    };
    if !session.tracer_on() {
        return;
    }
    let Some(location) = info.location() else {
        return;
    };
    let thread = std::thread::current();
    let payload = info.payload();
    let message = payload
        .downcast_ref::<&str>()
        .copied()
        .or_else(|| payload.downcast_ref::<String>().map(String::as_str));
    let line = panic_line(
        location.file(),
        location.line(),
        thread.name().unwrap_or("unnamed"),
        message,
    );
    let _guard = WritingGuard::enter();
    session.write_line(&line);
}

/// The text of a panic line. The message is added only for a GPU location.
#[must_use]
pub fn panic_line(file: &str, line: u32, thread: &str, message: Option<&str>) -> String {
    let file = source_file(file);
    let mut text = format!("ERROR panic at {file}:{line} on {thread}");
    if let Some(message) = message.filter(|_| gpu_location(&file)) {
        let _ = write!(text, ": {message}");
    }
    one_line(&text)
}

/// True when a source path belongs to the GPU code (`wgpu`, `naga`, `renderer/gpu`).
#[must_use]
pub fn gpu_location(file: &str) -> bool {
    let file = file.replace('\\', "/");
    GPU_LOCATION_MARKS.iter().any(|mark| file.contains(mark))
}

/// A source path with forward slashes and without the builder's registry folder, so no
/// home folder name is written.
fn source_file(file: &str) -> String {
    let file = file.replace('\\', "/");
    let Some((_, rest)) = file.split_once("/registry/src/") else {
        return file;
    };
    // `rest` starts with the registry index folder; the crate folder follows it.
    rest.split_once('/')
        .map_or_else(|| rest.to_owned(), |(_, crate_path)| crate_path.to_owned())
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use tracing_subscriber::layer::SubscriberExt as _;

    use super::*;

    /// A layer that stores `format_event` of every event.
    struct Capture(Arc<Mutex<Vec<String>>>);

    impl<S: Subscriber> Layer<S> for Capture {
        fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
            self.0.lock().expect("lock").push(format_event(event));
        }
    }

    fn captured(run: impl FnOnce()) -> Vec<String> {
        let lines = Arc::new(Mutex::new(Vec::new()));
        let subscriber = tracing_subscriber::registry().with(Capture(Arc::clone(&lines)));
        tracing::subscriber::with_default(subscriber, run);
        let out = lines.lock().expect("lock").clone();
        out
    }

    #[test]
    fn the_allow_list_is_five_trees_not_a_prefix() {
        for target in [
            "pleochroma::tracer",
            "pleochroma::renderer::gpu",
            "pleochroma::renderer::gpu::whitted::pipeline",
            "pleochroma::renderer::gpu_backend",
            "pleochroma::renderer::gpu_backend::cpu",
            "pleochroma_mobile_core::render",
            "pleochroma_mobile_core::render::pool",
            "pleochroma_mobile_core::diagnostics::probe",
        ] {
            assert!(target_allowed(target), "{target}");
        }
        for target in [
            "pleochroma",
            "pleochroma::renderer",
            "pleochroma::renderer::gpu_driver",
            "pleochroma::renderer::gpuish",
            "pleochroma::tracer_extra",
            "pleochroma_mobile_core",
            "pleochroma_mobile_core::hdr",
            "pleochroma_mobile_core::document::autosave",
            "pleochroma_mobile_core::viewport::render_view",
            "pleochroma_mobile_android::platform_android::logging",
            "wgpu_core",
        ] {
            assert!(!target_allowed(target), "{target}");
        }
    }

    #[test]
    fn an_event_becomes_one_line_with_its_fields() {
        let lines = captured(|| {
            tracing::info!(target: "pleochroma::tracer", kernel = "walk", class = "uniaxial", ms = 12_u64, "compile finished");
        });
        assert_eq!(
            lines,
            vec!["INFO compile finished kernel=walk class=uniaxial ms=12".to_owned()]
        );
    }

    #[test]
    fn other_targets_keep_their_name_and_denied_fields_are_dropped() {
        let lines = captured(|| {
            tracing::warn!(target: "pleochroma::renderer::gpu::context", path = "/secret/x", bytes = 7_u32, "cache missing");
        });
        assert_eq!(
            lines,
            vec!["WARN pleochroma::renderer::gpu::context cache missing bytes=7".to_owned()]
        );
    }

    #[test]
    fn a_line_break_never_splits_a_line() {
        let lines = captured(|| {
            tracing::info!(target: "pleochroma::tracer", "one\ntwo");
        });
        assert_eq!(lines, vec!["INFO one two".to_owned()]);
    }

    #[test]
    fn a_panic_line_names_location_and_thread_only() {
        let line = panic_line(
            "crates/pleochroma/src/design.rs",
            42,
            "render-1",
            Some("secret"),
        );
        assert_eq!(
            line,
            "ERROR panic at crates/pleochroma/src/design.rs:42 on render-1"
        );
    }

    #[test]
    fn a_gpu_location_adds_the_message() {
        let line = panic_line(
            "crates/pleochroma/src/renderer/gpu/context.rs",
            7,
            "render-aux",
            Some("device lost"),
        );
        assert!(line.ends_with("on render-aux: device lost"), "{line}");
        assert!(gpu_location("C:\\x\\wgpu-core-27\\src\\a.rs"));
        assert!(gpu_location("src/naga/valid.rs"));
        assert!(!gpu_location("src/gpu_backend.rs"));
    }

    #[test]
    fn the_registry_folder_is_cut_from_a_dependency_path() {
        assert_eq!(
            source_file(
                "/home/dev/.cargo/registry/src/index.crates.io-1949/wgpu-core-27.0.0/src/a.rs"
            ),
            "wgpu-core-27.0.0/src/a.rs"
        );
        assert_eq!(source_file("src\\run.rs"), "src/run.rs");
    }
}
