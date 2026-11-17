//! The coordinator's log: two `tracing` layers, the ring the Log page reads and the daily
//! files.
//!
//! - [`ring_layer`] formats every event into a [`LogLine`], keeps the last [`RING_CAPACITY`]
//!   lines in a process-wide ring and hands each new line to every receiver made by
//!   [`subscribe`] (receivers that were dropped are pruned on the next line).
//! - [`file_layer`] appends the same lines to `coordinator-YYYY-MM-DD.log` in a folder, one
//!   file per day (UTC), and deletes files older than [`KEEP_DAYS`] days at start and again
//!   each time the day changes.
//! - [`scrub`] runs over every message before it is stored. A pairing token, a pairing link
//!   and a private key never reach the ring or a file, even if some code path logged one by
//!   mistake. Fields named like a secret are replaced as well.
//!
//! # Installing the layers
//!
//! Both layers work with any subscriber, so they go on the registry beside the usual output:
//!
//! ```ignore
//! use tracing_subscriber::{EnvFilter, fmt, layer::SubscriberExt, util::SubscriberInitExt};
//! tracing_subscriber::registry()
//!     .with(EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")))
//!     .with(fmt::layer().with_target(true))
//!     .with(pleochroma_worker::settings::log::ring_layer())
//!     .with(pleochroma_worker::settings::log::file_layer(&log_dir)) // an Option: None adds nothing
//!     .try_init()
//!     .ok();
//! ```
//!
//! The `EnvFilter` added first filters for every layer after it, so the ring and the files see
//! what the filter lets through (the default `info` keeps debug lines out).

use pleochroma_net::admin::status::{LogLevel, LogLine, UnixSeconds};
use std::{
    collections::VecDeque,
    fs::{self, File, OpenOptions},
    io::Write as _,
    path::{Path, PathBuf},
    sync::{
        Mutex, MutexGuard, PoisonError,
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    time::{SystemTime, UNIX_EPOCH},
};
use tracing::{
    Event, Subscriber,
    field::{Field, Visit},
};
use tracing_subscriber::{Layer, layer::Context};

/// How many lines the ring keeps.
pub const RING_CAPACITY: usize = 5000;

/// How many unread lines one [`subscribe`] receiver holds before it starts missing lines.
pub const SUBSCRIBER_CAPACITY: usize = 2048;

/// How many whole days of files are kept besides today's.
pub const KEEP_DAYS: i64 = 14;

/// What the file names start with.
const FILE_PREFIX: &str = "coordinator-";

/// What the file names end with.
const FILE_SUFFIX: &str = ".log";

/// What replaces a removed secret.
const REMOVED_KEY: &str = "[private key removed]";

/// The ring and the receivers waiting for new lines.
struct Hub {
    ring: VecDeque<LogLine>,
    subscribers: Vec<SyncSender<LogLine>>,
}

/// The process-wide ring.
static HUB: Mutex<Hub> = Mutex::new(Hub {
    ring: VecDeque::new(),
    subscribers: Vec::new(),
});

/// Locks the hub. A poisoned lock still holds a usable ring.
fn hub() -> MutexGuard<'static, Hub> {
    HUB.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Adds a line to the ring and sends it to every live receiver.
fn publish(line: LogLine) {
    let mut hub = hub();
    // A full receiver misses this line but stays; only a dropped receiver is pruned.
    hub.subscribers.retain(|tx| {
        !matches!(
            tx.try_send(line.clone()),
            Err(TrySendError::Disconnected(_))
        )
    });
    if hub.ring.len() >= RING_CAPACITY {
        hub.ring.pop_front();
    }
    hub.ring.push_back(line);
}

/// The last `limit` lines at `min` or more severe, oldest first.
#[must_use]
pub fn recent(min: tracing::Level, limit: usize) -> Vec<LogLine> {
    let min = LogLevel::from(min);
    let mut lines: Vec<LogLine> = {
        let hub = hub();
        hub.ring
            .iter()
            .rev()
            .filter(|line| line.level.passes(min))
            .take(limit)
            .cloned()
            .collect()
    };
    lines.reverse();
    lines
}

/// A receiver of every line logged from now on. Drop it to stop; the ring forgets it when the
/// next line is logged.
///
/// The receiver holds at most [`SUBSCRIBER_CAPACITY`] unread lines; a receiver that falls
/// further behind misses the lines logged meanwhile (the ring still has them).
#[must_use]
pub fn subscribe() -> Receiver<LogLine> {
    let (tx, rx) = mpsc::sync_channel(SUBSCRIBER_CAPACITY);
    hub().subscribers.push(tx);
    rx
}

/// [`recent`] and [`subscribe`] in one step: the backlog and a receiver that continues it with
/// no line lost or repeated between the two.
#[must_use]
pub fn subscribe_with_backlog(
    min: tracing::Level,
    limit: usize,
) -> (Vec<LogLine>, Receiver<LogLine>) {
    let min = LogLevel::from(min);
    let (tx, rx) = mpsc::sync_channel(SUBSCRIBER_CAPACITY);
    let mut backlog: Vec<LogLine> = {
        let mut hub = hub();
        hub.subscribers.push(tx);
        hub.ring
            .iter()
            .rev()
            .filter(|line| line.level.passes(min))
            .take(limit)
            .cloned()
            .collect()
    };
    backlog.reverse();
    (backlog, rx)
}

// ------------------------------------------------------------------ the layers

/// The layer that feeds the ring. See the module docs for how to install it.
#[derive(Debug, Clone, Copy, Default)]
pub struct RingLayer;

/// A layer that feeds the ring and every [`subscribe`] receiver.
#[must_use]
pub const fn ring_layer() -> RingLayer {
    RingLayer
}

impl<S: Subscriber> Layer<S> for RingLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        publish(line_from_event(event));
    }
}

/// The writer for the console layer (`fmt::layer().with_writer(console_writer())`).
///
/// Standard output, with [`scrub`] applied to every formatted event, so the console journal of a
/// service never keeps a token, a pairing link or a private key either.
#[must_use]
pub const fn console_writer() -> ScrubbedConsole {
    ScrubbedConsole
}

/// The `MakeWriter` behind [`console_writer`].
#[derive(Debug, Clone, Copy, Default)]
pub struct ScrubbedConsole;

/// One scrubbed write to the standard output.
#[derive(Debug)]
pub struct ScrubbedConsoleWriter(std::io::Stdout);

impl std::io::Write for ScrubbedConsoleWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        // The formatter hands over one whole event per write.
        let clean = scrub(&String::from_utf8_lossy(buf));
        self.0.write_all(clean.as_bytes())?;
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        self.0.flush()
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for ScrubbedConsole {
    type Writer = ScrubbedConsoleWriter;

    fn make_writer(&'a self) -> Self::Writer {
        ScrubbedConsoleWriter(std::io::stdout())
    }
}

/// The layer that writes the daily files. See the module docs for how to install it.
#[derive(Debug)]
pub struct FileLayer {
    state: Mutex<FileState>,
}

/// The open file and the day it belongs to.
#[derive(Debug)]
struct FileState {
    dir: PathBuf,
    day: i64,
    file: Option<File>,
}

/// A layer that appends to `coordinator-YYYY-MM-DD.log` in `dir` (created when missing).
///
/// Files older than [`KEEP_DAYS`] days are deleted now and each time the day changes. `None` when
/// the folder cannot be written.
#[must_use]
pub fn file_layer(dir: &Path) -> Option<FileLayer> {
    fs::create_dir_all(dir).ok()?;
    let day = day_of(now().0);
    let file = open_day_file(dir, day)?;
    prune(dir, day);
    Some(FileLayer {
        state: Mutex::new(FileState {
            dir: dir.to_path_buf(),
            day,
            file: Some(file),
        }),
    })
}

impl<S: Subscriber> Layer<S> for FileLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        let line = line_from_event(event);
        let (secs, millis) = now();
        let text = file_text(&line, millis);
        let day = day_of(secs);
        let mut guard = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let state = &mut *guard;
        if day != state.day || state.file.is_none() {
            let rolled = day != state.day;
            state.file = open_day_file(&state.dir, day);
            state.day = day;
            if rolled {
                prune(&state.dir, day);
            }
        }
        if let Some(file) = state.file.as_mut()
            && file.write_all(text.as_bytes()).is_err()
        {
            // Opened again on the next line.
            state.file = None;
        }
        drop(guard);
    }
}

/// Where the coordinator keeps its log files inside its data folder.
#[must_use]
pub fn log_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("logs")
}

/// The bytes the log files in `dir` take, or `None` when there are none.
#[must_use]
pub fn disk_usage(dir: &Path) -> Option<u64> {
    let entries = fs::read_dir(dir).ok()?;
    let mut total = 0_u64;
    let mut any = false;
    for entry in entries.flatten() {
        if entry
            .file_name()
            .to_str()
            .and_then(parse_file_day)
            .is_some()
        {
            any = true;
            total += entry.metadata().map_or(0, |m| fs::Metadata::len(&m));
        }
    }
    any.then_some(total)
}

/// Opens (or creates) the file of `day` for appending.
fn open_day_file(dir: &Path, day: i64) -> Option<File> {
    OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(file_name(day)))
        .ok()
}

/// Deletes the log files more than [`KEEP_DAYS`] days older than `today`. Returns how many.
fn prune(dir: &Path, today: i64) -> usize {
    let Ok(entries) = fs::read_dir(dir) else {
        return 0;
    };
    let mut removed = 0;
    for entry in entries.flatten() {
        let Some(day) = entry.file_name().to_str().and_then(parse_file_day) else {
            continue;
        };
        if today - day > KEEP_DAYS && fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

// ------------------------------------------------------------------ formatting

/// Collects an event's message and fields.
#[derive(Default)]
struct Fields {
    message: String,
    rest: String,
}

impl Fields {
    /// The one-line text: the message, then `name=value` for each field.
    fn into_text(self) -> String {
        let rest = self.rest.trim();
        if rest.is_empty() {
            self.message
        } else if self.message.is_empty() {
            rest.to_owned()
        } else {
            format!("{} {rest}", self.message)
        }
    }
}

impl Visit for Fields {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        let name = field.name();
        if name == "message" {
            self.message = format!("{value:?}");
            return;
        }
        let text = if secret_field(name) {
            "<removed>".to_owned()
        } else {
            format!("{value:?}")
        };
        self.rest.push(' ');
        self.rest.push_str(name);
        self.rest.push('=');
        self.rest.push_str(&text);
    }
}

/// True for a field whose name says it carries a secret.
fn secret_field(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    name.contains("secret")
        || name.contains("password")
        || name.contains("passphrase")
        || name.contains("private_key")
        || name.ends_with("token")
        || (name.contains("pair") && name.contains("link"))
}

/// The line an event makes: severity, module, scrubbed text and the time it was logged.
fn line_from_event(event: &Event<'_>) -> LogLine {
    let meta = event.metadata();
    let mut fields = Fields::default();
    event.record(&mut fields);
    LogLine {
        time: now().0,
        level: LogLevel::from(*meta.level()),
        target: meta.target().to_owned(),
        message: scrub(&fields.into_text()),
    }
}

/// One line of a file: `2026-11-16 14:02:07.114 INFO  target message`, UTC, on one line.
fn file_text(line: &LogLine, millis: u32) -> String {
    let (y, m, d) = civil_from_days(day_of(line.time));
    let message = line.message.replace('\r', "").replace('\n', "\\n");
    format!(
        "{y:04}-{m:02}-{d:02} {}.{millis:03} {:<5} {} {message}\n",
        clock(line.time),
        line.level.label(),
        line.target,
    )
}

/// A line as plain text for copying: `14:02:07 INFO  target message` (UTC).
#[must_use]
pub fn plain_text(line: &LogLine) -> String {
    format!(
        "{} {:<5} {} {}",
        clock(line.time),
        line.level.label(),
        line.target,
        line.message
    )
}

/// The time of day of a unix time, `14:02:07` (UTC).
#[must_use]
pub fn clock(time: UnixSeconds) -> String {
    let s = time % 86_400;
    format!("{:02}:{:02}:{:02}", s / 3600, s % 3600 / 60, s % 60)
}

// ------------------------------------------------------------------ the scrubber

/// Removes what must never be in a log from `text`.
///
/// A private key block, a `pleochroma://pair` link (it carries a one-time token), a `GW1-` token,
/// and the value of a `name=value` field whose name says it is a secret (the console prints events
/// already formatted, so the field-level check of the ring never sees them). A second line of
/// defense; no code path should log these.
#[must_use]
pub fn scrub(text: &str) -> String {
    let text = remove_links(text);
    let text = remove_private_keys(&text);
    let text = remove_tokens(&text);
    remove_secret_fields(&text)
}

/// Where a colored console puts a style: `ESC [ digits ; digits m`.
const ESCAPE: u8 = 0x1b;

/// The end of the style sequence that starts at `at` (just after it), or `at` itself when none
/// does.
fn escape_end(bytes: &[u8], at: usize) -> usize {
    if bytes.get(at) != Some(&ESCAPE) || bytes.get(at + 1) != Some(&b'[') {
        return at;
    }
    let mut end = at + 2;
    while bytes
        .get(end)
        .is_some_and(|b| b.is_ascii_digit() || *b == b';')
    {
        end += 1;
    }
    if bytes.get(end) == Some(&b'm') {
        end + 1
    } else {
        at
    }
}

/// The start of the style sequence that ends at `end` (exclusive), or `end` itself when none
/// does.
const fn escape_start(bytes: &[u8], end: usize) -> usize {
    if end == 0 || bytes[end - 1] != b'm' {
        return end;
    }
    let mut start = end - 1;
    while start > 0 && (bytes[start - 1].is_ascii_digit() || bytes[start - 1] == b';') {
        start -= 1;
    }
    if start >= 2 && bytes[start - 1] == b'[' && bytes[start - 2] == ESCAPE {
        start - 2
    } else {
        end
    }
}

/// The name of the field whose `=` is at `equals`, skipping the style sequences a colored
/// console puts around it.
fn field_name_before(text: &str, equals: usize) -> Option<&str> {
    let bytes = text.as_bytes();
    let mut end = equals;
    loop {
        let before = escape_start(bytes, end);
        if before == end {
            break;
        }
        end = before;
    }
    let mut start = end;
    while start > 0
        && (bytes[start - 1].is_ascii_alphanumeric() || matches!(bytes[start - 1], b'_' | b'.'))
    {
        start -= 1;
    }
    (start < end).then(|| &text[start..end])
}

/// The end of a field value that starts at `start`: a quoted string through its closing quote
/// (a backslash escapes the next byte), otherwise up to a space or a style sequence.
fn field_value_end(bytes: &[u8], start: usize) -> usize {
    if bytes.get(start) == Some(&b'"') {
        let mut at = start + 1;
        while at < bytes.len() {
            match bytes[at] {
                b'\\' => at += 2,
                b'"' => return at + 1,
                _ => at += 1,
            }
        }
        return bytes.len();
    }
    bytes[start..]
        .iter()
        .position(|b| b.is_ascii_whitespace() || *b == ESCAPE)
        .map_or(bytes.len(), |n| start + n)
}

/// Replaces the value of every `name=value` whose name [`secret_field`] says is a secret with
/// `<removed>`.
fn remove_secret_fields(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut copied = 0;
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] != b'=' {
            at += 1;
            continue;
        }
        let value_start = {
            let mut start = at + 1;
            loop {
                let next = escape_end(bytes, start);
                if next == start {
                    break start;
                }
                start = next;
            }
        };
        let secret = field_name_before(text, at).is_some_and(secret_field);
        let value_end = field_value_end(bytes, value_start);
        if secret && value_end > value_start {
            out.push_str(&text[copied..value_start]);
            out.push_str("<removed>");
            copied = value_end;
            at = value_end;
        } else {
            at += 1;
        }
    }
    out.push_str(&text[copied..]);
    out
}

/// Replaces every `pleochroma://pair...` link up to the next space or quote.
fn remove_links(text: &str) -> String {
    const SCHEME: &str = "pleochroma://pair";
    let lower = text.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while let Some(pos) = lower[at..].find(SCHEME) {
        let start = at + pos;
        let mut end = start + SCHEME.len();
        end += text[end..]
            .bytes()
            .take_while(|b| {
                !b.is_ascii_whitespace() && !matches!(b, b'"' | b'\'' | b'<' | b'>' | b')' | b'`')
            })
            .count();
        out.push_str(&text[at..start]);
        out.push_str("pleochroma://pair?[removed]");
        at = end;
    }
    out.push_str(&text[at..]);
    out
}

/// Replaces every `-----BEGIN ... PRIVATE KEY-----` block through its END line (or to the end
/// of the text when the END line is missing).
fn remove_private_keys(text: &str) -> String {
    const BEGIN: &str = "-----BEGIN ";
    const END: &str = "-----END ";
    const DASHES: &str = "-----";
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(BEGIN) {
        let after = &rest[start + BEGIN.len()..];
        let Some(label_len) = after.find(DASHES) else {
            if after.contains("PRIVATE KEY") {
                out.push_str(&rest[..start]);
                out.push_str(REMOVED_KEY);
                rest = "";
            }
            break;
        };
        if !after[..label_len].ends_with("PRIVATE KEY") {
            // A certificate or a public key: not secret, kept as it is.
            let keep = start + BEGIN.len() + label_len + DASHES.len();
            out.push_str(&rest[..keep]);
            rest = &rest[keep..];
            continue;
        }
        out.push_str(&rest[..start]);
        out.push_str(REMOVED_KEY);
        let body = &after[label_len + DASHES.len()..];
        rest = body
            .find(END)
            .and_then(|e| {
                let tail = &body[e + END.len()..];
                tail.find(DASHES).map(|t| &tail[t + DASHES.len()..])
            })
            .unwrap_or("");
    }
    out.push_str(rest);
    out
}

/// Replaces every `GW1-[A-Za-z0-9-]{20,}` token. The prefix and the letters match in either
/// case, because the token decoder accepts a token typed in lower case.
fn remove_tokens(text: &str) -> String {
    const PREFIX: &str = "GW1-";
    // ASCII lower-casing keeps every byte offset, so positions in `lower` index `text`.
    let lower = text.to_ascii_lowercase();
    let needle = PREFIX.to_ascii_lowercase();
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    while let Some(pos) = lower[at..].find(&needle) {
        let start = at + pos;
        let run_start = start + PREFIX.len();
        let run = text[run_start..]
            .bytes()
            .take_while(|b| b.is_ascii_alphanumeric() || *b == b'-')
            .count();
        out.push_str(&text[at..start]);
        if run >= 20 {
            out.push_str("GW1-[removed]");
            at = run_start + run;
        } else {
            out.push_str(&text[start..run_start]);
            at = run_start;
        }
    }
    out.push_str(&text[at..]);
    out
}

// ------------------------------------------------------------------ dates (UTC)

/// Now as unix seconds and the milliseconds within the second.
fn now() -> (UnixSeconds, u32) {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or((0, 0), |d| (d.as_secs(), d.subsec_millis()))
}

/// Days since 1970-01-01 of a unix time.
fn day_of(time: UnixSeconds) -> i64 {
    i64::try_from(time / 86_400).unwrap_or(0)
}

/// The file name of a day: `coordinator-2026-11-16.log`.
fn file_name(day: i64) -> String {
    let (y, m, d) = civil_from_days(day);
    format!("{FILE_PREFIX}{y:04}-{m:02}-{d:02}{FILE_SUFFIX}")
}

/// The day a log file name stands for; `None` for any other name.
fn parse_file_day(name: &str) -> Option<i64> {
    let date = name.strip_prefix(FILE_PREFIX)?.strip_suffix(FILE_SUFFIX)?;
    let bytes = date.as_bytes();
    if bytes.len() != 10 || bytes[4] != b'-' || bytes[7] != b'-' {
        return None;
    }
    let y: i64 = date.get(0..4)?.parse().ok()?;
    let m: u32 = date.get(5..7)?.parse().ok()?;
    let d: u32 = date.get(8..10)?.parse().ok()?;
    ((1..=12).contains(&m) && (1..=31).contains(&d)).then(|| days_from_civil(y, m, d))
}

/// The calendar date (year, month, day) of a day count since 1970-01-01.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    (
        y,
        u32::try_from(m).unwrap_or(1),
        u32::try_from(d).unwrap_or(1),
    )
}

/// The day count since 1970-01-01 of a calendar date.
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let m = i64::from(month);
    let d = i64::from(day);
    let y = year - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;
    use tracing_subscriber::layer::SubscriberExt;

    /// A pairing token the way the coordinator makes them (built here, not written out).
    fn sample_token() -> String {
        format!("GW1-{}-{}-{}", "ABCDEFGHJK", "LMNPQRSTUV", "WXYZ234567")
    }

    fn sample_link() -> String {
        format!(
            "pleochroma://pair?v=1&host=192.168.1.5&port=7878&t={}",
            sample_token()
        )
    }

    fn sample_pem() -> String {
        [
            "-----BEGIN PRIVATE KEY-----",
            "MIIEvQIBADANBgkqhkiG9w0BAQEFAASC",
            "Zm9vYmFyYmF6",
            "-----END PRIVATE KEY-----",
        ]
        .join("\n")
    }

    /// A folder of this test's own.
    fn temp_dir(tag: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("pleochroma-log-test-{}-{tag}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn dates_round_trip() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        for day in (-800..40_000).step_by(37) {
            let (y, m, d) = civil_from_days(day);
            assert_eq!(days_from_civil(y, m, d), day);
        }
        assert_eq!(
            civil_from_days(days_from_civil(2026, 11, 16)),
            (2026, 11, 16)
        );
    }

    #[test]
    fn file_names_parse_back() {
        let day = days_from_civil(2026, 11, 16);
        assert_eq!(file_name(day), "coordinator-2026-11-16.log");
        assert_eq!(parse_file_day("coordinator-2026-11-16.log"), Some(day));
        assert_eq!(parse_file_day("coordinator-2026-13-16.log"), None);
        assert_eq!(parse_file_day("coordinator-2026-11-16.log.bak"), None);
        assert_eq!(parse_file_day("notes.txt"), None);
    }

    #[test]
    fn clock_is_time_of_day() {
        assert_eq!(clock(14 * 3600 + 2 * 60 + 7), "14:02:07");
        assert_eq!(clock(86_400 + 5), "00:00:05");
    }

    #[test]
    fn scrub_removes_tokens_links_and_keys() {
        let token = sample_token();
        let out = scrub(&format!("issued {token} for the phone"));
        assert!(!out.contains("GW1-ABCDEFGH"), "{out}");
        assert!(out.starts_with("issued GW1-[removed] for"), "{out}");

        let link = sample_link();
        let out = scrub(&format!("link ({link}) shown"));
        assert!(!out.contains("host=") && !out.contains("GW1-"), "{out}");
        assert_eq!(out, "link (pleochroma://pair?[removed]) shown");
        assert_eq!(
            scrub("PLEOCHROMA://PAIR?x=1 done"),
            "pleochroma://pair?[removed] done"
        );

        let pem = sample_pem();
        let out = scrub(&format!("key {pem} after"));
        assert!(!out.contains("MIIEvQ") && !out.contains("BEGIN"), "{out}");
        assert_eq!(out, "key [private key removed] after");
        let rsa = pem
            .replace("BEGIN PRIVATE", "BEGIN RSA PRIVATE")
            .replace("END PRIVATE", "END RSA PRIVATE");
        assert_eq!(scrub(&rsa), "[private key removed]");
        // No END line: everything after the header goes.
        assert_eq!(
            scrub("x -----BEGIN PRIVATE KEY-----\nMIIE"),
            "x [private key removed]"
        );
    }

    #[test]
    fn scrub_removes_lower_and_mixed_case_tokens() {
        let token = sample_token();
        let lower = token.to_ascii_lowercase();
        let out = scrub(&format!("issued {lower} for the phone"));
        assert_eq!(out, "issued GW1-[removed] for the phone");
        let mixed = format!("Gw1-{}", &token[4..]);
        assert_eq!(scrub(&mixed), "GW1-[removed]");
        assert_eq!(scrub("id=gw1-short"), "id=gw1-short");
    }

    /// The console scrubber works on formatted text, so it hides the value of a secret-named
    /// field itself: quoted or bare, also between the style sequences of a colored console.
    #[test]
    fn scrub_hides_secret_named_fields() {
        assert_eq!(
            scrub("claim ok pairing_token=\"abc def\" device=\"S23\""),
            "claim ok pairing_token=<removed> device=\"S23\""
        );
        assert_eq!(
            scrub("x password=hunter2 next=1"),
            "x password=<removed> next=1"
        );
        assert_eq!(
            scrub("secret=\"a \\\" b\" tail=ok"),
            "secret=<removed> tail=ok"
        );
        // A colored console: ESC[3mnameESC[0mESC[2m=ESC[0mvalue.
        let colored = "\u{1b}[3mpassphrase\u{1b}[0m\u{1b}[2m=\u{1b}[0m\"swordfish\" \u{1b}[3mid\u{1b}[0m\u{1b}[2m=\u{1b}[0m7";
        let out = scrub(colored);
        assert!(!out.contains("swordfish"), "{out}");
        assert!(out.contains("=\u{1b}[0m<removed>"), "{out}");
        assert!(out.ends_with("=\u{1b}[0m7"), "{out}");
        // Names that only look similar stay as they are; so does a lone `=`.
        assert_eq!(scrub("tokens=5 a = b"), "tokens=5 a = b");
        assert_eq!(scrub("secret="), "secret=");
    }

    #[test]
    fn scrub_keeps_harmless_text() {
        let text = "device connected name=\"S23\" id=GW1-SHORT cert -----BEGIN CERTIFICATE----- ok";
        assert_eq!(scrub(text), text);
        assert_eq!(scrub("Größe ✓ 7878"), "Größe ✓ 7878");
    }

    /// Events that carry secrets (as text or as fields) reach neither the ring nor a receiver
    /// with them in it.
    #[test]
    fn events_never_carry_secrets() {
        const MARKER: &str = "s4l-secret-test";
        let rx = subscribe();
        let token = sample_token();
        let link = sample_link();
        let pem = sample_pem();
        let subscriber = tracing_subscriber::registry().with(ring_layer());
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!("{MARKER} token {token}");
            tracing::warn!("{MARKER} link {link}");
            tracing::error!("{MARKER} key {pem}");
            tracing::info!(pairing_token = %token, secret = "hunter2", "{MARKER} fields");
            tracing::info!(device = "S23", "{MARKER} harmless");
        });
        let lines: Vec<LogLine> = rx
            .try_iter()
            .filter(|l| l.message.contains(MARKER))
            .collect();
        assert_eq!(lines.len(), 5);
        for line in &lines {
            for secret in [
                "GW1-ABCDEFGH",
                "WXYZ234567",
                "pleochroma://pair?v",
                "host=192",
                "BEGIN PRIVATE",
                "MIIEvQ",
                "Zm9vYmFy",
                "hunter2",
            ] {
                assert!(
                    !line.message.contains(secret),
                    "{secret} in {}",
                    line.message
                );
            }
        }
        assert!(lines[3].message.contains("pairing_token=<removed>"));
        assert!(lines[4].message.contains("device=\"S23\""));
        assert_eq!(lines[2].level, LogLevel::Error);
        assert_eq!(lines[0].target, module_path!());
    }

    /// The ring keeps the newest lines, `recent` filters by level and returns oldest first, and
    /// a receiver made with a backlog continues it without a gap.
    #[test]
    fn ring_keeps_the_newest_and_filters_by_level() {
        const MARKER: &str = "s4l-ring-test";
        let line = |i: usize, level: LogLevel| LogLine {
            time: 1,
            level,
            target: "t".to_owned(),
            message: format!("{MARKER}-{i}"),
        };
        for i in 0..RING_CAPACITY + 10 {
            let level = if i % 100 == 0 {
                LogLevel::Warn
            } else {
                LogLevel::Info
            };
            publish(line(i, level));
        }
        let ours = |lines: Vec<LogLine>| -> Vec<usize> {
            lines
                .iter()
                .filter_map(|l| {
                    l.message
                        .strip_prefix(MARKER)?
                        .strip_prefix('-')?
                        .parse()
                        .ok()
                })
                .collect()
        };
        let all = ours(recent(tracing::Level::TRACE, RING_CAPACITY * 2));
        assert!(all.len() <= RING_CAPACITY);
        assert!(all.len() >= RING_CAPACITY - 100, "{}", all.len());
        assert_eq!(all.last(), Some(&(RING_CAPACITY + 9)));
        assert!(all.windows(2).all(|w| w[0] < w[1]));
        let warns = ours(recent(tracing::Level::WARN, RING_CAPACITY));
        assert!(!warns.is_empty() && warns.iter().all(|i| i % 100 == 0));
        assert!(recent(tracing::Level::TRACE, 3).len() <= 3);

        let (backlog, rx) = subscribe_with_backlog(tracing::Level::INFO, 5);
        assert!(backlog.len() <= 5);
        publish(line(999_999, LogLevel::Info));
        assert!(rx.try_iter().any(|l| l.message.ends_with("999999")));
        drop(rx);
        publish(line(999_998, LogLevel::Info));
    }

    #[test]
    fn file_layer_writes_today_and_prunes_old_files() {
        let dir = temp_dir("files");
        fs::create_dir_all(&dir).unwrap();
        let today = day_of(now().0);
        let old = dir.join(file_name(today - 40));
        let edge = dir.join(file_name(today - KEEP_DAYS));
        let other = dir.join("notes.txt");
        for path in [&old, &edge, &other] {
            fs::write(path, "x").unwrap();
        }
        let layer = file_layer(&dir).expect("the folder is writable");
        assert!(!old.exists(), "a 40-day-old file is deleted");
        assert!(edge.exists(), "a file {KEEP_DAYS} days old is kept");
        assert!(other.exists(), "other files stay");

        let token = sample_token();
        let subscriber = tracing_subscriber::registry().with(layer);
        tracing::subscriber::with_default(subscriber, || {
            tracing::info!("s4l-file-test hello {token}");
        });
        let text = fs::read_to_string(dir.join(file_name(today))).unwrap();
        assert!(text.contains("s4l-file-test hello GW1-[removed]"), "{text}");
        assert!(text.contains(" INFO  "), "{text}");
        assert!(disk_usage(&dir).is_some_and(|n| n > 0));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn file_layer_is_none_when_the_folder_cannot_be_written() {
        let dir = temp_dir("blocked");
        fs::create_dir_all(&dir).unwrap();
        let blocker = dir.join("blocker");
        fs::write(&blocker, "x").unwrap();
        assert!(file_layer(&blocker.join("logs")).is_none());
        assert_eq!(disk_usage(&dir.join("missing")), None);
        let _ = fs::remove_dir_all(&dir);
    }
}
