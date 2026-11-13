//! The preview-then-handoff orchestrator, driven by a repeating `slint::Timer`.
//!
//! The timer polls `RenderContext`'s camera/light pose, feeds
//! `bridge::remote::handoff::HandoffMachine`, and dispatches to
//! `bridge::remote::remote_render` when the machine decides to hand off --
//! including the async guide-buffer and denoise generations that keep the (multi-second
//! at 4K) denoise pass off the Slint UI thread.
//!
//! `poll_tick` (in [`tick`]) is also the one place that decides "is the camera
//! currently moving" for `bridge::render_thread::local_preview`. It writes
//! `RenderContext::camera_moving` from this same `HandoffMachine` state every tick, so both
//! features share one definition of "settled".
//!
//! Every tick also checks whether the live remote lane may transmit
//! (`tick::poll::live_remote_allowed`: tab visible, not paused, no export or batch job
//! running). It suspends the lane as soon as that turns false
//! (`tick::poll::suspend_live_remote`), independently of the settle and redispatch
//! decisions. See [`tick`] for the suspend and resume mechanics.
//!
//! Submodules: [`generation`] runs the two background passes, [`compositing`] turns a remote
//! accumulator's sum into a denoised image, and [`tick`] holds the state and the timer tick
//! that drives both. `tick` depends on `generation`, which depends on `compositing`.

mod compositing;
mod generation;
mod tick;

pub use tick::{RemoteOrchestratorHandle, setup_remote_rendering};
