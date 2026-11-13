// GPU struct-layout self-test kernel, driven by `renderer::gpu::layout_check`. Not a physics
// kernel.
//
// The structs must match their Rust counterparts in `renderer::buffers` field for field. The
// `_pad*` fields of the Rust structs are not declared here: WGSL derives offsets and padding from
// field order and types (https://www.w3.org/TR/WGSL/#alignment-and-size), while `#[repr(C)]`
// needs the padding written out. The kernel echoes every field to an output buffer, and
// `layout_check` compares the raw bytes, padding included, which catches any mismatch.

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

struct AbsorptionBand {
    center_nm: f32,
    width_nm: f32,
    peak: f32,
    // Domain this band is Gaussian in: 0 = wavelength, 1 = energy. See `optics::absorption`.
    shape: u32,
}

// `renderer::buffers::GpuBiaxialCurves` (80 bytes), appended at offset 2720.
struct GpuBiaxialCurves {
    types: vec4<u32>,
    alpha_a: vec4<f32>,
    alpha_b: vec4<f32>,
    gamma_a: vec4<f32>,
    gamma_b: vec4<f32>,
}

// `renderer::buffers::GpuInclusionBlock` (224 bytes), appended at offset 2800.
struct GpuInclusionBlock {
    header: vec4<u32>,
    fam_dir: array<vec4<f32>, 4>,
    fam_params: array<vec4<f32>, 4>,
    haze: vec4<f32>,
    structure: array<vec4<f32>, 3>,
    edge: vec4<f32>,
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
    // Appended after the earlier fields so that no existing offset shifts. See
    // `renderer::buffers::GpuGemMaterial`.
    has_beta_ray: u32,
    beta_ray_band_count: u32,
    beta_ray_bands: array<AbsorptionBand, 8>,
    absorption_path_scale: f32,
    has_extraordinary_dispersion: u32,
    extraordinary_model_type: u32,
    extraordinary_param_a: vec4<f32>,
    extraordinary_param_b: vec4<f32>,
    // The zone table (592..2720) is echoed by `zoning/zone_table_echo.wgsl`. Here it is a
    // 2128-byte placeholder (133 vec4) that only places the blocks after it, and it stays zero.
    zones_skip: array<vec4<u32>, 133>,
    biaxial_curves: GpuBiaxialCurves,
    inclusions: GpuInclusionBlock,
}

@group(0) @binding(0) var<storage, read> input_material: GpuGemMaterial;
@group(0) @binding(1) var<storage, read_write> output_material: GpuGemMaterial;

@compute @workgroup_size(1)
fn main() {
    output_material.dispersion.model_type = input_material.dispersion.model_type;
    output_material.dispersion.param_a = input_material.dispersion.param_a;
    output_material.dispersion.param_b = input_material.dispersion.param_b;
    output_material.dispersion.param_c = input_material.dispersion.param_c;
    output_material.dispersion.c_axis_and_birefringence = input_material.dispersion.c_axis_and_birefringence;
    output_material.dispersion.is_anisotropic = input_material.dispersion.is_anisotropic;
    output_material.dispersion.biaxial_delta_beta_alpha = input_material.dispersion.biaxial_delta_beta_alpha;
    output_material.dispersion.has_biaxial_delta = input_material.dispersion.has_biaxial_delta;
    output_material.dispersion.a_axis_and_flag = input_material.dispersion.a_axis_and_flag;

    output_material.crystal_system = input_material.crystal_system;
    output_material.optical_character = input_material.optical_character;
    output_material.is_pleochroic = input_material.is_pleochroic;
    output_material.o_ray_band_count = input_material.o_ray_band_count;
    output_material.e_ray_band_count = input_material.e_ray_band_count;

    for (var i: u32 = 0u; i < 8u; i = i + 1u) {
        output_material.o_ray_bands[i].center_nm = input_material.o_ray_bands[i].center_nm;
        output_material.o_ray_bands[i].width_nm = input_material.o_ray_bands[i].width_nm;
        output_material.o_ray_bands[i].peak = input_material.o_ray_bands[i].peak;
        output_material.o_ray_bands[i].shape = input_material.o_ray_bands[i].shape;
        output_material.e_ray_bands[i].center_nm = input_material.e_ray_bands[i].center_nm;
        output_material.e_ray_bands[i].width_nm = input_material.e_ray_bands[i].width_nm;
        output_material.e_ray_bands[i].peak = input_material.e_ray_bands[i].peak;
        output_material.e_ray_bands[i].shape = input_material.e_ray_bands[i].shape;
    }

    output_material.scattering_sigma_s = input_material.scattering_sigma_s;
    output_material.scattering_g = input_material.scattering_g;
    output_material.edge_rounding_radius = input_material.edge_rounding_radius;

    output_material.has_beta_ray = input_material.has_beta_ray;
    output_material.beta_ray_band_count = input_material.beta_ray_band_count;
    for (var i: u32 = 0u; i < 8u; i = i + 1u) {
        output_material.beta_ray_bands[i].center_nm = input_material.beta_ray_bands[i].center_nm;
        output_material.beta_ray_bands[i].width_nm = input_material.beta_ray_bands[i].width_nm;
        output_material.beta_ray_bands[i].peak = input_material.beta_ray_bands[i].peak;
        output_material.beta_ray_bands[i].shape = input_material.beta_ray_bands[i].shape;
    }

    output_material.absorption_path_scale = input_material.absorption_path_scale;

    output_material.has_extraordinary_dispersion = input_material.has_extraordinary_dispersion;
    output_material.extraordinary_model_type = input_material.extraordinary_model_type;
    output_material.extraordinary_param_a = input_material.extraordinary_param_a;
    output_material.extraordinary_param_b = input_material.extraordinary_param_b;

    output_material.biaxial_curves.types = input_material.biaxial_curves.types;
    output_material.biaxial_curves.alpha_a = input_material.biaxial_curves.alpha_a;
    output_material.biaxial_curves.alpha_b = input_material.biaxial_curves.alpha_b;
    output_material.biaxial_curves.gamma_a = input_material.biaxial_curves.gamma_a;
    output_material.biaxial_curves.gamma_b = input_material.biaxial_curves.gamma_b;

    output_material.inclusions.header = input_material.inclusions.header;
    for (var i: u32 = 0u; i < 4u; i = i + 1u) {
        output_material.inclusions.fam_dir[i] = input_material.inclusions.fam_dir[i];
        output_material.inclusions.fam_params[i] = input_material.inclusions.fam_params[i];
    }
    output_material.inclusions.haze = input_material.inclusions.haze;
    for (var i: u32 = 0u; i < 3u; i = i + 1u) {
        output_material.inclusions.structure[i] = input_material.inclusions.structure[i];
    }
    output_material.inclusions.edge = input_material.inclusions.edge;
}
