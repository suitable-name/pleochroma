//! Cached tray variants (the Stone Shadow and the Tray Reflection) composed from the shipped
//! megakernel, tables and live-list routing. A missing anchor declines the variant; it never
//! silently renders an unshadowed or unmirrored scene.

use super::{SHADER_SRC, live_source::live_variant, specialisation::material_class, tables_source};
use std::{borrow::Cow, sync::OnceLock};

/// The lookup, environment wrapper and Tray Reflection helpers appended only to a tray variant.
pub const TRAY_SHADOW_WGSL: &str = include_str!("../../shaders/transport_bounce/tray_shadow.wgsl");

/// Generates a tray-parameter version of the shipped studio evaluator.
fn studio_tray_function(shipped: &str) -> Option<String> {
    const HEAD: &str = "fn sample_studio_environment_with_rig(";
    if shipped.matches(HEAD).count() != 1 {
        return None;
    }
    let start = shipped.find(HEAD)?;
    let close = start + shipped[start..].find("\n}")? + 2;
    let end = close + shipped[close..].find('\n')? + 1;
    let function = &shipped[start..end];
    let newline = if function.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    };
    let last = format!("    observer: vec3<f32>,{newline}) -> f32 {{");
    let replacement =
        format!("    observer: vec3<f32>,{newline}    tray: f32,{newline}) -> f32 {{");
    if function.matches("params.backdrop").count() != 1 || function.matches(&last).count() != 1 {
        return None;
    }
    Some(
        function
            .replacen(HEAD, "fn ts_studio_environment_tray(", 1)
            .replacen(&last, &replacement, 1)
            .replacen("params.backdrop", "tray", 1),
    )
}

/// The first line of the miss branch of `bounce_extend` (once in the shipped megakernel): the
/// Tray Reflection's floor bounce is routed in front of it.
const MISS_ANCHOR: &str = "var mis_weight: f32 = 1.0;";

/// The staging line of the generic arm's exit split (once in the shipped megakernel): Rule M is
/// routed in front of it, in the chromatic-termination branch of `bounce_arm_generic_core`.
const GENERIC_EXIT_ANCHOR: &str = "if (is_exit_event && original_stokes_k.x > 0.0) {";

/// The staging line of the uniaxial internal exit's split (once in the shipped megakernel): Rule
/// M is routed in front of it, in the mismatch branch of `bounce_arm_uni_internal_branches`.
const UNIAXIAL_EXIT_ANCHOR: &str = "if (original_stokes_i > 0.0) {";

/// The line break that ends the line holding `anchor` in `text`: a CRLF when the byte before
/// the line feed is a carriage return, a bare line feed otherwise (also for a missing anchor,
/// which the caller declines on its own).
fn line_break_after(text: &str, anchor: &str) -> &'static str {
    let line_feed = text
        .find(anchor)
        .and_then(|at| text[at..].find('\n').map(|offset| at + offset));
    match line_feed {
        Some(at) if text[..at].ends_with('\r') => "\r\n",
        _ => "\n",
    }
}

/// Adds shadow lookups at both escape sites, routes the Tray Reflection (the floor bounce at
/// the miss branch and Rule M into the two bounce arms, where the CPU applies it) and keeps
/// binding 190 reachable from the entry.
///
/// One source serves the shadow, the mirror and both: the header words of binding 190 say
/// which feature is on (`ts_environment` reads word 0, `tm_on` word 10), so a scene with only
/// one of them runs the other's code as a no-op. `None` unless every anchor occurs exactly once.
#[must_use]
pub fn tray_shadow_variant(source: &str) -> Option<String> {
    let generated = studio_tray_function(source)?;
    let mut text = source.to_owned();
    // Each route takes the line break of its own anchor's line: the routes land in different
    // shipped units, which need not share one line ending.
    let miss_newline = line_break_after(&text, MISS_ANCHOR);
    let generic_newline = line_break_after(&text, GENERIC_EXIT_ANCHOR);
    let uniaxial_newline = line_break_after(&text, UNIAXIAL_EXIT_ANCHOR);
    let miss_route = format!(
        "if (tm_floor_bounce(s, rc, observer, phase_pdf_this_check, phase_dir_this_check)) {{{miss_newline}            return PHASE_END_STEP;{miss_newline}        }}{miss_newline}        {MISS_ANCHOR}"
    );
    // Rule M: a mismatched channel whose own direction, or the hero's, meets the floor is
    // terminated chromatically (`path_pdf[k] = 0`) in front of the staging of its exit split.
    let rule_m = |hero_dir: &str, condition: &str, anchor: &str, newline: &str| {
        format!(
            "if ({condition}tm_split_blocked(hit_point, {hero_dir}, refr_wave_dir_k)) {{{newline}                    (*s).path_pdf[k] = 0.0;{newline}                    continue;{newline}                }}{newline}                {anchor}"
        )
    };
    let generic_route = rule_m(
        "final_refr_dir",
        "is_exit_event && ",
        GENERIC_EXIT_ANCHOR,
        generic_newline,
    );
    let uniaxial_route = rule_m(
        "hero_refr_dir_i",
        "",
        UNIAXIAL_EXIT_ANCHOR,
        uniaxial_newline,
    );
    for (from, to) in [
        (MISS_ANCHOR, miss_route.as_str()),
        (GENERIC_EXIT_ANCHOR, generic_route.as_str()),
        (UNIAXIAL_EXIT_ANCHOR, uniaxial_route.as_str()),
    ] {
        if text.matches(from).count() != 1 {
            return None;
        }
        text = text.replacen(from, to, 1);
    }
    for (from, to) in [
        (
            "sample_environment_with_rig((*s).dir, (*s).lambdas[k], rc.studio_key_dir, rc.studio_fill_dir, rc.studio_sin_lp, observer)",
            "ts_environment((*s).origin, (*s).dir, (*s).lambdas[k], rc.studio_key_dir, rc.studio_fill_dir, rc.studio_sin_lp, observer)",
        ),
        (
            "sample_environment_with_rig(dir_k, lambda_k, key_dir, fill_dir, sin_lp, observer)",
            "ts_environment(hit_point + dir_k * 1e-4, dir_k, lambda_k, key_dir, fill_dir, sin_lp, observer)",
        ),
    ] {
        if text.matches(from).count() != 1 {
            return None;
        }
        text = text.replacen(from, to, 1);
    }
    let entry = "fn transport_main(";
    if text.matches(entry).count() != 1 || text.contains("fn ts_reach(") {
        return None;
    }
    let start = text.find(entry)?;
    let body = start + text[start..].find('{')?;
    text.insert_str(body + 1, "\n    let ts_probe = ts_reach();");
    text.push('\n');
    text.push_str(TRAY_SHADOW_WGSL);
    text.push('\n');
    text.push_str(&generated);
    Some(text)
}

/// One cached shadow source for each combination of tables and live pixel lists.
fn shadow_source(tables: bool, live: bool) -> Option<&'static str> {
    static SOURCES: [OnceLock<Option<String>>; 4] = [const { OnceLock::new() }; 4];
    let index = usize::from(tables) + 2 * usize::from(live);
    SOURCES[index]
        .get_or_init(|| {
            let base = if tables {
                tables_source::megakernel_tables_source()?
            } else {
                SHADER_SRC
            };
            let text = tray_shadow_variant(base)?;
            if live {
                live_variant(&text)
            } else {
                Some(text)
            }
        })
        .as_deref()
}

/// Source selected by scene and material bits; the source of a scene with neither a shadow nor a
/// mirror stays byte-identical.
#[must_use]
pub fn megakernel_source_for(class: u32) -> Option<Cow<'static, str>> {
    if material_class::uses_tray_variant(class) {
        shadow_source(material_class::uses_optics_tables(class), false).map(Cow::Borrowed)
    } else {
        Some(Cow::Borrowed(tables_source::megakernel_source_for(class)))
    }
}

/// Live-list source selected by scene and material bits, sharing the same variant cache.
#[must_use]
pub fn megakernel_live_source_for(class: u32) -> Option<&'static str> {
    if material_class::uses_tray_variant(class) {
        shadow_source(material_class::uses_optics_tables(class), true)
    } else {
        tables_source::megakernel_live_source_for(class)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shadow_variant_builds_and_routes_each_escape_once() {
        for tables in [false, true] {
            for live in [false, true] {
                let source = shadow_source(tables, live).expect("shipped anchors match");
                // The routed shipped text: the appended unit (from `ts_reach` on) states
                // `ts_environment` once more, in the Tray Reflection's floor bounce.
                let shipped = &source[..source.find("fn ts_reach(").expect("appended unit")];
                assert_eq!(shipped.matches("ts_environment((*s).origin,").count(), 1);
                assert_eq!(
                    shipped
                        .matches("ts_environment(hit_point + dir_k * 1e-4,")
                        .count(),
                    1
                );
                assert_eq!(source.matches("let ts_probe = ts_reach();").count(), 1);
                assert_eq!(source.matches("fn ts_studio_environment_tray(").count(), 1);
            }
        }
        assert!(tray_shadow_variant("fn unrelated() {}").is_none());
    }

    /// The call routed in front of the miss branch.
    const MISS_CALL: &str =
        "tm_floor_bounce(s, rc, observer, phase_pdf_this_check, phase_dir_this_check)";
    /// Rule M as routed into the generic arm.
    const GENERIC_CALL: &str = "tm_split_blocked(hit_point, final_refr_dir, refr_wave_dir_k)";
    /// Rule M as routed into the uniaxial internal exit.
    const UNIAXIAL_CALL: &str = "tm_split_blocked(hit_point, hero_refr_dir_i, refr_wave_dir_k)";
    /// Each routed call with the shipped anchor it sits in front of.
    const ROUTES: [(&str, &str); 3] = [
        (MISS_CALL, MISS_ANCHOR),
        (GENERIC_CALL, GENERIC_EXIT_ANCHOR),
        (UNIAXIAL_CALL, UNIAXIAL_EXIT_ANCHOR),
    ];

    /// The routed text of the Tray Reflection, each once, in every table and live combination:
    /// the floor bounce in front of the miss branch, Rule M in front of the exit split of each
    /// bounce arm, and every `tm_` helper defined once.
    #[test]
    fn every_tray_variant_routes_the_floor_bounce_and_rule_m_once() {
        for tables in [false, true] {
            for live in [false, true] {
                let source = shadow_source(tables, live).expect("shipped anchors match");
                for (call, anchor) in ROUTES {
                    assert_eq!(source.matches(call).count(), 1, "{call}");
                    // The shipped anchors survive exactly once: the routes sit in front of them.
                    assert_eq!(source.matches(anchor).count(), 1, "{anchor}");
                    assert!(
                        source.find(call).expect("route") < source.find(anchor).expect("anchor"),
                        "{call} must sit in front of its anchor"
                    );
                }
                // The definition and the two arm calls.
                assert_eq!(source.matches("tm_split_blocked(").count(), 3);
                for name in [
                    "fn tm_floor(",
                    "fn tm_on(",
                    "fn tm_split_blocked(",
                    "fn tm_floor_bounce(",
                    "struct TmFloor",
                ] {
                    assert_eq!(source.matches(name).count(), 1, "{name}");
                }
            }
        }
    }

    #[test]
    fn the_shipped_anchors_of_the_tray_reflection_occur_exactly_once() {
        let anchors = [MISS_ANCHOR, GENERIC_EXIT_ANCHOR, UNIAXIAL_EXIT_ANCHOR];
        for base in [
            SHADER_SRC,
            tables_source::megakernel_tables_source().expect("tables variant"),
        ] {
            for anchor in anchors {
                assert_eq!(base.matches(anchor).count(), 1, "{anchor}");
            }
            assert!(!base.contains("fn tm_") && !base.contains("TmFloor"));
        }
        // A drifted or duplicated anchor declines the whole variant.
        for anchor in anchors {
            let missing = SHADER_SRC.replacen(anchor, "/* drifted */", 1);
            assert!(tray_shadow_variant(&missing).is_none(), "{anchor}");
            let duplicated = format!("{SHADER_SRC}\n// {anchor}\n");
            assert!(tray_shadow_variant(&duplicated).is_none(), "{anchor}");
        }
    }

    #[test]
    fn the_routes_follow_the_line_endings_of_the_unit() {
        let crlf = SHADER_SRC.replace("\r\n", "\n").replace('\n', "\r\n");
        let variant = tray_shadow_variant(&crlf).expect("CRLF unit routes");
        for (call, anchor) in ROUTES {
            assert_eq!(variant.matches(call).count(), 1, "{call}");
            // Every routed line break of the shipped unit is a CRLF; the inserted routes add
            // none of their own bare line feeds.
            let route = variant.find(call).expect("route");
            let anchor = variant.find(anchor).expect("anchor");
            assert!(!variant[route..anchor].replace("\r\n", "").contains('\n'));
        }
    }

    /// Sets the line break that ends the line holding `anchor` to a CRLF or a bare line feed.
    fn with_break_after(source: &str, anchor: &str, crlf: bool) -> String {
        let at = source.find(anchor).expect("anchor");
        let line_feed = at + source[at..].find('\n').expect("line break");
        let start = if source[..line_feed].ends_with('\r') {
            line_feed - 1
        } else {
            line_feed
        };
        let mut out = source.to_owned();
        out.replace_range(start..=line_feed, if crlf { "\r\n" } else { "\n" });
        out
    }

    #[test]
    fn each_route_follows_the_line_ending_of_its_own_anchor_line() {
        let lf = SHADER_SRC.replace("\r\n", "\n");
        let crlf = lf.replace('\n', "\r\n");
        // Every anchor line on its own against the opposite ending of the rest of the unit.
        for (index, &(_, own_anchor)) in ROUTES.iter().enumerate() {
            for (rest, own_crlf) in [(&lf, true), (&crlf, false)] {
                let mixed = with_break_after(rest, own_anchor, own_crlf);
                let variant = tray_shadow_variant(&mixed).expect("mixed unit routes");
                for (position, &(call, anchor)) in ROUTES.iter().enumerate() {
                    let expected_crlf = if position == index {
                        own_crlf
                    } else {
                        !own_crlf
                    };
                    let route = variant.find(call).expect("route");
                    let at = variant.find(anchor).expect("anchor");
                    let span = &variant[route..at];
                    let unexpected = if expected_crlf {
                        span.replace("\r\n", "")
                    } else {
                        span.replace('\n', "")
                    };
                    assert!(!unexpected.contains(['\r', '\n']), "{call}");
                    assert!(span.contains('\n'), "{call}");
                }
            }
        }
    }

    #[test]
    fn a_shadow_a_mirror_or_both_select_the_same_variant_source() {
        for tables in [0, material_class::OPTICS_TABLES] {
            let shadow = megakernel_source_for(material_class::TRAY_SHADOW | tables).expect("s");
            let mirror = megakernel_source_for(material_class::TRAY_MIRROR | tables).expect("m");
            let both = megakernel_source_for(
                material_class::TRAY_SHADOW | material_class::TRAY_MIRROR | tables,
            )
            .expect("b");
            // A failure prints whole sources: long, but it names the variant that differs.
            assert_eq!(shadow, mirror);
            assert_eq!(shadow, both);
            assert!(shadow.contains("fn tm_floor_bounce("));
            let live_shadow =
                megakernel_live_source_for(material_class::TRAY_SHADOW | tables).expect("ls");
            let live_mirror =
                megakernel_live_source_for(material_class::TRAY_MIRROR | tables).expect("lm");
            assert_eq!(live_shadow, live_mirror);
        }
    }

    #[test]
    fn the_mirror_helpers_state_the_cpu_floor_operations_in_order() {
        let wgsl = TRAY_SHADOW_WGSL;
        // The shadow lookup (`ts_plane_uv`) states the first two lines of `tm_floor` too.
        for line in [
            "if (!(dir.y < 0.0) || !(origin.y > plane_y)) {",
            "let t = (plane_y - origin.y) / dir.y;",
        ] {
            assert_eq!(wgsl.matches(line).count(), 2, "{line}");
        }
        assert_eq!(
            wgsl.matches(
                "return params.env_mode == 1u && params.backdrop > 0.0 && tray_shadow[10] != 0.0;"
            )
            .count(),
            1
        );
        // `TrayMirror::hit`'s plain multiplies and adds, repeated operation for operation and in
        // the CPU's order, inside the body of `tm_floor` alone.
        let body = &wgsl[wgsl.find("fn tm_floor(").expect("tm_floor")
            ..wgsl.find("fn ts_environment(").expect("ts_environment")];
        let mut at = 0;
        for line in [
            "if (!(dir.y < 0.0) || !(origin.y > plane_y)) {",
            "let t = (plane_y - origin.y) / dir.y;",
            "f.point = vec3<f32>(origin.x + dir.x * t, plane_y, origin.z + dir.z * t);",
            "let c = clamp(-dir.y, 0.0, 1.0);",
            "let m = 1.0 - c;",
            "let m2 = m * m;",
            "let m5 = m2 * m2 * m;",
            "f.fresnel = r0 + (1.0 - r0) * m5;",
        ] {
            let found = body[at..]
                .find(line)
                .unwrap_or_else(|| panic!("`{line}` is missing or out of order"));
            at += found + line.len();
        }
        assert!(
            !body.contains("fma("),
            "the floor hit states no explicit fma"
        );
        assert!(body.contains("tray_shadow[11]") && body.contains("tray_shadow[12]"));
    }

    /// Writes the four tray variants (tables x live) for a naga-cli check. The paths are
    /// printed; set `PLEOCHROMA_WGSL_DUMP_DIR` to choose the directory (the temp dir otherwise).
    #[test]
    #[ignore = "writes WGSL files for the owner's naga-cli validation; run with --ignored"]
    fn dump_the_tray_variants_for_naga_cli() {
        let dir = std::env::var_os("PLEOCHROMA_WGSL_DUMP_DIR")
            .map_or_else(std::env::temp_dir, std::path::PathBuf::from);
        for tables in [false, true] {
            for live in [false, true] {
                let source = shadow_source(tables, live).expect("shipped anchors match");
                let path = dir.join(format!(
                    "pleochroma_tray_variant_tables{}_live{}.wgsl",
                    u8::from(tables),
                    u8::from(live)
                ));
                std::fs::write(&path, source).expect("write the variant");
                println!("{}", path.display());
            }
        }
    }

    #[test]
    fn without_the_shadow_bit_the_shipped_source_is_unchanged() {
        let source = megakernel_source_for(material_class::ISOTROPIC).expect("plain source");
        assert_eq!(source.as_ref(), SHADER_SRC);
    }
}
