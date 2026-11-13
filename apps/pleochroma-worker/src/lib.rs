//! `pleochroma-worker`: a library server for a `pleochroma`-backed gemstone design catalogue,
//! with optional render capacity on top.
//!
//! **Default role: library server.** Running `serve` with no extra features accepts
//! `pleochroma-net`'s read-only design-library protocol (see [`serve::library`]) over
//! mutual TLS: listing and searching designs, fetching one in full, fetching an
//! attachment's bytes. Clients such as the mobile apps speak this protocol and have no
//! renderer compiled in.
//!
//! **`worker` feature (off by default): render capacity.** Gates `RenderRequest`
//! handling, `stream_emit`, `render_core`, `render_cmd`, the `render` subcommand, and
//! `Backend` advertisement in `WELCOME`. See `Cargo.toml` for what it turns on, and
//! [`serve`]'s module docs for how one connection dispatches between the library and
//! render protocols.
//!
//! - `render` (only meaningful with `worker` on): trace a scene straight to a PNG, no
//!   networking. See [`render_cmd::run`].
//! - `serve`: the coordinator. Serves the library protocol to viewers (always); under
//!   `worker` also accepts `join`ing workers on a separate worker port ([`coordinator`]).
//!   With `--render` it renders `RenderRequest`s with its own CPU/GPU. See [`serve::run`].
//! - `join` (`worker`): dial a coordinator's worker port and serve its render requests
//!   over that outbound connection. See [`join::run`].
//!
//! # GPU
//!
//! An optional `gpu` feature (off by default, implies `worker`) routes tracing through
//! `pleochroma`'s GPU megakernel instead of `optics::raytracer::trace_spectral_ray`. The
//! equivalence harness in `crates/pleochroma/examples/gpu_equivalence_harness/` checks it
//! against a real adapter.
//!
//! [`pleochroma::renderer::gpu_backend::GpuBackend`] is the decline and fallback wrapper
//! this crate and `apps/pleochroma-cut` both drive. It lives in `pleochroma` so that the
//! correctness policy (the HDR-environment decline) is not duplicated and cannot drift.

// Trait-solver resource limit, not a lint: under `--features gpu`, proving the
// `thread::spawn` closure in `serve`'s accept loop is `Send` recurses through `wgpu`'s
// internal types deeper than the default limit. Crate-wide, since it is a no-op without
// `wgpu`. Do not remove: the overflow reproduces on every `--features gpu` build.
#![recursion_limit = "256"]

#[cfg(feature = "worker")]
pub mod assets;
pub mod cli;
#[cfg(feature = "worker")]
pub mod coordinator;
pub mod enroll;
pub mod enroll_client;
#[cfg(feature = "worker")]
pub mod jobs;
#[cfg(feature = "worker")]
pub mod join;
pub mod pki;
#[cfg(feature = "worker")]
pub mod png_out;
pub mod radv_workaround;
#[cfg(feature = "worker")]
pub mod render_cmd;
#[cfg(feature = "worker")]
pub mod render_core;
pub mod serve;
#[cfg(feature = "worker")]
mod stream_emit;
#[cfg(feature = "worker")]
pub mod validate;
