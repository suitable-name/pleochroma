//! [`ConstraintTier`] (one authored facet tier) and [`ScheduleMeta`] (every
//! non-tier `.asc` schedule field) -- the two plain data types
//! [`super::Design`] is built from. See the parent module's doc comment for
//! why a tier's constraint, not a stored mast, is the field this crate treats
//! as authoritative.

use pleochroma::geometry::meet_solver::MeetConstraint;

/// One authored facet tier: geometry that fixes a plane's *direction*, not its
/// depth.
///
/// Carries the same angle/index-wheel-position/name fields
/// [`pleochroma_formats::asc::AscTier`] carries besides `mast` and `notes`, plus the
/// [`MeetConstraint`] that determines where its plane sits. There is
/// deliberately no `mast` field here at all -- see the module docs.
#[derive(Debug, Clone, PartialEq)]
pub struct ConstraintTier {
    /// Signed angle from the girdle plane, in degrees -- the same `GemCAD`
    /// convention [`pleochroma_formats::asc::AscTier::angle_deg`] documents (negative is
    /// pavilion, positive is crown; a zero angle is the table unless it is a
    /// sign-negative `-0.0`, the culet -- file order never decides the side).
    pub angle_deg: f64,
    /// Facet name(s), joined with `/` when a tier folds more than one -- exactly
    /// [`pleochroma_formats::asc::AscTier::name`]'s own convention; see
    /// [`Self::names`].
    pub name: String,
    /// Index-wheel positions this tier's facet occurs at. Empty means a single
    /// facet at azimuth 0.
    pub indices: Vec<f64>,
    /// What determines this tier's mast: meet an unspecified vertex, meet named
    /// facets, or an authored scale dimension. See the module docs.
    pub constraint: MeetConstraint,
    /// The meet that a real `.asc` file's `G` field stated for this tier at import.
    ///
    /// Classified by `meet_tier_inputs_from_asc`: [`MeetConstraint::MeetExisting`] or
    /// [`MeetConstraint::MeetNamed`] only. Import pins `constraint` to a
    /// [`MeetConstraint::ScaleReference`] whatever the file said.
    ///
    /// Display and adoption data only: [`super::Design::solve`] and
    /// [`super::Design::to_asc_schedule`] read `constraint` alone. The editor offers it for
    /// one-click adoption (`Edit::SetConstraint`). `None` when the file gave an explicit
    /// scale instruction (nothing to adopt), and for any tier not built by
    /// [`super::Design::from_asc_schedule`].
    pub imported_meet: Option<MeetConstraint>,
    /// The `G`-field text of [`pleochroma_formats::asc::AscTier::notes`] as the file gave it
    /// at import, verbatim (an empty string when the tier had none).
    ///
    /// Synthesizing `G` text from [`Self::constraint`] would lose phrasings such as
    /// `"Cut to TCP"` or `"GMP"` once import pins the constraint. `super::export` writes this
    /// text back while `constraint` is still the pinned value. Once the cutter adopts another
    /// constraint (`Edit::SetConstraint`), export synthesizes fresh text instead.
    ///
    /// `None` for a tier not built by [`super::Design::from_asc_schedule`], like
    /// [`Self::imported_meet`].
    pub original_notes: Option<String>,
    /// Index-wheel positions in `indices` that are exempted from
    /// [`crate::orbit`]'s orbit-wide propagation -- see that module's docs
    /// for what that means concretely. Always a subset of `indices` (not
    /// enforced by the type; [`super::Design::detach_orbit_member`] and
    /// [`super::Design::reattach_orbit_member`] are the only supported way to grow
    /// or shrink it, both ordinary `History`-mediated edits). Empty for
    /// every tier [`super::Design::from_asc_schedule`] produces and for a
    /// brand-new tier the user adds -- detaching is a deliberate act, never
    /// something import or tier-creation infers.
    pub detached: Vec<f64>,
}

impl ConstraintTier {
    /// Every distinct name this tier is known by -- see
    /// [`pleochroma_formats::asc::AscTier::names`], which this mirrors exactly (same `/`
    /// join convention, same empty-when-unnamed rule).
    #[must_use]
    pub fn names(&self) -> Vec<&str> {
        if self.name.is_empty() {
            Vec::new()
        } else {
            self.name.split('/').collect()
        }
    }

    /// The indices into `tiers` of the tiers this tier's [`MeetConstraint::MeetNamed`] names.
    ///
    /// For a cut sheet or inspector to show. Empty for any other constraint: `MeetExisting`
    /// has no name to resolve, and `ScaleReference` is not a meet.
    ///
    /// A name matches a tier's [`Self::names`] exactly first, then ASCII-case-insensitively,
    /// the same precedence `pleochroma::geometry::meet_solver::MeetNameResolver` uses for a
    /// plain name. An unresolved name is skipped, since this is a display helper and not a
    /// validator. For the full rules (girdle, culet and table synonyms, compound `"1-2-G1"`
    /// specs) see [`super::Design::facet_meets`].
    #[must_use]
    pub fn meet_target_indices(&self, tiers: &[Self]) -> Vec<usize> {
        let MeetConstraint::MeetNamed(names) = &self.constraint else {
            return Vec::new();
        };
        names
            .iter()
            .filter_map(|name| {
                tiers
                    .iter()
                    .position(|t| t.names().contains(&name.as_str()))
                    .or_else(|| {
                        tiers
                            .iter()
                            .position(|t| t.names().iter().any(|n| n.eq_ignore_ascii_case(name)))
                    })
            })
            .collect()
    }

    /// The most tiers one [`Self::step_series`] call builds; a larger `count` is
    /// cut down to this, so no caller can ask for millions of tiers by accident.
    pub const MAX_STEP_SERIES: usize = 64;

    /// Builds a step-cut ladder of up to `count` tiers (at most [`Self::MAX_STEP_SERIES`]).
    ///
    /// Angles step by `angle_step_deg` from `start_angle_deg`, all share `indices`, and tiers
    /// are named `"<name_prefix><n>"` from 1.
    ///
    /// The first tier takes `first_constraint`, typically an anchor or a
    /// [`MeetConstraint::MeetNamed`] into the rest of the design, since a ladder's first rung
    /// has nothing above it. Each later tier meets the one above via
    /// [`MeetConstraint::MeetExisting`].
    ///
    /// Names are unique only among the new tiers, since this sees no design. A caller
    /// appending them to one must rename any clash.
    ///
    /// Returns bare [`ConstraintTier`]s, not [`crate::edit::Edit`]s. An editor wraps them in
    /// `crate::edit::Edit::AddTier` and applies them through `crate::edit::History`, like the
    /// other constructors here (e.g. [`Self::standard_round_brilliant`]).
    #[must_use]
    pub fn step_series(
        name_prefix: &str,
        start_angle_deg: f64,
        angle_step_deg: f64,
        count: usize,
        indices: &[f64],
        first_constraint: &MeetConstraint,
    ) -> Vec<Self> {
        (0..count.min(Self::MAX_STEP_SERIES))
            .map(|n| {
                let angle_deg = angle_step_deg.mul_add(n as f64, start_angle_deg);
                let constraint = if n == 0 {
                    first_constraint.clone()
                } else {
                    MeetConstraint::MeetExisting
                };
                Self {
                    angle_deg,
                    name: format!("{name_prefix}{}", n + 1),
                    indices: indices.to_vec(),
                    constraint,
                    imported_meet: None,
                    original_notes: None,
                    detached: Vec::new(),
                }
            })
            .collect()
    }

    /// A girdle-band facet tier, pinned to `half_width` (the plane's distance from the
    /// vertical axis) via [`MeetConstraint::ScaleReference`].
    ///
    /// The angle is fixed at exactly `90.0` degrees. `classify_blocks` in
    /// `pleochroma::geometry::meet_solver::blocks` assigns
    /// [`pleochroma::geometry::meet_solver::Block::Girdle`] only at 90 degrees, within `1e-6`.
    /// A 0-degree girdle would classify as a crown and produce no girdle band.
    #[must_use]
    pub fn girdle(name: &str, half_width: f64, indices: &[f64]) -> Self {
        Self {
            angle_deg: 90.0,
            name: name.to_string(),
            indices: indices.to_vec(),
            constraint: MeetConstraint::ScaleReference(half_width),
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        }
    }

    /// Mirrors this tier to the opposite block (crown &lt;-&gt; pavilion): same
    /// indices and constraint, angle negated (see [`Self::angle_deg`]'s sign
    /// convention), name suffixed with `name_suffix` (e.g. `"'"` for a paired
    /// "Pavilion Main'" facing "Crown Main") -- the paired-tier half of "Mirror
    /// to other block", a routine emerald/step-cut operation.
    #[must_use]
    pub fn mirrored_to_other_block(&self, name_suffix: &str) -> Self {
        Self {
            angle_deg: -self.angle_deg,
            name: format!("{}{name_suffix}", self.name),
            indices: self.indices.clone(),
            constraint: self.constraint.clone(),
            imported_meet: None,
            original_notes: None,
            detached: self.detached.clone(),
        }
    }

    /// The standard round-brilliant tier table: table, star, crown main,
    /// upper girdle, girdle, pavilion main, lower girdle, culet -- eight
    /// tiers with real angle/index/mast figures for a well-formed RBC, each
    /// pinned via [`MeetConstraint::ScaleReference`] so a design built from
    /// this table [`super::Design::solve`]s immediately without needing any
    /// other anchor. Pair with [`ScheduleMeta::standard_round_brilliant`] for
    /// the schedule metadata (96-tooth gear, 8-fold mirrored symmetry) this
    /// table's `indices` assume; re-deriving `indices` for a different gear
    /// tooth count is `crate::orbit`'s job, not this constructor's.
    ///
    /// Uses the same angles, indices and masts as the `standard_round_brilliant_design` test
    /// fixture of the solid preview, so both describe the same stone.
    #[must_use]
    pub fn standard_round_brilliant() -> Vec<Self> {
        const GIRDLE_INDICES: [f64; 16] = [
            0.0, 6.0, 12.0, 18.0, 24.0, 30.0, 36.0, 42.0, 48.0, 54.0, 60.0, 66.0, 72.0, 78.0, 84.0,
            90.0,
        ];
        const BREAK_INDICES: [f64; 16] = [
            95.0, 1.0, 11.0, 13.0, 23.0, 25.0, 35.0, 37.0, 47.0, 49.0, 59.0, 61.0, 71.0, 73.0,
            83.0, 85.0,
        ];
        const MAIN_INDICES: [f64; 8] = [0.0, 12.0, 24.0, 36.0, 48.0, 60.0, 72.0, 84.0];
        const STAR_INDICES: [f64; 8] = [6.0, 18.0, 30.0, 42.0, 54.0, 66.0, 78.0, 90.0];

        fn tier(name: &str, angle_deg: f64, indices: &[f64], mast: f64) -> ConstraintTier {
            ConstraintTier {
                angle_deg,
                name: name.to_string(),
                indices: indices.to_vec(),
                constraint: MeetConstraint::ScaleReference(mast),
                imported_meet: None,
                original_notes: None,
                detached: Vec::new(),
            }
        }

        vec![
            tier("Table", 0.0, &[], 0.32),
            tier("Star", 15.0, &STAR_INDICES, 0.45),
            tier("Crown Main", 34.5, &MAIN_INDICES, 0.59),
            tier("Upper Girdle", 41.0, &BREAK_INDICES, 0.67),
            tier("Girdle", 90.0, &GIRDLE_INDICES, 1.0),
            tier("Pavilion Main", -41.0, &MAIN_INDICES, 0.67),
            tier("Lower Girdle", -42.5, &BREAK_INDICES, 0.68),
            tier("Culet", -0.0, &[], 0.88),
        ]
    }
}

/// Every non-tier field of a `.asc` schedule ([`pleochroma_formats::asc::AscSchedule`]
/// minus `tiers`): index gear, symmetry, refractive index, and free-text header and
/// footnote lines.
///
/// Split out because a [`super::Design`]'s tiers are [`ConstraintTier`]s (authored
/// constraints), not [`pleochroma_formats::asc::AscTier`]s (recorded masts). See the
/// module docs.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ScheduleMeta {
    /// `GemCad` version string from the schedule header.
    pub gemcad_version: String,
    /// Number of teeth on the index gear.
    pub gear_teeth: i32,
    /// Reference angle of the index gear, in degrees.
    pub gear_reference_angle: f64,
    /// Rotational symmetry order of the schedule.
    pub symmetry_order: u32,
    /// Whether the schedule carries mirror symmetry.
    pub mirror: bool,
    /// The legacy `.asc` `I` line's refractive index.
    ///
    /// [`super::Design::effective_refractive_index`] is what the crate uses for scoring,
    /// export and critical angles: the override, then a recognized material, then this field
    /// as the last fallback.
    ///
    /// Known limitation: `.asc` export writes the effective index to the `I` line, and import
    /// reads that line back into this field. So saving and reopening a design with a
    /// recognized material replaces this value with the material's own index, and the
    /// original cannot be recovered. This rarely matters, since the material or override
    /// already took precedence over this field.
    pub refractive_index: f64,
    /// Free-text header lines of the schedule.
    pub headers: Vec<String>,
    /// Free-text footnote lines of the schedule.
    pub footnotes: Vec<String>,
}

impl ScheduleMeta {
    /// The index wheel's tooth count as an unsigned magnitude -- see
    /// [`pleochroma_formats::asc::AscSchedule::gear_teeth_abs`], which this mirrors exactly.
    #[must_use]
    pub const fn gear_teeth_abs(&self) -> u32 {
        self.gear_teeth.unsigned_abs()
    }

    /// A round-brilliant-appropriate schedule header: 96-tooth index gear,
    /// 8-fold mirrored symmetry, `GemCad`'s legacy diamond refractive index --
    /// the metadata [`ConstraintTier::standard_round_brilliant`]'s tier table
    /// assumes for its `indices`.
    #[must_use]
    pub fn standard_round_brilliant() -> Self {
        Self {
            // Version number only: `to_asc_string` writes the `GemCad ` prefix itself.
            gemcad_version: "5.0".to_string(),
            gear_teeth: 96,
            gear_reference_angle: 0.0,
            symmetry_order: 8,
            mirror: true,
            refractive_index: 1.54,
            headers: Vec::new(),
            footnotes: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{design::Design, preform::PreformSpec};

    fn named_tier(name: &str, constraint: MeetConstraint) -> ConstraintTier {
        ConstraintTier {
            angle_deg: 0.0,
            name: name.to_string(),
            indices: Vec::new(),
            constraint,
            imported_meet: None,
            original_notes: None,
            detached: Vec::new(),
        }
    }

    /// `meet_target_indices` must resolve an exact name, an
    /// ASCII-case-insensitive name, skip an unresolved token, and return
    /// nothing for `MeetExisting`/`ScaleReference`.
    #[test]
    fn meet_target_indices_resolves_exact_and_case_insensitive_names() {
        let tiers = vec![
            named_tier("P1", MeetConstraint::MeetExisting),
            named_tier("P2", MeetConstraint::MeetExisting),
            named_tier(
                "Star",
                MeetConstraint::MeetNamed(vec![
                    "p1".to_string(),
                    "P2".to_string(),
                    "Nonexistent".to_string(),
                ]),
            ),
        ];
        assert_eq!(tiers[2].meet_target_indices(&tiers), vec![0, 1]);
        assert_eq!(tiers[0].meet_target_indices(&tiers), Vec::<usize>::new());
    }

    /// `girdle` builds an exactly 90-degree tier pinned to `half_width`, which
    /// `classify_blocks` reads as `Block::Girdle`. The same tier at 0 degrees reads as
    /// `Block::Crown`.
    #[test]
    fn girdle_builds_a_90_degree_tier_that_classifies_as_girdle() {
        use pleochroma::geometry::meet_solver::{Block, MeetTierInput, classify_blocks};

        let indices = [0.0, 90.0, 180.0, 270.0];
        let girdle = ConstraintTier::girdle("Girdle", 1.0, &indices);
        assert_eq!(girdle.angle_deg, 90.0);
        assert_eq!(girdle.name, "Girdle");
        assert_eq!(girdle.indices, indices);
        assert_eq!(girdle.constraint, MeetConstraint::ScaleReference(1.0));

        let as_input = |tier: &ConstraintTier| MeetTierInput {
            angle_deg: tier.angle_deg,
            indices: tier.indices.clone(),
            constraint: tier.constraint.clone(),
            names: vec![tier.name.clone()],
            pick: None,
        };
        // The same tier authored at 0 degrees instead of 90.
        let zero_degree_mistake = MeetTierInput {
            angle_deg: 0.0,
            ..as_input(&girdle)
        };
        let classified = classify_blocks(&[as_input(&girdle), zero_degree_mistake]);
        assert_eq!(classified[0], Block::Girdle);
        assert_eq!(classified[1], Block::Crown);
    }

    /// `step_series` must produce `count` tiers stepping by `angle_step_deg`,
    /// sharing `indices`, the first pinned to `first_constraint` and every
    /// later one meeting the tier above it.
    #[test]
    fn step_series_builds_a_ladder_meeting_the_tier_above() {
        let indices = [0.0, 90.0];
        let tiers = ConstraintTier::step_series(
            "Step",
            -20.0,
            -5.0,
            3,
            &indices,
            &MeetConstraint::ScaleReference(0.5),
        );
        assert_eq!(tiers.len(), 3);
        assert_eq!(tiers[0].angle_deg, -20.0);
        assert_eq!(tiers[1].angle_deg, -25.0);
        assert_eq!(tiers[2].angle_deg, -30.0);
        assert_eq!(tiers[0].name, "Step1");
        assert_eq!(tiers[2].name, "Step3");
        assert_eq!(tiers[0].constraint, MeetConstraint::ScaleReference(0.5));
        assert_eq!(tiers[1].constraint, MeetConstraint::MeetExisting);
        assert_eq!(tiers[2].constraint, MeetConstraint::MeetExisting);
        for tier in &tiers {
            assert_eq!(tier.indices, indices);
        }
    }

    /// A `count` past [`ConstraintTier::MAX_STEP_SERIES`] is cut down to it
    /// rather than allocating whatever was asked for.
    #[test]
    fn step_series_never_builds_more_than_the_cap() {
        let tiers = ConstraintTier::step_series(
            "S",
            10.0,
            0.5,
            usize::MAX,
            &[0.0],
            &MeetConstraint::MeetExisting,
        );
        assert_eq!(tiers.len(), ConstraintTier::MAX_STEP_SERIES);
        assert_eq!(
            tiers.last().map(|tier| tier.name.as_str()),
            Some("S64"),
            "the ladder is the first 64 rungs, numbered from 1"
        );
    }

    /// `mirrored_to_other_block` must negate the angle and keep everything
    /// else, with the name suffix appended.
    #[test]
    fn mirrored_to_other_block_negates_angle_and_suffixes_name() {
        let tier = ConstraintTier {
            angle_deg: 34.5,
            name: "Main".to_string(),
            indices: vec![0.0, 90.0],
            constraint: MeetConstraint::ScaleReference(0.59),
            imported_meet: None,
            original_notes: None,
            detached: vec![0.0],
        };
        let mirrored = tier.mirrored_to_other_block("'");
        assert_eq!(mirrored.angle_deg, -34.5);
        assert_eq!(mirrored.name, "Main'");
        assert_eq!(mirrored.indices, tier.indices);
        assert_eq!(mirrored.constraint, tier.constraint);
        assert_eq!(mirrored.detached, tier.detached);
    }

    /// The standard round-brilliant template must actually solve and close
    /// as a real solid -- not just produce eight plausible-looking tiers.
    #[test]
    fn standard_round_brilliant_template_solves_and_closes() {
        let design = Design::new(
            PreformSpec::block(2.0, 1.0, 2.0),
            ScheduleMeta::standard_round_brilliant(),
            ConstraintTier::standard_round_brilliant(),
        );
        assert_eq!(design.tiers.len(), 8);
        let solved = design
            .solve()
            .expect("every tier is pinned via ScaleReference");
        assert_eq!(solved.len(), 8);
        assert!(design.is_closed());
    }
}
