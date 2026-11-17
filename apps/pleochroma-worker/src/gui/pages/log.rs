//! The Log page: the coordinator's log lines with a level filter, a search field, a live tail
//! and Copy visible lines / Open log folder.
//!
//! The page reads the process-wide ring of [`crate::settings::log`] (the lines this process
//! logged) and a [`subscribe`](crate::settings::log::subscribe_with_backlog) receiver for new
//! ones; a timer moves new lines into the page four times a second. The list follows the
//! newest line until the user scrolls up (that is in `log.slint`). Every line was scrubbed of
//! tokens, pairing links and private keys when it was logged.

use crate::{
    gui::{
        app::{ActiveBackend, AppWindow, LogState, ShellState},
        local,
    },
    settings::log,
};
use pleochroma_net::admin::status::{LogLevel, LogLine};
use slint::{ComponentHandle, ModelRc, SharedString, Timer, TimerMode, VecModel};
use std::{cell::RefCell, collections::VecDeque, rc::Rc, sync::mpsc::Receiver, time::Duration};

/// How often new lines move into the page.
const TICK: Duration = Duration::from_millis(250);

/// How many ticks pass between two reads of a remote coordinator's log (one second).
const REMOTE_TICKS: u32 = 4;

/// How many ticks a result ("Copied 24 lines") stays up.
const NOTICE_TICKS: u32 = 12;

/// How many ticks pass between two looks at the files' size.
const KEPT_TICKS: u32 = 40;

thread_local! {
    /// The refresh timer; a `Timer` stops when dropped, so it lives for the UI thread.
    static REFRESH_TIMER: RefCell<Option<Timer>> = const { RefCell::new(None) };
}

/// What the page holds besides what the window shows.
struct View {
    /// New lines from the ring.
    rx: Receiver<LogLine>,
    /// Every line the page knows, oldest first (at most the ring's capacity).
    all: VecDeque<LogLine>,
    /// The lines that pass the filter and the search, in the same order as the models.
    shown: VecDeque<LogLine>,
    times: Rc<VecModel<SharedString>>,
    levels: Rc<VecModel<i32>>,
    messages: Rc<VecModel<SharedString>>,
    /// 0 Problems, 1 Info, 2 Details.
    filter: i32,
    /// The search text, trimmed and lower case.
    search: String,
    clipboard: Option<arboard::Clipboard>,
    /// Ticks until the notice goes away.
    notice_left: u32,
    /// Ticks until the files' size is read again.
    kept_left: u32,
    /// True while the lines come from a remote coordinator instead of this process's log.
    remote: bool,
    /// Ticks until a remote coordinator's log is read again.
    remote_left: u32,
}

impl View {
    fn new(rx: Receiver<LogLine>, backlog: Vec<LogLine>) -> Self {
        Self {
            remote: false,
            remote_left: 0,
            rx,
            all: backlog.into(),
            shown: VecDeque::new(),
            times: Rc::new(VecModel::default()),
            levels: Rc::new(VecModel::default()),
            messages: Rc::new(VecModel::default()),
            filter: 1,
            search: String::new(),
            clipboard: None,
            notice_left: 0,
            kept_left: 0,
        }
    }

    /// True if `line` passes the level filter and the search.
    fn passes(&self, line: &LogLine) -> bool {
        line.level.passes(min_level(self.filter))
            && (self.search.is_empty()
                || format!("{} {} {}", line.message, line.target, line.level.label())
                    .to_lowercase()
                    .contains(&self.search))
    }

    /// Fills the models from `all` again (after a change of filter or search).
    fn rebuild(&mut self) {
        let shown: VecDeque<LogLine> = self
            .all
            .iter()
            .filter(|line| self.passes(line))
            .cloned()
            .collect();
        self.times.set_vec(
            shown
                .iter()
                .map(|l| SharedString::from(log::clock(l.time)))
                .collect::<Vec<_>>(),
        );
        self.levels.set_vec(
            shown
                .iter()
                .map(|l| level_number(l.level))
                .collect::<Vec<_>>(),
        );
        self.messages.set_vec(
            shown
                .iter()
                .map(|l| SharedString::from(l.message.as_str()))
                .collect::<Vec<_>>(),
        );
        self.shown = shown;
    }

    /// Adds one new line to `all` and, when it passes, to the models.
    fn add(&mut self, line: LogLine) {
        if self.all.len() >= log::RING_CAPACITY {
            self.all.pop_front();
        }
        if self.passes(&line) {
            if self.shown.len() >= log::RING_CAPACITY {
                self.shown.pop_front();
                self.times.remove(0);
                self.levels.remove(0);
                self.messages.remove(0);
            }
            self.times.push(log::clock(line.time).into());
            self.levels.push(level_number(line.level));
            self.messages.push(line.message.as_str().into());
            self.shown.push_back(line.clone());
        }
        self.all.push_back(line);
    }

    /// The count line under the list.
    fn count_text(&self) -> String {
        format!(
            "showing {} of {} lines · {}",
            group(self.shown.len()),
            group(self.all.len()),
            match self.filter {
                0 => "Problems only",
                1 => "Info and above",
                _ => "all details",
            }
        )
    }

    /// Copies the visible lines to the clipboard and says so.
    fn copy(&mut self, state: &LogState<'_>) {
        let count = self.shown.len();
        if count == 0 {
            return;
        }
        let text = self
            .shown
            .iter()
            .map(log::plain_text)
            .collect::<Vec<_>>()
            .join("\n");
        if self.clipboard.is_none() {
            self.clipboard = arboard::Clipboard::new().ok();
        }
        let copied = self
            .clipboard
            .as_mut()
            .is_some_and(|clipboard| clipboard.set_text(text).is_ok());
        let notice = if copied {
            format!("Copied {count} line{}", if count == 1 { "" } else { "s" })
        } else {
            "The clipboard is not available.".to_owned()
        };
        state.set_notice(notice.into());
        self.notice_left = NOTICE_TICKS;
    }

    /// Makes the page show the log of the coordinator that is selected: this process's own log
    /// (a live subscription) or, for a remote coordinator, the lines it pushed to this
    /// computer (read again once a second).
    fn follow(&mut self, remote: bool, backend: &ActiveBackend) {
        if remote {
            if !self.remote {
                // No local lines arrive any more: the sender is gone, so the receiver stays empty.
                self.rx = std::sync::mpsc::channel().1;
                self.remote = true;
                self.remote_left = 0;
                self.kept_left = 0;
            }
            if self.remote_left == 0 {
                self.remote_left = REMOTE_TICKS;
                let lines = backend
                    .get()
                    .logs(tracing::Level::TRACE, log::RING_CAPACITY);
                if !self.all.iter().eq(lines.iter()) {
                    self.all = lines.into();
                    self.rebuild();
                }
            }
            self.remote_left -= 1;
        } else if self.remote {
            let (backlog, rx) =
                log::subscribe_with_backlog(tracing::Level::TRACE, log::RING_CAPACITY);
            self.rx = rx;
            self.all = backlog.into();
            self.remote = false;
            self.kept_left = 0;
            self.rebuild();
        }
    }

    /// One timer tick: takes in new lines and refreshes the texts.
    fn tick(&mut self, state: &LogState<'_>) {
        let fresh: Vec<LogLine> = self.rx.try_iter().collect();
        for line in fresh {
            self.add(line);
        }
        state.set_count_text(self.count_text().into());
        if self.notice_left > 0 {
            self.notice_left -= 1;
            if self.notice_left == 0 {
                state.set_notice(SharedString::new());
            }
        }
        if self.kept_left == 0 {
            state.set_kept_text(kept_text(self.remote).into());
            self.kept_left = KEPT_TICKS;
        }
        self.kept_left -= 1;
    }
}

/// The lowest severity a filter shows.
const fn min_level(filter: i32) -> LogLevel {
    match filter {
        0 => LogLevel::Warn,
        1 => LogLevel::Info,
        _ => LogLevel::Trace,
    }
}

/// The level as the page's number: 0 error, 1 warning, 2 info, 3 debug, 4 trace.
const fn level_number(level: LogLevel) -> i32 {
    match level {
        LogLevel::Error => 0,
        LogLevel::Warn => 1,
        LogLevel::Info => 2,
        LogLevel::Debug => 3,
        LogLevel::Trace => 4,
    }
}

/// A count with thousands separators: `1,284`.
fn group(count: usize) -> String {
    let digits = count.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// "3.4 MB" or "120 KB".
fn size_text(bytes: u64) -> String {
    const MB: u64 = 1024 * 1024;
    if bytes < MB {
        format!("{} KB", bytes.div_ceil(1024))
    } else {
        format!("{:.1} MB", bytes as f64 / MB as f64)
    }
}

/// The folder the log files are in, once the coordinator folder is known.
fn log_folder() -> Option<std::path::PathBuf> {
    local::folder().map(|folder| log::log_dir(folder.data_dir()))
}

/// The right-hand text of the status line.
fn kept_text(remote: bool) -> String {
    if remote {
        // The remote coordinator's files are on its own computer.
        return "Times in UTC · Lines from the remote coordinator".to_owned();
    }
    log_folder()
        .as_deref()
        .and_then(log::disk_usage)
        .map_or_else(
            || "Times in UTC · Not saved to a file".to_owned(),
            |bytes| {
                format!(
                    "Times in UTC · Kept for {} days · {}",
                    log::KEEP_DAYS,
                    size_text(bytes)
                )
            },
        )
}

/// Connects the Log page to the coordinator's log.
///
/// Page contract: called once by `gui::app::run`. For this computer's coordinator the page shows
/// this process's own log; while a remote coordinator is selected it shows the lines that
/// coordinator sent (`backend.get().logs(..)`).
pub fn wire(window: &AppWindow, backend: &ActiveBackend) {
    let (backlog, rx) = log::subscribe_with_backlog(tracing::Level::TRACE, log::RING_CAPACITY);
    let view = Rc::new(RefCell::new(View::new(rx, backlog)));
    let state = window.global::<LogState>();
    {
        let mut view = view.borrow_mut();
        view.rebuild();
        state.set_times(ModelRc::from(Rc::clone(&view.times)));
        state.set_levels(ModelRc::from(Rc::clone(&view.levels)));
        state.set_messages(ModelRc::from(Rc::clone(&view.messages)));
        state.set_level_filter(view.filter);
        state.set_count_text(view.count_text().into());
        state.set_kept_text(kept_text(false).into());
        view.kept_left = KEPT_TICKS;
    }
    state.set_loaded(true);

    state.on_filter_changed({
        let view = Rc::clone(&view);
        let weak = window.as_weak();
        move |filter| {
            let mut view = view.borrow_mut();
            view.filter = filter;
            view.rebuild();
            if let Some(window) = weak.upgrade() {
                window
                    .global::<LogState>()
                    .set_count_text(view.count_text().into());
            }
        }
    });
    state.on_search_changed({
        let view = Rc::clone(&view);
        let weak = window.as_weak();
        move |text| {
            let mut view = view.borrow_mut();
            view.search = text.trim().to_lowercase();
            view.rebuild();
            if let Some(window) = weak.upgrade() {
                window
                    .global::<LogState>()
                    .set_count_text(view.count_text().into());
            }
        }
    });
    state.on_copy_visible({
        let view = Rc::clone(&view);
        let weak = window.as_weak();
        move || {
            if let Some(window) = weak.upgrade() {
                view.borrow_mut().copy(&window.global::<LogState>());
            }
        }
    });
    state.on_open_folder({
        let weak = window.as_weak();
        move || {
            // The log files are on the coordinator's own computer: a remote one is not opened here.
            let remote = weak
                .upgrade()
                .is_none_or(|window| window.global::<ShellState>().get_remote());
            if remote {
                return;
            }
            if let Some(dir) = log_folder() {
                crate::gui::util::open_folder(&dir);
            }
        }
    });

    let timer = Timer::default();
    timer.start(TimerMode::Repeated, TICK, {
        let weak = window.as_weak();
        let backend = backend.clone();
        move || {
            if let Some(window) = weak.upgrade() {
                let mut view = view.borrow_mut();
                view.follow(window.global::<ShellState>().get_remote(), &backend);
                view.tick(&window.global::<LogState>());
            }
        }
    });
    REFRESH_TIMER.with(|slot| *slot.borrow_mut() = Some(timer));
}

#[cfg(test)]
mod tests {
    use super::*;
    use slint::Model as _;

    fn line(level: LogLevel, message: &str) -> LogLine {
        LogLine {
            time: 3_600,
            level,
            target: "pleochroma_worker::serve".to_owned(),
            message: message.to_owned(),
        }
    }

    #[test]
    fn counts_get_separators() {
        assert_eq!(group(0), "0");
        assert_eq!(group(24), "24");
        assert_eq!(group(1284), "1,284");
        assert_eq!(group(1_000_000), "1,000,000");
    }

    #[test]
    fn sizes_read_plainly() {
        assert_eq!(size_text(100), "1 KB");
        assert_eq!(size_text(120 * 1024), "120 KB");
        assert_eq!(size_text(3_565_158), "3.4 MB");
    }

    #[test]
    fn filters_map_to_levels() {
        assert_eq!(min_level(0), LogLevel::Warn);
        assert_eq!(min_level(1), LogLevel::Info);
        assert_eq!(min_level(2), LogLevel::Trace);
        assert!(LogLevel::Error.passes(min_level(0)));
        assert!(!LogLevel::Info.passes(min_level(0)));
        assert!(!LogLevel::Debug.passes(min_level(1)));
        assert!(LogLevel::Trace.passes(min_level(2)));
        assert_eq!(level_number(LogLevel::Error), 0);
        assert_eq!(level_number(LogLevel::Trace), 4);
    }

    #[test]
    fn view_filters_by_level_and_search() {
        let (_tx, rx) = std::sync::mpsc::channel();
        let backlog = vec![
            line(LogLevel::Info, "listener started"),
            line(LogLevel::Warn, "TLS handshake rejected"),
            line(LogLevel::Debug, "keep-alive"),
            line(LogLevel::Error, "render failed"),
        ];
        let mut view = View::new(rx, backlog);
        view.rebuild();
        assert_eq!(view.shown.len(), 3, "Info and above");
        assert_eq!(view.messages.row_count(), 3);
        view.filter = 0;
        view.rebuild();
        assert_eq!(view.shown.len(), 2, "Problems");
        view.filter = 2;
        view.search = "keep".to_owned();
        view.rebuild();
        assert_eq!(view.shown.len(), 1);
        assert_eq!(view.count_text(), "showing 1 of 4 lines · all details");
        view.add(line(LogLevel::Info, "keep going"));
        assert_eq!(view.shown.len(), 2);
        assert_eq!(view.messages.row_count(), 2);
        assert_eq!(view.all.len(), 5);
    }

    #[test]
    fn view_keeps_at_most_the_ring_capacity() {
        let (_tx, rx) = std::sync::mpsc::channel();
        let mut view = View::new(rx, Vec::new());
        for i in 0..log::RING_CAPACITY + 5 {
            view.add(line(LogLevel::Info, &format!("line {i}")));
        }
        assert_eq!(view.all.len(), log::RING_CAPACITY);
        assert_eq!(view.shown.len(), log::RING_CAPACITY);
        assert_eq!(view.messages.row_count(), log::RING_CAPACITY);
        assert_eq!(view.messages.row_data(0).unwrap().as_str(), "line 5");
    }
}
