//! Shared output for the CLI and its observer. Terminal status occupies several bounded rows;
//! redirected output consists entirely of complete newline-terminated records.

use std::{
    io::{IsTerminal, Write, stderr, stdout},
    sync::{Arc, Mutex, MutexGuard},
};

const FALLBACK_WIDTH: usize = 80;

/// Clones serialize output and share both the live status and the latest completed metrics.
#[derive(Clone, Debug)]
pub struct Console(Arc<Mutex<ConsoleState>>);

#[derive(Debug)]
struct ConsoleState {
    live: bool,
    shown_rows: usize,
    status: Vec<String>,
    metrics: Vec<String>,
}

impl Console {
    pub fn new() -> Self {
        Self(Arc::new(Mutex::new(ConsoleState {
            live: stdout().is_terminal(),
            shown_rows: 0,
            status: Vec::new(),
            metrics: Vec::new(),
        })))
    }

    fn lock(&self) -> MutexGuard<'_, ConsoleState> {
        self.0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn is_live(&self) -> bool {
        self.lock().live
    }

    /// Replaces the status without losing the latest completed iteration's learning metrics.
    /// The observer controls the frequency of calls for redirected output.
    pub fn progress(&self, rows: &[String]) {
        let mut state = self.lock();
        state.erase();
        state.status = rows.to_vec();
        if state.live {
            state.draw();
        } else {
            println!("{}", rows.join(" | "));
            let _ = stdout().flush();
        }
    }

    /// Keeps a full iteration record in scrollback and refreshes the compact live metrics.
    pub fn iteration(&self, summary: &str, metrics: Vec<String>) {
        let mut state = self.lock();
        state.erase();
        println!("{summary}");
        state.metrics = metrics;
        state.draw();
        let _ = stdout().flush();
    }

    /// Refreshes metrics after an already-scheduled update synchronization.
    pub fn metrics(&self, metrics: Vec<String>) {
        let mut state = self.lock();
        if state.metrics == metrics {
            return;
        }
        state.erase();
        if !state.live {
            println!("{}", metrics.join(" | "));
            let _ = stdout().flush();
        }
        state.metrics = metrics;
        state.draw();
    }

    pub fn message(&self, text: &str) {
        self.message_block(|| println!("{text}"));
    }

    /// Prints a multi-line message without interleaving the observer's live dashboard.
    pub fn message_block(&self, output: impl FnOnce()) {
        let mut state = self.lock();
        state.erase();
        output();
        let _ = stdout().flush();
        state.draw();
    }

    pub fn warning(&self, text: &str) {
        let mut state = self.lock();
        state.erase();
        eprintln!("{text}");
        let _ = stderr().flush();
        state.draw();
    }

    /// Leaves the final dashboard visible, with the cursor on a fresh line.
    pub fn finish(&self) {
        let mut state = self.lock();
        if state.shown_rows > 0 {
            println!();
            let _ = stdout().flush();
        }
        state.shown_rows = 0;
        state.status.clear();
        state.metrics.clear();
    }
}

impl ConsoleState {
    fn erase(&mut self) {
        if self.shown_rows == 0 {
            return;
        }
        print!("\r\x1b[2K");
        for _ in 1..self.shown_rows {
            print!("\x1b[1A\r\x1b[2K");
        }
        let _ = stdout().flush();
        self.shown_rows = 0;
    }

    fn draw(&mut self) {
        if !self.live || self.status.is_empty() {
            return;
        }
        let size = terminal_size::terminal_size();
        let width = size.map_or(FALLBACK_WIDTH, |(w, _)| usize::from(w.0));
        let max_rows = size.map_or(23, |(_, h)| usize::from(h.0).saturating_sub(1).max(1));
        let mut rows = wrap_rows(self.status.iter().chain(&self.metrics), width);
        if rows.len() > max_rows {
            rows.truncate(max_rows);
            if let Some(last) = rows.last_mut() {
                *last = "more detail in iteration records / progress JSON"
                    .chars()
                    .take(width.saturating_sub(1).max(1))
                    .collect();
            }
        }
        print!("{}", rows.join("\n\r"));
        let _ = stdout().flush();
        self.shown_rows = rows.len();
    }
}

/// Wraps, rather than drops, information. All dashboard captions and runner task details are
/// ASCII; sanitize control/non-ASCII characters so a task detail cannot change cursor position.
fn wrap_rows<'a>(rows: impl Iterator<Item = &'a String>, width: usize) -> Vec<String> {
    let room = width.saturating_sub(1).max(1);
    let mut result = Vec::new();
    for row in rows {
        let ascii: String = row
            .chars()
            .map(|c| {
                if c.is_ascii() && !c.is_control() {
                    c
                } else {
                    '?'
                }
            })
            .collect();
        result.extend(
            ascii
                .as_bytes()
                .chunks(room)
                .map(|chunk| String::from_utf8_lossy(chunk).into_owned()),
        );
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn narrow_rows_preserve_every_metric_without_terminal_wrapping() {
        let rows = vec!["loss 0.123 | cos +0.71 | accepted 62% | rejected 38%".into()];
        for width in 2..100 {
            let wrapped = wrap_rows(rows.iter(), width);
            assert!(wrapped.iter().all(|line| line.len() < width));
            assert_eq!(wrapped.concat(), rows[0]);
        }
    }

    #[test]
    fn task_text_cannot_inject_terminal_control_sequences() {
        let rows = vec!["active \x1b[2J\n\r\tstone".into()];
        assert_eq!(wrap_rows(rows.iter(), 80), ["active ?[2J???stone"]);
    }

    #[test]
    fn clones_share_console_state() {
        let console = Console::new();
        let clone = console.clone();
        console.lock().metrics.push("completed update".into());
        assert_eq!(clone.lock().metrics, ["completed update"]);
    }
}
