// ---------------------------------------------------------------------------------
// Struct layouts: they must match `renderer::buffers` field for field. The layout echo tests check
// the offsets.
// ---------------------------------------------------------------------------------

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

struct GpuTransportParams {
    num_pixels: u32,
    max_bounces: u32,
    sample_offset: u32,
    env_mode: u32,
    l0: f32,
    studio_temp_k: f32,
    studio_spot_mult: f32,
    studio_exposure: f32,
    studio_light_yaw: f32,
    studio_light_pitch: f32,
    pixel_offset: u32,
    // Flag bits: bit 0 enables the per-channel debug writes, bit 1 is unused, bit 2 enables exit
    // collection.
    write_debug_buffers: u32,
    white_balance: vec3<f32>,
    // Selects the tabulated CIE D65 spectrum instead of `blackbody_spectrum`.
    studio_use_d65: u32,
    studio_model: u32,
    // `EnvironmentSource::Studio::backdrop`: radiance at exposure 1 of the lit tray below the
    // girdle plane (`albedo * E_rig / pi`), seen by every escape and by the camera. 0.0 means
    // As Lit, the preset's own ground.
    backdrop: f32,
    // `EnvironmentSource::Studio::surface_glare`: scale of the camera path's first-surface
    // specular reflection, 1.0 for no change.
    surface_glare: f32,
    // `EnvironmentSource::Studio::head_shadow_deg` as two cosines (`head_shadow_cosines`).
    head_shadow_outer_cos: f32,
    head_shadow_inner_cos: f32,
    // `TentParams::flat`, at offset 84.
    tent_flat: f32,
    // Number of concave-tool records appended to `planes` (`CT_RECORD_VECS` entries each, after
    // the real planes); 0 for a planar stone. At offset 88.
    num_tools: u32,
    // `EnvironmentSource::HdrMap::exposure`, read by `hdr_rgb_at_exposure` (env_mode 2 only).
    // At offset 92.
    hdr_exposure: f32,
    // `LightingRigParams::tent` (`TentParams`): the light-tent model's per-preset knobs.
    tent_walls: f32,
    tent_cards: f32,
    tent_spark: f32,
    tent_ground: f32,
    // The HDR map's yaw (`EnvRotation::cos_sin()`), read by `hdr_rot` (env_mode 2 only).
    // (1.0, 0.0) is the identity, which `hdr_world_to_map` and `hdr_map_to_world` skip.
    // The two pad words fill the last 16-byte row (offsets 112..128).
    hdr_rot_cos: f32,
    hdr_rot_sin: f32,
    _pad_hdr_rot0: u32,
    _pad_hdr_rot1: u32,
}

struct DispersionParams {
    model_type: u32,
    param_a: vec4<f32>,
    param_b: vec4<f32>,
    param_c: vec4<f32>,
    c_axis_and_birefringence: vec4<f32>,
    is_anisotropic: u32,
    biaxial_delta_beta_alpha: f32,
    has_biaxial_delta: u32,
    // `.xyz` = `GemMaterial::alpha_axis`, `.w` = 1.0 when set, else 0. Read by `biaxial_axes` only.
    a_axis_and_flag: vec4<f32>,
}

// Biaxial principal-index curves, at byte offset 2720 after the zone table. See
// `renderer::buffers::GpuBiaxialCurves`. `types` is `[alpha_type, gamma_type, has, 0]` with the
// `dispersion_model_type` discriminants. The parameters are encoded like `DispersionParams`
// `param_a` and `param_b`.
struct GpuBiaxialCurves {
    types: vec4<u32>,
    alpha_a: vec4<f32>,
    alpha_b: vec4<f32>,
    gamma_a: vec4<f32>,
    gamma_b: vec4<f32>,
}

// Oriented-inclusion block, at byte offset 2800. See `renderer::buffers::GpuInclusionBlock`:
// header = [family_count, structure_flag, domain_seed, lattice_posed]; fam_dir = world direction
// xyz and kind w; fam_params = [sigma, p, lobe, rho]; lattice = the columns of the domain map's
// `lattice_from_world` (read when lattice_posed is nonzero); scratch_up = [up.xyz, posed] and
// scratch_side = [side.xyz, 0], the directions the polishing scratches are measured against (all
// zero for a stone as designed, which reads as model +Y and +X).
struct GpuInclusionBlock {
    header: vec4<u32>,
    fam_dir: array<vec4<f32>, 4>,
    fam_params: array<vec4<f32>, 4>,
    haze: vec4<f32>,
    structure: array<vec4<f32>, 3>,
    edge: vec4<f32>,
    lattice: array<vec4<f32>, 3>,
    scratch_up: vec4<f32>,
    scratch_side: vec4<f32>,
}

struct GpuGemMaterial {
    dispersion: DispersionParams,
    crystal_system: u32,
    optical_character: u32,
    is_pleochroic: u32,
    o_ray_band_count: u32,
    e_ray_band_count: u32,
    o_ray_bands: array<AbsorptionBand, 8>,
    e_ray_bands: array<AbsorptionBand, 8>,
    scattering_sigma_s: f32,
    scattering_g: f32,
    edge_rounding_radius: f32,
    // Fields are only ever appended, so no existing offset shifts. See
    // `renderer::buffers::GpuGemMaterial`.
    has_beta_ray: u32,
    beta_ray_band_count: u32,
    beta_ray_bands: array<AbsorptionBand, 8>,
    absorption_path_scale: f32,
    has_extraordinary_dispersion: u32,
    extraordinary_model_type: u32,
    extraordinary_param_a: vec4<f32>,
    extraordinary_param_b: vec4<f32>,
}

struct FacetPlane {
    normal: vec3<f32>,
    d: f32,
}

// `HdrEnvDims` and `GpuDistDims` are defined in `transport_physics/05_nee_env_sampling.wgsl`.

@group(0) @binding(0) var<uniform> camera: GpuCameraParams;
@group(0) @binding(1) var<uniform> params: GpuTransportParams;
@group(0) @binding(2) var<storage, read> material: GpuGemMaterial;
@group(0) @binding(3) var<storage, read> planes: array<FacetPlane>;
@group(0) @binding(4) var<storage, read_write> out_xyz: array<f32>;
@group(0) @binding(5) var<storage, read_write> out_radiance: array<f32>;
@group(0) @binding(6) var<storage, read_write> out_lambdas: array<f32>;
@group(0) @binding(7) var<storage, read_write> out_path_pdf: array<f32>;
// `optics::raytracer::FacetFinish` per `planes[i]`. It is a separate binding, parallel to
// `planes`, so `FacetPlane` stays unchanged. `facet_finish::FROSTED` (1) sends the bounce through
// `apply_frosted_bounce`. Any other value, including an out-of-bounds index, is polished (0).
@group(0) @binding(8) var<storage, read> facet_finishes: array<u32>;
// `compat[k]` family bitmask per channel and tuple. Written only with the debug flag, like
// `out_radiance`, `out_lambdas` and `out_path_pdf`.
@group(0) @binding(9) var<storage, read_write> out_compat: array<u32>;
// HDR map texels, row-major `vec4<f32>` (alpha unused), uploaded by
// `renderer::env_map_gpu::HdrEnvGpuData::upload`. These two bindings are always present, whatever
// `params.env_mode` is, because bindings follow static reachability.
@group(0) @binding(10) var<storage, read> hdr_texels: array<vec4<f32>>;
@group(0) @binding(11) var<uniform> hdr_env_dims: HdrEnvDims;
// Flattened `Distribution2D` of the HDR map, layout as in `renderer::env_map_gpu`.
@group(0) @binding(12) var<storage, read> dist_func: array<f32>;
@group(0) @binding(13) var<storage, read> dist_cdf: array<f32>;
@group(0) @binding(14) var<uniform> dist_dims: GpuDistDims;

// The per-facet word is `surface::FacetSurface`: bits 0-1 are the kind (0 ideal, 1 frosted,
// 2 polished with roughness, 3 ground), the rest carry the roughness and scratch parameters. A
// frosted test must mask the kind first.
const FACET_FINISH_KIND_MASK: u32 = 3u;
const FACET_FINISH_FROSTED: u32 = 1u;

// Workgroup-shared plane cache. `intersect_ray` is the hottest loop of the kernel, and every
// thread of a workgroup re-reads the same `planes[]` on every bounce. When there are at most
// `PLANES_SHARED_CAPACITY` planes, `transport_main` copies them into `planes_shared` once per
// workgroup, before the barrier. The values read are the same, only the address space differs, so
// the result is bit-identical. Larger polyhedra read `planes[]` directly. The choice depends on
// `arrayLength(&planes)`, which is fixed for the whole dispatch, so the branch is uniform and the
// barrier in `transport_main` is reached by every thread.
const PLANES_SHARED_CAPACITY: u32 = 128u;

var<workgroup> planes_shared: array<FacetPlane, 128>;

// Reads plane `i` from the address space that `transport_main` filled for this dispatch.
fn plane_at(i: u32, plane_count: u32) -> FacetPlane {
    if (plane_count <= PLANES_SHARED_CAPACITY) {
        return planes_shared[i];
    }
    return planes[i];
}

// Concave tools (`transport_physics/10_concave_tools.wgsl`) are stored in the `planes` binding
// after the real planes: `params.num_tools` records of `CT_RECORD_VECS` entries each. Reusing the
// binding keeps the storage-buffer count unchanged. Loops over the real planes must use
// `scene_plane_count()`, not `arrayLength(&planes)`, which also counts the records.
//
// `params.num_tools` is a uniform, so every branch on it is uniform control flow.
fn scene_plane_count() -> u32 {
    return arrayLength(&planes) - CT_RECORD_VECS * params.num_tools;
}

// Tool `k`, whose records start at `planes[plane_count + CT_RECORD_VECS * k]`. A record is read
// as a `FacetPlane` by shape only: `normal` holds `xyz` and `d` the `w` of the `vec4`.
fn scene_tool(plane_count: u32, k: u32) -> CtTool {
    let base = plane_count + CT_RECORD_VECS * k;
    let r0 = planes[base];
    let r1 = planes[base + 1u];
    let r2 = planes[base + 2u];
    let r3 = planes[base + 3u];
    let r4 = planes[base + 4u];
    return ct_tool_from_vecs(
        vec4<f32>(r0.normal, r0.d),
        vec4<f32>(r1.normal, r1.d),
        vec4<f32>(r2.normal, r2.d),
        vec4<f32>(r3.normal, r3.d),
        vec4<f32>(r4.normal, r4.d),
    );
}
