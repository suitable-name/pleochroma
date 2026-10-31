//! Surface finish of tiers and facets: how well each facet is polished, or how coarsely it is
//! ground, as authored in the design.
//!
//! # Where it lives
//!
//! A [`TierSurface`] sits in [`Design::tier_surfaces`], keyed by the tier's [`TierId`] exactly
//! like [`Design::tier_frosting`], so it survives add, remove, move and undo. A tier absent from
//! the map has the ideal finish (a perfect polish). A design without any surface finish has an
//! empty map, which keeps its behavior and the bytes of its file and its scenes exactly as they
//! were. It changes no geometry, so it never needs a solve, and it changes only through the
//! undoable [`crate::edit::Edit::SetTierSurface`].
//!
//! The faceting machine (where the scratches point) is a per-user preference and is NOT stored
//! here: a saved design holds only what the cutter chose for its tiers and facets, so the same
//! design renders for each cutter's machine.
//!
//! # Tier and facet
//!
//! A [`SurfaceFinish`] names a grade ([`SurfaceGrade`]: a polish preset, a ground preset or
//! Custom) with the values that grade needs, plus the Custom Angle (a fixed scratch direction),
//! No Sweep and the roughness of a frosted facet. A [`TierSurface`] has one finish for the whole
//! tier and, optionally, a different finish for single facets, named by index-wheel position
//! like [`super::TierFrosting::Facets`].
//!
//! # From the design to the renderer
//!
//! The renderer wants one packed [`FacetSurface`] word per facet plane.
//! [`Design::facet_surfaces_from_solved`] gives them in [`Design::planes_from_solved`]'s
//! arrangement (empty when nothing is authored, which is the ideal scene), and
//! [`Design::facet_surfaces_in`] the same for any arrangement built from the same schedule. A
//! frosted facet keeps its frosted finish; the roughness the design states for it (the
//! `ground_alpha` of its finish) becomes its ground word. `.asc` has no notation for surface
//! finish, so none of this reaches an exported `.asc` schedule.

use super::{Design, TierId};
use glam::DVec3;
use pleochroma::{
    geometry::{cuts::StandardGemCuts, meet_solver::SolvedTier},
    optics::raytracer::surface::{DEFAULT_SCRATCH_STRENGTH, FacetSurface, PolishPreset},
};
use std::collections::BTreeMap;

/// Two index-wheel positions this close are the same facet.
const SAME_INDEX_EPSILON: f64 = 1e-6;

/// The roughness `α` of the "Ground (Fine)" grade (a 1200-grit finish).
pub const GROUND_FINE_ALPHA: f64 = 0.30;

/// The roughness `α` of the "Ground (Coarse)" grade (a 600-grit finish).
pub const GROUND_COARSE_ALPHA: f64 = 0.50;

/// The largest polish roughness of a Custom finish, in nanometres (the packed word's range).
pub const MAX_CUSTOM_RQ_NM: f64 = 100.0;

/// The largest anisotropy ratio of a Custom finish (the packed word's range).
pub const MAX_CUSTOM_ANISOTROPY: f64 = 15.0;

/// The polish roughness a new Custom finish starts with, in nanometres.
pub const DEFAULT_CUSTOM_RQ_NM: f64 = 6.0;

/// A named grade of surface finish.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SurfaceGrade {
    /// A perfect polish: the default.
    Ideal,
    /// A 50k to 100k diamond or alumina lap.
    Mirror,
    /// A 14k diamond lap.
    Fine,
    /// An 8k diamond lap.
    Commercial,
    /// A 3k diamond lap.
    Prepolish,
    /// A 1200-grit ground finish.
    GroundFine,
    /// A 600-grit ground finish.
    GroundCoarse,
    /// Roughness, anisotropy and angle typed by the cutter.
    Custom,
}

impl SurfaceGrade {
    /// Every grade, in the order a menu lists them.
    pub const ALL: [Self; 8] = [
        Self::Ideal,
        Self::Mirror,
        Self::Fine,
        Self::Commercial,
        Self::Prepolish,
        Self::GroundFine,
        Self::GroundCoarse,
        Self::Custom,
    ];

    /// The label shown to the user and written to the design file.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Ideal => "Ideal",
            Self::Mirror => "Mirror",
            Self::Fine => "Fine",
            Self::Commercial => "Commercial",
            Self::Prepolish => "Prepolish",
            Self::GroundFine => "Ground (Fine)",
            Self::GroundCoarse => "Ground (Coarse)",
            Self::Custom => "Custom",
        }
    }

    /// The grade a label names (ignoring case).
    #[must_use]
    pub fn from_label(label: &str) -> Option<Self> {
        Self::ALL
            .into_iter()
            .find(|grade| grade.label().eq_ignore_ascii_case(label.trim()))
    }

    /// The polish preset of a polish grade.
    #[must_use]
    pub const fn polish_preset(self) -> Option<PolishPreset> {
        match self {
            Self::Mirror => Some(PolishPreset::Mirror),
            Self::Fine => Some(PolishPreset::Fine),
            Self::Commercial => Some(PolishPreset::Commercial),
            Self::Prepolish => Some(PolishPreset::Prepolish),
            Self::Ideal | Self::GroundFine | Self::GroundCoarse | Self::Custom => None,
        }
    }

    /// The roughness `α` of a ground grade.
    #[must_use]
    pub const fn ground_alpha(self) -> Option<f64> {
        match self {
            Self::GroundFine => Some(GROUND_FINE_ALPHA),
            Self::GroundCoarse => Some(GROUND_COARSE_ALPHA),
            Self::Ideal
            | Self::Mirror
            | Self::Fine
            | Self::Commercial
            | Self::Prepolish
            | Self::Custom => None,
        }
    }
}

/// One surface finish: a grade and the values it needs. See the module docs.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SurfaceFinish {
    /// The grade.
    pub grade: SurfaceGrade,
    /// The RMS roughness in nanometres of a Custom polish; `0.0` for every other grade.
    pub rq_nm: f64,
    /// The anisotropy ratio (at least 1) of a Custom finish; `1.0` for every other grade.
    pub aniso: f64,
    /// The Custom Angle: the scratch direction in degrees from the girdle-parallel direction,
    /// ignoring the faceting machine. `None` follows the machine.
    pub lap_deg: Option<f64>,
    /// No Sweep: the fan of scratch directions collapses to the machine's nominal angle.
    pub no_sweep: bool,
    /// The GGX roughness `α` of a Custom ground finish, and of a facet of this finish that is
    /// frosted; `None` is the default roughness (a Custom grade with it set is ground, not
    /// polished).
    pub ground_alpha: Option<f64>,
}

impl SurfaceFinish {
    /// The ideal finish: a perfect polish.
    pub const IDEAL: Self = Self {
        grade: SurfaceGrade::Ideal,
        rq_nm: 0.0,
        aniso: 1.0,
        lap_deg: None,
        no_sweep: false,
        ground_alpha: None,
    };

    /// The finish of `grade` with its defaults (a Custom one starts as a 6 nm polish).
    #[must_use]
    pub const fn of_grade(grade: SurfaceGrade) -> Self {
        let rq_nm = if matches!(grade, SurfaceGrade::Custom) {
            DEFAULT_CUSTOM_RQ_NM
        } else {
            0.0
        };
        Self {
            grade,
            rq_nm,
            ..Self::IDEAL
        }
    }

    /// Whether this is exactly the ideal finish (nothing to store).
    #[must_use]
    pub fn is_ideal(&self) -> bool {
        *self == Self::IDEAL
    }

    /// This finish in canonical form: every value in its range and every value the grade does
    /// not use at its default, so a finish that went through a file compares equal to the one
    /// written.
    #[must_use]
    pub fn sanitised(self) -> Self {
        let finite = |value: f64, fallback: f64| if value.is_finite() { value } else { fallback };
        let custom = self.grade == SurfaceGrade::Custom;
        // The scratch direction and the sweep only matter where a lobe has a direction: a
        // polish, or a Custom finish (a ground preset is round).
        let directional = custom || self.grade.polish_preset().is_some();
        Self {
            grade: self.grade,
            rq_nm: if custom {
                finite(self.rq_nm, DEFAULT_CUSTOM_RQ_NM).clamp(0.0, MAX_CUSTOM_RQ_NM)
            } else {
                0.0
            },
            aniso: if custom {
                finite(self.aniso, 1.0).clamp(1.0, MAX_CUSTOM_ANISOTROPY)
            } else {
                1.0
            },
            lap_deg: self
                .lap_deg
                .filter(|angle| directional && angle.is_finite())
                .map(|angle| angle.rem_euclid(180.0)),
            no_sweep: directional && self.no_sweep,
            ground_alpha: self
                .ground_alpha
                .filter(|alpha| alpha.is_finite())
                .map(|alpha| alpha.clamp(0.001, 1.0)),
        }
    }

    /// The packed word of a facet with this finish; `frosted` says the design froze the facet.
    ///
    /// A frosted facet takes the roughness this finish states (the grade's own for a ground
    /// grade, else `ground_alpha`); with none stated it is [`FacetSurface::IDEAL`], because the
    /// frosted finish alone already names the default ground arm. A polished facet takes the
    /// grade's polish (a preset at the default Scratch Strength, or the Custom values), or the
    /// ground word of a ground grade.
    #[must_use]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the packed word quantises f32 values; the design keeps f64 so a TOML value round-trips exactly"
    )]
    pub fn facet_surface(&self, frosted: bool) -> FacetSurface {
        let alpha = self.ground_alpha.or_else(|| self.grade.ground_alpha());
        let aniso = self.aniso as f32;
        let word = if frosted {
            match alpha {
                Some(alpha) => FacetSurface::ground(alpha as f32, aniso),
                None => return FacetSurface::IDEAL,
            }
        } else if let Some(preset) = self.grade.polish_preset() {
            FacetSurface::from_preset(preset, DEFAULT_SCRATCH_STRENGTH)
        } else {
            match self.grade {
                SurfaceGrade::GroundFine | SurfaceGrade::GroundCoarse => {
                    FacetSurface::ground(alpha.unwrap_or(GROUND_FINE_ALPHA) as f32, 1.0)
                }
                SurfaceGrade::Custom => self.ground_alpha.map_or_else(
                    || FacetSurface::polished(self.rq_nm as f32, aniso),
                    |alpha| FacetSurface::ground(alpha as f32, aniso),
                ),
                SurfaceGrade::Ideal
                | SurfaceGrade::Mirror
                | SurfaceGrade::Fine
                | SurfaceGrade::Commercial
                | SurfaceGrade::Prepolish => return FacetSurface::IDEAL,
            }
        };
        if word == FacetSurface::IDEAL {
            return word;
        }
        let word = self
            .lap_deg
            .map_or(word, |angle| word.with_custom_angle(angle as f32));
        if self.no_sweep {
            word.with_no_sweep()
        } else {
            word
        }
    }
}

impl Default for SurfaceFinish {
    fn default() -> Self {
        Self::IDEAL
    }
}

/// The surface finish of one tier: its own, and the facets that differ. See the module docs.
#[derive(Debug, Clone, PartialEq)]
pub struct TierSurface {
    /// The finish of every facet of the tier that has no entry in [`Self::facets`].
    pub finish: SurfaceFinish,
    /// The facets whose finish differs from the tier's, by index-wheel position: ascending,
    /// without repeats ([`Self::normalised`] keeps that true).
    pub facets: Vec<(f64, SurfaceFinish)>,
}

/// Every tier with a surface finish, keyed by the tier's [`TierId`].
pub type TierSurfaceMap = BTreeMap<TierId, TierSurface>;

impl TierSurface {
    /// A whole-tier finish with no facet differing.
    #[must_use]
    pub const fn of_tier(finish: SurfaceFinish) -> Self {
        Self {
            finish,
            facets: Vec::new(),
        }
    }

    /// This surface in canonical form (finishes sanitised, facet positions finite, ascending
    /// and merged); `None` when it states nothing (the ideal tier with no differing facet).
    #[must_use]
    pub fn normalised(self) -> Option<Self> {
        let mut facets: Vec<(f64, SurfaceFinish)> = self
            .facets
            .into_iter()
            .filter(|(position, _)| position.is_finite())
            .map(|(position, finish)| (position, finish.sanitised()))
            .collect();
        facets.sort_by(|a, b| a.0.total_cmp(&b.0));
        // The later entry of two equal positions wins, like a later edit.
        let mut merged: Vec<(f64, SurfaceFinish)> = Vec::with_capacity(facets.len());
        for entry in facets {
            match merged.last_mut() {
                Some(last) if (entry.0 - last.0).abs() < SAME_INDEX_EPSILON => *last = entry,
                _ => merged.push(entry),
            }
        }
        let surface = Self {
            finish: self.finish.sanitised(),
            facets: merged,
        };
        (!surface.is_default()).then_some(surface)
    }

    /// Whether this states nothing: the ideal tier finish and no differing facet.
    #[must_use]
    pub fn is_default(&self) -> bool {
        self.finish.is_ideal() && self.facets.is_empty()
    }

    /// The finish of the facet at index-wheel position `position`.
    #[must_use]
    pub fn finish_at(&self, position: f64) -> &SurfaceFinish {
        self.facets
            .iter()
            .find(|(at, _)| (at - position).abs() < SAME_INDEX_EPSILON)
            .map_or(&self.finish, |(_, finish)| finish)
    }

    /// This surface with the facet at `position` given `finish` (`None` removes its entry, so
    /// it follows the tier again). Normalised; `None` when nothing is left.
    #[must_use]
    pub fn with_facet(mut self, position: f64, finish: Option<SurfaceFinish>) -> Option<Self> {
        self.facets
            .retain(|(at, _)| (at - position).abs() >= SAME_INDEX_EPSILON);
        if let Some(finish) = finish {
            self.facets.push((position, finish));
        }
        self.normalised()
    }

    /// This surface with every facet position mapped through `map` (a gear change moves the
    /// positions); the tier's own finish is unchanged.
    #[must_use]
    pub fn with_positions_mapped(&self, map: impl Fn(f64) -> f64) -> Option<Self> {
        Self {
            finish: self.finish,
            facets: self
                .facets
                .iter()
                .map(|&(position, finish)| (map(position), finish))
                .collect(),
        }
        .normalised()
    }

    /// The label of the tier's own grade, with the number of facets that differ:
    /// `"Commercial"`, `"Commercial, 2 facets differ"`, `"Ideal, 1 facet differs"`.
    #[must_use]
    pub fn text(&self) -> String {
        let grade = self.finish.grade.label();
        match self.facets.len() {
            0 => grade.to_owned(),
            1 => format!("{grade}, 1 facet differs"),
            n => format!("{grade}, {n} facets differ"),
        }
    }
}

/// The bit pattern of a plane, the key a partial arrangement's plane is matched by.
const fn plane_key(plane: &(DVec3, f64)) -> [u64; 4] {
    [
        plane.0.x.to_bits(),
        plane.0.y.to_bits(),
        plane.0.z.to_bits(),
        plane.1.to_bits(),
    ]
}

impl Design {
    /// How the tier CURRENTLY at `index` is finished, `None` when it is ideal throughout.
    #[must_use]
    pub fn tier_surface(&self, index: usize) -> Option<&TierSurface> {
        self.tier_ids
            .get(index)
            .and_then(|id| self.tier_surfaces.get(id))
    }

    /// Whether any tier of the design has a surface finish.
    #[must_use]
    pub fn has_surface(&self) -> bool {
        !self.tier_surfaces.is_empty()
    }

    /// What the tier table says about the surface finish of the tier at `index`: `""` for an
    /// ideal tier, else [`TierSurface::text`].
    #[must_use]
    pub fn tier_surface_text(&self, index: usize) -> String {
        self.tier_surface(index)
            .map_or_else(String::new, TierSurface::text)
    }

    /// [`PartialEq`]'s check for the surface finish: equal by tier position, so two designs
    /// that finish the same tiers the same way compare equal whatever their id numbers.
    pub(super) fn tier_surface_eq(&self, other: &Self) -> bool {
        if self.tier_surfaces.is_empty() && other.tier_surfaces.is_empty() {
            return true;
        }
        self.tiers.len() == other.tiers.len()
            && (0..self.tiers.len())
                .all(|index| self.tier_surface(index) == other.tier_surface(index))
    }

    /// The surface finish of every facet plane of this design, parallel to
    /// [`Self::planes_from_solved`]'s arrangement (the preform's planes come first); a missing
    /// or [`FacetSurface::IDEAL`] entry is a perfect polish.
    ///
    /// Empty when the design has no surface finish, when nothing it states departs from ideal,
    /// and when `solved` does not fit the tiers. A tier's facets are matched to planes through
    /// the schedule's own plane provenance, so a facet whose plane was deduplicated against an
    /// earlier tier's identical plane shares that one retained plane (the earlier tier's
    /// finish wins).
    #[must_use]
    pub fn facet_surfaces_from_solved(&self, solved: &[SolvedTier]) -> Vec<FacetSurface> {
        if self.tier_surfaces.is_empty() {
            return Vec::new();
        }
        let Ok(schedule) = self.try_to_asc_schedule_from_solved(solved) else {
            return Vec::new();
        };
        let provenance = StandardGemCuts::asc_plane_provenance(&schedule);
        let preform_len = self.preform.planes().len();
        let mut surfaces: Vec<FacetSurface> = Vec::new();
        for (index, tier) in self.tiers.iter().enumerate() {
            let (Some(surface), Some(planes)) = (self.tier_surface(index), provenance.get(index))
            else {
                continue;
            };
            let frosting = self.tier_frosting(index);
            for (slot, &plane) in planes.iter().enumerate() {
                let Some(&position) = tier.indices.get(slot) else {
                    continue;
                };
                let frosted = frosting.is_some_and(|frosting| frosting.covers(position));
                let word = surface.finish_at(position).facet_surface(frosted);
                if word == FacetSurface::IDEAL {
                    continue;
                }
                let at = preform_len + plane;
                if surfaces.len() <= at {
                    surfaces.resize(at + 1, FacetSurface::IDEAL);
                }
                if let Some(slot) = surfaces.get_mut(at)
                    && *slot == FacetSurface::IDEAL
                {
                    *slot = word;
                }
            }
        }
        surfaces
    }

    /// The surface finish of each of `planes`, parallel to `planes`; empty when none departs
    /// from ideal.
    ///
    /// `planes` is an arrangement built from the same schedule as [`Self::planes_from_solved`]
    /// (the whole stone, or a prefix or subset of its tiers the Cut slider shows). A plane takes
    /// the finish of the plane of the whole stone it is bit for bit equal to, as
    /// [`Self::frosted_planes_in`] matches frosted planes.
    #[must_use]
    pub fn facet_surfaces_in(
        &self,
        solved: &[SolvedTier],
        planes: &[(DVec3, f64)],
    ) -> Vec<FacetSurface> {
        let whole_surfaces = self.facet_surfaces_from_solved(solved);
        if whole_surfaces.is_empty() {
            return Vec::new();
        }
        let whole = self.planes_from_solved(solved);
        let by_key: BTreeMap<[u64; 4], FacetSurface> = whole_surfaces
            .iter()
            .enumerate()
            .filter(|(_, word)| **word != FacetSurface::IDEAL)
            .filter_map(|(index, word)| whole.get(index).map(|plane| (plane_key(plane), *word)))
            .collect();
        let surfaces: Vec<FacetSurface> = planes
            .iter()
            .map(|plane| {
                by_key
                    .get(&plane_key(plane))
                    .copied()
                    .unwrap_or(FacetSurface::IDEAL)
            })
            .collect();
        if surfaces.iter().all(|word| *word == FacetSurface::IDEAL) {
            Vec::new()
        } else {
            surfaces
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma::optics::raytracer::surface::SurfaceKind;

    #[test]
    fn grade_labels_round_trip_and_unknown_ones_are_refused() {
        for grade in SurfaceGrade::ALL {
            assert_eq!(SurfaceGrade::from_label(grade.label()), Some(grade));
        }
        assert_eq!(
            SurfaceGrade::from_label("  ground (fine) "),
            Some(SurfaceGrade::GroundFine)
        );
        assert_eq!(SurfaceGrade::from_label("Satin"), None);
    }

    #[test]
    fn sanitising_is_idempotent_and_drops_what_a_grade_does_not_use() {
        let messy = SurfaceFinish {
            rq_nm: 40.0,
            aniso: 4.0,
            lap_deg: Some(-30.0),
            no_sweep: true,
            ..SurfaceFinish::of_grade(SurfaceGrade::Commercial)
        }
        .sanitised();
        // A preset takes its Rq and anisotropy from the preset; the scratch direction stays.
        assert_eq!(messy.rq_nm.to_bits(), 0.0f64.to_bits());
        assert_eq!(messy.aniso.to_bits(), 1.0f64.to_bits());
        assert_eq!(messy.lap_deg.map(f64::to_bits), Some(150.0f64.to_bits()));
        assert!(messy.no_sweep);
        assert_eq!(messy.sanitised(), messy);

        // A round ground preset has no scratch direction to keep.
        let ground = SurfaceFinish {
            lap_deg: Some(10.0),
            no_sweep: true,
            ..SurfaceFinish::of_grade(SurfaceGrade::GroundCoarse)
        }
        .sanitised();
        assert_eq!(ground.lap_deg, None);
        assert!(!ground.no_sweep);
        assert!(SurfaceFinish::of_grade(SurfaceGrade::Ideal).is_ideal());
    }

    #[test]
    fn a_finish_becomes_the_matching_packed_word() {
        let ideal = SurfaceFinish::IDEAL;
        assert_eq!(ideal.facet_surface(false), FacetSurface::IDEAL);
        assert_eq!(ideal.facet_surface(true), FacetSurface::IDEAL);

        let commercial = SurfaceFinish::of_grade(SurfaceGrade::Commercial);
        assert_eq!(
            commercial.facet_surface(false),
            FacetSurface::from_preset(PolishPreset::Commercial, DEFAULT_SCRATCH_STRENGTH)
        );
        // Frosted with no roughness stated: the frosted finish alone names the ground arm.
        assert_eq!(commercial.facet_surface(true), FacetSurface::IDEAL);

        let coarse = SurfaceFinish::of_grade(SurfaceGrade::GroundCoarse).facet_surface(false);
        assert_eq!(coarse.kind(), SurfaceKind::Ground);
        assert!((coarse.ground_alpha() - 0.5).abs() < 1e-6);

        let custom_ground = SurfaceFinish {
            ground_alpha: Some(0.25),
            aniso: 2.0,
            ..SurfaceFinish::of_grade(SurfaceGrade::Custom)
        }
        .sanitised()
        .facet_surface(false);
        assert_eq!(custom_ground.kind(), SurfaceKind::Ground);
        assert!((custom_ground.anisotropy() - 2.0).abs() < 0.07);
    }

    #[test]
    fn a_facet_entry_overrides_the_tier_and_a_gear_change_moves_it() {
        let tier = TierSurface::of_tier(SurfaceFinish::of_grade(SurfaceGrade::Fine))
            .with_facet(24.0, Some(SurfaceFinish::of_grade(SurfaceGrade::Prepolish)))
            .expect("states a finish");
        assert_eq!(tier.finish_at(24.0).grade, SurfaceGrade::Prepolish);
        assert_eq!(tier.finish_at(0.0).grade, SurfaceGrade::Fine);
        assert_eq!(tier.text(), "Fine, 1 facet differs");

        let moved = tier
            .with_positions_mapped(|position| position * 2.0)
            .expect("still states a finish");
        assert_eq!(moved.finish_at(48.0).grade, SurfaceGrade::Prepolish);
        assert_eq!(moved.finish_at(24.0).grade, SurfaceGrade::Fine);

        // Removing the last entry of an ideal tier leaves nothing to store.
        let only = TierSurface::of_tier(SurfaceFinish::IDEAL)
            .with_facet(1.0, Some(SurfaceFinish::of_grade(SurfaceGrade::Mirror)))
            .expect("states a finish");
        assert_eq!(only.with_facet(1.0, None), None);
    }
}
