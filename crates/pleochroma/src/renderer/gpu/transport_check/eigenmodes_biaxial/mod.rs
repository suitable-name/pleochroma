//! `optics::birefringence::BiaxialIndicatrix` GPU port -- the genuinely biaxial
//! (three-distinct-principal-index) generalization of the uniaxial machinery
//! `eigenmodes_uniaxial` checks. Every case bank here is compared against the REAL CPU
//! `BiaxialIndicatrix` methods, built via `BiaxialIndicatrix::from_gamma_axis` (never a
//! hand-written parallel reimplementation) -- see `shaders/transport_physics.wgsl`'s own
//! biaxial section for the WGSL port these exercise.
//!
//! Split into one file per case bank: [`wave_indices`], [`eigen_polarization`],
//! [`mode_poynting`], [`resolve_entry_mode`], [`pleochroic`], [`assigned_mode_alpha`]
//! (the assigned-mode absorption branch), and [`curves`] (the per-wavelength principal
//! indices of a material with per-axis curves). This file keeps only the fixtures every
//! bank shares: [`biaxial_test_indicatrices`] and the
//! [`biaxial_test_directions`]/[`biaxial_test_directions_index_stable`] direction sweeps.

use glam::Vec3;

use crate::optics::{birefringence::BiaxialIndicatrix, materials::GemMaterial};

mod assigned_mode_alpha;
mod curves;
mod eigen_polarization;
mod mode_poynting;
mod pleochroic;
mod resolve_entry_mode;
mod wave_indices;

pub use assigned_mode_alpha::{AssignedModeAlphaBiaxialCase, run_assigned_mode_alpha_biaxial};
pub use curves::{BiaxialCurvesCase, run_biaxial_curves};
pub use eigen_polarization::{BiaxialEigenPolarizationCase, run_biaxial_eigen_polarization};
pub use mode_poynting::{BiaxialModePoyntingCase, run_biaxial_mode_poynting};
pub use pleochroic::{BiaxialPleochroicCase, run_biaxial_pleochroic};
pub use resolve_entry_mode::{BiaxialResolveEntryModeCase, run_biaxial_resolve_entry_mode};
pub use wave_indices::{BiaxialWaveIndicesCase, run_biaxial_wave_indices};

/// The three principal indices and gamma axis of every real biaxial built-in (Alexandrite,
/// Topaz, Tanzanite) at the D line, plus one synthetic, deliberately off-axis case.
///
/// # Panics
///
/// Panics if any of "Alexandrite"/"Topaz"/"Tanzanite" is ever removed from
/// `GemMaterial::all_materials()`, or if `biaxial_indicatrix` ever returns `None` for
/// one of them (it always returns `Some` for a material with `biaxial_delta_beta_alpha
/// = Some(_)`, which all three built-ins have) -- both would be a change to this
/// crate's own material catalogue this self-test scaffolding needs to know about, not a
/// condition worth handling gracefully.
fn biaxial_test_indicatrices() -> Vec<(&'static str, f32, f32, f32, Vec3)> {
    const D_LINE_NM: f32 = 589.3;
    let materials = GemMaterial::all_materials();
    let mut out = Vec::new();
    for name in ["Alexandrite", "Topaz", "Tanzanite"] {
        let material = materials
            .iter()
            .find(|m| m.name == name)
            .unwrap_or_else(|| panic!("\"{name}\" must be a built-in biaxial material"));
        let ind = material
            .biaxial_indicatrix(D_LINE_NM)
            .unwrap_or_else(|| panic!("\"{name}\" must expose a BiaxialIndicatrix"));
        out.push((name, ind.n_alpha, ind.n_beta, ind.n_gamma, ind.axes.z_axis));
    }
    out.push((
        "synthetic",
        1.60,
        1.65,
        1.75,
        Vec3::new(0.35, 0.82, -0.45).normalize(),
    ));
    out
}

/// A representative spread of wave-normal directions -- axis-aligned, oblique, and
/// exactly along the gamma axis (the degenerate direction where two roots' local
/// components can coincide) -- shared by every case bank below.
fn biaxial_test_directions_unfiltered(gamma_axis: Vec3) -> Vec<Vec3> {
    vec![
        Vec3::X,
        Vec3::Y,
        Vec3::Z,
        gamma_axis,
        Vec3::new(0.2, 0.9, 0.1).normalize(),
        Vec3::new(-0.5, 0.3, 0.8).normalize(),
        Vec3::new(0.7, -0.6, 0.2).normalize(),
        Vec3::new(0.1, 0.1, 0.99).normalize(),
        Vec3::new(-0.9, -0.3, 0.2).normalize(),
    ]
}

/// Like [`biaxial_test_directions_unfiltered`], but drops directions that land essentially
/// on one of the material's two optic axes, where the slow and fast indices coincide.
///
/// The threshold is tiny (relative mode separation 1e-4). A real gem's birefringence is only
/// a few thousandths of its mean index, so a larger threshold would leave almost no
/// directions. This drops only directions sitting on a true optic axis.
fn biaxial_test_directions(n_alpha: f32, n_beta: f32, n_gamma: f32, gamma_axis: Vec3) -> Vec<Vec3> {
    biaxial_test_directions_filtered(n_alpha, n_beta, n_gamma, gamma_axis, None, 1e-4)
}

/// Stricter filter for `wave_indices`, which loses precision near the two optic axes.
///
/// The discriminant cancels there. See [`biaxial_test_directions`] for the looser filter the
/// other banks use.
///
/// `alpha_axis` is the frame the cases are evaluated in (`None`: the gamma-only
/// completion). Turning alpha about gamma moves the optic axes, so the filter has to see
/// the same frame as the comparison.
fn biaxial_test_directions_index_stable(
    n_alpha: f32,
    n_beta: f32,
    n_gamma: f32,
    gamma_axis: Vec3,
    alpha_axis: Option<Vec3>,
) -> Vec<Vec3> {
    biaxial_test_directions_filtered(n_alpha, n_beta, n_gamma, gamma_axis, alpha_axis, 0.02)
}

fn biaxial_test_directions_filtered(
    n_alpha: f32,
    n_beta: f32,
    n_gamma: f32,
    gamma_axis: Vec3,
    alpha_axis: Option<Vec3>,
    min_relative_mode_separation: f32,
) -> Vec<Vec3> {
    let ind =
        BiaxialIndicatrix::from_oriented_frame(n_alpha, n_beta, n_gamma, gamma_axis, alpha_axis);
    biaxial_test_directions_unfiltered(gamma_axis)
        .into_iter()
        .filter(|&d| {
            let (n_slow, n_fast) = ind.wave_indices(d);
            (n_slow - n_fast) / n_slow >= min_relative_mode_separation
        })
        .collect()
}
