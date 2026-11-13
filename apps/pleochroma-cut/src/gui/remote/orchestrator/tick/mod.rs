//! The orchestrator's timer tick and remote-render dispatch.
//!
//! [`state`] holds the `Orchestrator`, [`poll`] the timer tick, [`dispatch`] starts a remote
//! render for a settled pose, and [`update`] handles what comes back and redraws. `poll`,
//! `dispatch` and `update` call each other in a cycle: together they form one state machine
//! (`bridge::remote::handoff::HandoffMachine`), not a one-way pipeline.
//!
//! # Suspending the live remote lane
//!
//! Every tick also gates whether the live remote lane may transmit (`poll::live_remote_allowed`:
//! tab visible, not paused, no export or batch job), and reacts as soon as that turns false
//! (`poll::update_live_remote_suspension`, `poll::suspend_live_remote`). A display-only lane
//! is abandoned, since its one request cannot resume mid-stream. An accumulating lane has its
//! in-flight chunk cancelled and stays idle, and its valid prefix is merged on the confirmed
//! cancellation (`update::on_chunk_paused`). `poll::resume_idle_lane` and `poll::maybe_redispatch`
//! (via `decisions::RedispatchInputs::suspended`) start nothing new while this holds, so a
//! scene change while paused never sends a fresh epoch to the worker.

mod decisions;
mod dispatch;
mod poll;
mod progress;
mod state;
mod target;
mod update;

pub use poll::{RemoteOrchestratorHandle, setup_remote_rendering};
