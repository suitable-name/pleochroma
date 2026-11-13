// Path cache phase B on the GPU: the WGSL twin of
// `renderer::path_cache::shade::Shader::shade_sample`. It computes the `kept` value of each tuple
// and decides whether its residual is completed.
// The completion itself is in `shaders/path_cache/complete.wgsl`.
//
// Not a standalone module. It is appended to the megakernel unit (see
// `renderer::gpu::path_cache::source`) and reuses its declarations by concatenation:
//   - `camera` (binding 0) and `params` (binding 1): the megakernel's uniforms, so the environment,
//     white balance, surface glare and chunk bookkeeping are the live pass's;
//   - `out_xyz` (binding 4): one XYZ triple per (pixel, sample) tuple, read by the reduce kernel;
//   - `hdr_texels` / `hdr_env_dims` (bindings 10, 11): the HDR map;
//   - `material` (binding 2): the absorption bands, read through `spectral_absorption`;
//   - the RNG, camera-ray, environment, CMF and white-balance helpers and the spectrum constants.
//
// Own bindings are 180..184, unused by any shader unit: the packed tiles of a slab, the word
// offset of each tile, the real pixel of every virtual pixel, the pass parameters, and the list
// of samples whose residual the completion pass traces.
//
// One thread handles one (pixel, sample) tuple, step for step as the CPU's `shade_sample`:
//   1. the position: `pcb_pick_position`, a base-7 radical inverse rotated per pixel;
//   2. the camera ray: the jitter of the path tracer's sample index equal to the position's index;
//   3. the hero wavelengths of the sample, the path tracer's draw;
//   4. every record of the position: the absorption-free weight at each channel's wavelength
//      (merged records interpolate, per-bin records give their bin's value), times the
//      Beer-Lambert transmittance `exp(-tau)` at that wavelength, `tau` being the record's
//      optical-depth coefficients against the principal absorption spectra; the direction at that
//      wavelength (the stored octahedral direction plus the derivative term, or the camera ray for
//      a DIRECT record) and the environment along it; the glare scale on a GLARE record;
//   5. spectral integration with equal path densities (balance weight 1), white balance, clamp.
//
// Residual: a position's dropped energy is finished by an ordinary path-tracer sample with
// probability `min(1, 8 * residual)`, keeping only the paths no record covers. The draw is the
// CPU's (`hash_u32(seed ^ hash_u32(0 ^ stream))`). Fired samples are appended to `pcb_fired` with
// their tuple index. The completion pass then traces each one and adds it into the tuple's slot
// before the reduce. A renderer without that pass reads the list back and completes it on the host.
//
// No function-scope arrays (AMD/FXC): per-channel values live in pairs of `vec4`s.

const PCB_POSITION_BASE: u32 = 7u;
const PCB_POSITION_ROTATION_STREAM: u32 = 0x2C1B3C6Du;
const PCB_RESIDUAL_FIRE_STREAM: u32 = 0x297A2D39u;
const PCB_RESIDUAL_FLOOR: f32 = 1.0e-4;
const PCB_RESIDUAL_FIRE_GAIN: f32 = 8.0;

const PCB_FLAG_PER_BIN: u32 = 1u;
const PCB_FLAG_GLARE: u32 = 8u;
const PCB_FLAG_DIRECT: u32 = 16u;

const PCB_COMB_FIRST_NM: f32 = 410.0;
const PCB_COMB_STEP_NM: f32 = 50.0;
const PCB_BIN_ORIGIN_NM: f32 = 385.0;
const PCB_REFERENCE_NM: f32 = 560.0;
const PCB_DERIVATIVE_SPAN_NM: f32 = 100.0;

// Words of one exit record (`layout::RECORD_WORDS`): direction, derivative, four weight words,
// flags (word 6), signature (word 7) and three optical-depth coefficients (words 8 to 10).
const PCB_RECORD_WORDS: u32 = 11u;
// Word offset of a record's first optical-depth coefficient (`ExitRecord::depth_bits`).
const PCB_DEPTH_WORD: u32 = 8u;
// Bit position of a record's 8-bit interior bounce count in its flags word.
const PCB_BOUNCE_SHIFT: u32 = 16u;

struct PcbParams {
    // Tile edge in pixels.
    tile: u32,
    // Tiles per frame row.
    tiles_x: u32,
    // Frame width in pixels.
    width: u32,
    // Row-major index of the first tile of the slab; `pcb_tile_offsets[i]` belongs to tile
    // `first_tile + i`.
    first_tile: u32,
    // 1 when a position above the residual floor may append to `pcb_fired`.
    fire_enabled: u32,
    // Capacity of `pcb_fired.items`; a thread never writes past it.
    fired_capacity: u32,
    _pad0: u32,
    _pad1: u32,
}

struct PcbFired {
    count: atomic<u32>,
    // (virtual pixel, absolute sample index, position index in the pixel, chunk tuple index).
    items: array<vec4<u32>>,
}

// The packed tile buffers of the slab, back to back (`TileCache::as_words`).
@group(0) @binding(180) var<storage, read> pcb_words: array<u32>;
// Word offset in `pcb_words` of every tile of the slab.
@group(0) @binding(181) var<storage, read> pcb_tile_offsets: array<u32>;
// Real pixel index of every virtual pixel of the pass.
@group(0) @binding(182) var<storage, read> pcb_pixels: array<u32>;
@group(0) @binding(183) var<uniform> pcb: PcbParams;
@group(0) @binding(184) var<storage, read_write> pcb_fired: PcbFired;

// `shade::pick_position`.
fn pcb_pick_position(pixel: u32, sample_num: u32, count: u32) -> u32 {
    let rotation = low_discrepancy_base2(hash_u32(pixel ^ PCB_POSITION_ROTATION_STREAM));
    let unit = cranley_patterson_rotate(radical_inverse_base(sample_num, PCB_POSITION_BASE), rotation);
    return min(u32(unit * f32(count)), count - 1u);
}

// `record::oct_unpack`.
fn pcb_unpack_oct(word: u32) -> vec2<f32> {
    let x = f32(word & 0xffffu) / 65535.0;
    let y = f32(word >> 16u) / 65535.0;
    return vec2<f32>(fma(x, 2.0, -1.0), fma(y, 2.0, -1.0));
}

// `record::oct_decode`.
fn pcb_oct_decode(p: vec2<f32>) -> vec3<f32> {
    let z = 1.0 - abs(p.x) - abs(p.y);
    var v = vec3<f32>(p.x, p.y, z);
    if (z < 0.0) {
        let a = 1.0 - abs(p.y);
        let b = 1.0 - abs(p.x);
        v = vec3<f32>(select(-a, a, p.x >= 0.0), select(-b, b, p.y >= 0.0), z);
    }
    let len = length(v);
    if (len > 0.0) {
        return v / len;
    }
    return vec3<f32>(0.0, 0.0, 0.0);
}

// `ExitRecord::direction_at`.
fn pcb_record_direction(rec: u32, lambda_nm: f32) -> vec3<f32> {
    let ddir = unpack2x16float(pcb_words[rec + 1u]);
    let shift = (lambda_nm - PCB_REFERENCE_NM) / PCB_DERIVATIVE_SPAN_NM;
    return pcb_oct_decode(pcb_unpack_oct(pcb_words[rec]) + ddir * shift);
}

// Weight slot `i` (0..8) of a record.
fn pcb_weight_slot(rec: u32, i: u32) -> f32 {
    let pair = unpack2x16float(pcb_words[rec + 2u + i / 2u]);
    if ((i & 1u) == 0u) {
        return pair.x;
    }
    return pair.y;
}

// `ExitRecord::weight_at`: the absorption-free weight. `complete.wgsl`'s cover test reads it too.
fn pcb_weight_at(rec: u32, lambda_nm: f32) -> f32 {
    let flags = pcb_words[rec + 6u];
    if ((flags & PCB_FLAG_PER_BIN) != 0u) {
        let bin = (flags >> 8u) & 7u;
        let bin_of = u32(clamp(floor((lambda_nm - PCB_BIN_ORIGIN_NM) / PCB_COMB_STEP_NM), 0.0, 7.0));
        if (bin_of == bin) {
            return pcb_weight_slot(rec, bin);
        }
        return 0.0;
    }
    let position = clamp((lambda_nm - PCB_COMB_FIRST_NM) / PCB_COMB_STEP_NM, 0.0, 7.0);
    let lower_whole = min(floor(position), 6.0);
    let fraction = position - lower_whole;
    let lower = u32(lower_whole);
    return fma(pcb_weight_slot(rec, lower), 1.0 - fraction, pcb_weight_slot(rec, lower + 1u) * fraction);
}

// `ExitRecord::has_depth`: whether the record's branch crossed absorbing material.
fn pcb_has_depth(rec: u32) -> bool {
    return (pcb_words[rec + PCB_DEPTH_WORD] | pcb_words[rec + PCB_DEPTH_WORD + 1u] | pcb_words[rec + PCB_DEPTH_WORD + 2u]) != 0u;
}

// `ExitRecord::optical_depth`: the record's coefficients against the principal absorption
// coefficients `alphas` = (ordinary, extraordinary, beta) of one wavelength.
fn pcb_optical_depth(rec: u32, alphas: vec3<f32>) -> f32 {
    let d_o = bitcast<f32>(pcb_words[rec + PCB_DEPTH_WORD]);
    let d_e = bitcast<f32>(pcb_words[rec + PCB_DEPTH_WORD + 1u]);
    let d_beta = bitcast<f32>(pcb_words[rec + PCB_DEPTH_WORD + 2u]);
    return fma(d_o, alphas.x, fma(d_e, alphas.y, d_beta * alphas.z));
}

// `build::principal_alphas` for one wavelength: the ordinary, extraordinary and beta absorption
// coefficients, from the megakernel's own band sums (`transport_generate_ray` hoists the same).
fn pcb_principal_alphas(lambda_nm: f32) -> vec3<f32> {
    let alpha_o = spectral_absorption(material.o_ray_bands, material.o_ray_band_count, lambda_nm);
    let alpha_e = spectral_absorption(material.e_ray_bands, material.e_ray_band_count, lambda_nm);
    var alpha_beta: f32 = 0.0;
    if (material.has_beta_ray != 0u) {
        alpha_beta = spectral_absorption(material.beta_ray_bands, material.beta_ray_band_count, lambda_nm);
    }
    return vec3<f32>(alpha_o, alpha_e, alpha_beta);
}

// `TileCache::position_residual`: two f16 per word, the even position in the low half.
fn pcb_position_residual(residuals: u32, position: u32) -> f32 {
    let pair = unpack2x16float(pcb_words[residuals + position / 2u]);
    if ((position & 1u) == 0u) {
        return pair.x;
    }
    return pair.y;
}

// The path tracer's per-channel wavelength of a hero draw (`transport_generate_ray`).
fn pcb_lambda(lambda_hero: f32, k: u32) -> f32 {
    let channel_width = SPECTRUM_SPAN / f32(NUM_CHANNELS);
    let offset = fma(f32(k), channel_width, lambda_hero - SPECTRUM_MIN);
    return SPECTRUM_MIN + (offset % SPECTRUM_SPAN);
}

// Where one real pixel's positions live in the slab (`layout.rs`): word indices of its tile's
// position offsets, residuals and records, plus the pixel's first position (tile-local) and count.
struct PcbPixel {
    position_offsets: u32,
    residuals: u32,
    records: u32,
    first_position: u32,
    position_total: u32,
}

// The slab layout of real pixel `pixel` (its tile must be in the slab).
fn pcb_locate(pixel: u32) -> PcbPixel {
    let width = pcb.width;
    let x = pixel % width;
    let y = pixel / width;
    let tile_x = x / pcb.tile;
    let tile_y = y / pcb.tile;
    let tile_index = tile_y * pcb.tiles_x + tile_x;
    let base = pcb_tile_offsets[tile_index - pcb.first_tile];
    let pixel_count = pcb_words[base + 1u];
    let position_count = pcb_words[base + 2u];
    let pixel_offsets = base + 4u;
    let position_offsets = pixel_offsets + pixel_count + 1u;
    let residuals = position_offsets + position_count + 1u;
    let records = residuals + (position_count + 1u) / 2u;
    let x0 = tile_x * pcb.tile;
    let y0 = tile_y * pcb.tile;
    let x1 = min(x0 + pcb.tile, width);
    let local_pixel = (y - y0) * (x1 - x0) + (x - x0);
    let first_position = pcb_words[pixel_offsets + local_pixel];
    let position_total = pcb_words[pixel_offsets + local_pixel + 1u] - first_position;
    return PcbPixel(position_offsets, residuals, records, first_position, position_total);
}

@compute @workgroup_size(64)
fn pcb_phase_b_main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let idx = gid.x;
    let total = params.num_pixels * camera.num_samples;
    if (idx >= total) {
        return;
    }
    let virtual_pixel = idx / camera.num_samples + params.pixel_offset;
    let pixel = pcb_pixels[virtual_pixel];
    let sample_num = (idx % camera.num_samples) + params.sample_offset;

    // The pixel's tile and its place in the tile buffer (`layout.rs`).
    let px = pcb_locate(pixel);
    if (px.position_total == 0u) {
        out_xyz[idx * 3u + 0u] = 0.0;
        out_xyz[idx * 3u + 1u] = 0.0;
        out_xyz[idx * 3u + 2u] = 0.0;
        return;
    }

    // 1. The position. 2. Its camera ray (the jitter of sample index `local`).
    let local = pcb_pick_position(pixel, sample_num, px.position_total);
    let position = px.first_position + local;
    let rot_jx = low_discrepancy_base2(hash_u32(pixel ^ PIXEL_JITTER_X_ROTATION_STREAM));
    let rot_jy = low_discrepancy_base2(hash_u32(pixel ^ PIXEL_JITTER_Y_ROTATION_STREAM));
    let rot_hero = low_discrepancy_base2(hash_u32(pixel ^ HERO_WAVELENGTH_ROTATION_STREAM));
    let jx = cranley_patterson_rotate(low_discrepancy_base2(local), rot_jx) - 0.5;
    let jy = cranley_patterson_rotate(radical_inverse_base(local, 3u), rot_jy) - 0.5;
    let ray = generate_camera_ray(pixel, jx, jy);
    let observer = -ray.dir;

    // 3. The hero wavelengths of the sample.
    let hero_rand = cranley_patterson_rotate(radical_inverse_base(sample_num, 5u), rot_hero);
    let lambda_hero = fma(hero_rand, SPECTRUM_SPAN, SPECTRUM_MIN);

    let key_dir = studio_rig_key_dir(params.studio_light_yaw, params.studio_light_pitch);
    let fill_dir = studio_rig_fill_dir(params.studio_light_yaw, params.studio_light_pitch);
    let sin_lp = sin(params.studio_light_pitch);

    var glare_scale: f32 = 1.0;
    if (params.surface_glare < 1.0) {
        glare_scale = clamp(params.surface_glare, 0.0, 1.0);
    }

    // The principal absorption coefficients of the sample's eight wavelengths.
    var alpha_o_lo = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    var alpha_o_hi = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    var alpha_e_lo = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    var alpha_e_hi = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    var alpha_beta_lo = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    var alpha_beta_hi = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        let alphas = pcb_principal_alphas(pcb_lambda(lambda_hero, k));
        if (k < 4u) {
            alpha_o_lo[k] = alphas.x;
            alpha_e_lo[k] = alphas.y;
            alpha_beta_lo[k] = alphas.z;
        } else {
            alpha_o_hi[k - 4u] = alphas.x;
            alpha_e_hi[k - 4u] = alphas.y;
            alpha_beta_hi[k - 4u] = alphas.z;
        }
    }

    // 4. Every record of the position.
    let first_record = pcb_words[px.position_offsets + position];
    let last_record = pcb_words[px.position_offsets + position + 1u];
    var radiance_lo = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    var radiance_hi = vec4<f32>(0.0, 0.0, 0.0, 0.0);
    for (var r: u32 = first_record; r < last_record; r = r + 1u) {
        let rec = px.records + r * PCB_RECORD_WORDS;
        let flags = pcb_words[rec + 6u];
        var scale: f32 = 1.0;
        if ((flags & PCB_FLAG_GLARE) != 0u) {
            scale = glare_scale;
        }
        let direct = (flags & PCB_FLAG_DIRECT) != 0u;
        let absorbs = pcb_has_depth(rec);
        for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
            let lambda = pcb_lambda(lambda_hero, k);
            var weight: f32 = pcb_weight_at(rec, lambda) * scale;
            if (absorbs && weight > 0.0) {
                var alphas = vec3<f32>(alpha_o_lo[k % 4u], alpha_e_lo[k % 4u], alpha_beta_lo[k % 4u]);
                if (k >= 4u) {
                    alphas = vec3<f32>(alpha_o_hi[k - 4u], alpha_e_hi[k - 4u], alpha_beta_hi[k - 4u]);
                }
                weight = weight * exp(-pcb_optical_depth(rec, alphas));
            }
            if (weight > 0.0) {
                var dir = ray.dir;
                if (!direct) {
                    dir = pcb_record_direction(rec, lambda);
                }
                let env = sample_environment_with_rig(dir, lambda, key_dir, fill_dir, sin_lp, observer);
                if (k < 4u) {
                    radiance_lo[k] = fma(weight, env, radiance_lo[k]);
                } else {
                    radiance_hi[k - 4u] = fma(weight, env, radiance_hi[k - 4u]);
                }
            }
        }
    }

    // 5. Equal-density spectral integration (`integrate_channels_to_xyz_family` with every
    // `path_pdf` 1 and every family complete has balance weight 1), white balance, clamp.
    var xyz = vec3<f32>(0.0, 0.0, 0.0);
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        var radiance_k: f32 = 0.0;
        if (k < 4u) {
            radiance_k = radiance_lo[k];
        } else {
            radiance_k = radiance_hi[k - 4u];
        }
        let cmf = cie_1931_cmf(pcb_lambda(lambda_hero, k));
        xyz = xyz + cmf * (radiance_k * NORM_FACTOR);
    }
    if (params.env_mode == 1u) {
        xyz = apply_von_kries_white_balance(xyz, params.white_balance);
    }
    xyz = max(xyz, vec3<f32>(0.0, 0.0, 0.0));
    out_xyz[idx * 3u + 0u] = xyz.x;
    out_xyz[idx * 3u + 1u] = xyz.y;
    out_xyz[idx * 3u + 2u] = xyz.z;

    // The residual: decide here; the completion pass traces it.
    if (pcb.fire_enabled != 0u) {
        let residual = pcb_position_residual(px.residuals, position);
        if (residual >= PCB_RESIDUAL_FLOOR) {
            let probability = min(residual * PCB_RESIDUAL_FIRE_GAIN, 1.0);
            let seed0 = hash_u32((pixel * 0x9e3779b9u) ^ (sample_num * 0x85ebca6bu));
            let draw = f32(hash_u32(seed0 ^ hash_u32(0u ^ PCB_RESIDUAL_FIRE_STREAM))) / 4294967295.0;
            if (!(draw > probability)) {
                let slot = atomicAdd(&pcb_fired.count, 1u);
                if (slot < pcb.fired_capacity) {
                    pcb_fired.items[slot] = vec4<u32>(virtual_pixel, sample_num, local, idx);
                }
            }
        }
    }
}
