//! Shared adapter gate: absence skips locally, `PLEOCHROMA_REQUIRE_GPU=1` is strict.
use super::GpuContext;
use std::sync::OnceLock;

/// The shared GPU context, or `None` (the test skips) when no adapter is available.
///
/// # Panics
///
/// Panics when `PLEOCHROMA_REQUIRE_GPU=1` is set and no adapter can be acquired.
#[must_use]
pub fn require_gpu() -> Option<GpuContext> {
    static CONTEXT: OnceLock<Result<GpuContext, String>> = OnceLock::new();
    match CONTEXT.get_or_init(|| {
        pollster::block_on(super::whitted::context::acquire()).map_err(|error| error.to_string())
    }) {
        Ok(context) => Some(context.clone()),
        Err(error) => {
            assert!(
                !std::env::var("PLEOCHROMA_REQUIRE_GPU").is_ok_and(|value| value == "1"),
                "PLEOCHROMA_REQUIRE_GPU=1, but no GPU is available: {error}"
            );
            eprintln!("GPU tests skipped: {error}");
            None
        }
    }
}
