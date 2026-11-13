//! The remote half of an adaptive render.
//!
//! It holds [`RemoteLane`], the connection(s) that trace a share of each pass's live tiles on a
//! worker (live passes), and [`open`], which decides whether a render gets one.
//!
//! # When a render has a remote lane
//!
//! The ordinary export's checks apply (`lanes::resolve_remote_capability`: a worker is configured
//! and reachable, it can render, it takes color zones and oriented inclusions if the stone has
//! them, the frame fits its pixel cap). This loop adds one check: **a scene with an HDR
//! environment never goes remote**, because a live pass is not served for one. `Both` then
//! renders on this computer with a note saying why. `RemoteOnly` fails with the same sentence.
//!
//! # What a lane is
//!
//! One [`RemoteEngine`] per remote worker, in a fixed order, all behind one trait so the pass
//! loop and its tests do not care whether the other side is a socket or a function. A render
//! currently has at most one, since the export names one worker. The loop and
//! [`super::assignment`] handle any number.

use super::super::{
    super::types::{AccumulationCarry, AccumulationOutcome},
    lanes::{RenderJob, resolve_remote_capability},
};
use crate::bridge::export_thread::{
    params::ComputeTarget,
    remote::{LivePassClient, LivePassFailure, LiveStream, scene_state_from_snapshot},
};
use glam::Vec3;
use pleochroma::renderer::adaptive::LiveTiles;
use pleochroma_net::SceneState;
use std::sync::atomic::AtomicBool;

/// Message for `RemoteOnly` without a configured worker, saying where the render must run.
const NO_WORKER: &str = "Adaptive sampling with Remote Only needs a remote worker; without one it \
                         renders on this computer only: choose Local Only or set a worker up.";

/// One remote worker as the pass loop sees it: it traces a chunk of a pass.
pub(super) trait RemoteEngine: Send {
    /// Traces the samples `samples.0..samples.0 + samples.1` of `scene` for the pixels of `live`
    /// and returns `(pass total, odd-sample total)` for each, in live-pixel order.
    ///
    /// # Errors
    ///
    /// Why the worker could not trace them; the loop then traces the tiles itself.
    fn trace(
        &mut self,
        scene: &SceneState,
        samples: (u32, u32),
        live: &LiveTiles,
        cancel: &AtomicBool,
    ) -> Result<Vec<(Vec3, Vec3)>, LivePassFailure>;
}

impl<S: LiveStream + Send> RemoteEngine for LivePassClient<S> {
    fn trace(
        &mut self,
        scene: &SceneState,
        samples: (u32, u32),
        live: &LiveTiles,
        cancel: &AtomicBool,
    ) -> Result<Vec<(Vec3, Vec3)>, LivePassFailure> {
        Self::trace(self, scene, samples, live, cancel)
    }
}

/// The remote engines of one adaptive render, and what went with opening them.
pub(super) struct RemoteLane<'e> {
    /// The remote workers, in assignment order (after this computer, if it takes part).
    pub(super) engines: Vec<Box<dyn RemoteEngine + 'e>>,
    /// The scene as the wire carries it; `None` exactly when there are no engines.
    pub(super) scene: Option<SceneState>,
    /// Said once, before the first pass: why there is no remote lane although one was asked for.
    pub(super) note: Option<String>,
    /// Whether this computer sits out (`RemoteOnly`): a remote failure then ends the render.
    pub(super) remote_only: bool,
}

impl RemoteLane<'_> {
    /// No remote lane: every tile is traced on this computer.
    #[must_use]
    pub(super) const fn local_only() -> Self {
        Self {
            engines: Vec::new(),
            scene: None,
            note: None,
            remote_only: false,
        }
    }

    /// Whether this computer traces tiles: always, except for `RemoteOnly` with an engine to
    /// take them.
    #[must_use]
    pub(super) const fn local_participates(&self) -> bool {
        !self.remote_only || self.engines.is_empty()
    }
}

/// Opens the remote lane `job` asks for, or explains why it has none.
///
/// With `path_cache` every pass the worker is given asks it to shade the stone from a path cache
/// of its own (it builds and keeps the cache of its tiles; none travels).
///
/// # Errors
///
/// `Failed` only for `RemoteOnly`, which has no local fallback.
pub(super) fn open(
    job: &RenderJob<'_>,
    path_cache: bool,
) -> Result<RemoteLane<'static>, AccumulationOutcome> {
    if !job.compute_target.uses_remote(job.worker.is_some()) {
        return Ok(RemoteLane::local_only());
    }
    let remote_only = matches!(job.compute_target, ComputeTarget::RemoteOnly);
    if job.worker.is_none() {
        // Only `RemoteOnly` gets here: `Both` without a worker uses no remote lane at all.
        return Err(AccumulationOutcome::Failed(NO_WORKER.to_owned()));
    }
    let without_remote = |why: String| {
        if remote_only {
            Err(AccumulationOutcome::Failed(format!(
                "{why} Remote Only cannot render this scene; choose Local Only or Local + Remote."
            )))
        } else {
            Ok(RemoteLane {
                note: Some(format!("{why} Rendering on this computer only.")),
                ..RemoteLane::local_only()
            })
        }
    };
    if job.scene.env_map.is_some() {
        return without_remote(
            "A remote worker does not trace an adaptive render of a scene with an HDR \
             environment."
                .to_owned(),
        );
    }
    // The ordinary export's verdict, on a carry of its own: this loop caches nothing between
    // frames.
    let mut pending_note = None;
    let capability = resolve_remote_capability(
        &mut AccumulationCarry::default(),
        job.compute_target,
        job.worker,
        job.scene,
        job.width,
        job.height,
        &mut pending_note,
    )
    .map_err(AccumulationOutcome::Failed)?;
    let Some(capability) = capability else {
        return Ok(RemoteLane {
            note: pending_note,
            ..RemoteLane::local_only()
        });
    };
    match LivePassClient::connect(&capability) {
        Ok(client) => Ok(RemoteLane {
            engines: vec![Box::new(client.with_path_cache(path_cache))],
            scene: Some(scene_state_from_snapshot(
                job.scene,
                job.width,
                job.height,
                job.cam_yaw,
                job.cam_pitch,
            )),
            note: None,
            remote_only,
        }),
        Err(why) => without_remote(format!("The remote worker could not be reached ({why}).")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_lane_without_engines_keeps_this_computer_in_whatever_the_target() {
        let mut lane = RemoteLane::local_only();
        assert!(lane.local_participates());
        lane.remote_only = true;
        assert!(
            lane.local_participates(),
            "nothing else could trace the tiles"
        );
    }
}
