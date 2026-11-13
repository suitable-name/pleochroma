// Beam tracing (beam guide section 14.3): the WGSL twin of the Rust `branch_step`.
//
// The branch signature is two independent 32-bit lanes built with wrapping u32 arithmetic only
// (WGSL has no 64-bit integers). It must be bit-identical to the CPU `branch_step` in
// `path_cache/signature.rs`: the pin test `tests_reference.rs` runs it on fixed inputs and
// compares the bits with the literals of the guide.
//
//   word = (facet << 8) | (kind << 1) | is_ext
//   a    = (sig.x ^ word) * 0x01000193                      (FNV-1a style lane)
//   b    = (rotl(sig.y, 5) ^ word) * 0x9E3779B1             (multiplicative lane)
//   out  = (a, b ^ (b >> 15))
//
// `kind` is the event code, not an enum: Reflect and TotalReflect are 1, Transmit is 3.
// u32 `*` wraps in WGSL and `>>` on u32 is logical.

const BRANCH_SIG_SEED: vec2<u32> = vec2<u32>(0x811C9DC5u, 0x2545F491u);

const BRANCH_KIND_REFLECT: u32 = 1u;
const BRANCH_KIND_TRANSMIT: u32 = 3u;

fn branch_step(sig: vec2<u32>, facet: u32, kind: u32, is_ext: bool) -> vec2<u32> {
    let word = (facet << 8u) | (kind << 1u) | select(0u, 1u, is_ext);
    let a = (sig.x ^ word) * 0x01000193u;
    let rotated = (sig.y << 5u) | (sig.y >> 27u);
    let b = (rotated ^ word) * 0x9E3779B1u;
    return vec2<u32>(a, b ^ (b >> 15u));
}
