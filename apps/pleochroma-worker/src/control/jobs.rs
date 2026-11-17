//! The administrator's view of the durable job store: every owner's jobs, and cancelling or
//! deleting any of them.
//!
//! [`crate::jobs::JobStore`] answers one owner at a time (a viewer sees only its own jobs).
//! [`JobAdmin`] is the coordinator window's side: it finds each job's owner in the job's state
//! file (`<id>.state.json`, the store's commit record) and then asks the store, with that
//! owner, for the live numbers. The store stays the one place that changes a job; nothing
//! here writes a job file.
//!
//! The owners are cached by job id (an owner never changes), so a status snapshot every
//! second reads only the directory listing plus the state files of jobs it has not seen yet.

use crate::jobs::{JobError, JobService, Limits, configured_location, unix_now};
use pleochroma_net::{
    admin::status::{JobId as StatusJobId, JobState, JobStatus, JobsSummary, fingerprint_short},
    messages::{JobInfo, JobPhase, job_id_hex, parse_job_id_hex},
};
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};

/// The suffix of a job's state file (the store's own constant is private).
const STATE_SUFFIX: &str = ".state.json";

/// The job id on the wire.
type WireJobId = [u8; 16];

/// What the window says when the job is gone.
const UNKNOWN_JOB: &str = "That job no longer exists.";

/// Every owner's jobs, over one [`JobService`].
pub struct JobAdmin {
    service: Arc<JobService>,
    owners: Mutex<HashMap<WireJobId, String>>,
    /// The last [`Self::list_cached`] answer.
    listed: Mutex<Option<Listed>>,
}

/// A saved job list: when it was read and under which [`JobService::generation`].
struct Listed {
    taken: Instant,
    generation: u64,
    jobs: Vec<(String, JobInfo)>,
}

/// How long [`JobAdmin::list_cached`] reuses an answer. A job's progress is kept in memory only
/// and changes no generation, so this is also how late a progress bar can be; it is a little
/// under the once-a-second status refresh, so each refresh sees fresh numbers while several
/// sessions asking in the same moment share one read.
const LIST_TTL: Duration = Duration::from_millis(900);

impl JobAdmin {
    /// An admin view over `service`.
    #[must_use]
    pub fn new(service: Arc<JobService>) -> Self {
        Self {
            service,
            owners: Mutex::new(HashMap::new()),
            listed: Mutex::new(None),
        }
    }

    /// [`Self::list`], reused for [`LIST_TTL`]: the once-a-second status refresh does not list the
    /// job folder every time.
    ///
    /// Cancelling or deleting a job through this view drops the saved answer, and so does any
    /// submit, start or end of a job (the service's generation moved).
    #[must_use]
    pub fn list_cached(&self) -> Vec<(String, JobInfo)> {
        let generation = self.service.generation();
        {
            let saved = self.listed.lock().unwrap_or_else(PoisonError::into_inner);
            if let Some(listed) = saved.as_ref()
                && listed.generation == generation
                && listed.taken.elapsed() < LIST_TTL
            {
                return listed.jobs.clone();
            }
        }
        let jobs = self.list();
        *self.listed.lock().unwrap_or_else(PoisonError::into_inner) = Some(Listed {
            taken: Instant::now(),
            generation,
            jobs: jobs.clone(),
        });
        jobs
    }

    /// Drops the saved [`Self::list_cached`] answer.
    fn forget_listed(&self) {
        *self.listed.lock().unwrap_or_else(PoisonError::into_inner) = None;
    }

    /// The service this view reads.
    #[must_use]
    pub const fn service(&self) -> &Arc<JobService> {
        &self.service
    }

    /// The ids of every job that has a state file on disk.
    fn ids_on_disk(&self) -> Vec<WireJobId> {
        let Ok(entries) = std::fs::read_dir(self.service.store().dir()) else {
            return Vec::new();
        };
        entries
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name();
                let name = name.to_string_lossy();
                name.strip_suffix(STATE_SUFFIX).and_then(parse_job_id_hex)
            })
            .collect()
    }

    /// The owner key of job `id`: from the cache, else from its state file.
    fn owner_of(&self, id: &WireJobId) -> Option<String> {
        if let Some(owner) = self
            .owners
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(id)
        {
            return Some(owner.clone());
        }
        let path = self
            .service
            .store()
            .dir()
            .join(format!("{}{STATE_SUFFIX}", job_id_hex(id)));
        let owner = owner_in_state_file(&path)?;
        self.owners
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(*id, owner.clone());
        Some(owner)
    }

    /// Forgets the cached owners of jobs that are no longer on disk.
    fn forget_except(&self, keep: &[WireJobId]) {
        let keep: HashSet<&WireJobId> = keep.iter().collect();
        self.owners
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .retain(|id, _| keep.contains(id));
    }

    /// Every stored job with its owner key, newest first.
    #[must_use]
    pub fn list(&self) -> Vec<(String, JobInfo)> {
        let ids = self.ids_on_disk();
        self.forget_except(&ids);
        let mut jobs: Vec<(String, JobInfo)> = ids
            .iter()
            .filter_map(|id| {
                let owner = self.owner_of(id)?;
                let info = self.service.store().status(&owner, id).ok()?;
                Some((owner, info))
            })
            .collect();
        jobs.sort_by(|(_, a), (_, b)| {
            b.created_unix
                .cmp(&a.created_unix)
                .then_with(|| a.job_id.cmp(&b.job_id))
        });
        jobs
    }

    /// Cancels a queued or running job of any owner. Returns its owner key and its state
    /// after the request (a running job is stopped by its executor a moment later).
    ///
    /// # Errors
    ///
    /// A plain sentence when the job is unknown or has already ended.
    pub fn cancel(&self, id: &WireJobId) -> Result<(String, JobInfo), String> {
        let owner = self.owner_of(id).ok_or_else(|| UNKNOWN_JOB.to_owned())?;
        let info = self
            .service
            .store()
            .cancel(&owner, id, unix_now())
            .map_err(plain_error)?;
        self.forget_listed();
        Ok((owner, info))
    }

    /// Deletes a job of any owner with its picture. Returns the job's label.
    ///
    /// # Errors
    ///
    /// A plain sentence when the job is unknown or still rendering.
    pub fn delete(&self, id: &WireJobId) -> Result<String, String> {
        let owner = self.owner_of(id).ok_or_else(|| UNKNOWN_JOB.to_owned())?;
        let label = self
            .service
            .store()
            .status(&owner, id)
            .map(|info| info.label)
            .map_err(plain_error)?;
        self.service
            .store()
            .delete(&owner, id)
            .map_err(plain_error)?;
        self.owners
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(id);
        self.forget_listed();
        Ok(label)
    }

    /// The jobs footer: retention, how many jobs are stored and how much room they use.
    #[must_use]
    pub fn summary(&self, jobs: &[(String, JobInfo)], anchor: &Path) -> JobsSummary {
        let (_, limits) = configured_location(anchor);
        summarize(jobs, self.service.store().dir(), limits)
    }
}

/// The `owner` field of a state file, if it reads.
fn owner_in_state_file(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let value: serde_json::Value = serde_json::from_str(&text).ok()?;
    value.get("owner")?.as_str().map(str::to_owned)
}

/// A store failure in plain words (the first letter is made a capital).
fn plain_error(error: JobError) -> String {
    match error {
        JobError::Unknown => UNKNOWN_JOB.to_owned(),
        JobError::State(text) | JobError::Invalid(text) | JobError::Limit(text) => {
            capitalize(&text)
        }
        JobError::Io(text) => {
            tracing::warn!("jobs: {text}");
            "The job store could not be read or written. The log has the details.".to_owned()
        }
    }
}

/// `text` with its first character in upper case.
fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// The footer numbers for `jobs` stored in `folder` under `limits`.
fn summarize(jobs: &[(String, JobInfo)], folder: &Path, limits: Limits) -> JobsSummary {
    let used: u64 = jobs
        .iter()
        .filter_map(|(_, info)| info.result_len)
        .map(u64::from)
        .sum();
    JobsSummary {
        retention_days: u32::try_from(limits.retention_secs / 86_400).ok(),
        stored: u32::try_from(jobs.len()).unwrap_or(u32::MAX),
        max_jobs: u32::try_from(limits.max_jobs).unwrap_or(u32::MAX),
        used_bytes: used,
        limit_bytes: Some(limits.max_stored_bytes),
        folder: folder.display().to_string(),
    }
}

/// The name and the fingerprint (hex) an owner key stands for.
///
/// `certificate:<hex>` is looked up in `names` (fingerprint hex to device name) and falls back
/// to "Device" plus the short fingerprint; `address:<ip>` (a connection without TLS) is the
/// address and has no fingerprint.
#[must_use]
pub fn owner_display<S: std::hash::BuildHasher>(
    owner: &str,
    names: &HashMap<String, String, S>,
) -> (String, String) {
    if let Some(hex) = owner.strip_prefix("certificate:") {
        let name = names
            .get(hex)
            .cloned()
            .unwrap_or_else(|| format!("Device {}", fingerprint_short(hex)));
        return (name, hex.to_owned());
    }
    if let Some(address) = owner.strip_prefix("address:") {
        return (address.to_owned(), String::new());
    }
    (owner.to_owned(), String::new())
}

/// The window's state for a stored job's phase.
pub(crate) const fn state_of(phase: JobPhase) -> JobState {
    match phase {
        JobPhase::Queued => JobState::Queued,
        JobPhase::Running => JobState::Running,
        JobPhase::Done => JobState::Done,
        JobPhase::Failed => JobState::Failed,
        JobPhase::Cancelled => JobState::Cancelled,
    }
}

/// Seconds since the epoch as the status types carry them (never negative).
fn unix_u64(seconds: i64) -> u64 {
    u64::try_from(seconds).unwrap_or(0)
}

/// How far a job is, from 0.0 to 1.0 (0.0 when it asks for no samples).
fn progress_of(done: u32, total: u32) -> f32 {
    if total == 0 {
        return 0.0;
    }
    let permille = (u64::from(done) * 1000 / u64::from(total)).min(1000);
    f32::from(u16::try_from(permille).unwrap_or(1000)) / 1000.0
}

/// The jobs list the window shows. `names` maps device fingerprints (hex) to names;
/// `route` is what a running job is rendered on ("this computer").
#[must_use]
pub fn job_rows<S: std::hash::BuildHasher>(
    jobs: &[(String, JobInfo)],
    names: &HashMap<String, String, S>,
    route: &str,
) -> Vec<JobStatus> {
    jobs.iter()
        .map(|(owner, info)| {
            let (owner_name, owner_fingerprint) = owner_display(owner, names);
            JobStatus {
                id: StatusJobId(info.job_id),
                owner: owner_name,
                owner_fingerprint,
                state: state_of(info.phase),
                progress: progress_of(info.samples_done, info.samples_total),
                samples_done: info.samples_done,
                samples_total: info.samples_total,
                width: info.width,
                height: info.height,
                label: info.label.clone(),
                design_name: String::new(),
                route: if info.phase == JobPhase::Running {
                    route.to_owned()
                } else {
                    String::new()
                },
                created: unix_u64(info.created_unix),
                finished: info.finished_unix.map(unix_u64),
                result_len: info.result_len.map(u64::from),
                note: info.note.clone(),
            }
        })
        .collect()
}

/// Whether any listed job is rendering now.
#[must_use]
pub fn any_running(jobs: &[(String, JobInfo)]) -> bool {
    jobs.iter().any(|(_, info)| info.phase == JobPhase::Running)
}

/// The running job's owner key and label, if one renders now.
#[must_use]
pub fn running_job(jobs: &[(String, JobInfo)]) -> Option<(&str, &str)> {
    jobs.iter()
        .find(|(_, info)| info.phase == JobPhase::Running)
        .map(|(owner, info)| (owner.as_str(), info.label.as_str()))
}

/// The wire id of a window job id.
#[must_use]
pub const fn wire_id(id: StatusJobId) -> WireJobId {
    id.0
}

/// A job id's hex text, for events and logs.
#[must_use]
pub fn id_text(id: &WireJobId) -> String {
    job_id_hex(id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        jobs::JobStore,
        serve::tests::fixtures::{tiny_scene, unique_temp_dir},
    };
    use pleochroma_net::{
        SceneState,
        messages::{JobSubmit, WireColorSpace},
    };

    const NOW: i64 = 1_700_000_000;

    /// A job on the tiny scene, enlarged to the job store's 16-pixel minimum.
    fn submit(token: u8) -> JobSubmit {
        JobSubmit {
            client_token: [token; 16],
            label: format!("Job {token}"),
            scene: SceneState {
                width: 16,
                height: 16,
                ..tiny_scene()
            },
            samples: 4,
            color_space: WireColorSpace::Srgb,
        }
    }

    fn admin(label: &str) -> (JobAdmin, std::path::PathBuf) {
        let dir = unique_temp_dir(label);
        let store = JobStore::open(&dir, Limits::default(), NOW).unwrap();
        (JobAdmin::new(Arc::new(JobService::new(store, None))), dir)
    }

    #[test]
    fn the_list_spans_owners_newest_first_and_names_them() {
        let (admin, dir) = admin("admin-list");
        let store = admin.service().store();
        let alice = store.submit("certificate:aa", &submit(1), NOW).unwrap();
        let bob = store
            .submit("address:10.0.0.7", &submit(2), NOW + 10)
            .unwrap();

        let jobs = admin.list();
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].1.job_id, bob.job_id, "newest first");
        assert_eq!(jobs[1].1.job_id, alice.job_id);

        let mut names = HashMap::new();
        names.insert("aa".to_owned(), "Niklas's S23".to_owned());
        let rows = job_rows(&jobs, &names, "this computer");
        assert_eq!(rows[0].owner, "10.0.0.7");
        assert_eq!(rows[0].owner_fingerprint, "");
        assert_eq!(rows[1].owner, "Niklas's S23");
        assert_eq!(rows[1].owner_fingerprint, "aa");
        assert_eq!(rows[1].state, JobState::Queued);
        assert_eq!(rows[1].label, "Job 1");
        assert!(rows[1].route.is_empty(), "a queued job has no route yet");

        let summary = admin.summary(&jobs, &dir.join("library.sqlite"));
        assert_eq!(summary.stored, 2);
        assert_eq!(summary.retention_days, Some(7));
        assert_eq!(summary.max_jobs, 256);
        assert!(!summary.folder.is_empty());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn an_unknown_owner_shows_the_short_fingerprint() {
        let (name, fingerprint) =
            owner_display(&format!("certificate:{}", "ab".repeat(32)), &HashMap::new());
        assert_eq!(name, "Device abababab");
        assert_eq!(fingerprint, "ab".repeat(32));
    }

    #[test]
    fn cancel_stops_a_queued_job_of_any_owner_and_a_second_cancel_says_why_not() {
        let (admin, dir) = admin("admin-cancel");
        let queued = admin
            .service()
            .store()
            .submit("certificate:cc", &submit(3), NOW)
            .unwrap();

        let (owner, info) = admin.cancel(&queued.job_id).unwrap();
        assert_eq!(owner, "certificate:cc");
        assert_eq!(info.phase, JobPhase::Cancelled);
        let listed = admin.list();
        assert_eq!(listed[0].1.phase, JobPhase::Cancelled);

        let again = admin.cancel(&queued.job_id).unwrap_err();
        assert!(
            again.starts_with("Only a waiting or running job"),
            "{again}"
        );
        assert_eq!(admin.cancel(&[9; 16]).unwrap_err(), UNKNOWN_JOB);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn delete_removes_a_job_of_any_owner_with_its_files() {
        let (admin, dir) = admin("admin-delete");
        let job = admin
            .service()
            .store()
            .submit("certificate:dd", &submit(4), NOW)
            .unwrap();
        assert_eq!(admin.list().len(), 1);

        assert_eq!(admin.delete(&job.job_id).unwrap(), "Job 4");
        assert!(admin.list().is_empty());
        assert_eq!(admin.delete(&job.job_id).unwrap_err(), UNKNOWN_JOB);
        let leftovers = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .contains(&id_text(&job.job_id))
            })
            .count();
        assert_eq!(leftovers, 0, "the job's files are gone");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_running_job_cannot_be_deleted_but_can_be_cancelled() {
        let (admin, dir) = admin("admin-running");
        let job = admin
            .service()
            .store()
            .submit("certificate:ee", &submit(5), NOW)
            .unwrap();
        let stop = std::sync::atomic::AtomicBool::new(false);
        let claim = admin
            .service()
            .store()
            .claim_next(&stop, std::time::Duration::from_millis(50))
            .expect("the queued job is claimed");
        assert_eq!(claim.id, job.job_id);

        let jobs = admin.list();
        assert!(any_running(&jobs));
        assert_eq!(running_job(&jobs), Some(("certificate:ee", "Job 5")));
        let delete = admin.delete(&job.job_id).unwrap_err();
        assert!(
            delete.starts_with("A running job cannot be deleted"),
            "{delete}"
        );

        admin.cancel(&job.job_id).unwrap();
        assert!(
            claim.cancel.load(std::sync::atomic::Ordering::Acquire),
            "the running job's executor is told to stop"
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn small_helpers_behave() {
        assert_eq!(capitalize("only a waiting job"), "Only a waiting job");
        assert_eq!(capitalize(""), "");
        assert_eq!(wire_id(StatusJobId([3; 16])), [3; 16]);
        assert!((progress_of(1, 4) - 0.25).abs() < f32::EPSILON);
        assert!((progress_of(9, 4) - 1.0).abs() < f32::EPSILON);
        assert!(progress_of(1, 0).abs() < f32::EPSILON);
        let info = JobInfo {
            job_id: [1; 16],
            phase: JobPhase::Done,
            samples_done: 4,
            samples_total: 4,
            width: 16,
            height: 16,
            label: "x".to_owned(),
            created_unix: 5,
            finished_unix: Some(9),
            expires_unix: None,
            result_len: Some(100),
            note: String::new(),
        };
        let rows = job_rows(&[("x".to_owned(), info)], &HashMap::new(), "here");
        assert!((rows[0].progress - 1.0).abs() < f32::EPSILON);
        assert_eq!(rows[0].finished, Some(9));
        assert_eq!(rows[0].result_len, Some(100));
    }
}
