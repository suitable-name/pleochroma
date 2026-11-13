//! Beam-tracing settings and the reasons a stone is not beam traced (beam guide §14.2, §12.5).
//!
//! This module is compiled in every build, with or without the `beam` cargo feature, so render
//! jobs, settings and protocol messages that carry beam settings read and write everywhere. Beam
//! tracing is retired: the tracer (`renderer::beam`, feature `beam`) stays in the code,
//! off in every default build. Without the feature a render that asks for beams falls back with
//! [`BeamOff::Unavailable`]. The tracer re-exports these types.

use std::fmt;

use super::path_cache::eligibility::PathCacheOff;

/// Default retention cap of the leaf cache in exports: 8 GiB (beam guide §14.10).
pub const EXPORT_LEAF_CACHE_BYTES: u64 = 8 << 30;
/// Default retention cap of the leaf cache in the viewport: 4 GiB (beam guide §14.9).
pub const VIEWPORT_LEAF_CACHE_BYTES: u64 = 4 << 30;
/// Default memory cap per batch side (CPU and GPU each): 2 GiB (beam guide §14.7).
pub const DEFAULT_BEAM_MEMORY_CAP_BYTES: u64 = 2 << 30;
/// The residual share above which a beam render is abandoned for the plain render.
///
/// The share is the residual samples over `stone pixels x samples per pixel` of the first stone
/// batches: beyond it the residual alone costs more than half the plain render, and the tree and
/// fragments come on top.
pub const BEAM_RESIDUAL_SHARE_LIMIT: f64 = 0.5;
/// How many finished batches holding kept regions the rule reads before it decides.
pub const BEAM_DECLINE_AFTER_STONE_BATCHES: usize = 1;
/// The rule is armed only when the cap leaves room to save: `max_spp >= this x residual_min_spp`.
/// A test fixture of 16 samples at a minimum of 8 never trips it.
pub const BEAM_DECLINE_MIN_CAP_RATIO: u32 = 8;
/// Plain-render samples one unit of attempted tree work may replace, for a tile's work budget.
///
/// Measured at 32 bounces on an A100: a unit costs 0.6 to 1.1 ms of one thread, and the GPU
/// traces a stone sample in about 0.11 microseconds. With eleven tree threads a unit is worth
/// 500 to 900 samples of wall time. At 2048 the tree of a tile that spends its whole budget
/// costs a quarter to a half of the tile's plain render.
pub const BEAM_SAMPLES_PER_WORK_UNIT: u64 = 2048;
/// The smallest work budget of a tile, whatever the sample count: small test frames and previews
/// keep trees of a few thousand units whole.
pub const BEAM_MIN_TILE_BUDGET: u64 = 4096;
/// How many stone tiles (tiles with a primary beam), in tile order, the tree rule reads.
pub const BEAM_TREE_PROBE_TILES: u32 = 8;
/// The time the tree probe may take per wave of probe tiles at the smallest tile budget, in
/// milliseconds, when no engine of the render works on this computer's CPU.
///
/// A work unit is not a time unit: a tile that spends 4100 units takes between 0.5 and 6.0 s,
/// depending on the stone. The verdict is certain after the fourth
/// spent tile or the fifth unspent one; a probe that needs longer than this is not worth waiting
/// for when the plain render runs on a GPU, so it declines (a stone near the limit may therefore
/// render with beams on one machine and without on another; both pictures are valid). The bound
/// grows with the budget and with the waves the probe tiles need on fewer threads
/// (`beam::pipeline::probe_time_bound`).
pub const BEAM_TREE_PROBE_MILLIS: u64 = 4000;

/// Beam tracing options; `None` in `RenderAccel` means off (beam guide §14.2).
///
/// A field missing from a stored job takes its value from [`BeamSettings::default`], so jobs
/// written by a later build still load.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(default)
)]
pub struct BeamSettings {
    /// `τ_dir`: the apex-model distortion cap in radians (efficiency only, guide §8.3).
    ///
    /// Default 1e-2: at 1e-3 the entry cut every primary beam of a brilliant into about 500
    /// pieces of about 0.02 square pixels; at 1e-2 into three or four, with the interpolation
    /// checks (§8.4, §11.2) still bounding the accuracy.
    pub dir_tolerance_rad: f64,
    /// `ε_chord`: the exact-boundary refinement tolerance in pixels (guide §7.4).
    pub chord_tolerance_px: f64,
    /// `τ_w`: the weight interpolation tolerance (guide §8.4).
    pub weight_tolerance: f32,
    /// `τ_L`: the lighting smoothness tolerance (guide §11.4).
    pub lighting_tolerance: f32,
    /// Beams whose throughput falls below this are pruned to the residual (guide §12.1).
    pub cutoff: f32,
    /// `N_λ`: spectral nodes, a multiple of 8 (guide §9).
    pub spectral_nodes: u32,
    /// `S_λ`: absorption sub-nodes per node (guide §9.1).
    pub absorption_subnodes: u32,
    /// The deepest beam split before the rest goes to the residual.
    pub max_split_depth: u32,
    /// The deepest fragment subdivision (guide §11.4).
    pub max_fragment_levels: u32,
    /// Whether the residual is sampled (guide §12.3).
    pub residual: ResidualMode,
    /// The minimum residual samples per pixel (guide §12.4).
    pub residual_min_spp: u32,
    /// How kept regions are formed across a spectral interval (guide §9.2).
    pub region_mode: RegionMode,
    /// Tiles per batch (guide §12.4, §14.5); the default is 256 (65 536 pixels of 16-pixel tiles).
    ///
    /// A GPU pass over a batch is one round trip, so larger batches mean fewer of them; the memory
    /// cap ([`BeamSettings::memory_cap_bytes`]) still cuts a batch early, so nothing fails.
    pub batch_tiles: u32,
    /// The leaf-cache retention cap in bytes (guide §14.10).
    pub leaf_cache_bytes: u64,
    /// The memory cap per batch side in bytes, CPU and GPU each (guide §14.7).
    pub memory_cap_bytes: u64,
    /// Persist leaves for re-renders ("Keep Beams for Re-Renders", guide §14.10).
    pub keep_leaves: bool,
}

impl Default for BeamSettings {
    fn default() -> Self {
        Self {
            dir_tolerance_rad: 1e-2,
            chord_tolerance_px: 1e-3,
            weight_tolerance: 1e-3,
            lighting_tolerance: 1e-3,
            cutoff: 1e-3,
            spectral_nodes: 48,
            absorption_subnodes: 4,
            max_split_depth: 24,
            max_fragment_levels: 6,
            residual: ResidualMode::PathSamples,
            residual_min_spp: 64,
            region_mode: RegionMode::EndIntersection,
            batch_tiles: 256,
            leaf_cache_bytes: EXPORT_LEAF_CACHE_BYTES,
            memory_cap_bytes: DEFAULT_BEAM_MEMORY_CAP_BYTES,
            keep_leaves: false,
        }
    }
}

/// The fewest spectral nodes: mirrors `beam::leaf::MIN_NODES` (behind the `beam` feature).
pub const MIN_SPECTRAL_NODES: u32 = 8;
/// The most spectral nodes: mirrors `beam::leaf::MAX_NODES`, which is behind the `beam` feature.
pub const MAX_SPECTRAL_NODES: u32 = 1024;
/// The most absorption sub-nodes per node: mirrors `beam::subnodes::MAX_SUB_NODES`.
pub const MAX_ABSORPTION_SUBNODES: u32 = 16;
/// The deepest beam split a request may ask for (the default is 24).
pub const MAX_SPLIT_DEPTH_LIMIT: u32 = 32;
/// The deepest fragment subdivision either engine honors: mirrors the GPU clamp
/// `gpu::beam::fragments::MAX_REFINE_LEVELS`, so the CPU and the GPU agree.
pub const MAX_FRAGMENT_LEVELS_LIMIT: u32 = 6;
/// The most tiles per batch a request may ask for.
pub const MAX_BATCH_TILES: u32 = 4096;
/// The most residual samples per pixel a request may ask for as a minimum.
pub const MAX_RESIDUAL_MIN_SPP: u32 = 1 << 16;

/// The smallest accepted `chord_tolerance_px`.
///
/// The snap search halves it for its first half-width, doubles that to expand, and bisects down to
/// `Tolerances::bisect_px` (1e-9 pixels); a chord tolerance below the bisection stop asks for a
/// finer curve than the root finder resolves. The bound is far above the subnormal range, where
/// halving rounds to zero.
pub const MIN_CHORD_TOLERANCE_PX: f64 = 1e-9;
/// The smallest accepted `dir_tolerance_rad`.
///
/// The apex-model distortion is compared against it as an angle in radians; below a nanoradian the
/// comparison is decided by rounding noise of unit-length directions, so no model could pass.
pub const MIN_DIR_TOLERANCE_RAD: f64 = 1e-9;
/// The smallest accepted `weight_tolerance` and `lighting_tolerance`.
///
/// Both are stored as `f32` and compared against `f32` interpolation errors; below the `f32`
/// machine epsilon (about 1.2e-7) no interpolation can pass, so every leaf would subdivide to the
/// depth limit.
pub const MIN_F32_TOLERANCE: f64 = 1e-7;

/// Which [`BeamSettings`] field is outside its accepted range.
///
/// Not sent on the wire: a worker turns it into a refusal message.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BeamSettingsError {
    /// `spectral_nodes` is not a multiple of 8 in [`MIN_SPECTRAL_NODES`]..=[`MAX_SPECTRAL_NODES`].
    SpectralNodes(u32),
    /// The named tolerance is not finite and positive.
    NotPositive(&'static str),
    /// The named tolerance is positive but below its smallest usable value.
    TooSmall(&'static str),
    /// `cutoff` is not finite or not within `0..=1`.
    Cutoff,
    /// The named count is zero.
    Zero(&'static str),
    /// The named count is above its limit.
    TooLarge {
        /// The field name.
        field: &'static str,
        /// The largest accepted value.
        max: u32,
    },
}

impl fmt::Display for BeamSettingsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SpectralNodes(n) => write!(
                f,
                "spectral_nodes must be a multiple of 8 from {MIN_SPECTRAL_NODES} to \
                 {MAX_SPECTRAL_NODES} (got {n})"
            ),
            Self::NotPositive(field) => write!(f, "{field} must be finite and positive"),
            Self::TooSmall(field) => write!(
                f,
                "{field} is below the smallest usable value (see MIN_CHORD_TOLERANCE_PX, \
                 MIN_DIR_TOLERANCE_RAD and MIN_F32_TOLERANCE)"
            ),
            Self::Cutoff => write!(f, "cutoff must be finite and within 0 to 1"),
            Self::Zero(field) => write!(f, "{field} must be at least 1"),
            Self::TooLarge { field, max } => write!(f, "{field} must be at most {max}"),
        }
    }
}

impl std::error::Error for BeamSettingsError {}

impl BeamSettings {
    /// Checks every field against the one accepted range shared by the CPU and the GPU engine.
    ///
    /// A worker refuses anything else, so a custom client cannot force
    /// endless subdivision or make the two engines disagree.
    ///
    /// # Errors
    ///
    /// The first [`BeamSettingsError`] found.
    pub fn validate(&self) -> Result<(), BeamSettingsError> {
        let nodes = self.spectral_nodes;
        if !nodes.is_multiple_of(8) || !(MIN_SPECTRAL_NODES..=MAX_SPECTRAL_NODES).contains(&nodes) {
            return Err(BeamSettingsError::SpectralNodes(nodes));
        }
        if self.batch_tiles == 0 {
            return Err(BeamSettingsError::Zero("batch_tiles"));
        }
        let tolerances = [
            (
                "dir_tolerance_rad",
                self.dir_tolerance_rad,
                MIN_DIR_TOLERANCE_RAD,
            ),
            (
                "chord_tolerance_px",
                self.chord_tolerance_px,
                MIN_CHORD_TOLERANCE_PX,
            ),
            (
                "weight_tolerance",
                f64::from(self.weight_tolerance),
                MIN_F32_TOLERANCE,
            ),
            (
                "lighting_tolerance",
                f64::from(self.lighting_tolerance),
                MIN_F32_TOLERANCE,
            ),
        ];
        for (name, value, min) in tolerances {
            if !(value.is_finite() && value > 0.0) {
                return Err(BeamSettingsError::NotPositive(name));
            }
            if value < min {
                return Err(BeamSettingsError::TooSmall(name));
            }
        }
        if !(0.0..=1.0).contains(&self.cutoff) {
            return Err(BeamSettingsError::Cutoff);
        }
        if self.absorption_subnodes == 0 {
            return Err(BeamSettingsError::Zero("absorption_subnodes"));
        }
        if self.residual_min_spp == 0 {
            return Err(BeamSettingsError::Zero("residual_min_spp"));
        }
        let limits = [
            (
                "absorption_subnodes",
                self.absorption_subnodes,
                MAX_ABSORPTION_SUBNODES,
            ),
            (
                "max_split_depth",
                self.max_split_depth,
                MAX_SPLIT_DEPTH_LIMIT,
            ),
            (
                "max_fragment_levels",
                self.max_fragment_levels,
                MAX_FRAGMENT_LEVELS_LIMIT,
            ),
            ("batch_tiles", self.batch_tiles, MAX_BATCH_TILES),
            (
                "residual_min_spp",
                self.residual_min_spp,
                MAX_RESIDUAL_MIN_SPP,
            ),
        ];
        for (field, value, max) in limits {
            if value > max {
                return Err(BeamSettingsError::TooLarge { field, max });
            }
        }
        Ok(())
    }
}

/// The phase a beam render is in, for the two-phase progress (§12.4).
///
/// Here rather than in `beam` (which re-exports it) so the render-job progress of crates built
/// without the `beam` feature can carry it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BeamPhase {
    /// Tracing the beam trees of the batches.
    Tracing,
    /// Shading and running the residual ("finishing").
    Finishing,
}

impl BeamPhase {
    /// The phase as the render-job detail line shows it.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Tracing => "tracing beams",
            Self::Finishing => "finishing",
        }
    }
}

/// Whether the residual (everything the beams leave out) is sampled.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "snake_case")
)]
pub enum ResidualMode {
    /// Path samples with the reference tracer and the `kept` test (the default).
    #[default]
    PathSamples,
    /// No residual: tests only.
    Off,
}

/// How a kept region is formed across a spectral interval (guide §9.2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "snake_case")
)]
pub enum RegionMode {
    /// One region per interval: the intersection of the end nodes' regions (the default).
    #[default]
    EndIntersection,
    /// One region per node.
    PerNode,
}

/// Why a stone or a render is not beam traced (guide §12.5).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BeamOff {
    /// Beams retain escape directions but cannot locate their landings on the paper.
    StoneShadow,
    /// The tray reflection continues an escaping path along a floor bounce, which beams do not
    /// follow.
    TrayReflection,
    /// The stone fails a path-cache eligibility rule.
    NotEligible(PathCacheOff),
    /// The material rounds its edges.
    EdgeRounding,
    /// The stone is concave (tools present).
    Concave,
    /// The facet planes do not bound a closed convex solid the beam tracer can use.
    DegenerateStone,
    /// The lighting is outside beam tracing's scope; falls back silently.
    OutOfScopeLighting,
    /// This build has no beam tracer (the `beam` feature is off).
    Unavailable,
    /// The Ray-Traced engine has no residual; beams there only re-light (guide §14.10).
    RayTracedResidual,
    /// The residual of the first stone batches needed more than [`BEAM_RESIDUAL_SHARE_LIMIT`] of the
    /// plain render's samples: the beams save nothing, the plain render is faster.
    ResidualTooLarge,
    /// At least half of the first [`BEAM_TREE_PROBE_TILES`] stone tiles spent their work budget
    /// before their tree was finished, or the probe ran out of time ([`BEAM_TREE_PROBE_MILLIS`]):
    /// the tree costs more than the samples it would replace.
    TreeTooLarge,
}

impl BeamOff {
    /// The sentence the log line and the interface show; empty for a silent fallback.
    #[must_use]
    pub const fn message(self) -> &'static str {
        match self {
            Self::StoneShadow => "Beam tracing off: the stone shadow is traced without beams.",
            Self::TrayReflection => {
                "Beam tracing off: the tray reflection is traced without beams."
            }
            Self::NotEligible(off) => match off {
                PathCacheOff::ScatteringMedium => "Beam tracing off: the material scatters light",
                PathCacheOff::OrientedInclusions => {
                    "Beam tracing off: the stone has oriented inclusions"
                }
                PathCacheOff::MaterialLook => {
                    "Beam tracing off: the material has a play-of-color look"
                }
                PathCacheOff::FrostedFacets => "Beam tracing off: the stone has frosted facets",
                PathCacheOff::RoughPolish => "Beam tracing off: the polish has roughness",
                PathCacheOff::AbradedEdges => "Beam tracing off: the stone has abraded edges",
                PathCacheOff::ConcaveTiers => "Beam tracing off: the stone has concave tiers",
                PathCacheOff::DepthOfField => "Beam tracing off: the camera has depth of field",
                PathCacheOff::StoneShadow => Self::StoneShadow.message(),
                PathCacheOff::TrayReflection => Self::TrayReflection.message(),
                PathCacheOff::Fluorescence | PathCacheOff::ZonedColor => "",
            },
            Self::EdgeRounding => "Beam tracing off: the material rounds its edges",
            Self::Concave => "Beam tracing off: the stone is concave",
            Self::DegenerateStone => "Beam tracing off: the facets do not close the stone",
            Self::OutOfScopeLighting => "",
            Self::Unavailable => "Beam tracing off: this build has no beam tracer",
            Self::RayTracedResidual => "Beam tracing needs the Path-Traced engine",
            Self::ResidualTooLarge => {
                "Beam tracing off: the residual needed most of the plain render's samples on the first stone tiles, so the picture renders without beams"
            }
            Self::TreeTooLarge => {
                "Beam tracing off: the beam tree of this stone costs more than the samples it would replace, so the picture renders without beams"
            }
        }
    }

    /// `true` when the fallback shows no note.
    #[must_use]
    pub const fn is_silent(self) -> bool {
        matches!(
            self,
            Self::OutOfScopeLighting
                | Self::NotEligible(PathCacheOff::Fluorescence | PathCacheOff::ZonedColor)
        )
    }
}

impl fmt::Display for BeamOff {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}

/// Declines that need no beam tracer: they compile and run in every build.
#[cfg(test)]
mod tests_declines {
    use super::*;

    #[test]
    fn the_tray_reflection_decline_is_named_and_not_silent() {
        assert!(!BeamOff::TrayReflection.is_silent());
        assert_eq!(
            BeamOff::TrayReflection.message(),
            "Beam tracing off: the tray reflection is traced without beams."
        );
        assert_eq!(
            BeamOff::NotEligible(PathCacheOff::TrayReflection).message(),
            BeamOff::TrayReflection.message()
        );
        assert_eq!(
            PathCacheOff::TrayReflection.message(),
            "Path cache off: the tray reflection needs paths the cache does not keep."
        );
    }
}

#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests {
    use super::*;

    #[test]
    fn the_default_settings_are_accepted() {
        assert_eq!(BeamSettings::default().validate(), Ok(()));
    }

    #[test]
    fn the_residual_decline_is_not_silent() {
        assert!(!BeamOff::ResidualTooLarge.is_silent());
        assert_ne!(BeamOff::ResidualTooLarge.message(), "");
        assert!(!BeamOff::TreeTooLarge.is_silent());
        assert_ne!(BeamOff::TreeTooLarge.message(), "");
    }

    #[test]
    fn out_of_range_settings_are_refused_with_the_field() {
        let ok = BeamSettings::default();
        for nodes in [0, 4, 12, 1032, 2048] {
            let bad = BeamSettings {
                spectral_nodes: nodes,
                ..ok
            };
            assert_eq!(bad.validate(), Err(BeamSettingsError::SpectralNodes(nodes)));
        }
        let negative = BeamSettings {
            lighting_tolerance: -1e-3,
            ..ok
        };
        assert_eq!(
            negative.validate(),
            Err(BeamSettingsError::NotPositive("lighting_tolerance"))
        );
        let nan = BeamSettings {
            dir_tolerance_rad: f64::NAN,
            ..ok
        };
        assert_eq!(
            nan.validate(),
            Err(BeamSettingsError::NotPositive("dir_tolerance_rad"))
        );
        // A subnormal chord tolerance is refused at admission.
        for tiny in [f64::from_bits(1), f64::MIN_POSITIVE, 1e-12] {
            let bad = BeamSettings {
                chord_tolerance_px: tiny,
                ..ok
            };
            assert_eq!(
                bad.validate(),
                Err(BeamSettingsError::TooSmall("chord_tolerance_px"))
            );
        }
        let zero = BeamSettings {
            chord_tolerance_px: 0.0,
            ..ok
        };
        assert_eq!(
            zero.validate(),
            Err(BeamSettingsError::NotPositive("chord_tolerance_px"))
        );
        let tiny_weight = BeamSettings {
            weight_tolerance: f32::from_bits(1),
            ..ok
        };
        assert_eq!(
            tiny_weight.validate(),
            Err(BeamSettingsError::TooSmall("weight_tolerance"))
        );
        let cutoff = BeamSettings { cutoff: 1.5, ..ok };
        assert_eq!(cutoff.validate(), Err(BeamSettingsError::Cutoff));
        let deep = BeamSettings {
            max_fragment_levels: 7,
            ..ok
        };
        assert_eq!(
            deep.validate(),
            Err(BeamSettingsError::TooLarge {
                field: "max_fragment_levels",
                max: MAX_FRAGMENT_LEVELS_LIMIT
            })
        );
    }
}
