// Beam refine kernel (beam guide 14.7 item 2, "Subdivision", 11.4 steps 1-2).
//
// Appended after `fragments.wgsl` (it uses that unit's helpers and bindings). One invocation per
// slot on the refine list, i.e. per fragment the first level found not smooth. The fragment's
// pieces are visited depth first in a fixed order: a piece is quadrisected at its centroid and edge
// midpoints (the polygon's own vertices at the first split, quads below), each piece is lit by
// `bm_piece` (centroid rule when smooth, a further split otherwise, the 7-point rule at
// `max_levels`), and the pieces' XYZ are summed in that order into the fragment's slot. There are
// no float atomics and the order depends only on the geometry, so the result is deterministic.
//
// A piece is named by its path from the fragment: the child index at level 1 (4 bits, any vertex of
// the fragment polygon) and at levels 2.. (2 bits each); `bm_build_piece` rebuilds its polygon from
// the fragment polygon each time, so no polygon stack is needed (no function-scope arrays).

// The piece of `src` (n vertices at scratch offset `src`) around vertex `i`: the vertex, the
// midpoint of the edge to the next vertex, the centroid and the midpoint of the previous edge.
fn bm_quad_of(src: u32, n: u32, i: u32, dst: u32) {
    let c = bm_poly_centroid(src, n);
    let here = bm_sv(src, i);
    bm_set(dst, 0u, here);
    bm_set(dst, 1u, (here + bm_sv(src, bm_next(i, n))) * 0.5);
    bm_set(dst, 2u, c);
    bm_set(dst, 3u, (here + bm_sv(src, bm_prev(i, n))) * 0.5);
}

// The bit shift and mask of the child digit at `level` (1-based).
fn bm_digit_shift(level: u32) -> u32 {
    if (level <= 1u) {
        return 0u;
    }
    return 4u + 2u * (level - 2u);
}

fn bm_digit_mask(level: u32) -> u32 {
    if (level <= 1u) {
        return 15u;
    }
    return 3u;
}

// Builds the piece named by `digits` down to `level` from the fragment polygon (`n_root`
// vertices at `BM_ROOT`) and returns the scratch offset of its four vertices.
fn bm_build_piece(n_root: u32, level: u32, digits: u32) -> u32 {
    var src = BM_ROOT;
    var n = n_root;
    var dst = BM_SPARE;
    for (var l = 1u; l <= level; l = l + 1u) {
        let digit = (digits >> bm_digit_shift(l)) & bm_digit_mask(l);
        bm_quad_of(src, n, min(digit, n - 1u), dst);
        src = dst;
        n = 4u;
        dst = select(BM_SPARE, BM_SPARE + 4u, dst == BM_SPARE);
    }
    return src;
}

@compute @workgroup_size(16)
fn bm_refine_main(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
) {
    // `index` runs over the global refine list; the dispatch covers `total` entries from
    // `slot_base` (one chunk), and the entries past the atomic count are not there.
    let chunk_index = gid.y * bm_params.grid_width + gid.x;
    if (chunk_index >= bm_params.total) {
        return;
    }
    let index = bm_params.slot_base + chunk_index;
    if (index >= atomicLoad(&bm_refine_count)) {
        return;
    }
    let slot = bm_refine_list[index];
    let n_root = bm_setup(slot, local_index);
    if (n_root < 3u) {
        bm_slots[slot] = vec4<f32>(0.0, 0.0, 0.0, 0.0);
        return;
    }
    var total = vec3<f32>(0.0, 0.0, 0.0);
    var level = 1u;
    var digits = 0u;
    var finished = false;
    // A 12-vertex root over six levels has at most 12 * (4^6 - 1) / 3 pieces; the bound only guards
    // a logic error.
    for (var visited = 0u; visited < 20000u && !finished; visited = visited + 1u) {
        let off = bm_build_piece(n_root, level, digits);
        let result = bm_piece(off, 4u, level);
        if (result.needs_split && level < bm_params.max_levels) {
            level = level + 1u;
            continue;
        }
        total = total + result.xyz;
        // Next piece in depth-first order: the next sibling, else pop to the parent's next sibling.
        for (var pops = 0u; pops < 8u; pops = pops + 1u) {
            let shift = bm_digit_shift(level);
            let mask = bm_digit_mask(level);
            let digit = (digits >> shift) & mask;
            var children = 4u;
            if (level == 1u) {
                children = n_root;
            }
            if (digit + 1u < children) {
                digits = (digits & ~(mask << shift)) | ((digit + 1u) << shift);
                break;
            }
            digits = digits & ~(mask << shift);
            if (level == 1u) {
                finished = true;
                break;
            }
            level = level - 1u;
        }
    }
    bm_slots[slot] = vec4<f32>(total, 0.0);
}
