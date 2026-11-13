//! Which estimator the spectral path tracer runs at a partial Fresnel event.
//!
//! [`Estimator::Branching`] draws one branch per partial Fresnel event: at each event the path
//! draws reflect or transmit with the hero's clamped reflectance and carries on down the drawn
//! branch. [`Estimator::ExitCollection`] stops drawing: at every event it adds the
//! transmitted leaf to the pixel at once (an environment lookup, for a convex stone it can never
//! re-enter) and continues along the reflection with its physical weight. The coin flip, which
//! carries most of the variance of a gem sample, is gone.
//!
//! The switch is a per-material setting,
//! [`crate::optics::raytracer::transport::PreparedMaterial::with_estimator`], plus a process
//! default, [`set_default_estimator`], that `PreparedMaterial::new` reads once per frame.
//! Off means identical: with [`Estimator::Branching`] every trace is bit-identical to a build
//! without the switch.
//!
//! The library's process default stays [`Estimator::Branching`], so every golden, pin, twin and
//! harness keeps its values. The applications render with exit collection
//! ([`Estimator::APP_DEFAULT`], set with [`set_default_estimator`] at start-up). The command line
//! has no estimator flag: it uses exit collection, except for a run with a path cache or beam
//! flag, which uses branching. Branching is also the per-frame fallback where collection does
//! not apply.
//!
//! Only the prepared entry point (`trace_spectral_ray_prepared`) honours the switch; every other
//! entry point runs [`Estimator::Branching`]. Exit collection also needs the following, and a
//! trace that lacks any of them runs [`Estimator::Branching`] whatever was asked:
//!
//! - next-event estimation is off (a studio rig or the daylight dome; an HDR map and the
//!   Daylight + Sun rig turn it on);
//! - the material does not fluoresce;
//! - the surface scene is ideal (no ground or rough facet, no abraded edge);
//! - there is no Tray Reflection (a collected leaf cannot follow a floor bounce);
//! - the stone is convex (no tools), on every engine: a stone with tools renders with
//!   [`Estimator::Branching`] on the CPU and on the GPU alike, so a hybrid frame never mixes the
//!   two estimators.
//!
//! The CPU tracer, the GPU megakernel (the staged kernel and the single kernel, each with its
//! live, table and tray-shadow variants), the event wavefront and
//! the sorted wavefront implement it; `GpuTransportParams::with_estimator` in `renderer::buffers`
//! holds the GPU's side of the rule. The bounce wavefront, the path cache, the beam tracer and
//! the reference tracer always run [`Estimator::Branching`].

use std::sync::atomic::{AtomicBool, Ordering};

/// Which estimator a prepared trace uses at a partial Fresnel event.
///
/// The variant order is part of the wire format (postcard writes the variant index as a
/// varint): never reorder or insert before the last variant.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum Estimator {
    /// One branch per partial Fresnel event, chosen with the hero's clamped reflectance. The
    /// library default; every trace is identical to a build without the switch.
    #[default]
    Branching,
    /// The transmitted leaf is collected at every event and the path continues along the
    /// reflection, undivided. Falls back to [`Self::Branching`] where the module doc says so.
    ExitCollection,
}

impl Estimator {
    /// The estimator the applications start with. The library's own process default
    /// ([`default_estimator`]) stays [`Self::Branching`]; an application passes this to
    /// [`set_default_estimator`] at start-up.
    pub const APP_DEFAULT: Self = Self::ExitCollection;

    /// The word a job token and the messages use for this estimator: `"branching"` or
    /// `"collect"`.
    #[must_use]
    pub const fn cli_word(self) -> &'static str {
        match self {
            Self::Branching => "branching",
            Self::ExitCollection => "collect",
        }
    }
}

/// The process default [`PreparedMaterial::new`] reads; `false` is [`Estimator::Branching`].
///
/// [`PreparedMaterial::new`]: crate::optics::raytracer::transport::PreparedMaterial::new
static DEFAULT_COLLECTION: AtomicBool = AtomicBool::new(false);

/// Sets the process default estimator.
///
/// Call it once at start-up, before any frame is prepared: a [`PreparedMaterial`] reads the
/// default when it is built, so a change affects only frames prepared afterwards. The
/// applications apply [`Estimator::APP_DEFAULT`] here at start-up; the command line applies its
/// estimator per run (exit collection, or branching for a path cache or beam run).
///
/// [`PreparedMaterial`]: crate::optics::raytracer::transport::PreparedMaterial
pub fn set_default_estimator(mode: Estimator) {
    DEFAULT_COLLECTION.store(mode == Estimator::ExitCollection, Ordering::Relaxed);
}

/// The process default set by [`set_default_estimator`] ([`Estimator::Branching`] initially).
#[must_use]
pub fn default_estimator() -> Estimator {
    if DEFAULT_COLLECTION.load(Ordering::Relaxed) {
        Estimator::ExitCollection
    } else {
        Estimator::Branching
    }
}
