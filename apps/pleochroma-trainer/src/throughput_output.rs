//! Append-only diagnostics with startup heartbeats and explicit output ownership.

use std::{
    fs::{File, OpenOptions},
    io::{self, Write},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crate::runner::progress::TrainingProgress;
use serde_json::{Value, json};

#[derive(Clone)]
pub struct Journal(Arc<JournalInner>);

struct JournalInner {
    file: Mutex<(File, u64)>,
    started: Instant,
}

pub struct Output {
    pub journal: Journal,
    summary: File,
    pub directory: PathBuf,
}

impl Output {
    pub fn create(directory: &Path, metadata: &Value) -> io::Result<Self> {
        if let Some(parent) = directory
            .parent()
            .filter(|path| !path.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::create_dir(directory)?;
        // Every artifact is reserved before expensive work. Existing files are never opened
        // for truncation, including when a previous run crashed halfway through startup.
        let mut meta = new_file(&directory.join("metadata.json"))?;
        let events = new_file(&directory.join("events.jsonl"))?;
        let summary = new_file(&directory.join("summary.json"))?;
        write_json(&mut meta, metadata)?;
        Ok(Self {
            journal: Journal(Arc::new(JournalInner {
                file: Mutex::new((events, 0)),
                started: Instant::now(),
            })),
            summary,
            directory: directory.to_path_buf(),
        })
    }

    pub fn finish(&mut self, summary: &Value) -> io::Result<()> {
        write_json(&mut self.summary, summary)
    }
}

fn new_file(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| io::Error::new(error.kind(), format!("{}: {error}", path.display())))
}

fn write_json(file: &mut File, value: &Value) -> io::Result<()> {
    serde_json::to_writer(&mut *file, value).map_err(io::Error::other)?;
    file.write_all(b"\n")?;
    file.flush()?;
    file.sync_data()
}

impl Journal {
    pub fn event(&self, kind: &str, data: Value) -> io::Result<()> {
        let mut locked = self.0.file.lock().unwrap_or_else(PoisonError::into_inner);
        let sequence = locked.1;
        let row = json!({
            "sequence": sequence,
            "utc": crate::stats::rfc3339_utc_now(),
            "elapsed_wall_secs": self.0.started.elapsed().as_secs_f64(),
            "event": kind,
            "data": data,
        });
        write_json(&mut locked.0, &row)?;
        locked.1 += 1;
        Ok(())
    }
}

struct Watch {
    phase: String,
    progress: Option<TrainingProgress>,
}

pub struct Heartbeat {
    watch: Arc<Mutex<Watch>>,
    failed: Arc<AtomicBool>,
    stop: mpsc::Sender<()>,
    handle: Option<JoinHandle<io::Result<()>>>,
}

impl Heartbeat {
    pub fn start(journal: Journal) -> io::Result<Self> {
        let watch = Arc::new(Mutex::new(Watch {
            phase: "startup".into(),
            progress: None,
        }));
        let failed = Arc::new(AtomicBool::new(false));
        let (stop, receive) = mpsc::channel();
        let worker_watch = Arc::clone(&watch);
        let worker_failed = Arc::clone(&failed);
        let handle = thread::Builder::new()
            .name("throughput-heartbeat".into())
            .spawn(move || {
                while let Err(mpsc::RecvTimeoutError::Timeout) =
                    receive.recv_timeout(Duration::from_secs(2))
                {
                    let (phase, progress) = {
                        let watched = worker_watch.lock().unwrap_or_else(PoisonError::into_inner);
                        (watched.phase.clone(), watched.progress.clone())
                    };
                    let snapshot = progress.map(|progress| progress.snapshot());
                    if let Some(snapshot) = &snapshot {
                        eprintln!(
                            "probe: {phase}; {} {}/{} tasks; {}",
                            snapshot.phase,
                            snapshot.completed_tasks,
                            snapshot.total_tasks,
                            snapshot.oldest_active_task.as_deref().unwrap_or("waiting")
                        );
                    } else {
                        eprintln!("probe: {phase}; still running");
                    }
                    if let Err(error) =
                        journal.event("heartbeat", json!({"phase": phase, "snapshot": snapshot}))
                    {
                        worker_failed.store(true, Ordering::Relaxed);
                        eprintln!("probe heartbeat logging failed: {error}");
                        return Err(error);
                    }
                }
                Ok(())
            })?;
        Ok(Self {
            watch,
            failed,
            stop,
            handle: Some(handle),
        })
    }

    pub fn phase(&self, phase: &str, progress: Option<TrainingProgress>) {
        let mut watched = self.watch.lock().unwrap_or_else(PoisonError::into_inner);
        watched.phase = phase.to_owned();
        watched.progress = progress;
    }

    pub fn failed(&self) -> bool {
        self.failed.load(Ordering::Relaxed)
    }

    pub fn finish(&mut self) -> io::Result<()> {
        let _ = self.stop.send(());
        if let Some(handle) = self.handle.take() {
            handle
                .join()
                .map_err(|_| io::Error::other("heartbeat thread panicked"))??;
        }
        Ok(())
    }
}

impl Drop for Heartbeat {
    fn drop(&mut self) {
        if let Err(error) = self.finish() {
            eprintln!("probe heartbeat shutdown failed: {error}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn artifacts_refuse_overwrite_and_case_events_are_readable_before_shutdown() {
        let temporary = tempfile::tempdir().unwrap();
        let directory = temporary.path().join("new-run");
        let mut output = Output::create(&directory, &json!({"seed": 42})).unwrap();
        output
            .journal
            .event("case_complete", json!({"entry": 283}))
            .unwrap();
        let contents = std::fs::read_to_string(directory.join("events.jsonl")).unwrap();
        let first: Value = serde_json::from_str(contents.trim()).unwrap();
        assert_eq!(first["sequence"], 0);
        assert_eq!(first["data"]["entry"], 283);
        assert!(Output::create(&directory, &json!({"seed": 99})).is_err());
        output.finish(&json!({"status": "complete"})).unwrap();
        let metadata: Value = serde_json::from_str(
            &std::fs::read_to_string(directory.join("metadata.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(metadata["seed"], 42);
    }
}
