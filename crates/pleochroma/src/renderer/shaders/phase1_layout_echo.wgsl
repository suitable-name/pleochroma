// GPU struct-layout self-test kernels, driven by `renderer::gpu::layout_check`'s
// `run_facet_plane`, `run_camera_params`, `run_ray` and `run_hit_record`. Not physics kernels.
//
// Same mechanism as `layout_echo.wgsl`: every named field is echoed to an output buffer, and
// `layout_check` compares the raw bytes (padding included) with the `#[repr(C)]` offsets in
// `renderer::buffers`.
//
// The four structs have their own bind-group slots and entry points in one module. Each entry
// point touches only its own pair of globals.

struct FacetPlane {
    normal: vec3<f32>,
    d: f32,
}

struct GpuCameraParams {
    origin: vec3<f32>,
    fov_tan: f32,
    forward: vec3<f32>,
    width: f32,
    right: vec3<f32>,
    height: f32,
    up: vec3<f32>,
    num_samples: u32,
}

struct GpuRay {
    origin: vec3<f32>,
    dir: vec3<f32>,
}

struct GpuHitRecord {
    t: f32,
    facet_idx: i32,
    hit: u32,
    normal: vec3<f32>,
}

@group(0) @binding(0) var<storage, read> in_plane: FacetPlane;
@group(0) @binding(1) var<storage, read_write> out_plane: FacetPlane;

@group(0) @binding(2) var<storage, read> in_camera: GpuCameraParams;
@group(0) @binding(3) var<storage, read_write> out_camera: GpuCameraParams;

@group(0) @binding(4) var<storage, read> in_ray: GpuRay;
@group(0) @binding(5) var<storage, read_write> out_ray: GpuRay;

@group(0) @binding(6) var<storage, read> in_hit: GpuHitRecord;
@group(0) @binding(7) var<storage, read_write> out_hit: GpuHitRecord;

@compute @workgroup_size(1)
fn echo_plane() {
    out_plane.normal = in_plane.normal;
    out_plane.d = in_plane.d;
}

@compute @workgroup_size(1)
fn echo_camera() {
    out_camera.origin = in_camera.origin;
    out_camera.fov_tan = in_camera.fov_tan;
    out_camera.forward = in_camera.forward;
    out_camera.width = in_camera.width;
    out_camera.right = in_camera.right;
    out_camera.height = in_camera.height;
    out_camera.up = in_camera.up;
    out_camera.num_samples = in_camera.num_samples;
}

@compute @workgroup_size(1)
fn echo_ray() {
    out_ray.origin = in_ray.origin;
    out_ray.dir = in_ray.dir;
}

@compute @workgroup_size(1)
fn echo_hit() {
    out_hit.t = in_hit.t;
    out_hit.facet_idx = in_hit.facet_idx;
    out_hit.hit = in_hit.hit;
    out_hit.normal = in_hit.normal;
}

// Frosted girdle finish: the `array<u32>` upload of `renderer::buffers::facet_finish`. It is
// runtime-sized, so it gets a workgroup-parallel entry point (one thread per element) instead of
// the `@workgroup_size(1)` single-instance echo.
@group(0) @binding(8) var<storage, read> in_facet_finishes: array<u32>;
@group(0) @binding(9) var<storage, read_write> out_facet_finishes: array<u32>;

@compute @workgroup_size(64)
fn echo_facet_finishes(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&in_facet_finishes)) {
        return;
    }
    out_facet_finishes[idx] = in_facet_finishes[idx];
}
