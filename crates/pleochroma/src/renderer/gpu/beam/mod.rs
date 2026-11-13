//! Beam tracing on the GPU (beam guide §14.7): the reference-variant sources, the leaf and kept
//! buffers, the fragment kernels and the beam engine. Compiled with the `gpu` and `beam` features.

pub mod buffers;
pub mod device_caches;
pub mod engine;
pub mod fragments;
pub mod kept;
pub mod reference_source;

#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests_buffers;
#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests_d3_fragments;
#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests_engine;
#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests_fragments;
#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests_kept;
#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests_reference;
#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests_resident;
#[cfg(test)]
#[cfg(all(feature = "beam", beam_tests))]
mod tests_residual;
