//! Golden face-up figures: today's metrics, pinned bit for bit.
//!
//! Five stones (a round brilliant, an emerald cut, a grooved round brilliant, a frosted-girdle
//! round brilliant and a sapphire with a tilted optic axis) are evaluated under four lightings
//! (Ring Lights, the Grading tray, the Light tent and the Contrast view) at two poses (yaw 0
//! looking straight down the table, and yaw 0.6 with pitch 0.45). Every case records the six
//! figures of [`GemOpticalMetrics`] and the six fields of [`FaceUpTone`] as the hexadecimal bit
//! pattern of each value, so a change to the face-up pass that moves any figure shows up as a
//! differing bit, not as a tolerance that happened to hold.
//!
//! The recorded file is `tests/data/metrics_golden_0_11_1.txt` under the crate root. Each line
//! holds `stone`, `lighting`, `pose` and then the twelve bit patterns, separated by tabs.
//!
//! To record the file, run the writer once on a tree whose face-up pass is the one to pin:
//!
//! ```text
//! cargo test -p pleochroma --lib -- --ignored write_metrics_golden
//! ```
//!
//! The reader test skips with a message while the file does not exist.

use super::{
    FaceUpTone, FinishedStone, GemOpticalMetrics, evaluate_gem_optical_metrics_with_tone_finished,
};
use crate::{
    geometry::{cuts::StandardGemCuts, girdle_facet_finishes, plane::GpuFacetPlane, tool},
    optics::{
        materials::GemMaterial,
        raytracer::{FacetFinish, LightingPreset},
    },
};
use glam::Vec3;
use std::{f32::consts::FRAC_PI_2, fmt::Write as _, fs, path::PathBuf};

/// The recorded file, `tests/data/metrics_golden_0_11_1.txt` under the crate root.
fn golden_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("data")
        .join("metrics_golden_0_11_1.txt")
}

/// The lightings the golden covers, as `(name, preset)`.
const LIGHTINGS: [(&str, LightingPreset); 4] = [
    ("Ring Lights", LightingPreset::RingLights),
    ("Grading tray", LightingPreset::IsoHemisphere),
    ("Light tent", LightingPreset::LightTent),
    ("Contrast view", LightingPreset::Aset),
];

/// The poses the golden covers, as `(name, yaw, pitch)`.
const POSES: [(&str, f32, f32); 2] = [
    ("yaw 0 pitch 90", 0.0, FRAC_PI_2),
    ("yaw 0.6 pitch 0.45", 0.6, 0.45),
];

/// One stone of the golden: its planes, tools, finishes and material.
struct Case {
    /// The name the golden file records.
    name: &'static str,
    /// The facet planes.
    planes: Vec<GpuFacetPlane>,
    /// The concave tools (empty for a flat stone).
    tools: Vec<tool::ToolPrimitive>,
    /// The per-facet finishes (empty for a polished stone).
    finishes: Vec<FacetFinish>,
    /// The material the stone is traced in.
    material: GemMaterial,
}

/// A sapphire whose optic axis is tilted off the stone's vertical axis, so the two modes differ.
fn tilted_sapphire() -> GemMaterial {
    let mut material = GemMaterial::sapphire();
    material.c_axis = Vec3::new(0.3, 0.9, 0.2).normalize();
    material
}

/// Every stone the golden covers, in a fixed order.
fn cases() -> Vec<Case> {
    let round = StandardGemCuts::standard_round_brilliant();
    let girdle_frosted = girdle_facet_finishes(&round);
    vec![
        Case {
            name: "round brilliant",
            planes: round.clone(),
            tools: Vec::new(),
            finishes: Vec::new(),
            material: GemMaterial::diamond(),
        },
        Case {
            name: "emerald cut",
            planes: StandardGemCuts::emerald_cut(),
            tools: Vec::new(),
            finishes: Vec::new(),
            material: GemMaterial::diamond(),
        },
        Case {
            name: "concave round brilliant",
            planes: round.clone(),
            tools: vec![tool::ToolPrimitive::cylinder(
                Vec3::new(0.0, 0.44, 0.0),
                Vec3::X,
                0.3,
                2.0,
            )],
            finishes: Vec::new(),
            material: GemMaterial::diamond(),
        },
        Case {
            name: "frosted girdle round brilliant",
            planes: round.clone(),
            tools: Vec::new(),
            finishes: girdle_frosted,
            material: GemMaterial::diamond(),
        },
        Case {
            name: "sapphire with an axis",
            planes: round,
            tools: Vec::new(),
            finishes: Vec::new(),
            material: tilted_sapphire(),
        },
    ]
}

/// The six figures of `m` as bit patterns, in declaration order.
fn metric_bits(m: GemOpticalMetrics) -> [u32; 6] {
    [
        m.brilliance_pct.to_bits(),
        m.fire_index.to_bits(),
        m.scintillation_pct.to_bits(),
        m.windowing_pct.to_bits(),
        m.extinction_pct.to_bits(),
        m.doubling_arcmin.to_bits(),
    ]
}

/// The six fields of `tone` as bit patterns, in declaration order (the swatch packed as one word).
fn tone_bits(tone: FaceUpTone) -> [u32; 6] {
    let [r, g, b] = tone.srgb;
    [
        tone.l_star.to_bits(),
        tone.chroma.to_bits(),
        tone.hue_deg.to_bits(),
        u32::from_be_bytes([0, r, g, b]),
        tone.mean_path_units.to_bits(),
        tone.returned_fraction.to_bits(),
    ]
}

/// The golden text for the current face-up pass: one line per stone, lighting and pose.
fn golden_text() -> String {
    let mut text = String::new();
    for case in cases() {
        let geom = tool::StoneGeometry {
            planes: &case.planes,
            tools: &case.tools,
        };
        let stone = FinishedStone::new(geom, &case.finishes);
        for (lighting_name, preset) in LIGHTINGS {
            for (pose_name, yaw, pitch) in POSES {
                let (metrics, tone) = evaluate_gem_optical_metrics_with_tone_finished(
                    stone,
                    &case.material,
                    yaw,
                    pitch,
                    preset.studio(1.0, 0.85, 0.95),
                );
                write!(text, "{}\t{lighting_name}\t{pose_name}", case.name)
                    .expect("writing to a String cannot fail");
                for bits in metric_bits(metrics).into_iter().chain(tone_bits(tone)) {
                    write!(text, "\t{bits:08x}").expect("writing to a String cannot fail");
                }
                text.push('\n');
            }
        }
    }
    text
}

/// Writes the golden file from the current face-up pass. Run it once on the tree whose figures
/// are to be pinned; the reader test then guards every later change.
#[test]
#[ignore = "writes tests/data/metrics_golden_0_11_1.txt; run it once on the unchanged metrics"]
fn write_metrics_golden() {
    let path = golden_path();
    let dir = path.parent().expect("the golden path has a directory");
    fs::create_dir_all(dir).expect("the data directory can be created");
    let text = golden_text();
    assert!(!text.is_empty(), "no case was evaluated");
    fs::write(&path, text).expect("the golden file can be written");
}

/// Every golden case evaluates to the recorded figures and tone, bit for bit.
#[test]
fn face_up_figures_match_the_golden_bit_for_bit() {
    let path = golden_path();
    let Ok(recorded) = fs::read_to_string(&path) else {
        eprintln!(
            "skipped: {} does not exist yet; run the ignored test `write_metrics_golden` once \
             on the unchanged metrics to record it",
            path.display()
        );
        return;
    };
    let current = golden_text();
    let recorded_lines: Vec<&str> = recorded.lines().collect();
    let current_lines: Vec<&str> = current.lines().collect();
    let mut differences = Vec::new();
    for line in 0..recorded_lines.len().max(current_lines.len()) {
        let want = recorded_lines.get(line).copied().unwrap_or("<missing>");
        let got = current_lines.get(line).copied().unwrap_or("<missing>");
        if want != got {
            differences.push(format!(
                "line {}: recorded `{want}`, evaluated `{got}`",
                line + 1
            ));
        }
    }
    assert!(
        differences.is_empty(),
        "{} of {} golden lines differ:\n{}",
        differences.len(),
        recorded_lines.len(),
        differences
            .iter()
            .take(20)
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    );
}
