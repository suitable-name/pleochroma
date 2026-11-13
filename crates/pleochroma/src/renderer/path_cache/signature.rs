//! The branch signature: two 32-bit lanes with wrapping `u32` arithmetic, identical in the CPU
//! beam tracer, the CPU reference tracer and the WGSL reference variant (beam guide §14.3).
//!
//! The WGSL twin (`shaders/beam/branch_sig.wgsl`) writes `u32::from(is_ext)` as
//! `select(0u, 1u, is_ext)` and `rotate_left(5)` as `(x << 5u) | (x >> 27u)`; the constants and
//! the operations here must not change. The path cache's 64-bit `mix_sig` is a separate hash.

use super::bounce::EventKind;

/// The signature of a path that has not met a facet yet: the seed of both lanes.
pub const BRANCH_SIG_SEED: [u32; 2] = [0x811C_9DC5, 0x2545_F491];

/// One step of the branch signature: two 32-bit lanes, wrapping `u32` arithmetic only (WGSL twin).
///
/// `facet` is the original facet index, `kind` the merged event kind (a partial reflection and a
/// total internal reflection share one code) and `is_ext` the eigenmode after the event.
#[must_use]
pub fn branch_step(sig: [u32; 2], facet: u32, kind: EventKind, is_ext: bool) -> [u32; 2] {
    let code: u32 = match kind {
        EventKind::Reflect | EventKind::TotalReflect => 1,
        EventKind::Transmit => 3,
    };
    let word = (facet << 8) | (code << 1) | u32::from(is_ext);
    // FNV-1a style lane.
    let a = (sig[0] ^ word).wrapping_mul(0x0100_0193);
    // Multiplicative lane.
    let b = (sig[1].rotate_left(5) ^ word).wrapping_mul(0x9E37_79B1);
    [a, b ^ (b >> 15)]
}

/// Folds the two lanes into the 32-bit word an exit record stores (`ExitRecord::signature`).
///
/// The WGSL twin is `pcc_fold_sig` in `shaders/path_cache/complete.wgsl`.
#[must_use]
pub const fn fold_sig(sig: [u32; 2]) -> u32 {
    sig[0] ^ sig[1].rotate_left(16)
}

/// Packs the two lanes into the 64-bit key the CPU `kept` callback takes: lane 1 high, lane 0 low.
#[must_use]
pub fn pack_sig(sig: [u32; 2]) -> u64 {
    (u64::from(sig[1]) << 32) | u64::from(sig[0])
}
