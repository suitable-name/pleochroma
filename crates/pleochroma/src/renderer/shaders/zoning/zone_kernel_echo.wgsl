// Self-test shader for the zone kernel, a standalone test target. The
// `renderer::gpu::layout_check::run_zone_kernel_echo` host concatenates it with
// `01_zone_table.wgsl` and `02_zone_kernel.wgsl` (the kernel under test, the file the transport
// shaders are built from), uploads a `GpuZoneTable` and segments, and has each invocation
// evaluate `zone_lengths(a, b)` for one segment. The host compares the five per-zone lengths
// with `ZoneKernelF32::lengths`.
//
// Bindings: 0 = the table, wrapped so that `material.zones` resolves exactly as it does in
// the transport shaders (where `material: GpuGemMaterial` carries `zones`); 1 = the segments;
// 2 = the lengths, eight floats per segment (five used, three zero padding).

struct AbsorptionBand {
    center_nm: f32,
    width_nm: f32,
    peak: f32,
    shape: u32,
}

struct EchoMaterial {
    zones: GpuZoneTable,
}

struct EchoSegment {
    a: vec4<f32>,
    b: vec4<f32>,
}

@group(0) @binding(0) var<storage, read> material: EchoMaterial;
@group(0) @binding(1) var<storage, read> segments: array<EchoSegment>;
@group(0) @binding(2) var<storage, read_write> lengths_out: array<f32>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i = gid.x;
    if (i >= arrayLength(&segments)) {
        return;
    }
    let seg = segments[i];
    var l = zone_lengths(seg.a.xyz, seg.b.xyz);
    for (var k: u32 = 0u; k < 5u; k = k + 1u) {
        lengths_out[i * 8u + k] = l[k];
    }
}
