//! The two-mode (doubling) optics of the metric tracer.
//!
//! A birefringent crystal splits every ray that enters it into two eigenmodes. The renderer
//! follows one of them at random per ray; the metrics are deterministic, so for an anisotropic
//! material ([`is_anisotropic`], the renderer's own predicate) they trace BOTH, each carrying
//! half of the ray's energy:
//!
//! - **Uniaxial** (`Modes::Uniaxial`): the ordinary mode is the plain isotropic trace at
//!   `n_o(lambda)`, bit for bit. The extraordinary mode enters with the effective index
//!   `n_e(theta)` (`theta` the angle between the wave normal `k` and the optic axis `c`, resolved
//!   by a short fixed-point iteration), refracts and reflects with `k`, but TRAVELS along the
//!   Poynting direction `S(k)` (walk-off), so its path and its facet hits follow `S`.
//! - **Biaxial** (`Modes::Biaxial`): there is no ordinary ray; the two modes are the fast and the
//!   slow root of Fresnel's equation at `k` (`BiaxialIndicatrix::wave_indices`), each with its
//!   own Poynting direction (`mode_poynting_dir`).
//!
//! A mode is KEPT at every interior hit (no coupling): TIR is tested with the mode's index at the
//! incident `k`, and the exit Fresnel transmittance uses that index. The metrics are CPU only
//! (no WGSL twin).
//!
//! The mode-aware trace is [`trace_mode`]; an isotropic stone never reaches it (the integer fast
//! path of `evaluate`/`classify` is taken when `StoneArena::modes` is `None`).

use glam::{Mat3, Vec3};

use super::{
    frosted::path_seed,
    image_types::{ExtinctionReason, RayDetail},
    ray_trace::{
        EntryRay, Interior, RayFate, StoneArena, fresnel_transmittance, note_reason, refract_into,
        trace_entry, trace_interior,
    },
};
use crate::optics::{
    birefringence::{BiaxialIndicatrix, BirefringenceParams, assigned_mode_e_field_uniaxial},
    materials::{CrystalSystem, GemMaterial},
};

/// The three spectral lines the metrics trace, in the slot order of the per-line arrays: the
/// d-line (classification), the F-line and the C-line (the Fire pair).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Line {
    D,
    F,
    C,
}

impl Line {
    const fn slot(self) -> usize {
        match self {
            Self::D => 0,
            Self::F => 1,
            Self::C => 2,
        }
    }
}

/// Wavelengths of [`Line`]'s slots, nanometres (the ones `PreparedEval` reads the indices at).
const LINE_NM: [f32; 3] = [589.3, 486.1, 656.3];

/// Which of the two eigenmodes a ray is.
///
/// Uniaxial: the ordinary / extraordinary ray. Biaxial: the fast / the slow root (the
/// convention of `BiaxialIndicatrix::assigned_mode_e_field`: the "extraordinary" slot is the
/// slow one).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Mode {
    Ordinary,
    Extraordinary,
}

impl Mode {
    /// Both modes, in the order the pair results use.
    pub(super) const BOTH: [Self; 2] = [Self::Ordinary, Self::Extraordinary];
}

/// The energy share of each mode of an unpolarised ray (the metrics need no polarisation-
/// weighted selection: the full tracer's stochastic choice averages to the same half).
pub(super) const MODE_WEIGHT: f32 = 0.5;

/// Whether `material` is anisotropic by the renderer's own predicate: not cubic AND
/// `|birefringence_delta| > 1e-4` (`build_ray_material_context`, mirrored by the GPU material
/// encode). Not a threshold of the plan's: the figures follow the renders.
pub(super) fn is_anisotropic(material: &GemMaterial) -> bool {
    material.crystal_system != CrystalSystem::Cubic && material.birefringence_delta.abs() > 1e-4
}

/// The optics of one material the two-mode trace needs, per spectral [`Line`].
#[derive(Debug, Clone, Copy)]
pub(super) enum Modes {
    /// Isotropic (or isotropic-by-symmetry): one ray, the integer fast path.
    Isotropic,
    /// Uniaxial about `c_axis` (unit): the ordinary index, and the extraordinary PRINCIPAL index
    /// (`n_e` at theta = 90 degrees, from `GemMaterial::extraordinary_index_at`, so a material's
    /// own e-curve is honoured).
    Uniaxial {
        n_o: [f32; 3],
        n_e_principal: [f32; 3],
        c_axis: Vec3,
    },
    /// Genuinely biaxial: the full indicatrix at each line.
    Biaxial { indicatrix: [BiaxialIndicatrix; 3] },
}

impl Modes {
    /// The modes of `material`; `n_o` are the already clamped ordinary indices at the d-, F- and
    /// C-line (`PreparedEval`'s `nd`, `n_f`, `n_c`).
    pub(super) fn for_material(material: &GemMaterial, n_o: [f32; 3]) -> Self {
        if !is_anisotropic(material) {
            return Self::Isotropic;
        }
        if material.biaxial_delta_beta_alpha.is_some() {
            let at = |slot: usize| material.biaxial_indicatrix(LINE_NM[slot]);
            if let (Some(d), Some(f), Some(c)) = (at(0), at(1), at(2)) {
                return Self::Biaxial {
                    indicatrix: [d, f, c],
                };
            }
        }
        let n_e_principal = LINE_NM.map(|lambda| {
            material
                .extraordinary_index_at(lambda, material.dispersion.evaluate(lambda))
                .max(1.001)
        });
        Self::Uniaxial {
            n_o,
            n_e_principal,
            c_axis: material.c_axis.try_normalize().unwrap_or(Vec3::Y),
        }
    }

    /// Whether this is the isotropic case (the integer fast path).
    pub(super) const fn is_isotropic(&self) -> bool {
        matches!(self, Self::Isotropic)
    }

    /// `Some(self)` for an anisotropic material, `None` for an isotropic one: what the
    /// `StoneArena` carries.
    pub(super) const fn anisotropic(&self) -> Option<&Self> {
        if self.is_isotropic() {
            None
        } else {
            Some(self)
        }
    }

    /// A mode-independent index at `line` (the ordinary one; a biaxial crystal's `n_beta`), for
    /// the traces that do not split into modes (a frosted entry facet).
    const fn base_index(&self, line: Line) -> f32 {
        match self {
            Self::Isotropic => 1.0,
            Self::Uniaxial { n_o, .. } => n_o[line.slot()],
            Self::Biaxial { indicatrix } => indicatrix[line.slot()].n_beta,
        }
    }
}

/// One eigenmode of one spectral line of an anisotropic stone: everything
/// [`trace_interior`] asks of a ray that is not isotropic.
#[derive(Debug, Clone, Copy)]
pub(super) struct ModeCtx<'a> {
    modes: &'a Modes,
    line: Line,
    mode: Mode,
}

/// A mode's state just inside the entry facet.
#[derive(Debug, Clone, Copy)]
pub(super) struct ModeEntry {
    /// The refracted wave normal.
    pub(super) k: Vec3,
    /// The mode's index at `k`.
    pub(super) index: f32,
    /// Fresnel transmittance of the air-to-crystal interface for this mode.
    pub(super) transmittance: f32,
}

/// The extraordinary index at wave normal `k` for an optic axis `c` (both unit): `n_o` exactly
/// along the axis, else the renderer's `effective_extraordinary_index` at `theta = angle(k, c)`.
fn effective_index(n_o: f32, n_e: f32, k: Vec3, c: Vec3) -> f32 {
    let cos_t = k.dot(c).abs().min(1.0);
    if cos_t >= 1.0 - 1e-7 {
        return n_o;
    }
    BirefringenceParams::effective_extraordinary_index(n_o, n_e, cos_t.acos())
}

impl<'a> ModeCtx<'a> {
    /// The mode `mode` of `modes` at `line`.
    pub(super) const fn new(modes: &'a Modes, line: Line, mode: Mode) -> Self {
        Self { modes, line, mode }
    }

    /// The mode's refractive index for a wave travelling along the unit wave normal `k`.
    pub(super) fn index_at(&self, k: Vec3) -> f32 {
        let slot = self.line.slot();
        match (self.modes, self.mode) {
            (Modes::Isotropic, _) => 1.0,
            (Modes::Uniaxial { n_o, .. }, Mode::Ordinary) => n_o[slot],
            (
                Modes::Uniaxial {
                    n_o,
                    n_e_principal,
                    c_axis,
                },
                Mode::Extraordinary,
            ) => effective_index(n_o[slot], n_e_principal[slot], k, *c_axis),
            (Modes::Biaxial { indicatrix }, mode) => {
                let (n_slow, n_fast) = indicatrix[slot].wave_indices(k);
                if mode == Mode::Extraordinary {
                    n_slow
                } else {
                    n_fast
                }
            }
        }
    }

    /// The direction the mode's energy travels along (the Poynting direction) for the wave
    /// normal `k`: `k` itself for an ordinary ray.
    pub(super) fn ray_dir(&self, k: Vec3) -> Vec3 {
        let slot = self.line.slot();
        match (self.modes, self.mode) {
            (Modes::Isotropic, _) | (Modes::Uniaxial { .. }, Mode::Ordinary) => k,
            (
                Modes::Uniaxial {
                    n_o,
                    n_e_principal,
                    c_axis,
                },
                Mode::Extraordinary,
            ) => BirefringenceParams::extraordinary_poynting_dir(
                k,
                *c_axis,
                n_o[slot],
                n_e_principal[slot],
            ),
            (Modes::Biaxial { indicatrix }, mode) => {
                indicatrix[slot].mode_poynting_dir(k, mode == Mode::Extraordinary)
            }
        }
    }

    /// The mode's path weight on each principal absorption axis for the wave normal `k`: the
    /// squared projection of the mode's electric field on the axes (`axes` = the columns of the
    /// material's `PrincipalFrame`). An ordinary ray has no component along `c`.
    pub(super) fn principal_weights(&self, axes: Mat3, k: Vec3) -> [f32; 3] {
        let slot = self.line.slot();
        let e_field = match (self.modes, self.mode) {
            (Modes::Isotropic, _) => k,
            (
                Modes::Uniaxial {
                    n_o,
                    n_e_principal,
                    c_axis,
                },
                mode,
            ) => assigned_mode_e_field_uniaxial(
                k,
                *c_axis,
                mode == Mode::Extraordinary,
                n_o[slot],
                n_e_principal[slot],
            ),
            (Modes::Biaxial { indicatrix }, mode) => {
                indicatrix[slot].assigned_mode_e_field(k, mode == Mode::Extraordinary)
            }
        };
        let local = axes.transpose() * e_field;
        (local * local).to_array()
    }

    /// Refracts the camera ray of `entry` into the crystal as this mode: its wave normal, index
    /// and the interface's transmittance; `None` when there is no real refraction.
    ///
    /// The ordinary mode is exactly the isotropic entry. The other modes resolve the circularity
    /// `k <- Snell(n)`, `n <- n(k)` by a short fixed-point iteration seeded at the ordinary
    /// index (uniaxial: three Snell solves, converged to well below 1e-6 for any gem
    /// birefringence; biaxial: `BiaxialIndicatrix::resolve_entry_mode`).
    pub(super) fn enter(&self, entry: EntryRay) -> Option<ModeEntry> {
        let slot = self.line.slot();
        match (self.modes, self.mode) {
            (Modes::Isotropic, _) => None,
            (Modes::Uniaxial { n_o, .. }, Mode::Ordinary) => {
                let index = n_o[slot];
                let (k, cos_t) = refract_into(entry.dir, entry.normal, entry.cos_i, index)?;
                Some(ModeEntry {
                    k,
                    index,
                    transmittance: fresnel_transmittance(1.0, index, entry.cos_i, cos_t),
                })
            }
            (
                Modes::Uniaxial {
                    n_o,
                    n_e_principal,
                    c_axis,
                },
                Mode::Extraordinary,
            ) => {
                let mut index = n_o[slot];
                let mut k = entry.dir;
                for _ in 0..3 {
                    k = refract_into(entry.dir, entry.normal, entry.cos_i, index)?.0;
                    index = effective_index(n_o[slot], n_e_principal[slot], k, *c_axis);
                }
                Some(Self::finish_entry(entry, k, index))
            }
            (Modes::Biaxial { indicatrix }, mode) => {
                let crystal = &indicatrix[slot];
                let (_, k) = crystal.resolve_entry_mode(
                    entry.dir,
                    entry.normal,
                    entry.cos_i,
                    crystal.n_beta,
                    mode == Mode::Extraordinary,
                );
                if k.dot(entry.normal) >= 0.0 {
                    return None;
                }
                Some(Self::finish_entry(entry, k, self.index_at(k)))
            }
        }
    }

    /// The entry state for a wave normal `k` already resolved with index `index`.
    fn finish_entry(entry: EntryRay, k: Vec3, index: f32) -> ModeEntry {
        let cos_t = (-k.dot(entry.normal)).clamp(0.0, 1.0);
        ModeEntry {
            k,
            index,
            transmittance: fresnel_transmittance(1.0, index, entry.cos_i, cos_t),
        }
    }
}

/// Traces the camera ray of `entry` through the stone as eigenmode `mode` at spectral `line`;
/// `index_o` is the ordinary index of that line (`PreparedEval`'s `nd`/`n_f`/`n_c`).
///
/// A stone without modes, or a frosted entry facet (which scatters the ray before any mode
/// split), is the plain [`trace_entry`] at `index_o`. Otherwise the ordinary mode of a uniaxial
/// crystal is that same trace bit for bit (when the arena carries no principal frame), and the
/// other modes walk off along their Poynting direction; see the module docs.
pub(super) fn trace_mode(
    entry: EntryRay,
    stone: StoneArena<'_>,
    line: Line,
    mode: Mode,
    index_o: f32,
    mut detail: Option<&mut RayDetail>,
) -> RayFate {
    let Some(modes) = stone.modes else {
        return trace_entry(entry, stone, index_o, detail);
    };
    if stone.is_frosted(entry.facet) {
        return trace_entry(entry, stone, modes.base_index(line), detail);
    }
    note_reason(&mut detail, ExtinctionReason::BounceBudget);
    let ctx = ModeCtx::new(modes, line, mode);
    let Some(start) = ctx.enter(entry) else {
        note_reason(&mut detail, ExtinctionReason::EntryBlocked);
        return RayFate::EntryBlocked;
    };
    let seed = if stone.finishes.is_empty() {
        0
    } else {
        path_seed(entry.point, entry.dir)
    };
    trace_interior(
        Interior {
            hit_point: entry.point + ctx.ray_dir(start.k) * 1e-4,
            dir: start.k,
            entry_transmittance: start.transmittance,
            seed,
        },
        stone,
        start.index,
        Some(ctx),
        detail,
    )
}
