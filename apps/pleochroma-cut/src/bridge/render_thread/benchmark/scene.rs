//! Deterministic geometry inputs. Exported files never trigger a library lookup.

use anyhow::{Context, Result, bail, ensure};
use pleochroma::geometry::{cuts::StandardGemCuts, plane::GpuFacetPlane, tool::ToolPrimitive};
use std::path::Path;

pub(super) struct Scene {
    pub label: String,
    pub planes: Vec<GpuFacetPlane>,
    pub tools: Vec<ToolPrimitive>,
}

impl Scene {
    pub fn load(path: Option<&Path>, second: bool) -> Result<Self> {
        let Some(path) = path else {
            return Ok(Self {
                label: if second {
                    "builtin:emerald"
                } else {
                    "builtin:round-brilliant"
                }
                .into(),
                planes: if second {
                    StandardGemCuts::emerald_cut()
                } else {
                    StandardGemCuts::standard_round_brilliant()
                },
                tools: Vec::new(),
            });
        };
        let extension = path
            .extension()
            .and_then(|v| v.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        ensure!(
            matches!(extension.as_str(), "asc" | "psdd" | "pleochroma"),
            "{}: expected .asc or .psdd",
            path.display()
        );
        let bytes = std::fs::read(path).with_context(|| format!("reading {}", path.display()))?;
        let (planes, tools) = match extension.as_str() {
            "asc" => {
                let text = pleochroma_formats::asc::decode_asc_bytes(&bytes);
                let loaded = pleochroma_editor::loading::design_from_asc_text(
                    &path.display().to_string(),
                    &text,
                    None,
                )
                .map_err(anyhow::Error::msg)?;
                solved_geometry(&loaded.design)?
            }
            "psdd" | "pleochroma" => {
                let loaded =
                    pleochroma_cut_core::native::design_from_str(std::str::from_utf8(&bytes)?)?;
                solved_geometry(&loaded.design)?
            }
            _ => bail!("{}: expected .asc or .psdd", path.display()),
        };
        ensure!(!planes.is_empty(), "{} has no planes", path.display());
        ensure!(
            planes
                .iter()
                .all(|p| p.d.is_finite() && p.normal.iter().all(|v| v.is_finite())),
            "non-finite geometry"
        );
        Ok(Self {
            label: path.display().to_string(),
            planes,
            tools,
        })
    }
}

fn solved_geometry(
    design: &pleochroma_cut_core::Design,
) -> Result<(Vec<GpuFacetPlane>, Vec<ToolPrimitive>)> {
    let halfspaces = design.planes()?;
    let (tools, _) = design.concave_tools_from_planes(&halfspaces)?;
    let planes = halfspaces
        .into_iter()
        .map(|(normal, offset)| {
            // The native halfspace is n.x <= offset; GPU planes use n.x+d <= 0.
            let plane = normal.extend(-offset).as_vec4();
            GpuFacetPlane::new(plane.truncate(), plane.w)
        })
        .collect();
    Ok((planes, tools))
}

pub(super) fn checksum(bytes: &[u8]) -> String {
    let hash = bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0100_0000_01b3)
    });
    format!("{hash:016x}")
}
