// `fresnel` is the product of the Fresnel factors so far, `length`/`axial` the path inside;
// `weight` is fresnel times the path's transmission (the chain's Branch twin).
struct WtBranch {
    origin: vec3<f32>, dir: vec3<f32>, weight: vec3<f32>,
    fresnel: f32, length: f32, axial: f32,
    inside: bool, depth: u32, primary: bool,
}
// Scalar slots, not a function-local array: avoids AMD/FXC array lowering failures.
struct WtStack { a: WtBranch, b: WtBranch, c: WtBranch, d: WtBranch, count: u32 }
fn wt_push(stack: ptr<function, WtStack>, branch: WtBranch) {
    if (!wt_keeps(branch.weight, branch.depth)) { return; }
    switch ((*stack).count) {
        case 0u: { (*stack).a = branch; }
        case 1u: { (*stack).b = branch; }
        case 2u: { (*stack).c = branch; }
        case 3u: { (*stack).d = branch; }
        default: { return; }
    }
    (*stack).count = (*stack).count + 1u;
}
fn wt_pop(stack: ptr<function, WtStack>) -> WtBranch {
    (*stack).count = (*stack).count - 1u;
    switch ((*stack).count) {
        case 0u: { return (*stack).a; }
        case 1u: { return (*stack).b; }
        case 2u: { return (*stack).c; }
        default: { return (*stack).d; }
    }
}
// A branch continuing `parent`'s path from `point` along `dir`, scaled by the Fresnel `factor`.
fn wt_child(parent: WtBranch, point: vec3<f32>, dir: vec3<f32>, factor: f32, inside: bool,
    depth: u32) -> WtBranch {
    return WtBranch(wt_scale_add(dir, 1e-4, point), dir, parent.weight * factor,
        parent.fresnel * factor, parent.length, parent.axial, inside, depth, false);
}
fn wt_emit(stack: ptr<function, WtStack>, sum: ptr<function, vec3<f32>>, leaf: WtBranch,
    observer: vec3<f32>) {
    if (wt.counts.y == 0u || (*stack).count >= 3u) {
        (*sum) = (*sum) + leaf.weight * wt_leaf(leaf.dir, observer);
    } else {
        wt_push(stack, leaf);
    }
}
fn wt_chain(origin: vec3<f32>, direction: vec3<f32>, n: f32, observer: vec3<f32>) -> vec3<f32> {
    let empty = WtBranch(vec3<f32>(0.0), vec3<f32>(0.0), vec3<f32>(0.0), 0.0, 0.0, 0.0,
        false, 0u, false);
    var stack = WtStack(empty, empty, empty, empty, 0u);
    wt_push(&stack, WtBranch(origin, direction, vec3<f32>(1.0), 1.0, 0.0, 0.0, false, 0u, true));
    var sum = vec3<f32>(0.0);
    loop {
        if (stack.count == 0u) { break; }
        var branch = wt_pop(&stack);
        let hit = wt_intersect(branch.origin, branch.dir);
        if (!hit.valid) {
            // One lookup for every escape: a primary miss sees the same lit tray a window does.
            sum = sum + branch.weight * wt_leaf(branch.dir, observer);
            continue;
        }
        if (hit.side != 0u) { branch.inside = hit.side == 2u; }
        if (branch.inside) {
            branch.length = branch.length + hit.t;
            branch.axial = branch.axial + wt_axial(branch.dir, hit.t);
            branch.weight = wt_transmission(branch.length, branch.axial) * branch.fresnel;
        }
        let frost = wt_frost[hit.facet];
        if (frost.w != 0.0) { sum = sum + branch.weight * frost.xyz; continue; }
        let point = wt_scale_add(branch.dir, hit.t, branch.origin);
        let normal = select(hit.normal, -hit.normal, branch.inside);
        let ci = clamp(-wt_dot(branch.dir, normal), 0.0, 1.0);
        let eta = select(1.0 / n, n, branch.inside);
        let r = wt_fresnel(ci, eta);
        let reflected = wt_reflect(branch.dir, normal);
        let transmitted = wt_refract(branch.dir, normal, eta);
        let depth = branch.depth + 1u;
        if (branch.inside) {
            if (transmitted.valid) {
                wt_emit(&stack, &sum,
                    wt_child(branch, point, transmitted.dir, 1.0 - r, false, depth), observer);
            }
            wt_push(&stack, wt_child(branch, point, reflected, r, true, depth));
        } else {
            let glare = select(1.0, wt.values.x, branch.primary);
            wt_emit(&stack, &sum, wt_child(branch, point, reflected, r * glare, false, depth),
                observer);
            if (transmitted.valid) {
                wt_push(&stack, wt_child(branch, point, transmitted.dir, 1.0 - r, true, depth));
            }
        }
    }
    return sum;
}
