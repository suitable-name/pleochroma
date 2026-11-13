// The sorted wavefront (`renderer::gpu::frame::sorted`): the megakernel's bounce split in two
// halves, each run by kernels that see only the rays that need it. A pool of ray slots is refilled
// with new tuples while the chunk has tuples left.
//
// A bounce is `transport_bounce_step` cut after its surface phase. The front half (`sw_front`)
// is the extend, scatter and surface phases; the surface picks the arm. The back half is that
// arm, the deferred exit splits and the tail (`sw_after_arm`). The front kernel (`sw_front_main`)
// runs the front half for the rays of the front queue, among them the new tuples of the round.
// An arm kernel (`sw_arm_*`, one per arm) runs the back half for the rays of its queue and then,
// in the same thread, the front half of their next bounce. Every kernel files each ray into the
// queue of what it does next (`sw_route`): an arm queue, the front queue (the bounce ended in the
// medium) or none (the path ended: it is finalized into the output slot of its tuple and its slot
// goes to the free queue). One load and one store of the ray per bounce; each bounce of the
// megakernel's loop takes one round.
//
// Round r reads the queues of side r & 1 and fills the other side. `sw_prepare`
// (`sorted_prepare.wgsl`) opens it: new tuples for the freed slots, the indirect arguments.
// After the rounds the host encodes, `sw_prepare_drain` and `sw_drain_main` finish what is left
// (queued rays, tuples not yet started) one thread per path, like the megakernel. The number of
// rounds is a speed setting, never a correctness one.
//
// A sample does not depend on the schedule: the state is stored exactly, every draw is keyed by
// the tuple's seed and its own bounce index, the estimator is bound per invocation
// (`transport_generate_ray`, `ew_load`), and `out_xyz[tuple]` is written once, by the
// kernel that ends the path. Which round starts which tuple follows from counts (the path lengths
// of the earlier tuples), not from the order of atomics, so two runs compute every bounce in the
// same kernel and agree bit for bit. Against the megakernel a sample differs by compiler rounding
// between kernels only.
//
// Ray state: the event wavefront's records (`00_common.wgsl`: core, channels, hit), indexed by
// slot. `ray_meta.y` is the seed, `ray_meta.z` the index of the ray's current bounce (the event
// wavefront's tag word), `ray_meta.w` the tuple, `observer.w` is unused (0).

@group(0) @binding(160) var<uniform> sw: SwParams;
@group(0) @binding(161) var<storage, read_write> sw_queues: array<u32>;
@group(0) @binding(162) var<storage, read_write> sw_counts: array<atomic<u32>>;

// What the front half of a bounce leaves the ray to do.
const SW_NEXT_FINALIZE: u32 = 0u;
const SW_NEXT_FRONT: u32 = 1u;
const SW_NEXT_ARM: u32 = 2u;

// A path's seed and observer: what a kernel needs beside the stored ray.
struct SwStart {
    seed0: u32,
    observer: vec3<f32>,
}

// Touches every binding of the transport kernels on a branch no chunk takes (`sw.pool` is never
// 0xFFFFFFFF), so every entry point reaches exactly the bindings the host binds
// (`frame::sorted::TRANSPORT_SLOTS`) and one table serves them all.
fn sw_reach() {
    if (sw.pool != 0xFFFFFFFFu) {
        return;
    }
    let words = camera.num_samples + params.num_pixels + material.crystal_system + facet_finishes[0]
        + out_compat[0] + hdr_env_dims.width + dist_dims.width + ray_core[0].ray_meta.x
        + sw_queues[0] + atomicLoad(&sw_counts[0]);
    let floats = planes[0].d + out_radiance[0] + out_lambdas[0] + out_path_pdf[0] + hdr_texels[0].x
        + dist_func[0] + dist_cdf[0] + ray_chan[0].radiance[0] + ray_hit[0].hit_point.w;
    out_xyz[0] = f32(words) + floats;
}

// Appends `slot` to `queue`. A queue cannot overflow (a round files each live ray once, and the
// pool holds them all); the guard keeps a wrong count from writing into the next queue.
fn sw_push(queue: u32, slot: u32) {
    let at = atomicAdd(&sw_counts[queue], 1u);
    if (at < sw.pool) {
        sw_queues[queue * sw.pool + at] = slot;
    } else {
        _ = atomicAdd(&sw_counts[SW_C_OVERFLOW], 1u);
    }
}

// The camera ray of `tuple` into `*s`, as `ew_generate` and `transport_main` start it.
// `transport_generate_ray` binds the estimator of this invocation.
fn sw_start(tuple: u32, s: ptr<function, BounceRay>) -> SwStart {
    let gen = transport_generate_ray(tuple);
    (*s).origin = gen.origin;
    (*s).dir = gen.dir;
    (*s).k = gen.dir;
    (*s).prev_plane_normal = vec3<f32>(0.0, 0.0, 0.0);
    (*s).pending_light_mis_dir = vec3<f32>(0.0, 0.0, 0.0);
    (*s).nee_xyz = vec3<f32>(0.0, 0.0, 0.0);
    (*s).pending_light_mis = 0.0;
    (*s).inside_gem = false;
    (*s).is_extraordinary = false;
    (*s).have_prev_plane_normal = false;
    (*s).path_escaped = false;
    (*s).lambdas = gen.lambdas;
    for (var k: u32 = 0u; k < NUM_CHANNELS; k = k + 1u) {
        (*s).stokes[k] = vec4<f32>(1.0, 0.0, 0.0, 0.0);
        (*s).radiance[k] = 0.0;
        (*s).path_pdf[k] = 1.0;
        (*s).split_radiance[k] = 0.0;
        (*s).compat[k] = 0xFFu;
    }
    return SwStart(gen.seed0, -gen.dir);
}

// The seed and observer stored with the ray of `slot`.
fn sw_stored_start(slot: u32) -> SwStart {
    return SwStart(ray_core[slot].ray_meta.y, ray_core[slot].observer.xyz);
}

// Stores the ray of `slot` with the index of its current bounce.
fn sw_store(slot: u32, s: ptr<function, BounceRay>, bounce: u32) {
    ew_store(slot, *s);
    ray_core[slot].ray_meta.z = bounce;
}

// The words a new ray stores once: observer, seed, tuple (`ew_generate`'s).
fn sw_store_start(slot: u32, tuple: u32, start: SwStart) {
    ray_core[slot].observer = vec4<f32>(start.observer, 0.0);
    ray_core[slot].ray_meta.y = start.seed0;
    ray_core[slot].ray_meta.w = tuple;
}

// The hit record the arm reads from the front half (`ew_hit_store` without the split fields,
// which only the arm writes and the same thread reads).
fn sw_hit_store(slot: u32, h: ptr<function, BounceHit>) {
    ray_hit[slot].hit_point = vec4<f32>((*h).hit_point, (*h).hit.t);
    ray_hit[slot].hit_normal = vec4<f32>((*h).hit.normal, 0.0);
    ray_hit[slot].shading_normal = vec4<f32>((*h).shading_normal, 0.0);
    ray_hit[slot].wave_dir_at_bounce = vec4<f32>((*h).wave_dir_at_bounce, 0.0);
    ray_hit[slot].phase_dir_this_check = vec4<f32>((*h).phase_dir_this_check, (*h).phase_pdf_this_check);
    ray_hit[slot].misc = vec4<u32>(
        select(0u, 1u, (*h).hit.hit),
        select(0u, 1u, (*h).pre_bounce_inside_gem),
        (*h).finish,
        (*h).arm,
    );
    ray_hit[slot].split = vec4<u32>(0u, 0u, (*h).hit.facet_idx, 0u);
}

// `sw_hit_store`'s record back, the split fields blank (`ew_hit_load` without them).
fn sw_hit_load(slot: u32) -> BounceHit {
    var h: BounceHit = bounce_hit_blank();
    let hit_point = ray_hit[slot].hit_point;
    let phase = ray_hit[slot].phase_dir_this_check;
    let misc = ray_hit[slot].misc;
    h.hit.hit = misc.x != 0u;
    h.hit.t = hit_point.w;
    h.hit.normal = ray_hit[slot].hit_normal.xyz;
    h.hit.facet_idx = ray_hit[slot].split.z;
    h.hit_point = hit_point.xyz;
    h.shading_normal = ray_hit[slot].shading_normal.xyz;
    h.wave_dir_at_bounce = ray_hit[slot].wave_dir_at_bounce.xyz;
    h.phase_dir_this_check = phase.xyz;
    h.phase_pdf_this_check = phase.w;
    h.pre_bounce_inside_gem = misc.y != 0u;
    h.finish = misc.z;
    h.arm = misc.w;
    return h;
}

// The path of `tuple` into its output slot, as `transport_main` ends; with `SW_FLAG_CHECK` it is
// also counted and checksummed.
fn sw_finalize(tuple: u32, s: ptr<function, BounceRay>) {
    // `transport_finalize_ray` folds the staged exit splits into the radiance it is given by
    // pointer: the one function-scope array of the sorted wavefront.
    var radiance: array<f32, 8> = (*s).radiance;
    transport_finalize_ray(
        tuple, (*s).lambdas, &radiance, (*s).split_radiance, (*s).path_pdf, (*s).compat,
        (*s).path_escaped, (*s).nee_xyz,
    );
    if ((sw.flags & SW_FLAG_CHECK) != 0u) {
        _ = atomicAdd(&sw_counts[SW_C_FINALIZED], 1u);
        _ = atomicAdd(&sw_counts[SW_C_SUM], tuple);
        _ = atomicXor(&sw_counts[SW_C_XOR], hash_u32(tuple));
    }
}

// The path ended: its output, and its slot to the next round's free queue.
fn sw_retire(slot: u32, tuple: u32, side: u32, s: ptr<function, BounceRay>) {
    sw_finalize(tuple, s);
    sw_push(sw_q_free(1u - side), slot);
}

// The front half of bounce `bounce`: `transport_bounce_step` up to its arm. A zoned stone takes
// the megakernel's own surface phase (`build.rs` patches the per-zone absorption into it); every
// other stone the event wavefront's, which is the same statements with the arm chosen from the
// hero channel alone (the arm kernels compute the full index setup themselves).
fn sw_front(
    s: ptr<function, BounceRay>,
    h: ptr<function, BounceHit>,
    rc: RayConstants,
    bounce: u32,
    seed0: u32,
    observer: vec3<f32>,
) -> u32 {
    var r = bounce_extend(s, h, rc, bounce, seed0, observer);
    if (r == PHASE_NEXT) {
        r = bounce_scatter(s, h, rc, bounce, seed0, observer);
    }
    if (r == PHASE_TERMINATE) {
        return SW_NEXT_FINALIZE;
    }
    if (r == PHASE_END_STEP) {
        return SW_NEXT_FRONT;
    }
    if (material.zones.header.zone_count > 0u) {
        _ = bounce_surface(s, h, rc, bounce, seed0, observer);
    } else {
        _ = ew_surface_event(s, h, rc, bounce, seed0);
    }
    return SW_NEXT_ARM;
}

// The back half after the arm returned `arm_result`, as `transport_bounce_step` ends a bounce:
// the deferred exit splits and the tail while the path lives. True when the path ended.
fn sw_after_arm(
    s: ptr<function, BounceRay>,
    h: ptr<function, BounceHit>,
    rc: RayConstants,
    arm_result: u32,
    bounce: u32,
    seed0: u32,
    observer: vec3<f32>,
) -> bool {
    var r = arm_result;
    if (r == PHASE_NEXT) {
        r = bounce_resolve_splits(s, h, rc, observer);
    }
    if (r == PHASE_NEXT) {
        r = bounce_tail(s, h, rc, bounce, seed0);
    }
    return r == PHASE_TERMINATE;
}

// Files the ray of `slot` after the front half of bounce `bounce` returned `next`: finalized
// (also when the bounce that ended in the medium was the last one), into the next round's front
// queue with the following bounce, or with its hit record into the queue of its arm. True when
// the ray was stored.
fn sw_route(
    slot: u32,
    tuple: u32,
    side: u32,
    bounce: u32,
    next: u32,
    s: ptr<function, BounceRay>,
    h: ptr<function, BounceHit>,
) -> bool {
    let other = 1u - side;
    if (next == SW_NEXT_FINALIZE || (next == SW_NEXT_FRONT && bounce + 1u >= params.max_bounces)) {
        sw_retire(slot, tuple, side, s);
        return false;
    }
    if (next == SW_NEXT_FRONT) {
        sw_store(slot, s, bounce + 1u);
        sw_push(sw_q_front(other), slot);
        return true;
    }
    sw_store(slot, s, bounce);
    sw_hit_store(slot, h);
    sw_push(sw_q_arm(other, clamp((*h).arm, ARM_FROSTED, ARM_GENERIC)), slot);
    return true;
}

// An arm kernel's work after its arm: the rest of the bounce, then the front half of the next
// bounce in this thread and the ray filed for the next round.
fn sw_arm_finish(
    slot: u32,
    side: u32,
    ray_tag: vec4<u32>,
    start: SwStart,
    rc: RayConstants,
    arm_result: u32,
    s: ptr<function, BounceRay>,
    h: ptr<function, BounceHit>,
) {
    let bounce = ray_tag.z;
    if (sw_after_arm(s, h, rc, arm_result, bounce, start.seed0, start.observer)
        || bounce + 1u >= params.max_bounces) {
        sw_retire(slot, ray_tag.w, side, s);
        return;
    }
    var next_hit: BounceHit = bounce_hit_blank();
    let next = sw_front(s, &next_hit, rc, bounce + 1u, start.seed0, start.observer);
    _ = sw_route(slot, ray_tag.w, side, bounce + 1u, next, s, &next_hit);
}

// The slot invocation `index` of the kernel of `arm` takes this round, or `SW_NO_SLOT` past the
// end of the queue.
fn sw_arm_slot(side: u32, arm: u32, index: u32) -> u32 {
    let queue = sw_q_arm(side, arm);
    if (index >= min(atomicLoad(&sw_counts[queue]), sw.pool)) {
        return SW_NO_SLOT;
    }
    return sw_queues[queue * sw.pool + index];
}

// The front kernel: the rays of the front queue (filed by the last round), then the new tuples of
// this round, each into a freed slot.
@compute @workgroup_size(64)
fn sw_front_main(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
) {
    sw_reach();
    ew_fill_planes_shared(local_index);
    let side = atomicLoad(&sw_counts[SW_C_SIDE]);
    let queue = sw_q_front(side);
    if (gid.x >= min(atomicLoad(&sw_counts[queue]), sw.pool)) {
        return;
    }
    let keep = atomicLoad(&sw_counts[SW_C_KEEP]);
    let fresh = gid.x >= keep;
    var s: BounceRay = BounceRay();
    var slot: u32 = 0u;
    var tuple: u32 = 0u;
    var bounce: u32 = 0u;
    var start: SwStart = SwStart(0u, vec3<f32>(0.0, 0.0, 0.0));
    if (fresh) {
        let j = gid.x - keep;
        tuple = atomicLoad(&sw_counts[SW_C_GEN_BASE]) + j;
        slot = j;
        if (atomicLoad(&sw_counts[SW_C_GEN_IDENTITY]) == 0u) {
            slot = sw_queues[sw_q_free(side) * sw.pool + j];
        }
        start = sw_start(tuple, &s);
    } else {
        slot = sw_queues[queue * sw.pool + gid.x];
        s = ew_load(slot);
        let ray_tag = ray_core[slot].ray_meta;
        tuple = ray_tag.w;
        bounce = ray_tag.z;
        start = SwStart(ray_tag.y, ray_core[slot].observer.xyz);
    }
    if (bounce >= params.max_bounces) {
        // A bounce cap of 0: the megakernel finalizes the camera ray untraced.
        sw_retire(slot, tuple, side, &s);
        return;
    }
    let rc = transport_hoist_ray_constants(s.lambdas);
    var h: BounceHit = bounce_hit_blank();
    let next = sw_front(&s, &h, rc, bounce, start.seed0, start.observer);
    if (sw_route(slot, tuple, side, bounce, next, &s, &h) && fresh) {
        sw_store_start(slot, tuple, start);
    }
}

@compute @workgroup_size(64)
fn sw_arm_frosted(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
) {
    sw_reach();
    ew_fill_planes_shared(local_index);
    let side = atomicLoad(&sw_counts[SW_C_SIDE]);
    let slot = sw_arm_slot(side, ARM_FROSTED, gid.x);
    if (slot == SW_NO_SLOT) {
        return;
    }
    var s: BounceRay = ew_load(slot);
    var h: BounceHit = sw_hit_load(slot);
    let ray_tag = ray_core[slot].ray_meta;
    let start = sw_stored_start(slot);
    let rc = transport_hoist_ray_constants(s.lambdas);
    let r = bounce_arm_frosted(&s, &h, rc, ray_tag.z, start.seed0, start.observer);
    sw_arm_finish(slot, side, ray_tag, start, rc, r, &s, &h);
}

@compute @workgroup_size(64)
fn sw_arm_tir(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
) {
    sw_reach();
    ew_fill_planes_shared(local_index);
    let side = atomicLoad(&sw_counts[SW_C_SIDE]);
    let slot = sw_arm_slot(side, ARM_TIR, gid.x);
    if (slot == SW_NO_SLOT) {
        return;
    }
    var s: BounceRay = ew_load(slot);
    var h: BounceHit = sw_hit_load(slot);
    let ray_tag = ray_core[slot].ray_meta;
    let start = sw_stored_start(slot);
    let rc = transport_hoist_ray_constants(s.lambdas);
    let r = bounce_arm_tir(&s, &h, rc, ray_tag.z, start.seed0, start.observer);
    sw_arm_finish(slot, side, ray_tag, start, rc, r, &s, &h);
}

@compute @workgroup_size(64)
fn sw_arm_uni_entry(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
) {
    sw_reach();
    ew_fill_planes_shared(local_index);
    let side = atomicLoad(&sw_counts[SW_C_SIDE]);
    let slot = sw_arm_slot(side, ARM_UNI_ENTRY, gid.x);
    if (slot == SW_NO_SLOT) {
        return;
    }
    var s: BounceRay = ew_load(slot);
    var h: BounceHit = sw_hit_load(slot);
    let ray_tag = ray_core[slot].ray_meta;
    let start = sw_stored_start(slot);
    let rc = transport_hoist_ray_constants(s.lambdas);
    let r = bounce_arm_uni_entry(&s, &h, rc, ray_tag.z, start.seed0, start.observer);
    sw_arm_finish(slot, side, ray_tag, start, rc, r, &s, &h);
}

@compute @workgroup_size(64)
fn sw_arm_uni_internal(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
) {
    sw_reach();
    ew_fill_planes_shared(local_index);
    let side = atomicLoad(&sw_counts[SW_C_SIDE]);
    let slot = sw_arm_slot(side, ARM_UNI_INTERNAL, gid.x);
    if (slot == SW_NO_SLOT) {
        return;
    }
    var s: BounceRay = ew_load(slot);
    var h: BounceHit = sw_hit_load(slot);
    let ray_tag = ray_core[slot].ray_meta;
    let start = sw_stored_start(slot);
    let rc = transport_hoist_ray_constants(s.lambdas);
    let r = bounce_arm_uni_internal(&s, &h, rc, ray_tag.z, start.seed0, start.observer);
    sw_arm_finish(slot, side, ray_tag, start, rc, r, &s, &h);
}

@compute @workgroup_size(64)
fn sw_arm_generic(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
) {
    sw_reach();
    ew_fill_planes_shared(local_index);
    let side = atomicLoad(&sw_counts[SW_C_SIDE]);
    let slot = sw_arm_slot(side, ARM_GENERIC, gid.x);
    if (slot == SW_NO_SLOT) {
        return;
    }
    var s: BounceRay = ew_load(slot);
    var h: BounceHit = sw_hit_load(slot);
    let ray_tag = ray_core[slot].ray_meta;
    let start = sw_stored_start(slot);
    let rc = transport_hoist_ray_constants(s.lambdas);
    let r = bounce_arm_generic(&s, &h, rc, ray_tag.z, start.seed0, start.observer);
    sw_arm_finish(slot, side, ray_tag, start, rc, r, &s, &h);
}

// The drain: one thread per path the rounds left, in the order front leftovers, each arm's
// leftovers, then the tuples not yet started. Each path runs to its end in its thread with the
// same two halves, like `transport_main`'s loop. Nothing is filed or stored.
@compute @workgroup_size(64)
fn sw_drain_main(
    @builtin(global_invocation_id) gid: vec3<u32>,
    @builtin(local_invocation_index) local_index: u32,
) {
    sw_reach();
    ew_fill_planes_shared(local_index);
    if (gid.x >= atomicLoad(&sw_counts[SW_C_DRAIN_TOTAL])) {
        return;
    }
    let side = atomicLoad(&sw_counts[SW_C_DRAIN_SIDE]);
    let front_end = atomicLoad(&sw_counts[SW_C_DRAIN_FRONT_END]);
    var s: BounceRay = BounceRay();
    var h: BounceHit = bounce_hit_blank();
    var start: SwStart = SwStart(0u, vec3<f32>(0.0, 0.0, 0.0));
    var slot: u32 = SW_NO_SLOT;
    var tuple: u32 = 0u;
    var bounce: u32 = 0u;
    var arm_due: bool = false;
    if (gid.x < front_end) {
        slot = sw_queues[sw_q_front(side) * sw.pool + gid.x];
    } else {
        var arm_start: u32 = front_end;
        for (var arm: u32 = ARM_FROSTED; arm <= ARM_GENERIC; arm = arm + 1u) {
            let arm_end = atomicLoad(&sw_counts[SW_C_DRAIN_ARM_END + arm - 1u]);
            if (slot == SW_NO_SLOT && gid.x < arm_end) {
                slot = sw_queues[sw_q_arm(side, arm) * sw.pool + gid.x - arm_start];
                arm_due = true;
            }
            arm_start = arm_end;
        }
        if (slot == SW_NO_SLOT) {
            tuple = atomicLoad(&sw_counts[SW_C_NEXT]) + gid.x - arm_start;
            start = sw_start(tuple, &s);
        }
    }
    if (slot != SW_NO_SLOT) {
        s = ew_load(slot);
        let ray_tag = ray_core[slot].ray_meta;
        tuple = ray_tag.w;
        bounce = ray_tag.z;
        start = SwStart(ray_tag.y, ray_core[slot].observer.xyz);
        if (arm_due) {
            h = sw_hit_load(slot);
        }
    }
    let rc = transport_hoist_ray_constants(s.lambdas);
    loop {
        if (!arm_due) {
            if (bounce >= params.max_bounces) {
                break;
            }
            h = bounce_hit_blank();
            let next = sw_front(&s, &h, rc, bounce, start.seed0, start.observer);
            if (next == SW_NEXT_FINALIZE) {
                break;
            }
            if (next == SW_NEXT_FRONT) {
                bounce = bounce + 1u;
                continue;
            }
        }
        arm_due = false;
        let r = bounce_run_arm(&s, &h, rc, bounce, start.seed0, start.observer);
        if (sw_after_arm(&s, &h, rc, r, bounce, start.seed0, start.observer)) {
            break;
        }
        bounce = bounce + 1u;
    }
    sw_finalize(tuple, &s);
    if ((sw.flags & SW_FLAG_CHECK) != 0u) {
        _ = atomicAdd(&sw_counts[SW_C_DRAINED], 1u);
    }
}
