//! Birefringence: uniaxial and biaxial crystal optics, and pleochroic (polarization-
//! dependent) absorption.
//!
//! Module layout: [`helpers`] holds the vector helpers shared by the uniaxial and biaxial
//! code, [`uniaxial`] holds [`BirefringenceParams`], [`absorption_tensor`] holds
//! [`AbsorptionTensor3`] and the pleochroic-absorption functions built on it, [`frame`]
//! holds the full crystal frame constructors, and [`biaxial`] holds [`BiaxialIndicatrix`]
//! and the general Poynting-direction formula. The public items are re-exported below.

mod absorption_tensor;
mod biaxial;
mod frame;
mod helpers;
#[cfg(test)]
mod tests;
mod uniaxial;

pub use absorption_tensor::{
    AbsorptionTensor3, assigned_mode_alpha, assigned_mode_e_field_uniaxial,
    effective_pleochroic_alpha, pleochroic_channel_alpha,
};
pub use biaxial::{BiaxialIndicatrix, poynting_direction};
pub use frame::alpha_axis_from_turn;
pub use uniaxial::BirefringenceParams;
