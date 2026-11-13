//! The GPU twin of the optics tables: the buffer layout and the source variant.
//!
//! The WGSL side is `renderer/shaders/transport_physics/optics_tables.wgsl`. It reads one
//! storage buffer of `f32` words at [`OPTICS_TABLES_BINDING`]:
//!
//! | words | content |
//! |---|---|
//! | `0` | `1.0` when the tables are enabled, `0.0` otherwise |
//! | `1`, `2` | exponential cells, format version |
//! | `GPU_EXP_BASE ..` | the 129 values of `2^-f` |
//! | `GPU_HEADER_ENTRY ..` | entry branch header: valid, `n_lo`, `n_hi`, `1 / dn`, index cells, key cells, data offset, 0 |
//! | `GPU_HEADER_EXIT ..` | the same for the exit branch |
//! | `GPU_BRANCH_DATA ..` | the node values of the valid branches, entry first |
//!
//! Counts and offsets are stored as `f32` (exact below 2^24, far above any table size here).
//!
//! The shipped transport kernels are not changed. A kernel that should use the tables is built
//! at run time from [`optics_tables_variant`] (or [`exp_table_variant`] for the absorption
//! exponential alone), which routes the Beer-Lambert `exp_poly` calls through `ot_exp_of`, the
//! scalar Fresnel statements through `ot_amplitudes`, and appends the declaration. With the
//! buffer's first word at zero both fall back to the shipped expressions, so a disabled buffer
//! reproduces the shipped kernel.

use super::{EXP_CELLS, OpticsTableSet};

/// `@binding` of the table buffer. Taken by no other shader unit (the staged and sorted-wavefront
/// passes use 150 to 163).
pub const OPTICS_TABLES_BINDING: u32 = 170;

/// Format version stored in word 2 of the buffer.
const GPU_FORMAT_VERSION: f32 = 1.0;

/// First word of the exponential values.
pub const GPU_EXP_BASE: usize = 8;

/// First word of the entry branch header.
pub const GPU_HEADER_ENTRY: usize = 144;

/// First word of the exit branch header.
pub const GPU_HEADER_EXIT: usize = 152;

/// First word of the branch node values; also the length of a disabled buffer.
pub const GPU_BRANCH_DATA: usize = 160;

/// Words of one branch header.
const BRANCH_HEADER_WORDS: usize = 8;

/// The WGSL of the table lookups (binding declaration, `ot_exp_table`, `ot_fresnel`).
///
/// It is self-contained and references nothing from the transport units, so it can be validated
/// and run on its own.
pub const OPTICS_TABLES_WGSL: &str =
    include_str!("../../renderer/shaders/transport_physics/optics_tables.wgsl");

/// The test kernels of the tables (`ot_exp_main`, `ot_fresnel_main`), compiled after
/// [`OPTICS_TABLES_WGSL`]; bindings 171 to 174.
#[cfg(test)]
pub const OPTICS_TABLES_CHECK_WGSL: &str =
    include_str!("../../renderer/shaders/transport_functions/optics_tables_check.wgsl");

/// The routing function [`exp_table_variant`] appends; it needs the shipped `exp_poly`.
const EXP_OF_WGSL: &str = "
// Optics tables: the transmittance exponential from the table when the buffer is enabled and
// the argument is inside the table range; the shipped polynomial otherwise.
fn ot_exp_of(x: f32) -> f32 {
    if (x <= 0.0 && x >= OT_EXP_MIN_ARG && ot_enabled()) {
        return ot_exp_table(x);
    }
    return exp_poly(x);
}
";

/// The shipped definition line of the polynomial exponential, kept as it is.
const EXP_POLY_DEFINITION: &str = "fn exp_poly(x_in: f32) -> f32 {";

/// Stands in for the definition while the call sites are renamed; never present in a shader.
const EXP_POLY_PLACEHOLDER: &str = "\u{1}EXP_POLY_DEFINITION\u{1}";

/// The shipped Fresnel statements the variant routes through `ot_amplitudes`, in groups of two
/// adjacent statements: the first becomes the `ot_amplitudes` lookup, the
/// second reads the p amplitude from it. The groups are the decision (`r_s`, `r_p`), the
/// per-channel reflection (`r_s_k`, `r_p_k`) and the per-channel transmission (`t_s_k`,
/// `t_p_k`) of the CPU sites in `optics/raytracer/refraction`.
const FRESNEL_ROUTES: [[(&str, &str); 2]; 3] = [
    [
        (
            "let r_s = fma(n2_decision, -cos_t, n1 * cos_i) / fma(n2_decision, cos_t, n1 * cos_i);",
            "let ot_amp = ot_amplitudes(n1, n2_decision, cos_i, cos_t);\n    let r_s = ot_amp.r_s;",
        ),
        (
            "let r_p = fma(n1, -cos_t, n2_decision * cos_i) / fma(n1, cos_t, n2_decision * cos_i);",
            "let r_p = ot_amp.r_p;",
        ),
    ],
    [
        (
            "let r_s_k = fma(n2k, -cos_t_k, n1k * cos_i) / fma(n2k, cos_t_k, n1k * cos_i);",
            "let ot_amp_k = ot_amplitudes(n1k, n2k, cos_i, cos_t_k);\n let r_s_k = ot_amp_k.r_s;",
        ),
        (
            "let r_p_k = fma(n1k, -cos_t_k, n2k * cos_i) / fma(n1k, cos_t_k, n2k * cos_i);",
            "let r_p_k = ot_amp_k.r_p;",
        ),
    ],
    [
        (
            "let t_s_k = (2.0 * n1k * cos_i) / fma(n2k, cos_t_k, n1k * cos_i);",
            "let ot_amp_t_k = ot_amplitudes(n1k, n2k, cos_i, cos_t_k);\n let t_s_k = ot_amp_t_k.t_s;",
        ),
        (
            "let t_p_k = (2.0 * n1k * cos_i) / fma(n1k, cos_t_k, n2k * cos_i);",
            "let t_p_k = ot_amp_t_k.t_p;",
        ),
    ],
];

/// `source` with every `exp_poly(` call routed through `ot_exp_of`; `None` when `source` does not
/// define `exp_poly` exactly once (a drifted unit must not silently turn into a plain one) or
/// already holds the routing. The table functions are not appended yet.
fn route_exp_calls(source: &str) -> Option<String> {
    if source.matches(EXP_POLY_DEFINITION).count() != 1
        || source.contains("ot_exp_of(")
        || source.contains(EXP_POLY_PLACEHOLDER)
    {
        return None;
    }
    let protected = source.replacen(EXP_POLY_DEFINITION, EXP_POLY_PLACEHOLDER, 1);
    let routed = protected.replace("exp_poly(", "ot_exp_of(");
    Some(routed.replacen(EXP_POLY_PLACEHOLDER, EXP_POLY_DEFINITION, 1))
}

/// `source` with the shipped Fresnel statements of [`FRESNEL_ROUTES`] replaced; `None` when a
/// group is missing or its two statements do not occur equally often (the unit has drifted).
fn route_fresnel_sites(source: &str) -> Option<String> {
    let mut text = source.to_owned();
    for group in FRESNEL_ROUTES {
        let first = text.matches(group[0].0).count();
        if first == 0 || first != text.matches(group[1].0).count() {
            return None;
        }
        for (from, to) in group {
            text = text.replace(from, to);
        }
    }
    Some(text)
}

/// The routed `text` with the table functions, the binding and `ot_exp_of` appended.
fn finish_variant(mut text: String) -> String {
    text.push('\n');
    text.push_str(OPTICS_TABLES_WGSL);
    text.push_str(EXP_OF_WGSL);
    text
}

/// `source` with every `exp_poly(` call routed through `ot_exp_of`, and the table functions and
/// binding appended.
///
/// `None` when `source` does not define `exp_poly` exactly once (a drifted unit must not silently
/// turn into a plain one) or already holds the variant.
///
/// Only the absorption exponential uses the table; see [`optics_tables_variant`] for the Fresnel
/// sites as well. The table buffer must be bound at [`OPTICS_TABLES_BINDING`]; see
/// [`pack_gpu_words`] and [`pack_disabled_gpu_words`].
#[must_use]
pub fn exp_table_variant(source: &str) -> Option<String> {
    route_exp_calls(source).map(finish_variant)
}

/// [`exp_table_variant`] that also routes the shipped scalar Fresnel sites of the bounce kernels
/// through `ot_amplitudes`.
///
/// The routed sites are the reflect/transmit decision, the per-channel reflection and
/// transmission, and the exit split. `None` when any of them is missing from `source`, so it
/// applies to the megakernel and event-wavefront units but not to a unit without the bounce code.
///
/// With a disabled buffer (first word zero) every routed site computes the shipped closed form.
#[must_use]
pub fn optics_tables_variant(source: &str) -> Option<String> {
    let exp_routed = route_exp_calls(source)?;
    route_fresnel_sites(&exp_routed).map(finish_variant)
}

/// The enabled buffer for `set`, in the layout above.
#[must_use]
pub fn pack_gpu_words(set: &OpticsTableSet) -> Vec<f32> {
    let mut words = vec![0.0f32; GPU_BRANCH_DATA];
    words[0] = 1.0;
    words[1] = EXP_CELLS as f32;
    words[2] = GPU_FORMAT_VERSION;
    let exp_values = set.exp_table().values();
    words[GPU_EXP_BASE..GPU_EXP_BASE + exp_values.len()].copy_from_slice(exp_values);
    for (header, branch) in [
        (GPU_HEADER_ENTRY, set.entry()),
        (GPU_HEADER_EXIT, set.exit()),
    ] {
        let Some(branch) = branch else {
            continue;
        };
        let offset = words.len();
        let fields: [f32; BRANCH_HEADER_WORDS] = [
            1.0,
            branch.n_lo(),
            branch.n_hi(),
            branch.inv_dn(),
            branch.index_cells() as f32,
            branch.key_cells() as f32,
            offset as f32,
            0.0,
        ];
        words[header..header + BRANCH_HEADER_WORDS].copy_from_slice(&fields);
        words.extend_from_slice(branch.values());
    }
    words
}

/// A disabled buffer: the shader reads word 0 as zero and takes the shipped path everywhere.
#[must_use]
pub fn pack_disabled_gpu_words() -> Vec<f32> {
    vec![0.0f32; GPU_BRANCH_DATA]
}

/// Uploads `words` (from [`pack_gpu_words`] or [`pack_disabled_gpu_words`]) as the storage buffer
/// to bind at [`OPTICS_TABLES_BINDING`] of a kernel built with [`exp_table_variant`].
#[cfg(feature = "gpu")]
#[must_use]
pub fn upload_gpu_buffer(device: &wgpu::Device, words: &[f32]) -> wgpu::Buffer {
    crate::renderer::gpu::compute::upload(
        device,
        "pleochroma optics tables",
        words,
        wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
    )
}
