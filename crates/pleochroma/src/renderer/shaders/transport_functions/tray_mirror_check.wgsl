struct TmCheckCase {
    origin: vec4<f32>,
    direction: vec4<f32>,
};
@group(0) @binding(191) var<storage, read> tm_cases: array<TmCheckCase>;
@group(0) @binding(192) var<storage, read_write> tm_results: array<vec4<f32>>;
@group(0) @binding(193) var<uniform> tm_case_count: vec4<u32>;

// Output per case: (hit x, hit z, Schlick fresnel, 1 on a floor hit / 0 on a miss), the
// observable parts of `TmFloor`. The hit y is the plane height by construction.
@compute @workgroup_size(64)
fn tm_check_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if (gid.x >= tm_case_count.x) { return; }
    let input = tm_cases[gid.x];
    let floor_hit = tm_floor(input.origin.xyz, input.direction.xyz);
    var out = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    if (floor_hit.hit) {
        out = vec4<f32>(floor_hit.point.x, floor_hit.point.z, floor_hit.fresnel, 1.0);
    }
    tm_results[gid.x] = out;
}
