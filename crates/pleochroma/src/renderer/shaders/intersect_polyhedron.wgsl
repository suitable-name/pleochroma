// `intersect_polyhedron` kernel, driven by `renderer::gpu::polyhedron_check`.
//
// Translation of `optics::raytracer::intersect_polyhedron`, with the same branch order and the
// same `1e-4` / `1e-7` epsilons. Both the entry and the exit branch are needed: a ray that starts
// inside the solid must report its exit facet, not a miss.

struct FacetPlane {
    normal: vec3<f32>,
    d: f32,
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

@group(0) @binding(0) var<storage, read> planes: array<FacetPlane>;
@group(0) @binding(1) var<storage, read> rays: array<GpuRay>;
@group(0) @binding(2) var<storage, read_write> out_hits: array<GpuHitRecord>;

@compute @workgroup_size(64)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    if (idx >= arrayLength(&rays)) {
        return;
    }
    let ray = rays[idx];

    var t_near: f32 = -1e30;
    var t_far: f32 = 1e30;
    var near_facet: i32 = -1;
    var near_normal: vec3<f32> = vec3<f32>(0.0, 0.0, 0.0);
    var far_facet: i32 = -1;
    var far_normal: vec3<f32> = vec3<f32>(0.0, 0.0, 0.0);

    var result: GpuHitRecord;
    let num_planes = arrayLength(&planes);
    for (var i: u32 = 0u; i < num_planes; i = i + 1u) {
        let p = planes[i];
        let n = p.normal;
        let denom = dot(n, ray.dir);
        let side = p.d + dot(n, ray.origin);
        let numer = -side;

        if (abs(denom) > 1e-7) {
            let t = numer / denom;
            if (denom < 0.0) {
                if (t > t_near) {
                    t_near = t;
                    near_facet = i32(i);
                    near_normal = n;
                }
            } else if (t < t_far) {
                t_far = t;
                far_facet = i32(i);
                far_normal = n;
            }
        } else if (side > 0.0) {
            // Ray (near-)parallel to this plane with the origin outside its half-space: the
            // intersection is empty.
            result.hit = 0u;
            result.t = 0.0;
            result.facet_idx = -1;
            result.normal = vec3<f32>(0.0, 0.0, 0.0);
            out_hits[idx] = result;
            return;
        }
    }

    if (t_near > t_far) {
        // Ray direction is entirely outside the solid's half-space intersection.
        result.hit = 0u;
        result.t = 0.0;
        result.facet_idx = -1;
        result.normal = vec3<f32>(0.0, 0.0, 0.0);
    } else if (t_near > 1e-4) {
        // Origin is outside the solid: the ray enters through the near (entry) plane.
        result.hit = 1u;
        result.t = t_near;
        result.facet_idx = select(0, near_facet, near_facet >= 0);
        result.normal = near_normal;
    } else if (t_far > 1e-4) {
        // Origin inside the solid (every entry plane lies behind the ray): the ray leaves
        // through the far plane.
        result.hit = 1u;
        result.t = t_far;
        result.facet_idx = select(0, far_facet, far_facet >= 0);
        result.normal = far_normal;
    } else {
        result.hit = 0u;
        result.t = 0.0;
        result.facet_idx = -1;
        result.normal = vec3<f32>(0.0, 0.0, 0.0);
    }

    out_hits[idx] = result;
}
