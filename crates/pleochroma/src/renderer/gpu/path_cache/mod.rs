//! Path cache, phase B on the GPU.
//!
//! The cache of a frame is built on the CPU (phase A, `renderer::path_cache`); this module is the
//! GPU half of using it:
//!
//! - [`source`]: the phase B kernel, the WGSL twin of `renderer::path_cache::shade::Shader`. It
//!   is built by appending `shaders/path_cache/phase_b.wgsl` to the megakernel unit, so the
//!   environment, the hero-wavelength draw and the camera ray are the live pass's own code, never
//!   a retyped copy. The completion unit is the megakernel with its counted edits plus
//!   `phase_b.wgsl` and `shaders/path_cache/complete.wgsl`;
//! - `pass.rs` (mounted as a child of `renderer::gpu::frame`, which owns the private renderer
//!   state it needs): the slab upload and the chunk loop (phase B, the completion, the reduce and
//!   the readback).
//!
//! # How a pass uses the cache
//!
//! A live pass with a cache splits the live pixels in two. The **cached pixels** (stone pixels
//! the cache has positions for) shade in the phase B kernel; every other live pixel goes to the
//! ordinary live path tracer, with an explicit pixel list. Both add the samples
//! `sample_offset..sample_offset + spp` to the summed accumulator and, for the odd absolute
//! indices, to half-buffer B, exactly as an uncached live pass does.
//!
//! # Streaming
//!
//! The tiles of the cache are grouped into slabs (`gpu_backend::GpuPathCache`). A slab is
//! uploaded as one storage buffer (tile words back to back) plus a small table of tile word
//! offsets. The video-memory tier keeps every uploaded slab resident across passes; the
//! system-RAM tier uploads slab `s + 1` while slab `s` shades and drops slab `s` after it.
//! Uploads use `Queue::write_buffer`, which orders the copy before every later submission, so
//! the output of a pass never depends on when an upload happens. Phase A runs on worker threads
//! ahead of the pass (`GpuPathCache::with_prefetch`).
//!
//! # Residual completion
//!
//! The kernel decides, with the CPU's own draw, which samples trace the energy the cache
//! dropped, and appends them to the fired list with their tuple index. One chunk then runs:
//!
//! 1. phase B: writes the kept value of every tuple and the fired list;
//! 2. the completion: the megakernel's own `transport_main`, patched to trace one fired sample
//!    per invocation (the position's camera ray, the sample's seed and hero), count the interior
//!    reflections before the first exit, run the CPU's cover test against the position's
//!    resident records and add `traced / p` to the kept value in the sample's own tuple slot;
//! 3. the reduce, which sums each pixel's tuples in a fixed order, and the readback.
//!
//! Nothing is read back for the completion, and each tuple is written by exactly one invocation,
//! so the result does not depend on the fired list's atomic order. A renderer that cannot build
//! or bind the completion pass (an anchor drift, too few storage buffers) completes on the host
//! instead: the fired list is read back and the CPU path tracer and cover test
//! (`Shader::complete_residual`) add the samples to the pixel sums. That costs most of the pass
//! for a stone whose positions carry a large residual.

pub mod source;

pub use source::{
    COMPLETE_BINDINGS, COMPLETE_ENTRY, COMPLETE_SHADER_SRC, COMPLETE_STORAGE_BUFFERS,
    COMPLETE_UNIFORM_BUFFERS, PHASE_B_BINDINGS, PHASE_B_ENTRY, PHASE_B_SHADER_SRC,
    completion_variant, phase_b_source,
};
