//! Checks on the values of an administration request, so a remote administrator gets a plain
//! refusal before the coordinator touches its settings.
//!
//! The limits are copies of the ones the worker's settings model enforces
//! (`apps/pleochroma-worker/src/settings/model.rs`, `Settings::validate` and its
//! `MAX_NAME_CHARS`). Keep the two in step: the coordinator validates the patched settings
//! again with the model, so a limit here that is wider than the model's only costs a later,
//! equally plain refusal, and one that is narrower would wrongly refuse a legal value.

use super::{protocol::AdminRequest, status::SettingsPatch};

/// Most characters of a name (a device, the display name, a pinned render computer).
/// The settings model's `MAX_NAME_CHARS`.
pub const MAX_NAME_CHARS: usize = 64;

/// Job memory in MiB: the settings model's `max_job_memory_mib` range.
pub const JOB_MEMORY_MIB: (u32, u32) = (64, 1_048_576);

/// Seconds a whole-image render may take: the model's `whole_image_seconds` range.
pub const WHOLE_IMAGE_SECONDS: (u32, u32) = (1, 3600);

/// Pixels of a whole-image render: the model's `whole_image_pixels` range, the upper end
/// being its `MAX_WHOLE_IMAGE_PIXELS` (1 << 40).
pub const WHOLE_IMAGE_PIXELS: (u64, u64) = (1, 1 << 40);

/// Jobs one device may have at once: the model's `jobs_per_device` range.
pub const JOBS_PER_DEVICE: (u32, u32) = (1, 256);

/// CPU threads (0 = automatic), for `cpu_threads` and `render_threads`: the model's range.
pub const CPU_THREADS: (u32, u32) = (0, 1024);

/// Days a finished job is kept: the model's `job_retention_days` range.
pub const JOB_RETENTION_DAYS: (u32, u32) = (1, 3650);

/// The asset cache size in MiB: the model's `cache_size_mib` range.
pub const CACHE_SIZE_MIB: (u32, u32) = (64, 4_194_304);

/// `Ok` when `lo <= value <= hi`, otherwise a sentence naming `what` and the range.
fn range<T: PartialOrd + Copy + std::fmt::Display>(
    what: &str,
    value: Option<T>,
    (lo, hi): (T, T),
) -> Result<(), String> {
    match value {
        Some(v) if v < lo || v > hi => Err(format!("{what} must be between {lo} and {hi}.")),
        _ => Ok(()),
    }
}

/// A short single-line name: at most [`MAX_NAME_CHARS`] characters (after trimming), no
/// control characters, and not empty unless `may_be_empty`.
fn text(what: &str, value: &str, may_be_empty: bool) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() && !may_be_empty {
        return Err(format!("{what} cannot be empty."));
    }
    if value.chars().count() > MAX_NAME_CHARS {
        return Err(format!(
            "{what} can have at most {MAX_NAME_CHARS} characters."
        ));
    }
    if value.chars().any(char::is_control) {
        return Err(format!("{what} cannot contain control characters."));
    }
    Ok(())
}

/// Checks a device name (for `IssueToken` and `Rename`): not empty, short, one line.
///
/// # Errors
///
/// A sentence in plain words.
pub fn name(value: &str) -> Result<(), String> {
    text("The name", value, false)
}

/// Checks every set field of a settings patch against the allowed ranges. Unset fields are
/// not checked. An empty patch passes.
///
/// # Errors
///
/// The first problem, as a sentence naming the setting and its range.
pub fn patch(patch: &SettingsPatch) -> Result<(), String> {
    range("Job memory (MiB)", patch.max_job_memory_mib, JOB_MEMORY_MIB)?;
    range(
        "The whole-image time",
        patch.whole_image_seconds,
        WHOLE_IMAGE_SECONDS,
    )?;
    range(
        "The whole-image size",
        patch.whole_image_pixels,
        WHOLE_IMAGE_PIXELS,
    )?;
    range("Jobs per device", patch.jobs_per_device, JOBS_PER_DEVICE)?;
    range("CPU threads", patch.cpu_threads, CPU_THREADS)?;
    range(
        "CPU threads of this computer",
        patch.render_threads,
        CPU_THREADS,
    )?;
    range(
        "Job retention (days)",
        patch.job_retention_days,
        JOB_RETENTION_DAYS,
    )?;
    range("The cache size (MiB)", patch.cache_size_mib, CACHE_SIZE_MIB)?;
    if let Some(pinned) = &patch.pinned_worker {
        text("The pinned render computer", pinned, true)?;
    }
    if let Some(display) = &patch.display_name {
        text("The name", display, false)?;
    }
    Ok(())
}

/// Checks the values inside a request: device names and the settings patch. Requests with
/// nothing to check pass.
///
/// # Errors
///
/// The first problem, as a sentence.
pub fn request(req: &AdminRequest) -> Result<(), String> {
    match req {
        AdminRequest::IssueToken { name: n, .. } | AdminRequest::Rename { name: n, .. } => name(n),
        AdminRequest::ApplySettings { patch: p, .. } => patch(p),
        AdminRequest::Subscribe { .. }
        | AdminRequest::Status { .. }
        | AdminRequest::CancelToken { .. }
        | AdminRequest::Revoke { .. }
        | AdminRequest::CancelJob { .. }
        | AdminRequest::DeleteJob { .. }
        | AdminRequest::DisconnectWorker { .. }
        | AdminRequest::Restart { .. }
        | AdminRequest::FetchResult { .. } => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::messages::PeerRole;

    fn with(f: impl FnOnce(&mut SettingsPatch)) -> SettingsPatch {
        let mut p = SettingsPatch::default();
        f(&mut p);
        p
    }

    #[test]
    fn an_empty_patch_passes() {
        assert_eq!(patch(&SettingsPatch::default()), Ok(()));
    }

    #[test]
    fn range_bounds_are_inclusive_and_one_past_fails() {
        // (lower bound ok, one below fails, upper bound ok, one above fails)
        assert!(patch(&with(|p| p.max_job_memory_mib = Some(64))).is_ok());
        assert!(patch(&with(|p| p.max_job_memory_mib = Some(63))).is_err());
        assert!(patch(&with(|p| p.max_job_memory_mib = Some(1_048_576))).is_ok());
        assert!(patch(&with(|p| p.max_job_memory_mib = Some(1_048_577))).is_err());

        assert!(patch(&with(|p| p.whole_image_seconds = Some(1))).is_ok());
        assert!(patch(&with(|p| p.whole_image_seconds = Some(0))).is_err());
        assert!(patch(&with(|p| p.whole_image_seconds = Some(3600))).is_ok());
        assert!(patch(&with(|p| p.whole_image_seconds = Some(3601))).is_err());

        assert!(patch(&with(|p| p.whole_image_pixels = Some(1))).is_ok());
        assert!(patch(&with(|p| p.whole_image_pixels = Some(0))).is_err());
        assert!(patch(&with(|p| p.whole_image_pixels = Some(1 << 40))).is_ok());
        assert!(patch(&with(|p| p.whole_image_pixels = Some((1 << 40) + 1))).is_err());

        assert!(patch(&with(|p| p.jobs_per_device = Some(1))).is_ok());
        assert!(patch(&with(|p| p.jobs_per_device = Some(0))).is_err());
        assert!(patch(&with(|p| p.jobs_per_device = Some(256))).is_ok());
        assert!(patch(&with(|p| p.jobs_per_device = Some(257))).is_err());

        assert!(patch(&with(|p| p.cpu_threads = Some(0))).is_ok());
        assert!(patch(&with(|p| p.cpu_threads = Some(1024))).is_ok());
        assert!(patch(&with(|p| p.cpu_threads = Some(1025))).is_err());
        assert!(patch(&with(|p| p.render_threads = Some(0))).is_ok());
        assert!(patch(&with(|p| p.render_threads = Some(1024))).is_ok());
        assert!(patch(&with(|p| p.render_threads = Some(1025))).is_err());

        assert!(patch(&with(|p| p.job_retention_days = Some(1))).is_ok());
        assert!(patch(&with(|p| p.job_retention_days = Some(0))).is_err());
        assert!(patch(&with(|p| p.job_retention_days = Some(3650))).is_ok());
        assert!(patch(&with(|p| p.job_retention_days = Some(3651))).is_err());

        assert!(patch(&with(|p| p.cache_size_mib = Some(64))).is_ok());
        assert!(patch(&with(|p| p.cache_size_mib = Some(63))).is_err());
        assert!(patch(&with(|p| p.cache_size_mib = Some(4_194_304))).is_ok());
        assert!(patch(&with(|p| p.cache_size_mib = Some(4_194_305))).is_err());
    }

    #[test]
    fn a_range_error_names_the_setting_and_the_range() {
        let err = patch(&with(|p| p.jobs_per_device = Some(0))).unwrap_err();
        assert_eq!(err, "Jobs per device must be between 1 and 256.");
        let err = patch(&with(|p| p.cache_size_mib = Some(1))).unwrap_err();
        assert!(err.contains("cache") && err.contains("64") && err.contains("4194304"));
    }

    #[test]
    fn the_display_name_cannot_be_empty_or_odd() {
        assert!(patch(&with(|p| p.display_name = Some("Studio PC".to_string()))).is_ok());
        assert!(patch(&with(|p| p.display_name = Some(String::new()))).is_err());
        assert!(patch(&with(|p| p.display_name = Some("   ".to_string()))).is_err());
        assert!(patch(&with(|p| p.display_name = Some("a".repeat(64)))).is_ok());
        assert!(patch(&with(|p| p.display_name = Some("a".repeat(65)))).is_err());
        assert!(patch(&with(|p| p.display_name = Some("two\nlines".to_string()))).is_err());
        // Counts characters, not bytes.
        assert!(patch(&with(|p| p.display_name = Some("é".repeat(64)))).is_ok());
    }

    #[test]
    fn the_pinned_computer_may_be_empty_to_clear_the_pin() {
        assert!(patch(&with(|p| p.pinned_worker = Some(String::new()))).is_ok());
        assert!(patch(&with(|p| p.pinned_worker = Some("garage-rig".to_string()))).is_ok());
        assert!(patch(&with(|p| p.pinned_worker = Some("a".repeat(65)))).is_err());
        assert!(patch(&with(|p| p.pinned_worker = Some("a\tb".to_string()))).is_err());
    }

    #[test]
    fn unconstrained_fields_always_pass() {
        let p = with(|p| {
            p.interactive_workers = Some(u32::MAX);
            p.allow_pairing = Some(false);
            p.discovery = Some(true);
            p.render_enabled = Some(true);
        });
        assert_eq!(patch(&p), Ok(()));
    }

    #[test]
    fn the_first_problem_is_reported() {
        let p = with(|p| {
            p.max_job_memory_mib = Some(1);
            p.jobs_per_device = Some(0);
        });
        assert!(patch(&p).unwrap_err().starts_with("Job memory"));
    }

    #[test]
    fn request_validation_covers_names_and_patches() {
        let issue = |n: &str| AdminRequest::IssueToken {
            req_id: 1,
            role: PeerRole::Viewer,
            name: n.to_string(),
        };
        assert!(request(&issue("Attic PC")).is_ok());
        assert!(request(&issue("")).is_err());
        assert!(request(&issue(&"x".repeat(65))).is_err());
        let rename = AdminRequest::Rename {
            req_id: 2,
            fingerprint: "ab".repeat(32),
            name: "bad\u{0}name".to_string(),
        };
        assert!(request(&rename).is_err());
        let apply = |p: SettingsPatch| AdminRequest::ApplySettings {
            req_id: 3,
            patch: p,
        };
        assert!(request(&apply(SettingsPatch::default())).is_ok());
        assert!(request(&apply(with(|p| p.job_retention_days = Some(0)))).is_err());
        assert!(request(&AdminRequest::Restart { req_id: 4 }).is_ok());
    }
}
