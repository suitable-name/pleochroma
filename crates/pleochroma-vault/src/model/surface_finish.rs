//! The surface finish a stored design states, kept beside its rows.
//!
//! A design file can give a tier, or single facets of it, a surface finish other than the ideal
//! polish (`surface = "Commercial"` and its companion keys), and can abrade the edges of every
//! facet (`edge_abrasion_*`). The angle-table rows of a library design cannot hold either, so a
//! design imported from a design file keeps them in the `diagram_surface_finish` and
//! `diagram_edge_abrasion` side tables -- see `Database::set_diagram_surface_finish` and
//! `Database::diagram_surface_finish`. A library thumbnail or batch render then draws the polish
//! the cutter chose, and the cached-render fingerprints follow it, without opening the design
//! file.
//!
//! Like the frosting ([`super::frosting`]), a tier is named by its position in the design's
//! schedule and a facet by its slot in the tier's index list. The grade is stored the way the
//! file writes it (the grade name and its numbers as given): this crate neither knows the grades
//! nor does any physics with them.

/// One surface grade as the design file writes it. The default is the ideal polish, which is
/// what a facet override with every field unset means ("restore the perfect polish here").
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StoredSurfaceGrade {
    /// The grade name (`"Commercial"`, `"Ground (Fine)"`, `"Custom"`, ...); `None` is ideal.
    pub surface: Option<String>,
    /// The RMS roughness in nanometres of a `"Custom"` polish.
    pub rq_nm: Option<f64>,
    /// The fixed scratch direction in degrees (the Custom Angle); `None` follows the machine.
    pub lap_deg: Option<f64>,
    /// The anisotropy ratio of a `"Custom"` finish.
    pub aniso: Option<f64>,
    /// Whether the fan of scratch directions is collapsed to the machine's nominal angle.
    pub no_sweep: bool,
    /// The GGX roughness of a ground (or frosted) finish; `None` is the default.
    pub ground_alpha: Option<f64>,
}

impl StoredSurfaceGrade {
    /// Whether this is the ideal polish with nothing set.
    #[must_use]
    pub const fn is_ideal(&self) -> bool {
        self.surface.is_none()
            && self.rq_nm.is_none()
            && self.lap_deg.is_none()
            && self.aniso.is_none()
            && !self.no_sweep
            && self.ground_alpha.is_none()
    }
}

/// The surface of one tier: its own grade and the facets that differ from it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StoredTierSurface {
    /// The tier's position in the design's schedule (0-based).
    pub tier: u32,
    /// The grade of the tier itself; ideal when only single facets are set.
    pub grade: StoredSurfaceGrade,
    /// The facets whose grade differs from the tier's, by slot in the tier's index list.
    pub facets: Vec<(u32, StoredSurfaceGrade)>,
}

/// The abraded band along every facet edge.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct StoredEdgeAbrasion {
    /// The band width in model units.
    pub width: f64,
    /// The GGX roughness of the band.
    pub alpha: f64,
    /// The polish roughness (nm Rq) the neighbouring facet is dulled to at the band's rim.
    pub halo_rq_nm: f64,
}

impl StoredEdgeAbrasion {
    /// Whether the band is off (nothing set).
    #[must_use]
    pub fn is_off(&self) -> bool {
        self.width == 0.0 && self.alpha == 0.0 && self.halo_rq_nm == 0.0
    }
}

/// Everything a stored design states about its surface finish.
///
/// The default (no tiers, no edge) is a design that renders with the ideal polish everywhere,
/// which is also what every design that predates the tables reads as.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StoredSurfaceFinish {
    /// The tiers with a finish of their own or with facet overrides, ascending by position.
    pub tiers: Vec<StoredTierSurface>,
    /// The edge abrasion, when it is on.
    pub edge: Option<StoredEdgeAbrasion>,
}

impl StoredSurfaceFinish {
    /// Whether the design states no surface finish at all.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.tiers.is_empty() && self.edge.is_none()
    }

    /// This finish in canonical form: one entry per tier (a later entry for the same tier
    /// wins), tiers ascending, each tier's facets ascending by slot with a later entry for the
    /// same slot winning; a tier that states nothing and an edge that is off are dropped.
    #[must_use]
    pub fn normalised(self) -> Self {
        let mut by_tier: std::collections::BTreeMap<u32, StoredTierSurface> =
            std::collections::BTreeMap::new();
        for mut tier in self.tiers {
            let mut by_slot: std::collections::BTreeMap<u32, StoredSurfaceGrade> =
                std::collections::BTreeMap::new();
            for (slot, grade) in std::mem::take(&mut tier.facets) {
                by_slot.insert(slot, grade);
            }
            tier.facets = by_slot.into_iter().collect();
            by_tier.insert(tier.tier, tier);
        }
        Self {
            tiers: by_tier
                .into_values()
                .filter(|tier| !tier.grade.is_ideal() || !tier.facets.is_empty())
                .collect(),
            edge: self.edge.filter(|edge| !edge.is_off()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn commercial() -> StoredSurfaceGrade {
        StoredSurfaceGrade {
            surface: Some("Commercial".to_string()),
            ..StoredSurfaceGrade::default()
        }
    }

    #[test]
    fn the_default_grade_is_ideal_and_any_field_makes_it_not() {
        assert!(StoredSurfaceGrade::default().is_ideal());
        assert!(!commercial().is_ideal());
        for grade in [
            StoredSurfaceGrade {
                rq_nm: Some(3.0),
                ..StoredSurfaceGrade::default()
            },
            StoredSurfaceGrade {
                no_sweep: true,
                ..StoredSurfaceGrade::default()
            },
            StoredSurfaceGrade {
                ground_alpha: Some(0.2),
                ..StoredSurfaceGrade::default()
            },
        ] {
            assert!(!grade.is_ideal(), "{grade:?}");
        }
    }

    #[test]
    fn normalising_orders_tiers_and_slots_and_drops_what_states_nothing() {
        let finish = StoredSurfaceFinish {
            tiers: vec![
                StoredTierSurface {
                    tier: 5,
                    grade: commercial(),
                    facets: vec![
                        (3, commercial()),
                        (1, StoredSurfaceGrade::default()),
                        (3, StoredSurfaceGrade::default()),
                    ],
                },
                // States nothing: dropped.
                StoredTierSurface {
                    tier: 2,
                    ..StoredTierSurface::default()
                },
                // Only a facet override: kept.
                StoredTierSurface {
                    tier: 1,
                    grade: StoredSurfaceGrade::default(),
                    facets: vec![(0, commercial())],
                },
            ],
            edge: Some(StoredEdgeAbrasion::default()),
        }
        .normalised();
        let tiers: Vec<u32> = finish.tiers.iter().map(|t| t.tier).collect();
        assert_eq!(tiers, [1, 5]);
        assert_eq!(
            finish.tiers[1].facets,
            vec![
                (1, StoredSurfaceGrade::default()),
                (3, StoredSurfaceGrade::default())
            ],
            "a later entry for a slot wins"
        );
        assert_eq!(finish.edge, None, "an edge that is off is dropped");
        assert!(!finish.is_empty());
        assert!(StoredSurfaceFinish::default().is_empty());
    }
}
