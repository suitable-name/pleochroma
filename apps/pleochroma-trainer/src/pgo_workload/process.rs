//! Self-spawned production CLI runs with durable logs and child cleanup on errors.

use std::{
    fs,
    path::Path,
    process::{Child, Command, Stdio},
    thread,
    time::{Duration, Instant},
};

use serde_json::Value;

use super::{Result, args::Config, capture::Capture, files};

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        if !matches!(self.0.try_wait(), Ok(Some(_))) {
            let _ = self.0.kill();
        }
        let _ = self.0.wait();
    }
}

pub(super) fn invoke(
    binary: &Path,
    arguments: &[String],
    directory: &Path,
    name: &str,
    config: &Config,
    mut capture: Option<&mut Capture>,
) -> Result<f64> {
    let log_path = directory.join(format!("{name}.log"));
    let mut command = vec![files::text(binary)?];
    command.extend_from_slice(arguments);
    files::write_json(&directory.join(format!("{name}-command.json")), &command)?;
    println!("[{name}] log: {}", log_path.display());
    let log = fs::File::create(&log_path)?;
    let started = Instant::now();
    let mut last_report = Instant::now();
    // Do not clear the environment: LLVM_PROFILE_FILE, BOLT instrumentation and
    // CUDA runtime/cache configuration belong to the optimized-build caller.
    let mut child = ChildGuard(
        Command::new(binary)
            .args(arguments)
            .current_dir(directory)
            .env("DIAGRAM_DB_PATH", &config.db)
            .env(pleochroma_update::NO_UPDATE_CHECK_ENV, "1")
            .stdin(Stdio::null())
            .stdout(log.try_clone()?)
            .stderr(log)
            .spawn()?,
    );
    loop {
        if let Some(status) = child.0.try_wait()? {
            if !status.success() {
                return Err(format!("trainer exited {status}; see {}", log_path.display()).into());
            }
            return Ok(started.elapsed().as_secs_f64());
        }
        if let Some(capture) = capture.as_mut() {
            capture.poll()?;
        }
        if last_report.elapsed() >= Duration::from_secs(30) {
            report_progress(directory, name, started.elapsed());
            last_report = Instant::now();
        }
        thread::sleep(Duration::from_millis(500));
    }
}

fn report_progress(directory: &Path, name: &str, elapsed: Duration) {
    let progress = files::read_json::<Value>(&directory.join("run.json.progress.json"));
    let detail = progress.map_or_else(
        |_| "startup or checkpoint work".to_owned(),
        |progress| {
            format!(
                "iteration {}, {}, steps {}/{}",
                progress["iteration"],
                progress["phase"],
                progress["completed_steps"],
                progress["total_steps"],
            )
        },
    );
    println!("[{name}] {}s: {detail}", elapsed.as_secs());
}
