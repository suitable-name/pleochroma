//! The surface finish a design file states, in the form the library stores
//! (`diagram_surface_finish`, `diagram_edge_abrasion`; see `crate::model::surface_finish`).
//!
//! Read from the file's tiers (`surface`, `surface_rq_nm`, `surface_lap_deg`, `surface_aniso`,
//! `surface_no_sweep`, `ground_alpha`, `surface_facets`) and from its `[render_effects]`
//! (`edge_abrasion_*`). A file that sets none of them reads as the default, empty finish, so a
//! design that predates the keys is stored exactly as before.

use crate::model::surface_finish::{
    StoredEdgeAbrasion, StoredSurfaceFinish, StoredSurfaceGrade, StoredTierSurface,
};
use pleochroma_formats::native::{
    SurfaceFacetTable, TierTable,
    design::{self, DesignFile},
};

/// Two index-wheel positions this close are the same facet (the editor's own tolerance for
/// naming a facet, as for the frosting).
const SAME_INDEX_EPSILON: f64 = 1e-6;

/// The grade a tier's own keys state.
fn tier_grade(tier: &TierTable) -> StoredSurfaceGrade {
    StoredSurfaceGrade {
        surface: tier.surface.clone(),
        rq_nm: tier.surface_rq_nm,
        lap_deg: tier.surface_lap_deg,
        aniso: tier.surface_aniso,
        no_sweep: tier.surface_no_sweep,
        ground_alpha: tier.ground_alpha,
    }
}

/// The grade a facet entry states (an entry with nothing set is the ideal polish).
fn facet_grade(facet: &SurfaceFacetTable) -> StoredSurfaceGrade {
    StoredSurfaceGrade {
        surface: facet.surface.clone(),
        rq_nm: facet.rq_nm,
        lap_deg: facet.lap_deg,
        aniso: facet.aniso,
        no_sweep: facet.no_sweep,
        ground_alpha: facet.ground_alpha,
    }
}

/// The surface of tier `tier`, with each facet entry named by its slot in the tier's `indices`.
/// An entry whose position is none of the tier's `indices` names nothing and is dropped.
fn tier_surface(position: u32, tier: &TierTable) -> StoredTierSurface {
    let indices = tier.indices.as_deref().unwrap_or_default();
    let facets = tier
        .surface_facets
        .iter()
        .filter_map(|facet| {
            let slot = indices
                .iter()
                .position(|&index| (index - facet.at).abs() < SAME_INDEX_EPSILON)?;
            Some((u32::try_from(slot).ok()?, facet_grade(facet)))
        })
        .collect();
    StoredTierSurface {
        tier: position,
        grade: tier_grade(tier),
        facets,
    }
}

/// The surface finish `file` states: its tiers' grades by schedule position, and the edge
/// abrasion of its render effects. Empty for a file that states none.
#[must_use]
pub fn surface_finish_of(file: &DesignFile) -> StoredSurfaceFinish {
    let tiers = file
        .tiers
        .iter()
        .enumerate()
        .filter_map(|(position, tier)| Some(tier_surface(u32::try_from(position).ok()?, tier)))
        .collect();
    let edge = file
        .render_effects
        .as_ref()
        .map(|effects| StoredEdgeAbrasion {
            width: effects.edge_abrasion_width,
            alpha: effects.edge_abrasion_alpha,
            halo_rq_nm: effects.edge_abrasion_halo_rq_nm,
        });
    StoredSurfaceFinish { tiers, edge }.normalised()
}

/// The surface finish a stored design file states, as [`surface_finish_of`] reads it.
///
/// The bytes are a `.psdd` file; empty for bytes that are not a readable
/// design file. Used to backfill the tables for a library that predates them and to follow a
/// mirrored design's attached file.
#[must_use]
pub fn surface_finish_of_bytes(bytes: &[u8]) -> StoredSurfaceFinish {
    let Ok(text) = std::str::from_utf8(bytes) else {
        return StoredSurfaceFinish::default();
    };
    design::parse(text)
        .map(|file| surface_finish_of(&file))
        .unwrap_or_default()
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use pleochroma_formats::native::{
        MaterialTable, NativeMeetConstraint, NativePreformShape, PreformTable, SurfaceColumns,
        design::ScheduleTable,
    };

    fn tier(name: &str) -> TierTable {
        let mut tier = TierTable::new(
            name,
            NativeMeetConstraint::ScaleReference { mast: 0.5 },
            Vec::new(),
        );
        tier.angle_deg = Some(41.0);
        tier.indices = Some(vec![0.0, 24.0, 48.0, 72.0]);
        tier
    }

    fn file_of(tiers: Vec<TierTable>) -> DesignFile {
        let schedule = ScheduleTable {
            gemcad_version: "5.0".to_string(),
            gear_teeth: 96,
            gear_reference_angle: 0.0,
            symmetry_order: 4,
            mirror: true,
            refractive_index: 1.54,
            headers: Vec::new(),
            footnotes: Vec::new(),
            unknown: std::iter::empty().collect(),
        };
        DesignFile::new(
            PreformTable::new(NativePreformShape::Block, 1.0, 1.0, 2.0),
            MaterialTable::new(None, None, None),
            schedule,
            None,
            tiers,
        )
    }

    fn commercial() -> SurfaceColumns {
        SurfaceColumns {
            surface: Some("Commercial".to_string()),
            ..SurfaceColumns::default()
        }
    }

    /// The text of a one-tier design file whose tier has a Commercial finish and whose render
    /// effects abrade the edges (`surface_set`), or of a plain one (not), for the vault's
    /// migration and storage tests (so only with the `sqlite` feature).
    #[cfg(feature = "sqlite")]
    pub fn surface_design_text(surface_set: bool) -> String {
        let mut file = file_of(vec![if surface_set {
            tier("C1").with_surface(commercial())
        } else {
            tier("C1")
        }]);
        if surface_set {
            file = file.with_render_effects(design::RenderEffectsTable {
                edge_abrasion_width: 0.02,
                edge_abrasion_alpha: 0.3,
                ..design::RenderEffectsTable::default()
            });
        }
        design::to_string(&file).expect("serializes")
    }

    #[test]
    fn a_file_without_surface_keys_states_no_finish() {
        let file = file_of(vec![tier("P1"), tier("C1")]);
        assert!(surface_finish_of(&file).is_empty());
        let text = design::to_string(&file).expect("serializes");
        assert!(surface_finish_of_bytes(text.as_bytes()).is_empty());
        assert!(surface_finish_of_bytes(b"not a design").is_empty());
        assert!(surface_finish_of_bytes(&[0xFF, 0xFE]).is_empty());
    }

    #[test]
    fn tiers_and_facets_are_read_by_position_and_slot() {
        let facets = vec![
            SurfaceFacetTable {
                at: 48.0,
                surface: Some("Fine".to_string()),
                ..SurfaceFacetTable::default()
            },
            // Restores the ideal polish on one facet.
            SurfaceFacetTable {
                at: 24.0,
                ..SurfaceFacetTable::default()
            },
            // Names no facet of the tier: dropped.
            SurfaceFacetTable {
                at: 13.0,
                surface: Some("Mirror".to_string()),
                ..SurfaceFacetTable::default()
            },
        ];
        let file = file_of(vec![
            tier("P1"),
            tier("C1").with_surface(SurfaceColumns {
                facets,
                ..commercial()
            }),
            tier("C2").with_surface(SurfaceColumns {
                surface: Some("Custom".to_string()),
                rq_nm: Some(3.5),
                lap_deg: Some(90.0),
                aniso: Some(2.0),
                no_sweep: true,
                ..SurfaceColumns::default()
            }),
        ]);
        let finish = surface_finish_of(&file);
        assert_eq!(finish.edge, None);
        assert_eq!(finish.tiers.len(), 2);

        let c1 = &finish.tiers[0];
        assert_eq!(c1.tier, 1);
        assert_eq!(c1.grade.surface.as_deref(), Some("Commercial"));
        assert_eq!(
            c1.facets,
            vec![
                (1, StoredSurfaceGrade::default()),
                (
                    2,
                    StoredSurfaceGrade {
                        surface: Some("Fine".to_string()),
                        ..StoredSurfaceGrade::default()
                    }
                ),
            ]
        );

        let c2 = &finish.tiers[1];
        assert_eq!(c2.tier, 2);
        assert_eq!(c2.grade.rq_nm, Some(3.5));
        assert_eq!(c2.grade.lap_deg, Some(90.0));
        assert_eq!(c2.grade.aniso, Some(2.0));
        assert!(c2.grade.no_sweep);
    }

    #[test]
    fn the_finish_survives_the_file_text() {
        let file = file_of(vec![tier("C1").with_surface(commercial())]);
        let text = design::to_string(&file).expect("serializes");
        assert_eq!(
            surface_finish_of_bytes(text.as_bytes()),
            surface_finish_of(&file)
        );
        assert!(!surface_finish_of_bytes(text.as_bytes()).is_empty());
    }
}
