//! Where a path cache lives while it is used: video memory, system RAM, or nowhere.
//!
//! The decision is a pure function over numbers the caller supplies (this module reads no memory
//! sizes itself), so it can be tested without a machine. Disk is never a tier: a cache is loaded
//! into memory before a render. A cache that fits nowhere means the render goes on without it. That
//! is an outcome with a reason, never an error. See guide section 8.7.

use super::estimate::{SizeEstimate, format_bytes};

/// Share of the estimate added as a safety margin: 20 %, as the integer ratio `1 / 5`.
pub const MARGIN_DIVISOR: u64 = 5;

/// System RAM kept free for everything else: 4 GiB.
pub const RAM_HEADROOM_BYTES: u64 = 4 * 1024 * 1024 * 1024;

/// The memory the caller could measure, in bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MemoryBudget {
    /// Free video memory, or `None` when the adapter does not say (or no GPU is used).
    pub vram_free: Option<u64>,
    /// Video memory the render itself needs: the scene and the accumulation buffers.
    pub vram_reserved: u64,
    /// Available physical RAM, or `None` when it could not be read.
    pub ram_available: Option<u64>,
}

impl MemoryBudget {
    /// Video memory the cache may use, with the render's own buffers taken off.
    #[must_use]
    pub fn vram_usable(&self) -> Option<u64> {
        self.vram_free
            .map(|free| free.saturating_sub(self.vram_reserved))
    }

    /// System RAM the cache may use, with the headroom taken off.
    #[must_use]
    pub fn ram_usable(&self) -> Option<u64> {
        self.ram_available
            .map(|available| available.saturating_sub(RAM_HEADROOM_BYTES))
    }
}

/// Why no cache is used.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OffReason {
    /// No stone pixel was found, so there is nothing to cache.
    NothingToCache,
    /// The cache, with its margin, is larger than the biggest memory that is available.
    TooLarge {
        /// Bytes the cache needs including the margin.
        required_bytes: u64,
        /// Bytes available in the larger of the two tiers that could be measured.
        best_available_bytes: u64,
    },
    /// Neither memory size is known, so the fit cannot be judged.
    MemoryUnknown,
    /// A cached sample costs as much as a traced one or more, so the cache cannot pay.
    SlowerThanTracing {
        /// Mean exit records per position (rounded), the figure phase B pays for.
        records_per_position: u32,
    },
}

/// Cost of one exit record in phase B, in path-tracer samples, fitted together with
/// [`COMPLETION_COST`] to measured cached-pass times.
pub const PHASE_B_RECORD_COST: f64 = 0.006;

/// Cost of one fired completion, in path-tracer samples.
pub const COMPLETION_COST: f64 = 1.2;

/// The cost of one cached sample in path-tracer samples.
///
/// It is phase B over the position's records plus the fired share of completions.
#[must_use]
pub fn sample_cost(estimate: &SizeEstimate) -> f64 {
    PHASE_B_RECORD_COST.mul_add(
        estimate.records_per_position,
        COMPLETION_COST * estimate.fire_share,
    )
}

/// The status line of a render that asked for the cache without keeping it.
///
/// The status line of a render that would build a cache for itself alone, which is traced instead.
///
/// A cache pays its build only when it is kept and the same view is rendered again. The line
/// names the dialog's keep box and the command line's flag.
pub const ONE_SHOT_NOTE: &str = "Path cache: not used, it only pays when the cache is kept and the view is rendered again (tick the keep box under Path Cache, or pass --persist-cache)";

/// Where the cache goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Placement {
    /// In video memory next to the scene.
    Vram,
    /// In system RAM, streamed to the GPU tile by tile.
    SystemRam,
    /// Not at all; the render uses the path tracer alone.
    Off(OffReason),
}

impl Placement {
    /// Whether a cache is built and used.
    #[must_use]
    pub const fn is_cached(&self) -> bool {
        !matches!(self, Self::Off(_))
    }
}

/// The estimate with the 20 % safety margin added, rounded up.
#[must_use]
pub const fn with_margin(bytes: u64) -> u64 {
    bytes.saturating_add(bytes.div_ceil(MARGIN_DIVISOR))
}

/// Chooses the first tier the cache fits in, with the safety margin: video memory, then system RAM.
///
/// A cache that needs exactly the usable bytes fits. An estimate of 0 bytes means there is no
/// stone and gives [`OffReason::NothingToCache`]. Never fails.
#[must_use]
pub fn choose_placement(estimate_bytes: u64, budget: &MemoryBudget) -> Placement {
    if estimate_bytes == 0 {
        return Placement::Off(OffReason::NothingToCache);
    }
    let required = with_margin(estimate_bytes);
    let (vram, ram) = (budget.vram_usable(), budget.ram_usable());
    if vram.is_some_and(|usable| required <= usable) {
        return Placement::Vram;
    }
    if ram.is_some_and(|usable| required <= usable) {
        return Placement::SystemRam;
    }
    vram.max(ram).map_or(
        Placement::Off(OffReason::MemoryUnknown),
        |best_available_bytes| {
            Placement::Off(OffReason::TooLarge {
                required_bytes: required,
                best_available_bytes,
            })
        },
    )
}

/// The one-line status for the log and the export dialog, such as
/// "Path cache: about 11 GB, in system RAM".
///
/// A cache whose samples trace a path again says how many, for example
/// "Path cache: about 1.2 GB, 31.0 % of samples traced again, in video memory".
#[must_use]
pub fn status_text(estimate: &SizeEstimate, placement: &Placement) -> String {
    let size = estimate.size_text();
    let fired = fired_text(estimate.fire_share);
    match placement {
        Placement::Vram => format!("Path cache: {size}{fired}, in video memory"),
        Placement::SystemRam => format!("Path cache: {size}{fired}, in system RAM"),
        Placement::Off(OffReason::NothingToCache) => {
            "Path cache: not needed, the stone is not in view".to_owned()
        }
        Placement::Off(OffReason::TooLarge {
            required_bytes,
            best_available_bytes,
        }) => format!(
            "Path cache: {size}, needs {} with a safety margin but only {} is free, rendering without it",
            format_bytes(*required_bytes),
            format_bytes(*best_available_bytes),
        ),
        Placement::Off(OffReason::MemoryUnknown) => {
            format!("Path cache: {size}, free memory is unknown, rendering without it")
        }
        Placement::Off(OffReason::SlowerThanTracing {
            records_per_position,
        }) => format!(
            "Path cache: not used, this stone has about {records_per_position} exit records per position and shades slower from the cache than it traces"
        ),
    }
}

/// The fired share as the status line shows it, ", 31.0 % of samples traced again".
///
/// It is nothing when the share rounds to zero thousandths. The GPU traces those samples itself, so
/// they cost about one path-tracer sample each; the share is reported, not refused.
fn fired_text(fire_share: f64) -> String {
    let milli = u32::try_from(to_milli(fire_share)).unwrap_or(1000);
    if milli == 0 {
        return String::new();
    }
    format!(", {:.1} % of samples traced again", f64::from(milli) / 10.0)
}

/// Decides for an estimate and returns the decision with its status line.
///
/// A stone whose cached sample costs one path-tracer sample or more ([`sample_cost`]) is refused
/// before the memory tiers are looked at. Otherwise the fired share does not turn the cache off:
/// the GPU traces the fired samples itself (`shaders/path_cache/complete.wgsl`), and the status
/// line reports the share.
#[must_use]
pub fn plan(estimate: &SizeEstimate, budget: &MemoryBudget) -> (Placement, String) {
    let placement = if estimate.has_stone() && sample_cost(estimate) >= 1.0 {
        Placement::Off(OffReason::SlowerThanTracing {
            records_per_position: records_rounded(estimate.records_per_position),
        })
    } else {
        choose_placement(estimate.raw_bytes, budget)
    };
    let text = status_text(estimate, &placement);
    (placement, text)
}

/// A count, rounded to a whole number and clamped to the `u32` range.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the value is clamped to the u32 range first"
)]
fn records_rounded(records: f64) -> u32 {
    if records.is_nan() {
        return 0;
    }
    records.round().clamp(0.0, f64::from(u32::MAX)) as u32
}

/// A share as thousandths, rounded and clamped at zero.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "the share is in [0, 1] (clamped), so the thousandths fit easily"
)]
fn to_milli(share: f64) -> u64 {
    (share.clamp(0.0, 1.0) * 1000.0).round() as u64
}
