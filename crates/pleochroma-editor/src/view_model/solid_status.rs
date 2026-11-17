//! The validation-banner text and the viewport's plane and tool data for a design.
//!
//! [`status_text_and_is_problem`] and [`status_text_and_is_problem_from_solved`] turn a design (or
//! an already-solved mast list) into the one-sentence verdict of the Edit tab's banner.
//! [`design_to_gpu_planes`], [`design_to_gpu_geometry`] and [`tier_matches_filter`] are helpers
//! for the viewport and the tier table's filter.

use super::row_format::tier_label;
use crate::solve_policy;
use pleochroma::geometry::{
    GpuFacetPlane, ToolPrimitive,
    meet_solver::SolvedTier,
    stone_metrics::{SolidMetrics, SolidStatus, build_solid_mesh, measure_solid},
};
use pleochroma_cut_core::{Design, DesignSolveError, degenerate_suspects, design::ToolPlacements};

/// [`SolidStatus::Unbounded`]'s escaping planes, named by the tier that contributed each one
/// ([`Design::tier_for_plane_index`]). Falls back to `"plane <n>"` for a plane no tier claims,
/// such as a preform plane.
fn escaping_tier_text(design: &Design, solved: &[SolvedTier], escaping: &[usize]) -> String {
    escaping
        .iter()
        .map(|&plane_index| {
            design
                .tier_for_plane_index(solved, plane_index)
                .map_or_else(
                    || format!("plane {plane_index}"),
                    |tier_index| tier_label(design, tier_index),
                )
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// [`SolidStatus::Degenerate`]'s suspect tiers ([`pleochroma_cut_core::degenerate_suspects`])
/// as a trailing clause, e.g. `" -- check tier 5 (Girdle), tier 8"`. Empty when none is suspect.
fn degenerate_suspects_note(design: &Design, solved: &[SolvedTier]) -> String {
    let suspects = degenerate_suspects(solved);
    if suspects.is_empty() {
        return String::new();
    }
    let names: Vec<String> = suspects
        .into_iter()
        .map(|index| tier_label(design, index))
        .collect();
    format!(" -- check {}", names.join(", "))
}

/// `flat` (the [`measure_solid`] of the design's planes) with the volume of the stone carved
/// by the design's concave tools ([`Design::carved_metrics`]), so the banner, the cut sheet and
/// the CLI quote the same volume. A design without concave tiers, or whose tiers do not resolve,
/// keeps `flat` exactly. `solved` must be the design's own solve.
fn carved_volume(design: &Design, solved: &[SolvedTier], flat: SolidMetrics) -> SolidMetrics {
    if design.concave_tiers.is_empty() {
        return flat;
    }
    design
        .geometry_from_solved(solved)
        .ok()
        .and_then(|(planes, tools, _)| Design::carved_metrics(Some(flat), &planes, &tools))
        .unwrap_or(flat)
}

/// GemCad/GemCutStudio-style proportion ratios from `m`: `L/W`, `H/W`, and `C/W`/`P/W` where a
/// live girdle facet exists. The ratios are dimensionless, so they carry no unit.
///
/// `mm_per_unit` ([`Design::yield_report`]'s scale, `Some` only with a girdle diameter) adds the
/// absolute width and total height in millimetres. `None` omits that clause rather than show
/// model units beside millimetres. Returns `""` for a solid with no width.
pub(super) fn external_proportions_note(m: &SolidMetrics, mm_per_unit: Option<f64>) -> String {
    if m.width_axis <= 0.0 {
        return String::new();
    }
    let mut parts = vec![
        format!("L/W {:.3}", m.length_axis / m.width_axis),
        format!("H/W {:.3}", m.total_height / m.width_axis),
    ];
    if let Some(c) = m.crown_height {
        parts.push(format!("C/W {:.3}", c / m.width_axis));
    }
    if let Some(p) = m.pavilion_depth {
        parts.push(format!("P/W {:.3}", p / m.width_axis));
    }
    if let Some(mm) = mm_per_unit {
        parts.push(format!(
            "{:.2} x {:.2} mm",
            m.width_axis * mm,
            m.total_height * mm
        ));
    }
    format!(", {}", parts.join(", "))
}

/// [`Design::status`] as the Edit tab's validation banner text, and whether it is a problem
/// (red) or all clear (green).
///
/// Each branch is one sentence. `Unbounded` and `Degenerate` name the responsible tiers (see
/// [`escaping_tier_text`] and [`degenerate_suspects_note`]). `Unbounded` is practically
/// unreachable, since the preform caps every direction, but it still gets a message. Either may
/// instead be the plane-cap sentence of [`solve_policy::too_many_planes_message`].
#[must_use]
pub fn status_text_and_is_problem(design: &Design) -> (String, bool) {
    // One solve shared by every figure; `design.status()` and `measure()` would each solve again.
    status_text_and_is_problem_from_result(design, &design.solve())
}

/// [`status_text_and_is_problem`] for a caller that already ran [`Design::solve`].
///
/// `Ok` is [`status_text_and_is_problem_from_solved`]. `Err` shows the failure's own sentence
/// (for example [`pleochroma_cut_core::MissingAnchor`]'s remedy) as a problem, without solving
/// the design again. The tierless check runs first.
#[must_use]
pub fn status_text_and_is_problem_from_result(
    design: &Design,
    solved: &Result<Vec<SolvedTier>, DesignSolveError>,
) -> (String, bool) {
    if design.tiers.is_empty() {
        return ("Preform only -- add tiers.".to_string(), false);
    }
    match solved {
        Ok(solved) => status_text_and_is_problem_from_solved(design, solved),
        // `MissingAnchor`'s `Display` is already one complete sentence per named block.
        Err(failure) => (failure.to_string(), true),
    }
}

/// [`status_text_and_is_problem`] for a caller that already has an up-to-date `solved` mast list.
///
/// The mesh is built from [`Design::planes_from_solved`], so nothing is solved again. There is
/// no `MissingAnchor` arm: a real `solved` list implies the design solves.
#[must_use]
pub fn status_text_and_is_problem_from_solved(
    design: &Design,
    solved: &[SolvedTier],
) -> (String, bool) {
    // See `status_text_and_is_problem`'s identical guard just above.
    if design.tiers.is_empty() {
        return ("Preform only -- add tiers.".to_string(), false);
    }
    let planes = design.planes_from_solved(solved);
    match build_solid_mesh(&planes) {
        SolidStatus::Closed(_) => {
            // Re-meshed rather than cached. `solved` is at hand, so the scale needs no solve.
            let mm_per_unit = design.yield_report(solved).mm_per_unit;
            let volume_note = measure_solid(&planes)
                .map(|m| carved_volume(design, solved, m))
                .map(|m| {
                    format!(
                        " \u{b7} volume {:.4} (model units\u{b3}){}",
                        m.volume,
                        external_proportions_note(&m, mm_per_unit)
                    )
                })
                .unwrap_or_default();
            (format!("Closed solid{volume_note}."), false)
        }
        SolidStatus::Degenerate {
            vertex_count,
            volume,
        } => {
            // A list from `solve_policy::solve_cancellably`'s plane-cap fallback is all `Failed`,
            // with no real solve behind it. `likely_hit_plane_cap` reads its tells cheaply; only
            // a hit re-verifies via `too_many_planes_message`.
            if solve_policy::likely_hit_plane_cap(solved)
                && let Some(message) = solve_policy::too_many_planes_message(design)
            {
                return (message, true);
            }
            // See `status_text_and_is_problem`'s matching arm.
            let volume_text = volume.map_or_else(
                || "non-finite".to_string(),
                |v| format!("{v:.4} (model units\u{b3})"),
            );
            let suspects_note = degenerate_suspects_note(design, solved);
            (
                format!(
                    "Degenerate: only {vertex_count} distinct vertex(es), volume \
                     {volume_text}{suspects_note}."
                ),
                true,
            )
        }
        SolidStatus::Unbounded { escaping } => {
            if solve_policy::likely_hit_plane_cap(solved)
                && let Some(message) = solve_policy::too_many_planes_message(design)
            {
                return (message, true);
            }
            let tier_text = escaping_tier_text(design, solved, &escaping);
            if tier_text.is_empty() {
                // The builder named no escaping plane: do not print an empty list.
                return (
                    "Unbounded: the planes do not close the solid.".to_string(),
                    true,
                );
            }
            (
                format!("Unbounded: {tier_text} never close the solid."),
                true,
            )
        }
    }
}

/// The plane arrangement of `design` as the `GpuFacetPlane`s the viewport draws, with the
/// offset negated to match the viewport's convention.
///
/// On `Err` (a block with no scale-reference tier yet) this is empty rather than invented
/// geometry: a blank viewport is the truthful result.
#[must_use]
#[expect(
    clippy::cast_possible_truncation,
    reason = "f32 is the GPU plane precision; plane offsets are in the normal float range"
)]
pub fn design_to_gpu_planes(design: &Design) -> Vec<GpuFacetPlane> {
    design
        .planes()
        .unwrap_or_default()
        .into_iter()
        .map(|(normal, offset)| GpuFacetPlane::new(normal.as_vec3(), -offset as f32))
        .collect()
}

/// [`design_to_gpu_planes`] plus the design's concave tools and their placements.
///
/// The tools come from the design's own solve ([`Design::concave_tools_from_solved`]). A design
/// without concave tiers returns no tools and does not solve again. If the design does not
/// solve, or its concave tiers do not resolve, the tools are empty too: the flat stone alone is
/// shown rather than a partial set of tools.
#[must_use]
pub fn design_to_gpu_geometry(
    design: &Design,
) -> (Vec<GpuFacetPlane>, Vec<ToolPrimitive>, ToolPlacements) {
    let planes = design_to_gpu_planes(design);
    if design.concave_tiers.is_empty() {
        return (planes, Vec::new(), Vec::new());
    }
    let (tools, placements) = design
        .solve()
        .ok()
        .and_then(|solved| design.concave_tools_from_solved(&solved).ok())
        .unwrap_or_default();
    (planes, tools, placements)
}

/// Whether `haystack` contains `filter`, ignoring case: the tier search box's test.
///
/// An empty `filter` always matches. Slint has no substring operation, so the filter runs here.
#[must_use]
pub fn tier_matches_filter(haystack: &str, filter: &str) -> bool {
    let filter = filter.trim();
    if filter.is_empty() {
        return true;
    }
    haystack.to_lowercase().contains(&filter.to_lowercase())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::EditorSession;
    use pleochroma::geometry::meet_solver::MeetConstraint;
    use pleochroma_cut_core::{ConstraintTier, Edit};

    /// A one-tier design; `MeetExisting` leaves its block without an anchor (the solve
    /// fails with `MissingAnchor`), `ScaleReference` makes it solve.
    fn design_with(constraint: MeetConstraint) -> Design {
        let mut session = EditorSession::fresh();
        session
            .apply(Edit::AddTier {
                index: 0,
                tier: ConstraintTier {
                    angle_deg: -41.0,
                    name: "P1".to_string(),
                    indices: vec![0.0, 24.0],
                    constraint,
                    imported_meet: None,
                    original_notes: None,
                    detached: Vec::new(),
                },
            })
            .unwrap();
        session.design
    }

    #[test]
    fn status_from_result_matches_the_solving_form_for_failed_and_solved_designs() {
        let failing = design_with(MeetConstraint::MeetExisting);
        let failed = failing.solve();
        assert!(failed.is_err(), "an unanchored design must fail to solve");
        let (text, is_problem) = status_text_and_is_problem_from_result(&failing, &failed);
        assert!(is_problem);
        assert_eq!((text, is_problem), status_text_and_is_problem(&failing));

        let solving = design_with(MeetConstraint::ScaleReference(0.65));
        let solved = solving.solve();
        assert_eq!(
            status_text_and_is_problem_from_result(&solving, &solved),
            status_text_and_is_problem_from_solved(&solving, solved.as_ref().unwrap())
        );
        assert_eq!(
            status_text_and_is_problem_from_result(&solving, &solved),
            status_text_and_is_problem(&solving)
        );
    }

    #[test]
    fn status_from_result_keeps_the_tierless_guard_first() {
        let bare = EditorSession::fresh().design;
        assert_eq!(
            status_text_and_is_problem_from_result(&bare, &bare.solve()),
            ("Preform only -- add tiers.".to_string(), false)
        );
    }
}
