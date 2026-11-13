//! Per-kernel bind group slots of the event wavefront kernels.
//!
//! [`kernel_slots`] is the single table of which `@group(0)` bindings each kernel's entry point
//! reaches. The pipeline layout is derived from the shader, so a slot bound but unused, or used
//! but not bound, is a wgpu validation error. The test
//! `host_slots_equal_the_bindings_each_entry_point_reaches` compares this table with naga's
//! per-entry-point binding usage, so a lost or extra binding fails a test instead of producing
//! a silent black frame.

use super::{ChunkRayBuffers, SceneBuffers};
use crate::{
    optics::tables::OPTICS_TABLES_BINDING,
    renderer::gpu::{
        compute,
        event_wavefront::EwKernel,
        frame::{LIVE_PIXELS_BINDING, event_wavefront_uses_tables},
    },
};

/// The `@binding` numbers of `@group(0)` that `kernel` binds, ascending.
///
/// Slot meaning: 0 camera, 1 transport params, 2 material, 3 planes, 4..7 and 9 the finalize
/// outputs, 8 facet finishes, 10 HDR texels, 11 HDR dims, 12 distribution func, 13 distribution
/// cdf, 14 distribution dims, 40 `ew` params, 41 ray core, 42 ray chan, 43 ray hit.
#[must_use]
pub const fn kernel_slots(kernel: EwKernel) -> &'static [u32] {
    match kernel {
        EwKernel::Generate => &[0, 1, 40, 41, 42],
        EwKernel::Extend => &[1, 2, 3, 10, 11, 12, 14, 40, 41, 42, 43],
        EwKernel::Scatter | EwKernel::ScatterOriented => &[1, 2, 3, 40, 41, 42, 43],
        // No ray_hit: the parked scatter event lives in the core, which keeps this kernel at
        // 8 storage buffers (material, planes, finishes, texels, func, cdf, core, chan).
        EwKernel::NeeHg => &[1, 2, 3, 8, 10, 11, 12, 13, 14, 40, 41, 42],
        EwKernel::Surface => &[1, 2, 3, 8, 40, 41, 42, 43],
        EwKernel::ArmFrosted => &[1, 2, 3, 10, 11, 12, 13, 14, 40, 41, 42, 43],
        // `ZonedAbsorb` reads only the zone table of the material and the hit's `t` and arm.
        EwKernel::ArmTir
        | EwKernel::ArmUniEntry
        | EwKernel::ArmUniEntryTransmit
        | EwKernel::Tail
        | EwKernel::ZonedAbsorb => &[1, 2, 40, 41, 42, 43],
        EwKernel::ArmUniInternal | EwKernel::ArmGeneric => &[1, 2, 12, 14, 40, 41, 42, 43],
        EwKernel::SplitResolve => &[1, 2, 3, 10, 11, 40, 41, 42, 43],
        EwKernel::Finalize => &[1, 4, 5, 6, 7, 9, 40, 41, 42],
    }
}

/// The buffer behind `slot`, plus the explicit binding size for the two unsized arrays.
fn slot_buffer<'a>(
    slot: u32,
    scene: &'a SceneBuffers,
    chunk: &'a ChunkRayBuffers,
    planes_bytes: wgpu::BufferAddress,
    finishes_bytes: wgpu::BufferAddress,
) -> (&'a wgpu::Buffer, Option<wgpu::BufferAddress>) {
    match slot {
        0 => (&scene.camera, None),
        1 => (&scene.params, None),
        2 => (&scene.material, None),
        3 => (&scene.planes, Some(planes_bytes)),
        4 => (&chunk.out_xyz, None),
        5 => (&chunk.out_radiance, None),
        6 => (&chunk.out_lambdas, None),
        7 => (&chunk.out_path_pdf, None),
        8 => (&scene.facet_finishes, Some(finishes_bytes)),
        9 => (&chunk.out_compat, None),
        10 => (&scene.hdr_env.texels, None),
        11 => (&scene.hdr_env.dims, None),
        12 => (&scene.hdr_env.dist_func, None),
        13 => (&scene.hdr_env.dist_cdf, None),
        14 => (&scene.hdr_env.dist_dims, None),
        40 => (&chunk.ew_params, None),
        41 => (&chunk.ray_core, None),
        42 => (&chunk.ray_chan, None),
        43 => (&chunk.ray_hit, None),
        _ => unreachable!("kernel_slots lists only known slots"),
    }
}

/// The optics lookup table buffer `kernel` of `class` must hold at [`OPTICS_TABLES_BINDING`];
/// `None` when the pipeline is the shipped kernel (the class carries no table bit) or the scene
/// has built no table buffer.
///
/// [`kernel_slots`] is the table of the shipped kernels. A table variant reaches exactly one more
/// binding, this one; only the shipped kernels are checked against naga.
fn tables_buffer(scene: &SceneBuffers, kernel: EwKernel, class: u32) -> Option<&wgpu::Buffer> {
    if event_wavefront_uses_tables(kernel, class) {
        scene.optics_tables.buffer()
    } else {
        None
    }
}

/// Builds the bind group for `kernel` from `scene` and `chunk` buffers.
///
/// `class` is the material class the kernel's pipeline was compiled for: with
/// `material_class::OPTICS_TABLES` in it the pipeline is the table variant and the scene's table
/// buffer is bound as well; without it the bind group is exactly [`kernel_slots`].
pub fn bind_kernel(
    device: &wgpu::Device,
    pipeline: &wgpu::ComputePipeline,
    kernel: EwKernel,
    scene: &SceneBuffers,
    chunk: &ChunkRayBuffers,
    (planes_bytes, finishes_bytes): (wgpu::BufferAddress, wgpu::BufferAddress),
    class: u32,
) -> wgpu::BindGroup {
    let mut entries: Vec<(u32, &wgpu::Buffer, Option<wgpu::BufferAddress>)> = kernel_slots(kernel)
        .iter()
        .map(|&slot| {
            let (buffer, size) = slot_buffer(slot, scene, chunk, planes_bytes, finishes_bytes);
            (slot, buffer, size)
        })
        .collect();
    if let Some(table_buffer) = tables_buffer(scene, kernel, class) {
        entries.push((OPTICS_TABLES_BINDING, table_buffer, None));
    }
    compute::bind_buffers_sized(device, kernel.entry_point(), pipeline, &entries)
}

/// The bind group of the LIVE `ew_generate` of an adaptive pass: the ordinary
/// [`kernel_slots`] of [`EwKernel::Generate`] plus the live pixel list (real pixel indices) at
/// [`LIVE_PIXELS_BINDING`], which only the live variant of the kernel reaches, and the optics
/// table buffer when `class` carries `material_class::OPTICS_TABLES`. Ordinary passes never call
/// this, and [`kernel_slots`] is unchanged.
pub fn bind_generate_live(
    device: &wgpu::Device,
    pipeline: &wgpu::ComputePipeline,
    (scene, chunk): (&SceneBuffers, &ChunkRayBuffers),
    live_pixels: &wgpu::Buffer,
    class: u32,
) -> wgpu::BindGroup {
    let mut entries: Vec<(u32, &wgpu::Buffer, Option<wgpu::BufferAddress>)> =
        kernel_slots(EwKernel::Generate)
            .iter()
            .map(|&slot| {
                // `Generate` binds neither unsized scene array, so the sizes are not read.
                let (buffer, size) = slot_buffer(slot, scene, chunk, 0, 0);
                (slot, buffer, size)
            })
            .collect();
    entries.push((LIVE_PIXELS_BINDING, live_pixels, None));
    if let Some(table_buffer) = tables_buffer(scene, EwKernel::Generate, class) {
        entries.push((OPTICS_TABLES_BINDING, table_buffer, None));
    }
    compute::bind_buffers_sized(device, "ew_generate live", pipeline, &entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_tables_are_ascending_and_unique() {
        for kernel in EwKernel::ALL {
            let slots = kernel_slots(kernel);
            assert!(
                slots.windows(2).all(|w| w[0] < w[1]),
                "{kernel:?} slots not strictly ascending: {slots:?}"
            );
        }
    }

    /// The table must equal the bindings naga finds each entry point reaching: a binding the
    /// shader reaches but the host does not bind (or the reverse) is a wgpu validation error,
    /// i.e. a silent black frame in the worker.
    #[test]
    fn host_slots_equal_the_bindings_each_entry_point_reaches() {
        use naga::{
            front::wgsl,
            valid::{Capabilities, ValidationFlags, Validator},
        };

        for kernel in EwKernel::ALL {
            let source = kernel.source();
            let module = wgsl::parse_str(source).expect("generated kernel parses");
            let info = Validator::new(ValidationFlags::all(), Capabilities::all())
                .validate(&module)
                .expect("generated kernel validates");
            let ep_idx = module
                .entry_points
                .iter()
                .position(|ep| ep.name == kernel.entry_point())
                .expect("entry point exists");
            let ep_info = info.get_entry_point(ep_idx);
            let mut reached: Vec<u32> = module
                .global_variables
                .iter()
                .filter(|(handle, _)| !ep_info[*handle].is_empty())
                .filter_map(|(_, var)| var.binding.as_ref())
                .inspect(|binding| assert_eq!(binding.group, 0))
                .map(|binding| binding.binding)
                .collect();
            reached.sort_unstable();
            assert_eq!(
                reached,
                kernel_slots(kernel),
                "{kernel:?}: host bind table disagrees with the shader"
            );
        }
    }

    #[test]
    fn only_the_hdr_kernels_bind_the_hdr_buffers() {
        for kernel in EwKernel::ALL {
            let slots = kernel_slots(kernel);
            let uses_texels = slots.contains(&10);
            let expected = matches!(
                kernel,
                EwKernel::Extend | EwKernel::NeeHg | EwKernel::ArmFrosted | EwKernel::SplitResolve
            );
            assert_eq!(uses_texels, expected, "{kernel:?}");
        }
    }
}
