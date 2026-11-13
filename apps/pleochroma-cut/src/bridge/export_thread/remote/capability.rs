//! Whether remote compute is available for an export: probing a worker ([`probe_remote`])
//! and checking its advertised limits against the export ([`exceeds_pixel_cap`]).

use crate::{bridge::remote::remote_render, settings::WorkerSettings};
use pleochroma_net::messages::{Backend, RenderCapability};

/// Below this many remaining samples, a remote dispatch (handshake plus calibration round
/// trip) costs more than it saves. Same reasoning as `batch::HYBRID_MIN_SPP`, with the higher
/// latency of a network link.
pub(in crate::bridge::export_thread) const REMOTE_MIN_SPP: u32 = 32;

/// Why remote compute is unavailable. Shown verbatim in the export dialog via
/// [`RemoteUnavailable::message`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteUnavailable {
    NoWorkerConfigured,
    Unreachable(String),
    /// `Welcome::render` was `None` -- the worker is a library-only build. Mirrors
    /// `bridge::remote::remote_render::RemoteError::NoRenderCapacity`.
    LibraryOnly,
}

impl RemoteUnavailable {
    /// Human-readable explanation for the user.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::NoWorkerConfigured => "No remote coordinator is configured.".to_string(),
            Self::Unreachable(e) => format!("Remote coordinator unreachable ({e})."),
            Self::LibraryOnly => {
                "The configured remote serves the design library only -- it has no \
                 render capacity (no --render lane and no joined workers)."
                    .to_string()
            }
        }
    }
}

/// A remote worker confirmed reachable and render-capable, with everything a
/// [`pleochroma_net::messages::RenderRequest`] against it needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteCapability {
    /// Connection settings of the remote worker.
    pub worker: WorkerSettings,
    /// The worker's advertised `RenderCapability::max_pixels`. The export size is checked
    /// against it before dispatch.
    pub max_pixels: u32,
    /// The remote's `RenderCapability::hdr`: whether it can render scenes lit by an HDR map
    /// (input to `bridge::remote::remote_can_render`).
    pub hdr: bool,
    /// Whether the remote is a [`Backend::Coordinator`] rather than one plain worker. A
    /// coordinator takes larger requests (`rate::COORDINATOR_CHUNK_TARGET_SECS`) and its rate
    /// is measured over the whole request.
    pub coordinator: bool,
    /// Whether the remote advertised zoning in its `WELCOME` (input to
    /// `bridge::remote::zoned_scene_refusal`). Always `false` for a coordinator.
    pub zoning: bool,
}

impl RemoteCapability {
    /// The capability a worker's advertised `render` half describes, for `worker`.
    const fn from_render(worker: WorkerSettings, render: &RenderCapability) -> Self {
        Self {
            worker,
            max_pixels: render.max_pixels,
            hdr: render.hdr,
            coordinator: matches!(render.backend, Backend::Coordinator { .. }),
            zoning: false,
        }
    }
}

/// Whether an export at `width x height` exceeds `capability`'s advertised `max_pixels`.
#[must_use]
pub(in crate::bridge::export_thread) fn exceeds_pixel_cap(
    width: u32,
    height: u32,
    capability: &RemoteCapability,
) -> bool {
    u64::from(width) * u64::from(height) > u64::from(capability.max_pixels)
}

/// Probes the configured remote (`AppSettings::remote`) for render capacity. Blocking, since
/// it performs a TLS handshake; callers run it off the UI thread.
///
/// # Errors
///
/// See [`RemoteUnavailable`]'s variants.
pub fn probe_remote(
    worker: Option<&WorkerSettings>,
) -> Result<RemoteCapability, RemoteUnavailable> {
    let Some(worker) = worker.cloned() else {
        return Err(RemoteUnavailable::NoWorkerConfigured);
    };
    match remote_render::connect_and_handshake(&worker) {
        Ok((_stream, welcome)) => {
            let zoning = welcome.zoning;
            welcome
                .render
                .map_or(Err(RemoteUnavailable::LibraryOnly), |render| {
                    let capability = RemoteCapability::from_render(worker, &render);
                    let capability = RemoteCapability {
                        zoning,
                        ..capability
                    };
                    Ok(capability)
                })
        }
        Err(e) => Err(RemoteUnavailable::Unreachable(e.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_remote_reports_no_worker_configured_without_a_remote() {
        assert_eq!(
            probe_remote(None),
            Err(RemoteUnavailable::NoWorkerConfigured)
        );
    }

    #[test]
    fn probe_remote_reports_unreachable_for_a_bogus_address() {
        let worker = WorkerSettings {
            address: "127.0.0.1:1".to_string(), // nothing listens here
            cert_dir: std::env::temp_dir().display().to_string(),
            ..WorkerSettings::default()
        };
        let err = probe_remote(Some(&worker)).unwrap_err();
        assert!(matches!(err, RemoteUnavailable::Unreachable(_)));
    }

    fn capability(max_pixels: u32) -> RemoteCapability {
        RemoteCapability {
            worker: WorkerSettings::default(),
            max_pixels,
            hdr: false,
            coordinator: false,
            zoning: false,
        }
    }

    fn render_capability(backend: Backend) -> RenderCapability {
        RenderCapability {
            backend,
            max_pixels: 1_000_000,
            min_cadence_ms: 100,
            hdr: true,
        }
    }

    #[test]
    fn from_render_marks_only_a_coordinator_backend_as_a_coordinator() {
        let coordinator = RemoteCapability::from_render(
            WorkerSettings::default(),
            &render_capability(Backend::Coordinator {
                workers: 1,
                threads: 8,
                gpus: 2,
            }),
        );
        assert!(coordinator.coordinator);
        assert_eq!(coordinator.max_pixels, 1_000_000);
        assert!(coordinator.hdr);

        for backend in [
            Backend::Cpu { threads: 8 },
            Backend::Gpu {
                adapter: "RTX A6000".to_string(),
            },
        ] {
            let plain = RemoteCapability::from_render(
                WorkerSettings::default(),
                &render_capability(backend),
            );
            assert!(!plain.coordinator);
        }
    }

    #[test]
    fn exceeds_pixel_cap_uses_the_workers_own_advertised_cap_not_a_hardcoded_constant() {
        // A smaller-than-default advertised cap must still be honoured.
        let small_worker = capability(100);
        assert!(exceeds_pixel_cap(11, 10, &small_worker)); // 110 > 100
        assert!(!exceeds_pixel_cap(10, 10, &small_worker)); // 100 == 100, not over

        let real_default = capability(7680 * 4320);
        assert!(!exceeds_pixel_cap(3840, 2160, &real_default)); // 4K fits
        assert!(exceeds_pixel_cap(8192, 8192, &real_default)); // max custom export size does not
    }

    #[test]
    fn remote_unavailable_messages_are_distinct_and_human_readable() {
        assert!(
            RemoteUnavailable::NoWorkerConfigured
                .message()
                .contains("No remote coordinator")
        );
        assert!(
            RemoteUnavailable::LibraryOnly
                .message()
                .contains("library only")
        );
        assert!(
            RemoteUnavailable::Unreachable("boom".to_string())
                .message()
                .contains("boom")
        );
    }
}
