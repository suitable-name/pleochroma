use super::{
    BTreeMap, DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2, Design, FacetSide, FlatRing, GIRDLE_COS_EPS,
    InvalidReason, MIN_WIDTH, ManufacturabilityWarning, SolidStatus, SolvedTier, StoneAnalysis,
    StoneProportions, check_manufacturability, girdle_band_in, is_horizontal_angle_deg,
    mean_height, polygon_area, rings_by_plane, solid_facets_in, tier_hinges_in, tier_plane_ranges,
};
use pleochroma::geometry::stone_metrics::build_solid_mesh_and_measure;

/// Solves `design` and measures the stone -- [`analyze_solved`] after [`Design::solve`].
///
/// `with_hinges` also reads the girdle corners (the hinge) of every flat crown or pavilion tier
/// (needed for the ORIGINAL stone only).
///
/// # Errors
///
/// [`InvalidReason::DoesNotSolve`] when the design does not solve, [`InvalidReason::NotClosed`]
/// when its planes do not bound a finite solid.
pub fn analyze(design: &Design, with_hinges: bool) -> Result<StoneAnalysis, InvalidReason> {
    analyze_controlled(
        design,
        with_hinges,
        &pleochroma::geometry::meet_solver::SolveControl::default(),
    )
}

/// Analysis with cancellation checked inside every authoring solve.
///
/// # Errors
///
/// As [`analyze`], plus [`InvalidReason::Preservation`] when `with_hinges` is set and the
/// meet baseline cannot be captured (or the solve was cancelled).
pub fn analyze_controlled(
    design: &Design,
    with_hinges: bool,
    control: &pleochroma::geometry::meet_solver::SolveControl<'_>,
) -> Result<StoneAnalysis, InvalidReason> {
    let solved = design
        .solve_with(control)
        .map_err(|err| InvalidReason::DoesNotSolve(err.to_string()))?;
    analyze_solved_controlled(design, solved, with_hinges, control)
}

/// Measures the stone `solved` describes for `design`.
///
/// # Errors
///
/// [`InvalidReason::NotClosed`] when the planes do not bound a finite solid.
pub fn analyze_solved(
    design: &Design,
    solved: Vec<SolvedTier>,
    with_hinges: bool,
) -> Result<StoneAnalysis, InvalidReason> {
    analyze_solved_controlled(
        design,
        solved,
        with_hinges,
        &pleochroma::geometry::meet_solver::SolveControl::default(),
    )
}

fn analyze_solved_controlled(
    design: &Design,
    solved: Vec<SolvedTier>,
    with_hinges: bool,
    control: &pleochroma::geometry::meet_solver::SolveControl<'_>,
) -> Result<StoneAnalysis, InvalidReason> {
    let preservation = if with_hinges {
        Some(std::sync::Arc::new(
            pleochroma_cut_core::meet_preservation::MeetPreservationBaseline::capture_controlled(
                design, control,
            )
            .map_err(|e| InvalidReason::Preservation(e.to_string()))?,
        ))
    } else {
        None
    };
    let planes = design.planes_from_solved(&solved);
    // One arrangement for the mesh and the measures.
    let (SolidStatus::Closed(mesh), metrics) = build_solid_mesh_and_measure(&planes) else {
        return Err(InvalidReason::NotClosed);
    };
    let metrics = metrics.ok_or(InvalidReason::NotClosed)?;
    let proportions = StoneProportions::from_solid(&metrics, &mesh, &planes);
    let facets = solid_facets_in(design, &solved, &planes, &mesh);
    let ranges = tier_plane_ranges(design, &solved);
    let girdle_thinnest_percent = girdle_band_in(design, &ranges, &planes, &mesh)
        .filter(|_| metrics.width_axis > MIN_WIDTH)
        .map(|band| 100.0 * band.min_thickness / metrics.width_axis);
    let rings = rings_by_plane(&mesh);

    let mut flats = Vec::new();
    let mut band: Option<(f64, f64)> = None;
    for (tier_index, (tier, range)) in design.tiers.iter().zip(&ranges).enumerate() {
        if is_horizontal_angle_deg(tier.angle_deg) {
            let ring = range.clone().find_map(|plane| rings.get(&plane));
            if let Some(ring) = ring {
                flats.push(FlatRing {
                    tier_index,
                    side: FacetSide::of_angle_deg(tier.angle_deg),
                    area: polygon_area(ring),
                    height: mean_height(ring),
                });
            }
        } else if tier.angle_deg.abs().to_radians().cos().abs() <= GIRDLE_COS_EPS {
            for plane in range.clone() {
                if let Some(&ring) = rings.get(&plane) {
                    for vertex in ring {
                        band = Some(band.map_or((vertex.y, vertex.y), |(lo, hi)| {
                            (lo.min(vertex.y), hi.max(vertex.y))
                        }));
                    }
                }
            }
        }
    }

    let undersized =
        check_manufacturability(design, &solved, DEFAULT_MIN_FACET_AREA_FRACTION_OF_W2)
            .into_iter()
            .filter_map(|warning| match warning {
                ManufacturabilityWarning::UndersizedFacet { tier_index, .. } => Some(tier_index),
                _ => None,
            })
            .collect();
    let hinges = if with_hinges {
        tier_hinges_in(design, &solved, &planes, &mesh)
    } else {
        BTreeMap::new()
    };
    let crown_to_pavilion = match (proportions.crown_height, proportions.pavilion_depth) {
        (Some(crown), Some(pavilion)) if pavilion > MIN_WIDTH => Some(crown / pavilion),
        _ => None,
    };
    let depth_percent = (metrics.width_axis > MIN_WIDTH)
        .then(|| 100.0 * proportions.total_depth / metrics.width_axis);
    Ok(StoneAnalysis {
        preservation,
        authored: Some(std::sync::Arc::new(design.clone())),
        girdle_percent: proportions.girdle_to_width_percent,
        girdle_thinnest_percent,
        table_percent: proportions.table_percent,
        crown_height: proportions.crown_height,
        pavilion_depth: proportions.pavilion_depth,
        crown_to_pavilion,
        depth_percent,
        facets,
        flats,
        girdle_band: band,
        undersized,
        hinges,
        ranges,
        solved,
        planes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma::geometry::stone_metrics::{build_solid_mesh, measure_solid};
    use pleochroma_cut_core::{ConstraintTier, PreformSpec, ScheduleMeta};

    fn brilliant() -> Design {
        Design::new(
            PreformSpec::block(2.0, 1.0, 4.0),
            ScheduleMeta::standard_round_brilliant(),
            ConstraintTier::standard_round_brilliant(),
        )
    }

    fn bits(value: Option<f64>) -> Option<u64> {
        value.map(f64::to_bits)
    }

    /// The analysis builds the mesh and the measures from one arrangement; every figure it
    /// reads from them equals the one the two separate calls give, bit for bit.
    #[test]
    fn the_shared_arrangement_gives_the_figures_of_the_separate_calls_bit_for_bit() {
        let analysis = analyze(&brilliant(), false).unwrap();
        let planes = analysis.planes.clone();
        let SolidStatus::Closed(mesh) = build_solid_mesh(&planes) else {
            panic!("the brilliant closes");
        };
        let metrics = measure_solid(&planes).expect("the brilliant measures");
        let expected = StoneProportions::from_solid(&metrics, &mesh, &planes);

        assert_eq!(
            bits(analysis.girdle_percent),
            bits(expected.girdle_to_width_percent)
        );
        assert_eq!(bits(analysis.table_percent), bits(expected.table_percent));
        assert_eq!(bits(analysis.crown_height), bits(expected.crown_height));
        assert_eq!(bits(analysis.pavilion_depth), bits(expected.pavilion_depth));
        let depth_percent = (metrics.width_axis > MIN_WIDTH)
            .then(|| 100.0 * expected.total_depth / metrics.width_axis);
        assert_eq!(bits(analysis.depth_percent), bits(depth_percent));
        let crown_to_pavilion = match (expected.crown_height, expected.pavilion_depth) {
            (Some(crown), Some(pavilion)) if pavilion > MIN_WIDTH => Some(crown / pavilion),
            _ => None,
        };
        assert_eq!(bits(analysis.crown_to_pavilion), bits(crown_to_pavilion));
    }
}
