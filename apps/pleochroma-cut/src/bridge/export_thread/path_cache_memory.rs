//! Free memory as the path cache sees it, for the export thread and the viewport.
//!
//! wgpu reports limits, not free video memory, so VRAM is unknown unless [`VRAM_ENV`] sets it.
//! RAM is `MemAvailable` from `/proc/meminfo` on Linux and [`ASSUMED_RAM_BYTES`] elsewhere.
//! [`VRAM_ENV`] and [`RAM_ENV`] override both, in gigabytes.

use pleochroma::renderer::gpu_backend::MemoryHints;

/// One gibibyte in bytes.
const GIB: u64 = 1 << 30;

/// The RAM assumed when the OS figure is not read (anywhere but Linux) and the environment
/// gives none.
pub const ASSUMED_RAM_BYTES: u64 = 32 * GIB;

/// Environment variable that sets the free video memory the placement may use, in gigabytes.
pub const VRAM_ENV: &str = "PLEOCHROMA_CACHE_VRAM_GB";

/// Environment variable that sets the available RAM the placement may use, in gigabytes.
pub const RAM_ENV: &str = "PLEOCHROMA_CACHE_RAM_GB";

/// Parses a size in gigabytes from the text of an environment variable.
#[must_use]
pub fn gigabytes(text: &str) -> Option<u64> {
    let value = text.trim().parse::<f64>().ok()?;
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "a finite non-negative gigabyte count, saturating at the u64 range"
    )]
    Some((value * GIB as f64) as u64)
}

/// `MemAvailable` of a `/proc/meminfo` text, in bytes.
#[must_use]
pub fn meminfo_available(text: &str) -> Option<u64> {
    let line = text
        .lines()
        .find(|line| line.starts_with("MemAvailable:"))?;
    let kib = line.split_whitespace().nth(1)?.parse::<u64>().ok()?;
    Some(kib.saturating_mul(1024))
}

/// Free memory for the cache placement.
///
/// Without [`VRAM_ENV`] the cache goes to system RAM, from which the GPU streams slabs.
/// Available RAM is [`RAM_ENV`], else `MemAvailable` on Linux, else [`ASSUMED_RAM_BYTES`].
#[must_use]
pub fn memory_hints() -> MemoryHints {
    let vram_free = std::env::var(VRAM_ENV).ok().and_then(|v| gigabytes(&v));
    let ram_available = std::env::var(RAM_ENV)
        .ok()
        .and_then(|v| gigabytes(&v))
        .or_else(|| {
            std::fs::read_to_string("/proc/meminfo")
                .ok()
                .and_then(|text| meminfo_available(&text))
        })
        .or(Some(ASSUMED_RAM_BYTES));
    MemoryHints {
        vram_free,
        ram_available,
    }
}
