//! Computes a stable content hash of this crate's own `src/**/*.rs` and `src/**/*.wgsl`
//! tree (plus `Cargo.toml`) and exposes it to `lib.rs` as `PLEOCHROMA_SOURCE_HASH`
//! (`pleochroma::SOURCE_HASH`), and a hash of the crate version as `PLEOCHROMA_BUILD_ID`
//! (`pleochroma::BUILD_ID`). Both feed the handshake identity (see
//! `pleochroma-net`'s `handshake` module): `BUILD_ID` is the primary, always-enforced
//! check; `SOURCE_HASH` travels in HELLO and WELCOME for diagnostics only and is never
//! compared, so builds of the same version from different machines pair normally.
//!
//! # Why this exists
//!
//! `pleochroma-net`'s wire protocol handshake refuses to pair a viewer and a remote
//! worker whose `pleochroma` builds disagree -- see `pleochroma-net`'s `handshake`
//! module. Mixing samples traced by two different physics implementations produces a
//! silently, plausibly wrong image: no crash, just numbers that look like a render and
//! aren't. `BUILD_ID` alone can't fully catch this on its own: it's a hash of the crate
//! VERSION, so it only catches a mismatch when the release rule below ("bump the
//! version whenever the physics changes") was actually followed. `SOURCE_HASH`, being a
//! content hash of the actual source tree, catches a physics-affecting edit that never
//! bumped the version at all. A plain "protocol
//! version" field could never catch either case (the wire format didn't change, the
//! *physics* did).
//!
//! # Why a content hash, not `git describe`
//!
//! This repository has a `.git` directory but no commits yet, so `git describe`/
//! `git rev-parse HEAD` both fail unconditionally. A content hash of the source works
//! regardless of VCS state, and (unlike a commit hash) also flags uncommitted local
//! edits that a `git`-based check would silently treat as whatever the last commit was.
//!
//! # Why not `DefaultHasher`
//!
//! `DefaultHasher`'s output is explicitly NOT guaranteed stable across Rust versions --
//! two machines building the same source on different toolchains could disagree and
//! refuse a compatible pairing. FNV-1a below is a fixed, fully-specified algorithm: same
//! input bytes always produce the same output, on any platform or Rust version, forever,
//! for about ten lines and zero dependencies.
//!
//! # Determinism
//!
//! - Files are visited in sorted, POSIX-normalized relative-path order (`/`, not `\`),
//!   so the hash does not depend on the host OS's directory-listing order or path
//!   separator.
//! - The relative path itself is hashed alongside each file's contents, so renaming a
//!   file changes the id even if no byte of any file's content changed.
//! - Line endings are normalized (`\r\n` -> `\n`) before hashing, so an identical
//!   checkout differing only by CRLF/LF (e.g. Windows author, Linux worker) hashes
//!   identically.

use std::{
    fs,
    path::{Path, PathBuf},
};

/// The 64-bit FNV-1a offset basis and prime -- see
/// <https://en.wikipedia.org/wiki/Fowler%E2%80%93Noll%E2%80%93Vo_hash_function>. Fully
/// specified, versioned nowhere, and will never change out from under this build.
const FNV_OFFSET_BASIS: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn fnv1a_update(mut hash: u64, bytes: &[u8]) -> u64 {
    for &b in bytes {
        hash ^= u64::from(b);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// Recursively collects every file under `dir`, returned as paths relative to `root`.
fn collect_files(dir: &Path, root: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_files(&path, root, out);
        } else if let Ok(rel) = path.strip_prefix(root) {
            out.push(rel.to_path_buf());
        }
    }
}

/// POSIX-normalizes a relative path (`/` separators) so the hash is identical whether
/// computed on Windows or Linux.
fn normalize_path(rel: &Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// `transport_physics.wgsl`, `transport_bounce.wgsl`, and `transport_functions.wgsl` were
/// each one oversized file; each is now a directory of ordered pieces (pure text
/// slices, split along real responsibility boundaries -- see each piece's own leading
/// comment) that must be concatenated in exactly this order to reproduce the original
/// file's text byte-for-byte. `read_concat` below joins them with NO separator (not even
/// a newline) for exactly that reason: every piece is a contiguous slice of the original
/// file, so plain concatenation reproduces it exactly, and each piece already ends with
/// its own trailing newline.
const TRANSPORT_PHYSICS_PIECES: &[&str] = &[
    "01_prelude_polarization.wgsl",
    "02_biaxial_eigenmodes.wgsl",
    "03_dispersion_absorption_frosted.wgsl",
    "04_scattering.wgsl",
    "05_nee_env_sampling.wgsl",
    "06_uniaxial_fullwave_complex.wgsl",
    "07_uniaxial_fullwave_solve.wgsl",
    "08_exit_splitting.wgsl",
    "09_lighting_presets.wgsl",
    "10_concave_tools.wgsl",
    "11_oriented_scattering.wgsl",
    "12_bragg_domains.wgsl",
    "13_surface_ggx.wgsl",
];

/// See [`TRANSPORT_PHYSICS_PIECES`]'s doc comment.
const TRANSPORT_BOUNCE_PIECES: &[&str] = &[
    "01_header_and_constants.wgsl",
    "02_scene_bindings.wgsl",
    "03_rng_and_spectral_tables.wgsl",
    "04_studio_and_hdr_environment.wgsl",
    "05_intersect_and_edge_rounding.wgsl",
    "06_nee_sampling.wgsl",
    "07_camera_and_mode_selection.wgsl",
    "07b_bounce_phases_types.wgsl",
    "08a_index_setup.wgsl",
    "08b_extend_scatter.wgsl",
    "08c_surface.wgsl",
    "08d_arms_frosted_tir_uni_entry.wgsl",
    "08e_arms_uni_internal_generic.wgsl",
    "08f_dispatch_split_tail_step.wgsl",
    "09_finalize_and_ray_gen.wgsl",
];

/// See [`TRANSPORT_PHYSICS_PIECES`]'s doc comment.
const TRANSPORT_FUNCTIONS_PIECES: &[&str] = &[
    "01_polarization_fresnel_tir.wgsl",
    "02_dispersion_absorption_uniaxial_index.wgsl",
    "03_frosted_and_scattering.wgsl",
    "04_biaxial_and_scatter_extinguish.wgsl",
    "05_uniaxial_fullwave_and_exit_splitting.wgsl",
    "06_assigned_mode_and_balance.wgsl",
    "07_env_distribution_sampling.wgsl",
    "08_nee_and_intersect.wgsl",
    "09_oriented_scattering.wgsl",
    "10_bragg.wgsl",
    "11_surface.wgsl",
];

/// The zoned-absorption units under `shaders/zoning/`, in concatenation order. They are
/// appended to the shared bounce text of every generated transport shader.
const ZONING_PIECES: &[&str] = &[
    "01_zone_table.wgsl",
    "02_zone_kernel.wgsl",
    "03_zone_absorption.wgsl",
];

/// The event wavefront kernels under `shaders/event_wavefront/`, in dispatch order.
///
/// `ScatterOriented`, `ZonedAbsorb` and `ArmUniEntryTransmit` are the halves split off `Scatter`,
/// `Surface` and `ArmUniEntry` to keep every kernel's browser compile time small; the order is the
/// dispatch order and also the `KERNEL_SET_HASH` input order.
const EW_KERNELS: &[(&str, &str)] = &[
    ("Generate", "ew_generate.wgsl"),
    ("Extend", "ew_extend.wgsl"),
    ("Scatter", "ew_scatter.wgsl"),
    ("ScatterOriented", "ew_scatter_oriented.wgsl"),
    ("NeeHg", "ew_nee_hg.wgsl"),
    ("Surface", "ew_surface.wgsl"),
    ("ZonedAbsorb", "ew_zoned_absorb.wgsl"),
    ("ArmFrosted", "ew_arm_frosted.wgsl"),
    ("ArmTir", "ew_arm_tir.wgsl"),
    ("ArmUniEntry", "ew_arm_uni_entry.wgsl"),
    ("ArmUniEntryTransmit", "ew_arm_uni_entry_transmit.wgsl"),
    ("ArmUniInternal", "ew_arm_uni_internal.wgsl"),
    ("ArmGeneric", "ew_arm_generic.wgsl"),
    ("SplitResolve", "ew_split_resolve.wgsl"),
    ("Tail", "ew_tail.wgsl"),
    ("Finalize", "ew_finalize.wgsl"),
];

const WHITTED_PIECES: &[&str] = &[
    "00_bindings.wgsl",
    "10_exp.wgsl",
    "20_studio.wgsl",
    "30_environment.wgsl",
    "40_intersect.wgsl",
    "50_chain.wgsl",
    "60_frame.wgsl",
];

/// Replaces the single occurrence of the one-line `anchor` in `text` with `replacement`.
///
/// Panics when `anchor` is absent or ambiguous, so a later edit to the default shader text
/// that moves an anchor fails the build loudly instead of silently dropping the
/// zoned absorption.
fn patch_once(text: &str, anchor: &str, replacement: &str, what: &str) -> String {
    let count = text.matches(anchor).count();
    assert!(
        count == 1,
        "zoning shader patch `{what}`: the anchor {anchor:?} occurs {count} times in the \
         transport_bounce pieces (expected exactly 1); the default shader text moved, update \
         build.rs"
    );
    text.replacen(anchor, replacement, 1)
}

/// Diagnostic reverts for bisecting a GPU-only fault (2026-10-22: uniaxial stones return
/// ~1e38 on AMD RADV since the render-physics waves; every per-function Tier 2 check passes).
///
/// `PLEOCHROMA_WGSL_BISECT` is a comma list of the keys below (or `all`). Each key swaps one
/// render-physics change on the bounce path back to its pre-change form or switches a new
/// feature off; for a stone without e-ray curves, surface roughness, abraded edges or
/// inclusions some reverts are numerically equivalent. A changed result helps isolate
/// sensitive code generation; it does not by itself prove a driver defect. Other switches
/// deliberately change the estimator and are not valid production fixes. Unset (the default) the
/// shaders are untouched. The event-wavefront kernel-set hash moves with any revert, so its pin
/// test fails in a bisect build; that is expected.
const BISECT_PATCHES: &[(&str, &str, &str)] = &[
    // 08a: the hero's walk-off index from the e-ray curve -> the constant offset.
    (
        "ne_hero",
        "        n_e_hero = n_e_hero_seed;",
        "        n_e_hero = n_o_hero + birefringence_delta;",
    ),
    // 08d: the uniaxial entry arm's per-channel n_e -> the constant offset.
    (
        "ne_entry",
        "let n_e_eff_k = n_e_raw_ch[k];",
        "let n_e_eff_k = n_o_k + birefringence_delta;",
    ),
    // 08e: the generic arm's per-channel n_e -> the constant offset.
    (
        "ne_generic",
        "let n_e_k = idx.n_e_raw_ch[k];",
        "let n_e_k = n_o_hoisted[k] + birefringence_delta;",
    ),
    // 08f: no surface-finish correction after the four non-frosted arms.
    (
        "finish",
        "    surface_finish_event(s, h, rc, bounce, seed0);",
        "",
    ),
    // 08c: `surface_begin` returns the facet word and normal unchanged (no edge band, no halo).
    (
        "begin",
        "if (surf_is_ground(word)) {",
        "if (surf_is_ground(word) || true) {",
    ),
    // 02/08a/09: the biaxial per-axis curves are never read.
    ("curves", "material.biaxial_curves.types.z != 0u,", "false,"),
    // 08b: the zoned oriented estimator (5 zones x 8 channels of local arrays, a 48-step
    // bisection) is never called by a zoned stone without a domain structure; neutral for a stone
    // without zones. (A zoned stone WITH a structure reaches the estimator through
    // `bounce_scatter_sample_zoned_bragg`; `bragg_zoned` switches that route off.)
    (
        "ori_zoned",
        "            sc = bounce_scatter_sample_zoned(s, h, rc, m, bounce, seed0, &local_stokes, &local_path_pdf);",
        "            sc.scattered = 0u;",
    ),
    // 08b: a zoned stone with a domain structure takes the plain zoned estimator, without the
    // Bragg tracker on top (the pre-D3 behavior: the structure is inert there); neutral for a
    // stone without zones or without a structure.
    (
        "bragg_zoned",
        "        if (zoned_structure) {",
        "        if (false) {",
    ),
    // 08b: the Bragg delta tracker is never called; neutral for a stone without a domain
    // structure.
    (
        "bragg",
        "        if (material.inclusions.header.y != 0u) {",
        "        if (false) {",
    ),
    // 08b: no oriented inclusions or Bragg structure (all their code becomes unreachable).
    (
        "oriented",
        "return ORIENTED_INCLUSIONS && (material.inclusions.header.x > 0u || material.inclusions.header.y != 0u);",
        "return false;",
    ),
    // 08b + ew_scatter: no scattering-point NEE and no MIS carry; the phase-sampled
    // continuation alone at full weight (still unbiased: measures the continuation).
    (
        "nee_hg_off",
        "params.env_mode == 2u && scatter_nee_zone_free() && ev.bragg == 0u) {",
        "false) {",
    ),
    // 08b: the scattering-point NEE deposits, but the continuation keeps full weight (biased
    // high by exactly the NEE share: with `nee_hg_off` it measures the NEE deposit).
    (
        "nee_carry_off",
        "    (*s).pending_light_mis = mis_phase;",
        "    (*s).pending_light_mis = 0.0;",
    ),
];

/// Applies the `PLEOCHROMA_WGSL_BISECT` reverts (see [`BISECT_PATCHES`]) to `text`. Every
/// requested key must match somewhere in the generated shaders; `bisect_report` checks that.
fn bisect(text: &str, keys: &[String], hits: &mut [usize]) -> String {
    let mut out = text.to_owned();
    for (i, (key, anchor, replacement)) in BISECT_PATCHES.iter().enumerate() {
        if keys.iter().any(|k| k == key || k == "all") {
            hits[i] += out.matches(anchor).count();
            out = out.replace(anchor, replacement);
        }
    }
    out
}

/// The requested bisect keys; unknown keys fail the build.
fn bisect_keys() -> Vec<String> {
    let keys: Vec<String> = std::env::var("PLEOCHROMA_WGSL_BISECT")
        .unwrap_or_default()
        .split(',')
        .map(|k| k.trim().to_owned())
        .filter(|k| !k.is_empty())
        .collect();
    for key in &keys {
        assert!(
            key == "all" || BISECT_PATCHES.iter().any(|(k, _, _)| k == key),
            "PLEOCHROMA_WGSL_BISECT: unknown key {key:?}; known: all, {}",
            BISECT_PATCHES
                .iter()
                .map(|(k, _, _)| *k)
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    keys
}

/// Prints what the bisect build changed and fails the build when a requested revert matched
/// nothing (the shader text moved; the list above needs updating).
fn bisect_report(keys: &[String], hits: &[usize]) {
    if keys.is_empty() {
        return;
    }
    for (i, (key, _, _)) in BISECT_PATCHES.iter().enumerate() {
        if keys.iter().any(|k| k == key || k == "all") {
            println!(
                "cargo:warning=WGSL bisect: {key} reverted at {} site(s)",
                hits[i]
            );
            assert!(hits[i] > 0, "WGSL bisect: {key} matched no shader text");
        }
    }
}

/// The zoned variant of the shared bounce text (`transport_bounce/*` concatenated): two
/// anchored patches plus the zoned units appended. Everything else is the default text.
///
/// 1. the `GpuGemMaterial` struct gains its trailing `zones: GpuZoneTable` field (the Rust
///    twin appends the table at offset 592, `renderer::buffers::GpuGemMaterial::zones`), then
///    `biaxial_curves: GpuBiaxialCurves` (offset 2720) and `inclusions: GpuInclusionBlock`
///    (offset 2800). The patch is applied to every generated transport shader, so all variants
///    declare the whole 3024-byte struct; the two block structs themselves are declared in
///    `transport_bounce/02_scene_bindings.wgsl`;
/// 2. the homogeneous interior-absorption branch of `transport_bounce_step` becomes
///    `if (zoned) { zoned_interior_absorption(..) } else if (<the original condition>) {`.
///
/// WGSL allows module-scope declarations in any order, so the units are simply appended.
fn zoned_bounce_shared(shaders_dir: &Path, bounce_shared: &str) -> String {
    let with_table = patch_once(
        bounce_shared,
        "extraordinary_param_b: vec4<f32>,",
        "extraordinary_param_b: vec4<f32>,\n    // Zone table, appended at byte offset 592.\n    zones: GpuZoneTable,\n    \
         // Biaxial principal-index curves, appended at byte offset 2720.\n    \
         biaxial_curves: GpuBiaxialCurves,\n    \
         // Oriented-inclusion block, appended at byte offset 2800 (the struct is 3024 bytes).\n    \
         inclusions: GpuInclusionBlock,",
        "GpuGemMaterial.zones",
    );
    let with_call = patch_once(
        &with_table,
        "if (!material_scatters()) {",
        "if (material.zones.header.zone_count > 0u && !material_scatters()) {\n                \
         // Zoned stone: per-zone absorption of this segment.\n                \
         zoned_interior_absorption(\n                    \
         hit.t, (*current_origin), (*current_dir), wave_dir_at_bounce, lambdas,\n                    \
         is_anisotropic, is_biaxial, c_axis, n_alpha_hero, n_beta_hero, n_gamma_hero,\n                    \
         biax_ax0, biax_ax1, biax_ax2, n_o_hero_seed, n_e_hero_seed, (*is_extraordinary),\n                    \
         alpha_o_hoisted, alpha_e_hoisted, alpha_beta_hoisted, stokes,\n                \
         );\n            \
         } else if (!material_scatters()) {",
        "zoned interior absorption branch",
    );
    let mut out = with_call;
    out.push_str("\n// ---- zoning units (shaders/zoning/*.wgsl) ----\n");
    for piece in ZONING_PIECES {
        let text = fs::read_to_string(shaders_dir.join("zoning").join(piece))
            .unwrap_or_else(|e| panic!("failed to read shaders/zoning/{piece}: {e}"));
        out.push_str(&text);
        out.push('\n');
    }
    out
}

/// Concatenates `shaders/transport_physics.wgsl` (the single shared source of the
/// transport physics functions) ahead of each of `spectral_transport.wgsl`,
/// `transport_functions.wgsl`, and `wavefront_transport.wgsl`, and writes each result
/// into `$OUT_DIR`, since WGSL has no `#include` and every one of those files assumes
/// the prelude's symbols are already in scope.
///
/// `shaders/transport_bounce.wgsl` -- the scene bindings plus the
/// per-bounce `transport_bounce_step`/`transport_finalize_xyz` functions shared between
/// the megakernel and the wavefront pipeline (see that file's own header comment) -- is
/// ALSO concatenated in, for `spectral_transport.wgsl` and `wavefront_transport.wgsl`
/// only: `transport_functions.wgsl` (Tier 2's standalone per-function kernels) never
/// calls either shared function or needs their scene bindings, so it is not
/// concatenated there.
///
/// Deliberately writes into `$OUT_DIR` (under `target/`), never under `src/`: the
/// content hash below walks `src/**/*.wgsl` on disk, and a generated file leaked into
/// `src/` would either double-count as a redundant hash input or, if `.gitignore`d, go
/// missing on a clean checkout and make the hash nondeterministic across `cargo clean`.
///
/// `include_str!(concat!(env!("OUT_DIR"), "/..."))` in `renderer::gpu::estimator_check`,
/// `renderer::gpu::transport_check`, and `renderer::gpu::frame` reads these generated
/// files back at compile time.
fn generate_transport_shaders(manifest_dir: &Path, out_dir: &Path) {
    let shaders_dir = manifest_dir.join("src").join("renderer").join("shaders");
    let read = |name: &str| -> String {
        fs::read_to_string(shaders_dir.join(name))
            .unwrap_or_else(|e| panic!("failed to read shaders/{name}: {e}"))
    };
    // Reads every named piece under `shaders/<subdir>/` in order and concatenates them
    // with no separator -- see `TRANSPORT_PHYSICS_PIECES`'s doc comment above for why
    // that reproduces the pre-split file exactly.
    let read_concat = |subdir: &str, pieces: &[&str]| -> String {
        pieces
            .iter()
            .map(|piece| {
                let rel = format!("{subdir}/{piece}");
                fs::read_to_string(shaders_dir.join(subdir).join(piece))
                    .unwrap_or_else(|e| panic!("failed to read shaders/{rel}: {e}"))
            })
            .collect()
    };
    let prelude = read_concat("transport_physics", TRANSPORT_PHYSICS_PIECES);
    // Independent unit and admission hash: event-wavefront text/hash stay unchanged.
    let mut whitted_tools = read("transport_physics/10_concave_tools.wgsl");
    // The shared tool prelude predates the small-kernel explicit-initializer rule.
    // Initialize its fully assigned records in this unit alone; old shaders/hash
    // must remain byte-identical.
    for (name, ty) in [
        ("t", "CtTool"),
        ("r", "CtInterval"),
        ("p", "CtPiece"),
        ("out", "CtSd"),
        ("s", "CtScan"),
        ("st", "CtCarve"),
    ] {
        whitted_tools = whitted_tools.replace(
            &format!("var {name}: {ty};"),
            &format!("var {name}: {ty} = {ty}();"),
        );
    }
    let whitted = format!(
        "{}\n{}",
        whitted_tools,
        read_concat("whitted", WHITTED_PIECES)
    );
    fs::write(out_dir.join("whitted.generated.wgsl"), &whitted)
        .expect("write whitted generated shader");
    let whitted_hash = fnv1a_update(FNV_OFFSET_BASIS, whitted.replace("\r\n", "\n").as_bytes());
    fs::write(
        out_dir.join("whitted_kernel_hash.rs"),
        format!("pub const WHITTED_KERNEL_HASH: &str = \"{whitted_hash:016x}\";\n"),
    )
    .expect("write whitted kernel hash");
    let bounce_shared = read_concat("transport_bounce", TRANSPORT_BOUNCE_PIECES);
    // The shared bounce text is its zoned variant (see `zoned_bounce_shared`).
    let bounce_shared = zoned_bounce_shared(&shaders_dir, &bounce_shared);
    // Diagnostic reverts (`PLEOCHROMA_WGSL_BISECT`), none by default.
    let bisect_keys = bisect_keys();
    let mut bisect_hits = vec![0usize; BISECT_PATCHES.len()];
    let prelude = bisect(&prelude, &bisect_keys, &mut bisect_hits);
    let bounce_shared = bisect(&bounce_shared, &bisect_keys, &mut bisect_hits);
    bisect_report(&bisect_keys, &bisect_hits);

    // (body file, whether `transport_bounce.wgsl` is concatenated in between the
    // prelude and the body) -- see this function's own doc comment.
    let units: [(&str, bool); 3] = [
        ("spectral_transport.wgsl", true),
        ("transport_functions.wgsl", false),
        ("wavefront_transport.wgsl", true),
    ];

    for (name, needs_bounce_shared) in units {
        // `transport_functions.wgsl` is likewise now a directory of ordered pieces (see
        // `TRANSPORT_FUNCTIONS_PIECES`'s doc comment above); `spectral_transport.wgsl`
        // and `wavefront_transport.wgsl` are still single files, read as before.
        let body = if name == "transport_functions.wgsl" {
            read_concat("transport_functions", TRANSPORT_FUNCTIONS_PIECES)
        } else {
            read(name)
        };
        let generated = if needs_bounce_shared {
            format!(
                "// GENERATED by build.rs: shaders/transport_physics.wgsl + \
                 shaders/transport_bounce.wgsl + shaders/{name}.\n\
                 // Do not edit this file -- edit the sources above instead.\n\n\
                 {prelude}\n{bounce_shared}\n{body}"
            )
        } else {
            format!(
                "// GENERATED by build.rs: shaders/transport_physics.wgsl + shaders/{name}.\n\
                 // Do not edit this file -- edit the two sources above instead.\n\n{prelude}\n{body}"
            )
        };
        let out_name = name.replace(".wgsl", ".generated.wgsl");
        fs::write(out_dir.join(&out_name), generated)
            .unwrap_or_else(|e| panic!("failed to write {out_name} to OUT_DIR: {e}"));
    }

    let common = fs::read_to_string(shaders_dir.join("event_wavefront").join("00_common.wgsl"))
        .unwrap_or_else(|e| panic!("failed to read shaders/event_wavefront/00_common.wgsl: {e}"));

    let mut ew_hash = FNV_OFFSET_BASIS;
    for &(_name, file) in EW_KERNELS {
        let kernel_body = fs::read_to_string(shaders_dir.join("event_wavefront").join(file))
            .unwrap_or_else(|e| panic!("failed to read shaders/event_wavefront/{file}: {e}"));
        let generated = format!(
            "// GENERATED by build.rs: shaders/transport_physics.wgsl + \
             shaders/transport_bounce.wgsl + shaders/event_wavefront/00_common.wgsl + \
             shaders/event_wavefront/{file}.\n\
             // Do not edit this file -- edit the sources above instead.\n\n\
             {prelude}\n{bounce_shared}\n{common}\n{kernel_body}"
        );
        let out_name = file.replace(".wgsl", ".generated.wgsl");
        fs::write(out_dir.join(&out_name), &generated)
            .unwrap_or_else(|e| panic!("failed to write {out_name} to OUT_DIR: {e}"));

        let normalized = generated.replace("\r\n", "\n");
        ew_hash = fnv1a_update(ew_hash, normalized.as_bytes());
    }

    let hash_file_content = format!("pub const KERNEL_SET_HASH: &str = \"{ew_hash:016x}\";\n");
    fs::write(out_dir.join("ew_kernel_set_hash.rs"), hash_file_content)
        .unwrap_or_else(|e| panic!("failed to write ew_kernel_set_hash.rs to OUT_DIR: {e}"));
}

fn main() {
    let manifest_dir = PathBuf::from(
        std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR is set by cargo"),
    );
    let src_dir = manifest_dir.join("src");
    let cargo_toml = manifest_dir.join("Cargo.toml");
    let out_dir =
        PathBuf::from(std::env::var("OUT_DIR").expect("OUT_DIR is set by cargo for build scripts"));

    generate_transport_shaders(&manifest_dir, &out_dir);
    println!("cargo:rerun-if-env-changed=PLEOCHROMA_WGSL_BISECT");
    println!(
        "cargo:rerun-if-changed={}",
        src_dir.join("renderer/shaders/whitted").display()
    );

    let ew_shaders_dir = manifest_dir
        .join("src")
        .join("renderer")
        .join("shaders")
        .join("event_wavefront");
    println!("cargo:rerun-if-changed={}", ew_shaders_dir.display());
    println!(
        "cargo:rerun-if-changed={}",
        ew_shaders_dir.join("00_common.wgsl").display()
    );
    for &(_, file) in EW_KERNELS {
        println!(
            "cargo:rerun-if-changed={}",
            ew_shaders_dir.join(file).display()
        );
    }

    let mut rel_paths = Vec::new();
    collect_files(&src_dir, &manifest_dir, &mut rel_paths);
    // .rs and .wgsl both carry physics this hash must fingerprint: a worker running a
    // GPU backend executes WGSL as its physics, not as decoration on top of it. Two
    // workers with byte-identical Rust but divergent WGSL would otherwise handshake as
    // identical, which is exactly the silent-divergence failure this hash exists to
    // catch. Deliberately still ONE hash, not two: verify_compatible has no "close
    // enough" tier, so any change to either language is a build-identity change.
    rel_paths.retain(|p| {
        p.extension()
            .is_some_and(|ext| ext == "rs" || ext == "wgsl")
    });

    // Pathological case: no source files found under src/ at all. Rather than silently
    // emit a hash computed from Cargo.toml alone, fall back to an explicit sentinel --
    // parse_build_id maps any non-16-hex-char string (this included) to
    // UNKNOWN_BUILD_HASH, which verify_compatible refuses unconditionally.
    if rel_paths.is_empty() {
        println!("cargo:rerun-if-changed={}", src_dir.display());
        println!("cargo:rustc-env=PLEOCHROMA_SOURCE_HASH=unknown");
        println!("cargo:rustc-env=PLEOCHROMA_BUILD_ID=unknown");
        return;
    }

    rel_paths.push(PathBuf::from("Cargo.toml"));
    rel_paths.sort();

    let mut hash = FNV_OFFSET_BASIS;
    for rel in &rel_paths {
        let abs = manifest_dir.join(rel);
        let Ok(contents) = fs::read(&abs) else {
            continue;
        };
        // Normalize CRLF -> LF before hashing so a Windows checkout and a Linux
        // checkout of byte-identical source hash identically.
        let normalized: Vec<u8> = {
            let mut out = Vec::with_capacity(contents.len());
            let mut i = 0;
            while i < contents.len() {
                if contents[i] == b'\r' && contents.get(i + 1) == Some(&b'\n') {
                    // Drop the \r; the loop picks up the \n on the next iteration.
                } else {
                    out.push(contents[i]);
                }
                i += 1;
            }
            out
        };

        let rel_str = normalize_path(rel);
        hash = fnv1a_update(hash, rel_str.as_bytes());
        hash = fnv1a_update(hash, &[0u8]); // separator between path and contents
        hash = fnv1a_update(hash, &normalized);

        println!("cargo:rerun-if-changed={}", abs.display());
    }
    // Also react to files being added/removed (rerun-if-changed on the directory
    // itself catches that on most platforms/cargo versions).
    println!("cargo:rerun-if-changed={}", src_dir.display());
    println!("cargo:rerun-if-changed={}", cargo_toml.display());

    let source_hash = format!("{hash:016x}");
    println!("cargo:rustc-env=PLEOCHROMA_SOURCE_HASH={source_hash}");

    // The PRIMARY handshake identity is keyed on the crate VERSION, not on the source
    // hash: a worker built on Linux and a viewer built on Windows from the same release
    // must pair even when their checkouts differ in metadata, docs, or platform-only
    // code. That guarantee rests on the release rule "bump the version whenever
    // anything under src/ that affects a traced sample changes" -- a promise nothing
    // enforces. The source hash above (`pleochroma::SOURCE_HASH`) is carried in the
    // handshake for diagnostics only and is never compared (see
    // `pleochroma-net::handshake`'s module doc comment).
    // `CARGO_PKG_VERSION` is the package's resolved version: cargo substitutes a
    // `version = { workspace = true }` inheritance before running this script, and
    // `cargo package` writes the literal version into the published manifest, so a
    // workspace build and a crates.io build of the same release agree. An empty or
    // missing value must not hash into a plausible id, so it falls back to the
    // `unknown` sentinel the handshake refuses.
    let build_id = match std::env::var("CARGO_PKG_VERSION") {
        Ok(version) if !version.trim().is_empty() => {
            format!(
                "{:016x}",
                fnv1a_update(FNV_OFFSET_BASIS, version.trim().as_bytes())
            )
        }
        _ => "unknown".to_string(),
    };
    println!("cargo:rustc-env=PLEOCHROMA_BUILD_ID={build_id}");
}
