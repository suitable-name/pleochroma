// Optics lookup tables: the GPU twin of `optics::tables` (CPU: `ExpTable`, `FresnelBranch`).
//
// NOT one of the `TRANSPORT_PHYSICS_PIECES` build.rs concatenates, so the shipped transport units
// do not contain it. A kernel that should use the tables is built at run time from
// `optics::tables::exp_table_variant`, which appends this file; the optics-tables test kernels
// (`transport_functions/optics_tables_check.wgsl`) are compiled after it.
//
// The buffer layout is documented in `optics/tables/gpu.rs`. Word 0 is 1.0 when the tables are
// enabled; with 0.0 every function here reports "not served" and `ot_exp_of` falls back to
// `exp_poly`. No function-scope array, every `var` initialised.
//
// Binding 170 is not used by any other shader unit.
@group(0) @binding(170) var<storage, read> optics_tables: array<f32>;

const OT_LOG2E: f32 = 1.4426950408889634;
const OT_EXP_BASE: u32 = 8u;
const OT_EXP_CELLS: u32 = 128u;
const OT_EXP_MIN_ARG: f32 = -87.0;
const OT_HDR_ENTRY: u32 = 144u;
const OT_HDR_EXIT: u32 = 152u;
const OT_MIN_WORDS: u32 = 160u;

fn ot_enabled() -> bool {
    return arrayLength(&optics_tables) >= OT_MIN_WORDS && optics_tables[0] == 1.0;
}

fn ot_lerp(a: f32, b: f32, t: f32) -> f32 {
    return fma(t, b - a, a);
}

// `ExpTable::exp` for a in [OT_EXP_MIN_ARG, 0]: 2^-n by exponent bits times the interpolated
// 2^-f. The caller guarantees the range.
fn ot_exp_table(a: f32) -> f32 {
    let x = -a * OT_LOG2E;
    let n = floor(x);
    let pos = (x - n) * f32(OT_EXP_CELLS);
    let cell = min(u32(pos), OT_EXP_CELLS - 1u);
    let t = pos - f32(cell);
    let lo = optics_tables[OT_EXP_BASE + cell];
    let hi = optics_tables[OT_EXP_BASE + cell + 1u];
    let pow2 = bitcast<f32>(u32(127 - i32(n)) << 23u);
    return fma(t, hi - lo, lo) * pow2;
}

struct OtFresnel {
    // 1u when a table served the inputs; the amplitudes are meaningful only then.
    ok: u32,
    r_s: f32,
    r_p: f32,
    t_s: f32,
    t_p: f32,
}

// `OpticsTableSet::fresnel`: the four Fresnel amplitudes of an interface with air on one side
// (n1 == 1 entry, n2 == 1 exit), or ok == 0u when no table serves the inputs.
fn ot_fresnel(n1: f32, n2: f32, cos_i: f32, cos_t: f32) -> OtFresnel {
    var res = OtFresnel(0u, 0.0, 0.0, 0.0, 0.0);
    let entry_side = n1 == 1.0 && n2 > 1.0;
    let exit_side = n2 == 1.0 && n1 > 1.0;
    if (!entry_side && !exit_side) {
        return res;
    }
    if (!ot_enabled()) {
        return res;
    }
    let hdr = select(OT_HDR_EXIT, OT_HDR_ENTRY, entry_side);
    let n = select(n1, n2, entry_side);
    let u = select(cos_t, cos_i, entry_side);
    if (optics_tables[hdr] != 1.0) {
        return res;
    }
    let n_lo = optics_tables[hdr + 1u];
    let n_hi = optics_tables[hdr + 2u];
    let inv_dn = optics_tables[hdr + 3u];
    let index_cells = optics_tables[hdr + 4u];
    let key_cells = optics_tables[hdr + 5u];
    let data = u32(optics_tables[hdr + 6u]);
    if (!(n >= n_lo && n <= n_hi && u >= 0.0 && u <= 1.0)) {
        return res;
    }
    let pn = (n - n_lo) * inv_dn;
    let pu = u * key_cells;
    let j = min(u32(pn), u32(index_cells) - 1u);
    let i = min(u32(pu), u32(key_cells) - 1u);
    let tn = pn - f32(j);
    let tu = pu - f32(i);
    let stride = u32(key_cells) + 1u;
    let lo = data + 2u * (j * stride + i);
    let hi = lo + 2u * stride;
    let a = ot_lerp(
        ot_lerp(optics_tables[lo], optics_tables[lo + 2u], tu),
        ot_lerp(optics_tables[hi], optics_tables[hi + 2u], tu),
        tn,
    );
    let b = ot_lerp(
        ot_lerp(optics_tables[lo + 1u], optics_tables[lo + 3u], tu),
        ot_lerp(optics_tables[hi + 1u], optics_tables[hi + 3u], tu),
        tn,
    );
    res.ok = 1u;
    if (entry_side) {
        res.r_s = fma(-n, cos_t, cos_i) * a;
        res.r_p = fma(n, cos_i, -cos_t) * b;
        res.t_s = 2.0 * cos_i * a;
        res.t_p = 2.0 * cos_i * b;
    } else {
        res.r_s = fma(n, cos_i, -cos_t) * b;
        res.r_p = fma(-n, cos_t, cos_i) * a;
        res.t_s = 2.0 * n * cos_i * b;
        res.t_p = 2.0 * n * cos_i * a;
    }
    return res;
}

// The four amplitudes of an interface: from the table when one serves it, else the closed forms
// the shipped kernels compute inline (the same `fma` forms, so a disabled buffer reproduces them).
// This is what `optics::tables::optics_tables_variant` substitutes at the shipped Fresnel sites.
fn ot_amplitudes(n1: f32, n2: f32, cos_i: f32, cos_t: f32) -> OtFresnel {
    var res = ot_fresnel(n1, n2, cos_i, cos_t);
    if (res.ok == 0u) {
        res.r_s = fma(n2, -cos_t, n1 * cos_i) / fma(n2, cos_t, n1 * cos_i);
        res.r_p = fma(n1, -cos_t, n2 * cos_i) / fma(n1, cos_t, n2 * cos_i);
        res.t_s = (2.0 * n1 * cos_i) / fma(n2, cos_t, n1 * cos_i);
        res.t_p = (2.0 * n1 * cos_i) / fma(n1, cos_t, n2 * cos_i);
    }
    return res;
}
