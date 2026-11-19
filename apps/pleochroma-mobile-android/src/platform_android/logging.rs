//! Logging on Android: everything ends up in logcat under the tag `pleochroma`, and the
//! tracer log gets its share.
//!
//! Three sources feed logcat: `log` records (through [`CompositeLogger`], which wraps
//! `android_logger`), Rust panics with a backtrace (through `log-panics`, which needs the
//! unwinding release profile), and the `tracing` events of the shared crates (through a
//! `fmt` layer whose writer forwards each formatted line to `log`). Slint itself logs under
//! the tag `slint`.
//!
//! `log` records pass from debug level up in a debug build and from info in a release
//! build, except the shader compiler `naga` (warnings only) and `wgpu_core` / `wgpu_hal`
//! (info and up): in a debug build naga logs every expression it checks, thousands of
//! lines per shader, which bury the app's own lines and slow every pipeline compile.
//!
//! The tracer log (developer page) is a second destination, written only while its switch is
//! on: [`TracerFileLayer`] takes the allow-listed `tracing` events, and the composite `log`
//! logger takes the warnings of `wgpu_core`, `wgpu_hal` and `naga`. The panic line follows
//! `log-panics` in the hook chain.
//!
//! Read it with `adb logcat -s pleochroma slint`.

use std::{io, sync::Once};

use tracing::{Level, Metadata};
use tracing_subscriber::{EnvFilter, Layer as _, fmt::MakeWriter, layer::SubscriberExt as _};

use crate::tracer_layer::{TracerFileLayer, install_panic_line};

/// The `log` targets the tracer file accepts, at warn level and above.
const FILE_LOG_TARGETS: [&str; 3] = ["wgpu_core", "wgpu_hal", "naga"];

/// Installs the logger, the panic hooks and the `tracing` subscriber. Safe to call more
/// than once; only the first call has an effect.
pub(crate) fn init() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        let debug_build = cfg!(debug_assertions);
        let level = if debug_build {
            log::LevelFilter::Debug
        } else {
            log::LevelFilter::Info
        };
        install_logger(level);

        // Release builds are stripped, so symbol names would be empty: addresses only.
        let backtrace = if debug_build {
            log_panics::BacktraceMode::Resolved
        } else {
            log_panics::BacktraceMode::Unresolved
        };
        log_panics::Config::new()
            .backtrace_mode(backtrace)
            .install_panic_hook();
        // After `log_panics`: the panic line calls the hook it finds, so logcat keeps the record.
        install_panic_line();

        install_tracing();
    });
}

/// Installs the composite `log` logger. It replaces `android_logger::init_once`, which
/// would leave no place for the tracer file.
fn install_logger(level: log::LevelFilter) {
    let config = android_logger::Config::default()
        .with_tag("pleochroma")
        .with_max_level(level)
        .with_filter(record_filter(level));
    let logger = CompositeLogger {
        android: android_logger::AndroidLogger::new(config),
    };
    if log::set_boxed_logger(Box::new(logger)).is_ok() {
        log::set_max_level(level);
    }
}

/// The `log` filter: `level` for every target except `naga` (warnings only) and
/// `wgpu_core` / `wgpu_hal` (info and up).
fn record_filter(level: log::LevelFilter) -> android_logger::Filter {
    android_logger::FilterBuilder::new()
        .filter_level(level)
        .filter_module("naga", log::LevelFilter::Warn)
        .filter_module("wgpu_core", log::LevelFilter::Info)
        .filter_module("wgpu_hal", log::LevelFilter::Info)
        .build()
}

/// The `log` logger: every record goes to logcat; the GPU libraries' warnings also go to
/// the tracer file while its switch is on.
struct CompositeLogger {
    android: android_logger::AndroidLogger,
}

impl log::Log for CompositeLogger {
    fn enabled(&self, metadata: &log::Metadata<'_>) -> bool {
        self.android.enabled(metadata) || file_accepts(metadata.target(), metadata.level())
    }

    fn log(&self, record: &log::Record<'_>) {
        self.android.log(record);
        if file_accepts(record.target(), record.level()) {
            write_record(record);
        }
    }

    fn flush(&self) {
        self.android.flush();
    }
}

/// True for a warning or error of `wgpu_core`, `wgpu_hal` or `naga`. Nothing else is
/// accepted: not the `log-panics` record (the panic line writes panics), and not the records
/// that [`LogLine`] re-emits from `tracing`, which would write every line twice.
fn file_accepts(target: &str, level: log::Level) -> bool {
    level <= log::Level::Warn
        && FILE_LOG_TARGETS.iter().any(|accepted| {
            target
                .strip_prefix(accepted)
                .is_some_and(|rest| rest.is_empty() || rest.starts_with("::"))
        })
}

/// Writes one accepted record to the current session if its tracer log is on.
fn write_record(record: &log::Record<'_>) {
    let Some(session) = pleochroma_mobile_core::diagnostics::current() else {
        return;
    };
    if !session.tracer_on() {
        return;
    }
    let line = format!("{} {} {}", record.level(), record.target(), record.args());
    session.write_line(&line.replace(['\n', '\r'], " "));
}

/// Installs the `tracing` registry: the logcat layer (default filter `info`, as a per-layer
/// filter) and the [`TracerFileLayer`], which decides for itself in `on_event`.
///
/// `SubscriberBuilder::try_init` and `.init()` are deliberately not used: with the
/// `tracing-log` feature they would also install `LogTracer`, which fights the composite
/// logger for the global `log` logger and would feed our own output back into `tracing`.
/// So the registry is installed with `set_global_default`.
fn install_tracing() {
    let logcat = tracing_subscriber::fmt::layer()
        .with_writer(LogMakeWriter)
        .with_ansi(false)
        .without_time()
        .with_filter(EnvFilter::new("info"));
    let subscriber = tracing_subscriber::registry()
        .with(logcat)
        .with(TracerFileLayer);
    if tracing::subscriber::set_global_default(subscriber).is_err() {
        log::warn!(
            "a tracing subscriber was already installed; tracing output may not reach logcat"
        );
    }
}

/// Builds one [`LogLine`] per `tracing` event.
struct LogMakeWriter;

impl<'a> MakeWriter<'a> for LogMakeWriter {
    type Writer = LogLine;

    fn make_writer(&'a self) -> Self::Writer {
        LogLine::new(log::Level::Info)
    }

    fn make_writer_for(&'a self, meta: &Metadata<'_>) -> Self::Writer {
        LogLine::new(to_log_level(meta.level()))
    }
}

/// Maps a `tracing` level to the `log` level with the same meaning.
fn to_log_level(level: &Level) -> log::Level {
    if *level == Level::ERROR {
        log::Level::Error
    } else if *level == Level::WARN {
        log::Level::Warn
    } else if *level == Level::INFO {
        log::Level::Info
    } else if *level == Level::DEBUG {
        log::Level::Debug
    } else {
        log::Level::Trace
    }
}

/// One formatted event; it is written to `log` as a single record when dropped.
struct LogLine {
    level: log::Level,
    text: Vec<u8>,
}

impl LogLine {
    const fn new(level: log::Level) -> Self {
        Self {
            level,
            text: Vec::new(),
        }
    }
}

impl io::Write for LogLine {
    fn write(&mut self, data: &[u8]) -> io::Result<usize> {
        self.text.extend_from_slice(data);
        Ok(data.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for LogLine {
    fn drop(&mut self) {
        let text = String::from_utf8_lossy(&self.text);
        let line = text.trim_end();
        if !line.is_empty() {
            log::log!(self.level, "{line}");
        }
    }
}
