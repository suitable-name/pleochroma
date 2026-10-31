//! Unit tests for the spectral transport loop, split by topic: `mode_coupling_tests`
//! (o<->e re-coupling at internal reflections), `exit_splitting_tests` (exit-event
//! spectral splitting's unbiasedness/variance), `nee_tests` (next-event-estimation
//! unbiasedness), and `wave_normal_tests` (the wave-normal-vs-Poynting-direction walk-off
//! through a plane-parallel uniaxial slab).

/// O<->e mode re-coupling at internal reflections.
mod mode_coupling_tests;

mod exit_splitting_tests;

mod nee_tests;

/// The inside/outside state of the bounce loop on a concave stone.
mod concave_state_tests;

mod surface_glare_tests;

mod attribution_tests;

mod surface_plumbing_tests;

mod wave_normal_tests;
