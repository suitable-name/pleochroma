//! Derives a design's measured proportions and shape from its own reconstructed
//! facet planes, and merges a re-import's fresh metadata over an existing catalogue
//! row without discarding hand-entered fields.
//!
//! The rule that turns a measured solid into the stored fields, the shape classification
//! and the merge are `pleochroma_vault::local`'s (shared with the Android core and the
//! browser); this module supplies the one thing the vault does not own, the geometry:
//! [`FlatPlanes`] reconstructs the planes the viewport shows and measures them.

use crate::gui::library::detail::reconstruct_planes;
use glam::DVec3;
use pleochroma::geometry::{GpuFacetPlane, cuts::FacetSpec, girdle, stone_metrics};
use pleochroma_vault::{
    local::{SolidMeasure, SolidSource},
    model::{angle::AngleSetting, detail::FacetingDiagramDetail},
};

pub use pleochroma_vault::local::merge_reimport_metadata;

/// Fills in the proportion fields `pleochroma_vault::local::import_asc` always leaves
/// `None` -- it only reads the `.asc` file's header/tiers, which don't carry these.
/// Reconstructs the same 3D facet planes the viewport shows ([`reconstruct_planes`],
/// reused from `gui::detail::load_diagram_detail` rather than re-parsed) and measures
/// them with `stone_metrics::measure_solid`, measured at ~0.01% median error on L/W and
/// ~0.09% on Vol/W^3 against the real `.asc` corpus. `measure_solid` returns `None` for
/// a degenerate/unbounded arrangement, and every field below stays `None` rather than
/// fabricated when that happens -- never store a number this couldn't measure.
///
/// Values are written with plain `f64::to_string()`, matching `local::import_asc`'s own
/// convention one call site up. The real `facet_diagrams.sqlite`'s ratio/volume columns
/// are all REAL-affinity, so SQLite normalises whatever numeric text is written here
/// regardless of decimal-place convention -- it only has to parse as a plain number.
pub fn apply_measured_metadata(detail: &mut FacetingDiagramDetail) {
    pleochroma_vault::local::apply_measured_metadata(detail, &mut FlatPlanes::default());
    apply_concave_rows(detail);
}

/// Records a design file's concave tiers on `detail`: one tool-carrying row per tier
/// appended to the angle table (facet line in the flat rows' own format, plus the
/// formatted tool line), and the `concave_tiers` / `concave_facets` counts the
/// library's `has_concave` filter and the sync stamp read. Read from the `.psdd`
/// attachment, the design of record, so every writer that attaches one (import, save
/// write-back) records the same thing. A design without concave tiers is left exactly
/// as it was (no rows, both counts `0`). Runs after the measurement, which must see
/// the flat rows only.
pub fn apply_concave_rows(detail: &mut FacetingDiagramDetail) {
    detail.angle_settings_table.retain(|row| row.tool.is_none());
    detail.concave_tiers = 0;
    detail.concave_facets = 0;
    let Some(position) = pleochroma_vault::local::native_design_attachment_position(
        detail.attached_files.iter().map(|f| f.name.as_str()),
    ) else {
        return;
    };
    let Ok(text) = std::str::from_utf8(&detail.attached_files[position].content) else {
        return;
    };
    // A planar file (the usual case) skips the full load.
    if !text.contains("[[concave_tiers]]") {
        return;
    }
    let Ok(loaded) = pleochroma_cut_core::native::design_from_str(text) else {
        return;
    };
    let first = detail.angle_settings_table.len();
    for (i, tier) in loaded.design.concave_tiers.iter().enumerate() {
        detail.angle_settings_table.push(AngleSetting {
            order_index: u32::try_from(first + i).expect("tier row count fits u32"),
            facet: tier.name.clone(),
            angle: format!("{}\u{b0}", tier.angle_deg),
            index: tier
                .indices
                .iter()
                .map(f64::to_string)
                .collect::<Vec<_>>()
                .join(", "),
            notes: tier.instructions.clone(),
            tool: Some(tier.tool.code().to_owned()),
            tool_line: Some(pleochroma_editor::view_model::rows::concave_tool_line(tier)),
        });
        detail.concave_facets +=
            u32::try_from(tier.indices.len()).expect("facet index count fits u32");
    }
    detail.concave_tiers =
        u32::try_from(loaded.design.concave_tiers.len()).expect("tier count fits u32");
}

/// The desktop's [`SolidSource`]: the viewport's reconstructed planes over the flat rows,
/// measured with `stone_metrics::measure_solid`.
#[derive(Default)]
struct FlatPlanes {
    /// The planes of the last [`SolidSource::measure`], for the girdle count.
    planes: Vec<GpuFacetPlane>,
}

impl SolidSource for FlatPlanes {
    fn measure(&mut self, detail: &FacetingDiagramDetail) -> Option<SolidMeasure> {
        let facet_specs: Vec<FacetSpec> = detail
            .angle_settings_table
            .iter()
            .filter(|a| a.tool.is_none())
            .map(|a| FacetSpec {
                facet: a.facet.clone(),
                angle: a.angle.clone(),
                index: a.index.clone(),
                notes: a.notes.clone(),
            })
            .collect();
        self.planes = reconstruct_planes(None, detail.index_gear.as_deref(), &facet_specs);
        let dvec_planes: Vec<(DVec3, f64)> = self
            .planes
            .iter()
            .map(|p| {
                (
                    DVec3::new(
                        f64::from(p.normal[0]),
                        f64::from(p.normal[1]),
                        f64::from(p.normal[2]),
                    ),
                    // `measure_solid` wants unit outward normals with `n.x <= m`;
                    // `GpuFacetPlane`'s convention is `n.x + d = 0` (see
                    // `optics::raytracer::intersect`'s ray-plane test), i.e. `m = -d`.
                    -f64::from(p.d),
                )
            })
            .collect();
        let m = stone_metrics::measure_solid(&dvec_planes)?;
        Some(SolidMeasure {
            length_axis: m.length_axis,
            width_axis: m.width_axis,
            total_height: m.total_height,
            crown_height: m.crown_height,
            pavilion_depth: m.pavilion_depth,
            volume: m.volume,
        })
    }

    /// Classified from the SAME planes, so the girdle outline is measured rather than
    /// inferred from the schedule's fold count.
    fn girdle_sides(&mut self) -> usize {
        girdle::classify_girdle_plane_indices(&self.planes).len()
    }
}

/// [`pleochroma_vault::local::classify_shape_by_sides`] over `planes`' girdle outline,
/// for the tests that pin the rule against real stones.
#[cfg(test)]
pub(super) fn classify_shape(planes: &[GpuFacetPlane], lw: f64) -> Option<String> {
    pleochroma_vault::local::classify_shape_by_sides(
        girdle::classify_girdle_plane_indices(planes).len(),
        lw,
    )
}
