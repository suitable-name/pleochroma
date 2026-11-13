struct TsCheckCase {
    origin: vec4<f32>,
    direction_lambda: vec4<f32>,
};
@group(0) @binding(191) var<storage, read> ts_cases: array<TsCheckCase>;
@group(0) @binding(192) var<storage, read_write> ts_results: array<vec4<f32>>;
@group(0) @binding(193) var<uniform> ts_case_count: vec4<u32>;

@compute @workgroup_size(64)
fn ts_check_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= ts_case_count.x) { return; }
    let input = ts_cases[gid.x];
    let hit = ts_plane_uv(input.origin.xyz, input.direction_lambda.xyz);
    var factor = 1.0;
    if (hit.z != 0.0) { factor = ts_factor(hit.xy, input.direction_lambda.w); }
    ts_results[gid.x] = vec4<f32>(hit, factor);
}
