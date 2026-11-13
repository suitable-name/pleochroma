pub mod accel;
// Beam-tracing settings and refusal reasons: compiled in every build (not behind the `beam`
// feature), so jobs, settings and protocol messages that carry them read everywhere.
pub mod accel_beam;
pub mod adaptive;
/// Per-pixel attribution of the spectral renderer's own samples.
///
/// See the module's own doc comment.
pub mod attribution_map;
/// Beam tracing of faceted stones, compiled only with the `beam` feature.
///
/// See the module's own doc comment.
#[cfg(feature = "beam")]
pub mod beam;
pub mod buffers;
/// Wasm-safe, thread-free per-pixel CPU tracer.
///
/// See the module's own doc comment.
pub mod cpu_frame;
pub mod denoise;
pub mod engine;
pub mod frame_denoise;
/// The backend-independent frame-scene bundle.
///
/// See the module's own doc comment.
pub mod frame_scene;
pub mod guide_pass;
pub mod optical_overlay;
/// The attribution map as an overlay layer, in the analytical overlay's palette.
///
/// See the module's own doc comment.
pub mod optical_overlay_spectral;
pub mod path_cache;
// CPU statistics helpers shared by the path-cache and beam tests.
#[cfg(test)]
pub mod test_stats;
pub mod tonemap;
pub mod whitted;

// Parses and validates every WGSL shader with `naga`, so a shader error fails `cargo test`
// on any machine. Declared outside the `gpu` gate so plain `cargo test` runs it. The files
// stay under `gpu/` because their `include_str!` paths are relative to the file.
#[cfg(test)]
#[path = "gpu/shader_validation_sorted_tests.rs"]
mod shader_validation_sorted_tests;
#[cfg(test)]
#[path = "gpu/shader_validation_tests.rs"]
mod shader_validation_tests;
#[cfg(test)]
#[path = "gpu/shader_validation_whitted_tests.rs"]
mod shader_validation_whitted_tests;

/// CPU-side HDR environment-map loading and importance sampling.
///
/// Always available (no `gpu` feature required); see module docs for the `hdr`
/// feature gating actual file decoding.
pub mod env_map;

#[cfg(feature = "gpu")]
pub mod env_map_gpu;

// GPU compute infrastructure and its mandatory self-tests; see that module's doc comment.
#[cfg(feature = "gpu")]
pub mod gpu;

// Not gated behind `feature = "gpu"`: this decline-and-fall-back wrapper exists in both
// configurations so callers' render loops need no `#[cfg]` of their own.
pub mod gpu_backend;

// The RADV scheduler workaround shared by every GPU-running binary (start-up environment and
// recorded adapter identity). Compiled in every build.
pub mod gpu_driver;
