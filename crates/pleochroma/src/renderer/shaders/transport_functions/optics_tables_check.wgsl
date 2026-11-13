// Test kernels of the optics lookup tables (`optics/tables/gpu_tests.rs`).
//
// NOT one of the `TRANSPORT_FUNCTIONS_PIECES` build.rs concatenates. It is compiled after
// `transport_physics/optics_tables.wgsl` (which declares the table buffer at binding 170) and
// needs nothing else. Cases and results are flat `f32` buffers (four floats per vec4).
//
// ot_exp_main:     case = one argument a <= 0; result = ot_exp_table(a).
// ot_fresnel_main: case = (n1, n2, cos_i, cos_t); result = two vec4: (served by a table, r_s,
//                  r_p, 0) and (t_s, t_p, 0, 0), the amplitudes from ot_amplitudes (the table
//                  where it serves the inputs, else the closed forms).
@group(0) @binding(171) var<storage, read> ot_exp_cases: array<f32>;
@group(0) @binding(172) var<storage, read_write> ot_exp_results: array<f32>;
@group(0) @binding(173) var<storage, read> ot_fresnel_cases: array<vec4<f32>>;
@group(0) @binding(174) var<storage, read_write> ot_fresnel_results: array<vec4<f32>>;

@compute @workgroup_size(64)
fn ot_exp_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&ot_exp_cases)) {
        return;
    }
    ot_exp_results[idx] = ot_exp_table(ot_exp_cases[idx]);
}

@compute @workgroup_size(64)
fn ot_fresnel_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&ot_fresnel_cases)) {
        return;
    }
    let c = ot_fresnel_cases[idx];
    let tabled = ot_fresnel(c.x, c.y, c.z, c.w);
    let res = ot_amplitudes(c.x, c.y, c.z, c.w);
    ot_fresnel_results[2u * idx] = vec4<f32>(f32(tabled.ok), res.r_s, res.r_p, 0.0);
    ot_fresnel_results[2u * idx + 1u] = vec4<f32>(res.t_s, res.t_p, 0.0, 0.0);
}
