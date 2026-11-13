//! Optional lookup tables for the tracer's Fresnel amplitudes and absorption exponential.
//!
//! Off by default and off means identical: with [`OpticsTables::Off`] no table is built and every
//! site runs the closed form it always ran, bit for bit. The switch is a per-material setting,
//! [`crate::optics::raytracer::transport::PreparedMaterial::with_optics_tables`], plus a process
//! default, [`set_default_optics_tables`], that [`PreparedMaterial::new`] reads when it builds a
//! material.
//!
//! Two tables exist (see their modules for the error bounds):
//!
//! - [`ExpTable`]: `exp(-x)` for the Beer-Lambert transmittance, 129 values of `2^-f`.
//! - [`FresnelBranch`]: the two smooth denominators `A` and `B` of the scalar Fresnel amplitudes,
//!   one table for an entry into the glass and one for an exit, built per material.
//!
//! The eigenmode solves of the uniaxial and biaxial paths are not tabulated: their result
//! depends on the index pair, the angle between the wave normal and the optic axis, the incidence
//! angle and the mode, which has no clean one- or two-parameter form.
//!
//! The GPU twin is `renderer/shaders/transport_physics/optics_tables.wgsl`; [`pack_gpu_words`]
//! writes the buffer it reads (binding [`OPTICS_TABLES_BINDING`]).
//!
//! [`PreparedMaterial::new`]: crate::optics::raytracer::transport::PreparedMaterial::new

mod exp;
mod fresnel;
mod gpu;
#[cfg(all(test, feature = "gpu", not(target_arch = "wasm32")))]
mod gpu_tests;
#[cfg(test)]
mod tests;

use std::sync::atomic::{AtomicBool, Ordering};

use crate::optics::materials::GemMaterial;

pub use exp::{EXP_CELLS, EXP_TABLE_MIN_ARG, EXP_VALUES, ExpTable};
pub use fresnel::{
    FRESNEL_REL_TOLERANCE, FresnelAmplitudes, FresnelBranch, FresnelSide, closed_form_reflection,
    material_index_range,
};
#[cfg(feature = "gpu")]
pub use gpu::upload_gpu_buffer;
pub use gpu::{
    GPU_BRANCH_DATA, GPU_EXP_BASE, GPU_HEADER_ENTRY, GPU_HEADER_EXIT, OPTICS_TABLES_BINDING,
    OPTICS_TABLES_WGSL, exp_table_variant, optics_tables_variant, pack_disabled_gpu_words,
    pack_gpu_words,
};

/// Whether the optics lookup tables are used.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OpticsTables {
    /// Every site runs its closed form. The default; output is identical to a build without the
    /// tables.
    #[default]
    Off,
    /// The Fresnel amplitudes and the absorption exponential come from the tables where a table
    /// serves the inputs.
    On,
}

/// The process default [`PreparedMaterial::new`] reads; `false` is [`OpticsTables::Off`].
///
/// [`PreparedMaterial::new`]: crate::optics::raytracer::transport::PreparedMaterial::new
static DEFAULT_ON: AtomicBool = AtomicBool::new(false);

/// Sets the process default of the optics lookup tables.
///
/// Call it once at start-up, before any frame is prepared: a [`PreparedMaterial`] reads the
/// default when it is built, so a change affects only frames prepared afterwards. This is where
/// a command line or settings flag for the tables must be applied.
///
/// [`PreparedMaterial`]: crate::optics::raytracer::transport::PreparedMaterial
pub fn set_default_optics_tables(mode: OpticsTables) {
    DEFAULT_ON.store(mode == OpticsTables::On, Ordering::Relaxed);
}

/// The process default set by [`set_default_optics_tables`] ([`OpticsTables::Off`] initially).
#[must_use]
pub fn default_optics_tables() -> OpticsTables {
    if DEFAULT_ON.load(Ordering::Relaxed) {
        OpticsTables::On
    } else {
        OpticsTables::Off
    }
}

/// `pos` as a cell index in `[0, cells - 1]`, with the fractional remainder in cell units.
///
/// `pos` is a finite, non-negative table position; the clamp makes the last node (`pos == cells`)
/// land in the last cell with a remainder of one.
#[expect(
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "pos is finite and non-negative, so the truncation is the floor"
)]
fn split_cell(pos: f32, cells: usize) -> (usize, f32) {
    let cell = (pos as usize).min(cells - 1);
    (cell, pos - cell as f32)
}

/// The tables of one render: the exponential and, per side of the interface, a Fresnel table.
#[derive(Clone, Debug)]
pub struct OpticsTableSet {
    exp: ExpTable,
    entry: Option<FresnelBranch>,
    exit: Option<FresnelBranch>,
}

impl OpticsTableSet {
    /// Builds the tables for `material`'s index range (see [`material_index_range`]).
    ///
    /// A material whose index range cannot be scanned gets the exponential table only.
    #[must_use]
    pub fn for_material(material: &GemMaterial) -> Self {
        material_index_range(material).map_or_else(Self::exponential_only, |(n_lo, n_hi)| {
            Self::for_index_range(n_lo, n_hi)
        })
    }

    /// Builds the tables for glass indices in `[n_lo, n_hi]`.
    #[must_use]
    pub fn for_index_range(n_lo: f32, n_hi: f32) -> Self {
        Self {
            exp: ExpTable::new(),
            entry: FresnelBranch::build(FresnelSide::Entry, n_lo, n_hi),
            exit: FresnelBranch::build(FresnelSide::Exit, n_lo, n_hi),
        }
    }

    /// The exponential table with no Fresnel branch: every Fresnel lookup returns `None`.
    #[must_use]
    pub fn exponential_only() -> Self {
        Self {
            exp: ExpTable::new(),
            entry: None,
            exit: None,
        }
    }

    /// The exponential table.
    #[must_use]
    pub const fn exp_table(&self) -> &ExpTable {
        &self.exp
    }

    /// The Fresnel table of an entry into the glass, if one was built.
    #[must_use]
    pub const fn entry(&self) -> Option<&FresnelBranch> {
        self.entry.as_ref()
    }

    /// The Fresnel table of an exit from the glass, if one was built.
    #[must_use]
    pub const fn exit(&self) -> Option<&FresnelBranch> {
        self.exit.as_ref()
    }

    /// The four Fresnel amplitudes of an interface with air on one side, or `None` when no table
    /// serves it (neither side is exactly air, the index is outside the built range, or the
    /// branch was not built).
    ///
    /// `n1`/`n2` are the indices on the incident and transmitted side; `cos_i`/`cos_t` the
    /// cosines of the incident and refracted angles, consistent with Snell's law.
    #[must_use]
    pub fn fresnel(&self, n1: f32, n2: f32, cos_i: f32, cos_t: f32) -> Option<FresnelAmplitudes> {
        if n1 == 1.0 && n2 > 1.0 {
            self.entry.as_ref()?.amplitudes(n2, cos_i, cos_t)
        } else if n2 == 1.0 && n1 > 1.0 {
            self.exit.as_ref()?.amplitudes(n1, cos_i, cos_t)
        } else {
            None
        }
    }
}

/// The reflection amplitudes `(r_s, r_p)` of an interface: from `tables` where one serves it,
/// else the closed form the tracer always used (bit for bit).
#[must_use]
pub fn reflection_amplitudes(
    tables: Option<&OpticsTableSet>,
    n1: f32,
    n2: f32,
    cos_i: f32,
    cos_t: f32,
) -> (f32, f32) {
    if let Some(found) = tables.and_then(|set| set.fresnel(n1, n2, cos_i, cos_t)) {
        return (found.r_s, found.r_p);
    }
    closed_form_reflection(n1, n2, cos_i, cos_t)
}

/// All four amplitudes of an interface: from `tables` where one serves it, else the closed forms
/// the tracer always used (bit for bit).
#[must_use]
pub fn fresnel_amplitudes(
    tables: Option<&OpticsTableSet>,
    n1: f32,
    n2: f32,
    cos_i: f32,
    cos_t: f32,
) -> FresnelAmplitudes {
    tables
        .and_then(|set| set.fresnel(n1, n2, cos_i, cos_t))
        .unwrap_or_else(|| FresnelAmplitudes::closed_form(n1, n2, cos_i, cos_t))
}

/// The Beer-Lambert factors `exp(args)`: from the exponential table where every lane is in its
/// range, else the polynomial [`crate::simd::exp_f32x8`] the tracer always used (bit for bit).
#[must_use]
pub fn beer_lambert_factors(tables: Option<&OpticsTableSet>, args: [f32; 8]) -> [f32; 8] {
    tables.map_or_else(
        || crate::simd::exp_f32x8(args),
        |set| set.exp.exp_array(args),
    )
}
