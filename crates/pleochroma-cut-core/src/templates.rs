//! Built-in "New Design" templates.
//!
//! A curated gallery of classic faceting designs. Each is a static [`TemplateSpec`] entry in
//! [`TEMPLATES`], built by the constructors of this module.
//!
//! Every template uses the topology of
//! [`crate::design::ConstraintTier::standard_round_brilliant`]: a 96-tooth index gear, symmetric
//! repeats (8-fold, or 2- and 4-fold for the non-round shapes), and every tier pinned by
//! [`pleochroma::geometry::meet_solver::MeetConstraint::ScaleReference`] (an authored exact
//! depth). No tier refers to another tier (`MeetNamed` or `MeetExisting`), so each tier's
//! position depends only on the design's own masts, and the tests of this module can verify
//! every design directly.
//!
//! This module ships nine designs: the five featured shapes (round brilliant, oval, cushion,
//! emerald step cut, princess) and four round-brilliant and teaching variants. The oval,
//! cushion, emerald and princess are tier tables (`Row`) with full-precision masts, worked out
//! so that each is a meet-point design (below) whose every facet is alive and large enough to
//! cut, at the intended length-to-width ratio, on its default preform and after a remap to each
//! gear [`TemplateSpec::allowed_gears_among`] offers. The `#[cfg(test)]` module checks each entry.
//!
//! # Tier order
//!
//! Every tier list is stored in the order it is cut ([`crate::design::Design::cutting_order`]),
//! so the tier table, the Cut slider and the printed cutting sheet all start with the same tier:
//! the pavilion section (the girdle tiers belong to it), then the crown, and the table last.
//! Only [`crate::design::ConstraintTier::standard_round_brilliant`], the fixture about 160 tests
//! build on, stays top-down.
//!
//! - The round brilliants and the teaching designs cut the pavilion mains first, then the girdle;
//!   the round brilliants then cut the lower girdle and the culet, and their crown upper girdle
//!   first, then the crown mains and the stars (see `round_brilliant_crown`).
//! - The oval, cushion, emerald and princess cut their girdle outline first (the first girdle
//!   tier sets the stone size, the others are cut to depth before any facet leaves a point).
//!   The oval, the cushion and the princess have no culet tier: their pavilion mains meet in a
//!   point, or, on the oval, the side mains in a short keel. The oval, the princess and the cushion
//!   cut their breaks before the mains they meet, the emerald leads each half with its end steps.
//!
//! # Meet points
//!
//! Every template is a meet-point design: cut in order on its own rough, every tier either fixes
//! a dimension or reaches a point already on the stone ([`crate::design::Design::cut_meets`]),
//! and the girdle is level at its corners ([`crate::design::Design::girdle_level`]). The masts
//! that meet are full-precision values worked out that way, each commented with what it meets; a
//! rounded mast floats off its point.
//!
//! - The round brilliants' star is at 27 degrees (the fixture's is 15) so that the meet-point
//!   crown has a 55.5 % table; their culet is a pin-prick that meets the mains' point.
//! - The teaching designs are anchored at every tier: the pavilion mains are cut to the
//!   centerpoint and the girdle sets the stone size; the Simple design's table, its only crown
//!   tier, sets the girdle width, and the Rich design's table, above crown mains that meet only
//!   in one point, sets the table size.
//! - The oval's pavilion tiers each meet a point on the girdle that a retarget turns them about:
//!   a girdle corner, or the V a pair of lower girdles leaves in the middle of a girdle face; the
//!   side mains meet in a short keel. Its crown's diagonal mains meet in the crown's point and
//!   the side and end mains meet it at the angles that put each level on its own girdle face.
//! - The cushion's second breaks and corner mains run level along the rounding and corner faces,
//!   so its girdle line is level whatever the angles.
//! - The emerald's table meets the tops of its third corner steps, so it is a rectangle.
//!
//! Every template can be created in another material: the retarget that adapts its angles keeps
//! every meet, since each point a tier meets is formed by planes that keep clear angles between
//! them (`templates::tests` in the editor checks Quartz, Sapphire, Diamond and Emerald).
//!
//! A remap to another gear keeps a meet only where every index lands on a whole tooth: the
//! emerald (indices in steps of 12) keeps them all on every gear it offers; the oval, the cushion
//! and the princess keep their stones intact on gears 80 and 64, but some of their breaks, mains
//! and stars float there, by at most about 0.1 mm at 10 mm, inside the default reach of Snap to
//! Meets. Their girdle line is then not level.
//!
//! # Metadata
//!
//! Besides the tier table, a [`TemplateSpec`] names its outline family
//! ([`ShapeFamily`]), whether it is one of the featured shapes, the refractive
//! index its angles were designed for ([`TemplateSpec::design_ri`]) together with the
//! built-in material that index belongs to ([`TemplateSpec::default_material`]),
//! its default preform, and -- derived, not listed -- the gears it can be remapped
//! onto ([`TemplateSpec::allows_gear`]).
//!
//! The templates are only ever APPENDED to [`TEMPLATES`]: the position in that table is
//! the template's identity for the web app, the worked-example guide and the saved
//! test fixtures (`EditorSession::from_template`'s index is `position + 1`).

use crate::{
    design::{
        ConcaveTier, ConcaveTool, ConstraintTier, Design, FreshDesignSpec, ScheduleMeta, ToolMotion,
    },
    edit::remap_ratio,
    material::MaterialSelection,
    preform::PreformSpec,
};
use pleochroma::geometry::meet_solver::MeetConstraint;

/// The outline family a template belongs to -- what the gallery and the shape
/// verdicts group by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShapeFamily {
    /// Round brilliant and its variants and teaching designs.
    Round,
    /// An elongated brilliant.
    Oval,
    /// A rounded-square brilliant.
    Cushion,
    /// A rectangular step cut with cut corners.
    Emerald,
    /// A square modified brilliant.
    Princess,
}

impl ShapeFamily {
    /// The family's display name.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Round => "Round",
            Self::Oval => "Oval",
            Self::Cushion => "Cushion",
            Self::Emerald => "Emerald",
            Self::Princess => "Princess",
        }
    }
}

/// One authored tier of a table-driven template: `(name, angle_deg, indices, mast)`.
type Row = (&'static str, f64, &'static [f64], f64);

/// One gallery entry: display metadata plus a pure constructor for its tier table.
///
/// `build` is a plain function pointer, not a closure, so that [`TEMPLATES`] can be a `const`
/// table of static data.
#[derive(Debug, Clone)]
pub struct TemplateSpec {
    /// Shown as the gallery card's title and the "Start From" selection text.
    pub name: &'static str,
    /// A short shape/fold description, e.g. "8-fold round brilliant" -- shown
    /// under the name on the gallery card, distinct from `description` (a
    /// full sentence) so a narrow card can show just this line.
    pub shape: &'static str,
    /// The index gear tooth count this template's `indices` were authored for.
    pub gear_teeth: i32,
    /// The symmetry order (repeat count) this template's `indices` were
    /// authored for.
    pub symmetry_order: u32,
    /// Whether this template's repeats are also mirrored.
    pub mirror: bool,
    /// One sentence describing the design, shown on hover/selection.
    pub description: &'static str,
    /// The outline family -- see [`ShapeFamily`].
    pub family: ShapeFamily,
    /// Whether this is one of the "top five shapes" the New Design dialog lists first;
    /// the others (round-brilliant variants and teaching designs) form its second group.
    pub featured: bool,
    /// The refractive index (`n_D`) the facet angles were designed for. Creating the
    /// design in a material whose index differs adapts the angles
    /// (`pleochroma_editor::templates::create_from_template`).
    pub design_ri: f64,
    /// The built-in material whose `n_D` is [`Self::design_ri`] -- the material a new
    /// design from this template starts in.
    pub default_material: &'static str,
    /// The preform the dialog cuts this template from. Tall enough that the finished
    /// stone is never clipped by the rough's own bottom or walls.
    pub preform: PreformSpec,
    /// Builds this template's tier table on each call (not cached; the tables are small).
    build: fn() -> Vec<ConstraintTier>,
}

/// How close two gear positions must be to a half tooth for the remap to count as a
/// tie (a tie rounds one way for an index and the other for its mirror image).
const TIE_EPSILON: f64 = 1e-9;

impl TemplateSpec {
    /// The fresh-design spec for this template at its own gear, symmetry and mirror, on
    /// its default preform, started in `material`.
    #[must_use]
    pub fn fresh_spec(&self, material: MaterialSelection) -> FreshDesignSpec {
        FreshDesignSpec {
            gear_teeth: self.gear_teeth,
            symmetry_order: self.symmetry_order,
            mirror: self.mirror,
            material,
            preform: self.preform.clone(),
        }
    }

    /// The material selection a new design from this template starts with: the
    /// [`Self::default_material`] by name, nothing overridden.
    #[must_use]
    pub fn default_material_selection(&self) -> MaterialSelection {
        MaterialSelection {
            name: Some(self.default_material.to_string()),
            specific_gravity_override: None,
            refractive_index_override: None,
            body_color_override: None,
            body_color_bands_override: None,
            absorption_path_scale_override: None,
            color_pattern_override: None,
            color_variant_override: None,
        }
    }

    /// Whether this template's index set can be remapped onto `gear` teeth by symmetry:
    ///
    /// - `gear` is the template's own gear (always allowed), or
    /// - `gear` is a multiple of the symmetry order (so the repeats still land on whole
    ///   teeth), no scaled position falls exactly half way between two teeth (such a tie
    ///   rounds an index and its mirror image differently and breaks the mirror), and no
    ///   two positions of one tier land on the same tooth.
    ///
    /// The rule uses the same arithmetic as `Edit::RemapIndices` with nearest rounding.
    /// Whether the remapped stone still has every facet is checked by this module's
    /// tests for each template and each gear the dialog offers, not decided here.
    #[must_use]
    pub fn allows_gear(&self, gear: i32) -> bool {
        if gear == self.gear_teeth {
            return true;
        }
        let Ok(teeth) = u32::try_from(gear) else {
            return false;
        };
        if teeth == 0 || self.symmetry_order == 0 || !teeth.is_multiple_of(self.symmetry_order) {
            return false;
        }
        let ratio = remap_ratio(self.gear_teeth, gear);
        let wheel = f64::from(teeth);
        self.tiers().iter().all(|tier| {
            let mut landed: Vec<u64> = Vec::with_capacity(tier.indices.len());
            for &index in &tier.indices {
                let scaled = index * ratio;
                if (scaled - scaled.floor() - 0.5).abs() < TIE_EPSILON {
                    return false;
                }
                // `+ 0.0` turns a negative zero into a plain one so equal teeth compare equal.
                let tooth = (scaled.round().rem_euclid(wheel) + 0.0).to_bits();
                if landed.contains(&tooth) {
                    return false;
                }
                landed.push(tooth);
            }
            true
        })
    }

    /// The gears from `candidates` this template can be remapped onto
    /// ([`Self::allows_gear`]), in the order given, always starting with the template's
    /// own gear.
    #[must_use]
    pub fn allowed_gears_among(&self, candidates: &[i32]) -> Vec<i32> {
        let mut gears = vec![self.gear_teeth];
        for &gear in candidates {
            if !gears.contains(&gear) && self.allows_gear(gear) {
                gears.push(gear);
            }
        }
        gears
    }

    /// Builds this template's tier table.
    #[must_use]
    pub fn tiers(&self) -> Vec<ConstraintTier> {
        (self.build)()
    }

    /// The schedule metadata (`gear_teeth`/`symmetry_order`/`mirror`) this
    /// template's own `indices` were authored to match, paired with
    /// [`Self::tiers`] the same way [`ScheduleMeta::standard_round_brilliant`]
    /// pairs with [`ConstraintTier::standard_round_brilliant`].
    #[must_use]
    pub fn schedule_meta(&self) -> ScheduleMeta {
        ScheduleMeta {
            // The version number only: `pleochroma_formats::asc::to_asc_string` writes the
            // `GemCad ` prefix.
            gemcad_version: "5.0".to_string(),
            gear_teeth: self.gear_teeth,
            gear_reference_angle: 0.0,
            symmetry_order: self.symmetry_order,
            mirror: self.mirror,
            refractive_index: 1.54,
            headers: Vec::new(),
            footnotes: Vec::new(),
        }
    }
}

/// Index-wheel positions of the 96-tooth, 8-fold layout of
/// [`ConstraintTier::standard_round_brilliant`], copied because that function keeps its own
/// copies private.
mod indices {
    /// The 16 girdle-band facet positions on a 96-tooth wheel.
    pub const GIRDLE: [f64; 16] = [
        0.0, 6.0, 12.0, 18.0, 24.0, 30.0, 36.0, 42.0, 48.0, 54.0, 60.0, 66.0, 72.0, 78.0, 84.0,
        90.0,
    ];
    /// The 16 upper-/lower-girdle ("break") facet positions on a 96-tooth wheel.
    pub const BREAK: [f64; 16] = [
        95.0, 1.0, 11.0, 13.0, 23.0, 25.0, 35.0, 37.0, 47.0, 49.0, 59.0, 61.0, 71.0, 73.0, 83.0,
        85.0,
    ];
    /// The 8 crown-/pavilion-main facet positions on a 96-tooth wheel.
    pub const MAIN: [f64; 8] = [0.0, 12.0, 24.0, 36.0, 48.0, 60.0, 72.0, 84.0];
    /// The 8 star facet positions on a 96-tooth wheel.
    pub const STAR: [f64; 8] = [6.0, 18.0, 30.0, 42.0, 54.0, 66.0, 78.0, 90.0];
}

/// Builds one `ScaleReference`-pinned tier. A copy of the private helper of
/// [`ConstraintTier::standard_round_brilliant`].
fn scale_tier(name: &str, angle_deg: f64, indices: &[f64], mast: f64) -> ConstraintTier {
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

/// The meet-point crown every round brilliant template shares, in the order it is cut, above the
/// girdle.
///
/// - The upper girdle facets come first and set the girdle width. Each pair either side of a
///   main meets on the main's girdle face, in the V a brilliant's girdle has, and each facet runs
///   through a corner where two girdle faces meet, so the top of the girdle is level at its
///   corners. Their mast puts those corners 0.055 above the standard's pavilion line: the girdle
///   is 2.75 % of the width thick at its corners.
/// - The crown mains meet the girdle and the upper girdles at the bottom of those V's: the kite's
///   lower point.
/// - The stars meet the crown mains and the upper girdles where two kites and two upper girdles
///   meet: the star's point.
/// - The table meets the crown mains and the stars at the top of each kite.
///
/// The crown mains cannot come first: an upper girdle facet 41 degrees steep, one tooth from a
/// main at 34.5 degrees, that reaches a crown main's corner on the girdle cuts nothing, and a star
/// cut before the upper girdles has no point to meet. In this order the star points stand 96 %
/// of the way out to the girdle whatever the masts, so the star angle alone sets the table: at
/// 27 degrees the table is 55.5 % of the width (a 15-degree star would leave 80 %).
fn round_brilliant_crown() -> [ConstraintTier; 4] {
    [
        // Set girdle width.
        scale_tier(
            "Upper Girdle",
            41.0,
            &indices::BREAK,
            0.690_757_432_895_065_9,
        ),
        // Meets Girdle and Upper Girdle at the kite's lower point.
        scale_tier("Crown Main", 34.5, &indices::MAIN, 0.605_830_002_977_678_6),
        // Meets Upper Girdle and Crown Main at the star's point.
        scale_tier("Star", 27.0, &indices::STAR, 0.548_005_339_109_188_4),
        // Meets Crown Main and Star at the top of each kite.
        scale_tier("Table", 0.0, &[], 0.354_003_743_907_870_76),
    ]
}

/// How far the culet's plane sits above the point where the pavilion mains meet: half the meet
/// tolerance (`crate::MEET_TOLERANCE_FRACTION`) of a stone 2.0 across. The culet then meets that
/// point and stays a facet of its own, a pin-prick.
const CULET_ABOVE_THE_MAINS_POINT: f64 = 5e-5;

/// The standard round brilliant, a meet-point design stored in the order it is cut
/// ([`crate::design::Design::cutting_order`]).
///
/// - The pavilion mains (mast `0.67`) are cut first, then the girdle sets the stone size.
/// - The lower girdle facets meet the pavilion mains and the girdle at the corners where two
///   girdle faces meet, so the bottom of the girdle is level; each pair meets in a V on the
///   girdle face between two mains.
/// - The culet meets the point where the mains meet (see [`CULET_ABOVE_THE_MAINS_POINT`]).
/// - Then the crown, [`round_brilliant_crown`].
///
/// The tiers, indices and angles are those of [`ConstraintTier::standard_round_brilliant`],
/// except the star (27 degrees instead of 15, see [`round_brilliant_crown`]). The masts and the
/// cut order are this template's own; the fixture keeps its round masts and top-down order
/// because about 160 tests build on it.
fn round_brilliant_tiers() -> Vec<ConstraintTier> {
    let mut tiers = vec![
        // Nothing to meet yet: the first tier.
        scale_tier("Pavilion Main", -41.0, &indices::MAIN, 0.67),
        // Set stone size.
        scale_tier("Girdle", 90.0, &indices::GIRDLE, 1.0),
        // Meets Pavilion Main and Girdle at the girdle-face corners.
        scale_tier(
            "Lower Girdle",
            -42.5,
            &indices::BREAK,
            0.696_551_802_061_467_2,
        ),
        // Meets Pavilion Main where the mains meet.
        scale_tier(
            "Culet",
            -0.0,
            &[],
            0.887_758_702_743_958_7 - CULET_ABOVE_THE_MAINS_POINT,
        ),
    ];
    tiers.extend(round_brilliant_crown());
    tiers
}

/// A shallower pavilion (mains at -38 instead of -41, lower girdle following at
/// -40) -- the same meet-point design as the standard template, but with less
/// pavilion angle margin over a typical critical angle, useful for
/// demonstrating windowing risk in the guide/inspector's margin readout. The
/// pavilion mains are cut to the standard's girdle line, so the girdle and the
/// crown are the standard's.
fn shallow_pavilion_tiers() -> Vec<ConstraintTier> {
    let mut tiers = vec![
        // Nothing to meet yet: the first tier, cut to the standard template's girdle line.
        scale_tier(
            "Pavilion Main",
            -38.0,
            &indices::MAIN,
            0.630_217_613_472_574_4,
        ),
        // Set stone size.
        scale_tier("Girdle", 90.0, &indices::GIRDLE, 1.0),
        // Meets Pavilion Main and Girdle at the girdle-face corners.
        scale_tier(
            "Lower Girdle",
            -40.0,
            &indices::BREAK,
            0.663_924_057_952_181_6,
        ),
        // Meets Pavilion Main where the mains meet.
        scale_tier(
            "Culet",
            -0.0,
            &[],
            0.799_757_606_050_000_3 - CULET_ABOVE_THE_MAINS_POINT,
        ),
    ];
    tiers.extend(round_brilliant_crown());
    tiers
}

/// A deeper pavilion (mains at -43.5, lower girdle at -44.5) -- closer to a
/// typical critical angle than the standard template, useful for demonstrating
/// a "Marginal"/"Windows" risk band in the guide/inspector. The same meet-point
/// design as the standard template; the pavilion mains are cut to the standard's
/// girdle line, so the girdle and the crown are the standard's.
fn deep_pavilion_tiers() -> Vec<ConstraintTier> {
    let mut tiers = vec![
        // Nothing to meet yet: the first tier, cut to the standard template's girdle line.
        scale_tier(
            "Pavilion Main",
            -43.5,
            &indices::MAIN,
            0.701_753_705_723_322,
        ),
        // Set stone size.
        scale_tier("Girdle", 90.0, &indices::GIRDLE, 1.0),
        // Meets Pavilion Main and Girdle at the girdle-face corners.
        scale_tier(
            "Lower Girdle",
            -44.5,
            &indices::BREAK,
            0.721_702_178_718_296_9,
        ),
        // Meets Pavilion Main where the mains meet.
        scale_tier(
            "Culet",
            -0.0,
            &[],
            0.967_436_506_364_174_6 - CULET_ABOVE_THE_MAINS_POINT,
        ),
    ];
    tiers.extend(round_brilliant_crown());
    tiers
}

/// The simplest possible closed design on this layout: a table, a girdle band,
/// and one ring of pavilion mains -- no crown facets, no star/break rings. Three
/// tiers total, meant as a first design smaller than the standard round
/// brilliant's eight, for a cutter learning the tier table. Stored in cutting
/// order: pavilion main, girdle, table.
fn simple_teaching_tiers() -> Vec<ConstraintTier> {
    vec![
        scale_tier("Pavilion Main", -40.0, &indices::MAIN, 0.70),
        scale_tier("Girdle", 90.0, &indices::GIRDLE, 1.0),
        scale_tier("Table", 0.0, &[], 0.30),
    ]
}

/// One step up from [`simple_teaching_tiers`]: adds a single crown-main ring
/// above the girdle, so both blocks have one real facet ring each -- pavilion
/// main, girdle, crown main, table (the order they are cut in). Four tiers, still
/// no star/break rings.
fn rich_teaching_tiers() -> Vec<ConstraintTier> {
    vec![
        scale_tier("Pavilion Main", -40.0, &indices::MAIN, 0.70),
        scale_tier("Girdle", 90.0, &indices::GIRDLE, 1.0),
        scale_tier("Crown Main", 34.5, &indices::MAIN, 0.60),
        scale_tier("Table", 0.0, &[], 0.30),
    ]
}

/// Turns a table of [`Row`]s into pinned tiers.
fn tiers_from_rows(rows: &[Row]) -> Vec<ConstraintTier> {
    rows.iter()
        .map(|&(name, angle_deg, indices, mast)| scale_tier(name, angle_deg, indices, mast))
        .collect()
}

mod rows;
use rows::{CUSHION_ROWS, EMERALD_ROWS, OVAL_ROWS, PRINCESS_ROWS};

fn oval_tiers() -> Vec<ConstraintTier> {
    tiers_from_rows(OVAL_ROWS)
}

fn cushion_tiers() -> Vec<ConstraintTier> {
    tiers_from_rows(CUSHION_ROWS)
}

fn emerald_tiers() -> Vec<ConstraintTier> {
    tiers_from_rows(EMERALD_ROWS)
}

fn princess_tiers() -> Vec<ConstraintTier> {
    tiers_from_rows(PRINCESS_ROWS)
}

/// `n_D` of the built-in Diamond (2.41726) rounded to the figure templates are
/// designed for; every figure below is checked against the catalogue by a test.
const RI_DIAMOND: f64 = 2.417;
/// Sapphire (corundum), `n_D` 1.76808.
const RI_SAPPHIRE: f64 = 1.768;
/// Quartz, `n_D` 1.54421 -- also the 1.54 that the teaching designs assume.
const RI_QUARTZ: f64 = 1.544;
/// Emerald (beryl), `n_D` 1.5791.
const RI_BERYL: f64 = 1.579;

/// The preform every template is cut from: a 96-sided cylinder of half-width 1.5, `l_over_w`
/// long, and tall enough (2.2) that no finished stone touches the rough's own bottom. The
/// Standard Round Brilliant is cut from a taller one, [`tall_rough`].
const fn rough(l_over_w: f64) -> PreformSpec {
    PreformSpec::cylinder(96, 1.5, l_over_w, 2.2)
}

/// The Standard Round Brilliant's rough: the [`rough`] cylinder, 3.4 tall instead of 2.2.
///
/// The template is designed for diamond. In another material the pavilion angles move with the
/// critical angle (+10.0 degrees for sapphire, +14.9 for emerald, +15.9 for quartz) and the mains
/// turn about the girdle line, so their point drops from 0.89 below the middle of the stone to
/// 1.25, 1.49 and 1.55. A 2.2-tall rough ends 1.1 down and would cut that point away, so the
/// retarget refuses the change. At 3.4 the quartz stone clears the bottom by 0.15, and the stone
/// fits every built-in material down to Glass (N-BK7, n 1.517); opal (n 1.45) would need 3.6 or
/// more.
const fn tall_rough(l_over_w: f64) -> PreformSpec {
    PreformSpec::cylinder(96, 1.5, l_over_w, 3.4)
}

/// The built-in template gallery.
///
/// **Only append.** The position in this table is the template's identity for the web
/// app, the worked-example guide and saved fixtures: `EditorSession::from_template`'s
/// index `n >= 1` is `TEMPLATES[n - 1]`, with `0` reserved for "Empty". The display
/// order of the New Design dialog (featured shapes first) is built from
/// [`TemplateSpec::featured`] and does not depend on this order.
pub const TEMPLATES: &[TemplateSpec] = &[
    TemplateSpec {
        name: "Standard Round Brilliant",
        shape: "8-fold round brilliant",
        gear_teeth: 96,
        symmetry_order: 8,
        mirror: true,
        description: "The classic eight-main round brilliant: table, star, crown main, upper girdle, girdle, pavilion main, lower girdle, culet.",
        family: ShapeFamily::Round,
        featured: true,
        design_ri: RI_DIAMOND,
        default_material: "Diamond",
        preform: tall_rough(1.0),
        build: round_brilliant_tiers,
    },
    TemplateSpec {
        name: "Round Brilliant -- Shallow Pavilion",
        shape: "8-fold round brilliant",
        gear_teeth: 96,
        symmetry_order: 8,
        mirror: true,
        description: "The standard round brilliant with a shallower pavilion main -- more windowing risk, a teaching example for the critical-angle margin.",
        family: ShapeFamily::Round,
        featured: false,
        design_ri: RI_QUARTZ,
        default_material: "Quartz",
        preform: rough(1.0),
        build: shallow_pavilion_tiers,
    },
    TemplateSpec {
        name: "Round Brilliant -- Deep Pavilion",
        shape: "8-fold round brilliant",
        gear_teeth: 96,
        symmetry_order: 8,
        mirror: true,
        description: "The standard round brilliant with a deeper pavilion main, closer to a typical critical angle -- a teaching example for the Marginal/Windows risk bands.",
        family: ShapeFamily::Round,
        featured: false,
        design_ri: RI_QUARTZ,
        default_material: "Quartz",
        preform: rough(1.0),
        build: deep_pavilion_tiers,
    },
    TemplateSpec {
        name: "Simple Teaching Design",
        shape: "8-fold, 3 tiers",
        gear_teeth: 96,
        symmetry_order: 8,
        mirror: true,
        description: "A table, a girdle band and one ring of pavilion facets -- the smallest design that solves and closes, for a first look at the tier table.",
        family: ShapeFamily::Round,
        featured: false,
        design_ri: RI_QUARTZ,
        default_material: "Quartz",
        preform: rough(1.0),
        build: simple_teaching_tiers,
    },
    TemplateSpec {
        name: "Rich Teaching Design",
        shape: "8-fold, 4 tiers",
        gear_teeth: 96,
        symmetry_order: 8,
        mirror: true,
        description: "A table, a girdle band, one crown ring and one pavilion ring -- one step past the Simple Teaching Design, with a real crown to edit.",
        family: ShapeFamily::Round,
        featured: false,
        design_ri: RI_QUARTZ,
        default_material: "Quartz",
        preform: rough(1.0),
        build: rich_teaching_tiers,
    },
    TemplateSpec {
        name: "Oval Brilliant",
        shape: "2-fold oval, L/W 1.35",
        gear_teeth: 96,
        symmetry_order: 2,
        mirror: true,
        description: "An elongated brilliant with a length-to-width ratio of 1.35, cut for corundum-like stones (sapphire, ruby): the side mains meet in a short keel, pavilion mains 40.5 to 41.5 degrees, crown 32 to 40.",
        family: ShapeFamily::Oval,
        featured: true,
        design_ri: RI_SAPPHIRE,
        default_material: "Sapphire",
        preform: rough(1.35),
        build: oval_tiers,
    },
    TemplateSpec {
        name: "Cushion Brilliant",
        shape: "4-fold cushion",
        gear_teeth: 96,
        symmetry_order: 4,
        mirror: true,
        description: "A rounded-square brilliant with a deeper pavilion, cut for quartz-like stones (quartz, citrine, amethyst): crown 38, pavilion 42 degrees.",
        family: ShapeFamily::Cushion,
        featured: true,
        design_ri: RI_QUARTZ,
        default_material: "Quartz",
        preform: rough(1.0),
        build: cushion_tiers,
    },
    TemplateSpec {
        name: "Emerald Step Cut",
        shape: "2-fold step cut, L/W 1.4",
        gear_teeth: 96,
        symmetry_order: 2,
        mirror: true,
        description: "A rectangular step cut with cut corners and a length-to-width ratio of 1.4: three crown steps and three pavilion steps, cut for beryl (emerald, aquamarine).",
        family: ShapeFamily::Emerald,
        featured: true,
        design_ri: RI_BERYL,
        default_material: "Emerald",
        preform: rough(1.4),
        build: emerald_tiers,
    },
    TemplateSpec {
        name: "Princess Cut",
        shape: "4-fold square brilliant",
        gear_teeth: 96,
        symmetry_order: 4,
        mirror: true,
        description: "A square modified brilliant with clipped corners, cut for corundum-like stones: crown 35, pavilion 41 degrees, mains on the sides and the corners.",
        family: ShapeFamily::Princess,
        featured: true,
        design_ri: RI_SAPPHIRE,
        default_material: "Sapphire",
        preform: rough(1.0),
        build: princess_tiers,
    },
];

impl Design {
    /// The concave-facet fixture that tests pin their numbers against.
    ///
    /// A five-tier 16-tooth round brilliant on a 2 x 1 x 2 block, plus two concave tiers:
    ///
    /// - `Groove`: `CYL`, φ = −42, the eight even indices, θ = 0,
    ///   X = 0, Y = 0.15, Z = 0.03, D = 0.25, reciprocating;
    /// - `Dimple`: `SPH`, φ = +36, indices 0/4/8/12, X = Y = 0, Z = 0.02,
    ///   D = 0.1, plunge (a crown-side dimple).
    ///
    /// **Frozen**: tests in other crates pin geometry, hashes and text against these exact
    /// numbers, so change them only by adding a second fixture. Not a template for users. The
    /// concave tiers use the provisional `v0` frame conventions.
    ///
    /// # Panics
    ///
    /// Never in practice: the embedded `.asc` text is a constant that parses.
    #[must_use]
    pub fn concave_fixture() -> Self {
        const ASC: &str = "GemCad 5.0\n\
             g 16 0.0\n\
             y 4 n\n\
             I 1.54\n\
             a 90.000000 1.00000000 0 4 8 12 G Set girdle thickness\n\
             a -42.000000 0.60000000 0 4 8 12 G Set stone size\n\
             a -38.000000 0.55000000 2 6 10 14 G Set stone size\n\
             a 32.000000 0.45000000 0 4 8 12 G Set stone size\n\
             a 40.000000 0.40000000 2 6 10 14 G Set stone size\n";
        let schedule =
            pleochroma_formats::asc::parse_asc(ASC).expect("the fixture's .asc text must parse");
        let mut design = Self::from_asc_schedule(PreformSpec::block(2.0, 1.0, 2.0), &schedule);
        design.girdle_diameter_mm = Some(6.5);
        design.material = MaterialSelection {
            name: Some("Diamond".to_string()),
            specific_gravity_override: None,
            refractive_index_override: Some(1.54),
            body_color_override: None,
            body_color_bands_override: None,
            absorption_path_scale_override: None,
            color_pattern_override: None,
            color_variant_override: None,
        };
        design.concave_tiers = vec![
            ConcaveTier {
                name: "Groove".to_owned(),
                angle_deg: -42.0,
                indices: (0..8).map(|i| f64::from(2 * i)).collect(),
                instructions: String::new(),
                tool: ConcaveTool::Cylinder,
                tool_azimuth_deg: 0.0,
                displacement: [0.0, 0.15, 0.03],
                diameter_ratio: 0.25,
                tool_angle_deg: None,
                motion: ToolMotion::Reciprocating,
            },
            ConcaveTier {
                name: "Dimple".to_owned(),
                angle_deg: 36.0,
                indices: vec![0.0, 4.0, 8.0, 12.0],
                instructions: String::new(),
                tool: ConcaveTool::Sphere,
                tool_azimuth_deg: 0.0,
                displacement: [0.0, 0.0, 0.02],
                diameter_ratio: 0.1,
                tool_angle_deg: None,
                motion: ToolMotion::Plunge,
            },
        ];
        design.ensure_concave_tier_ids();
        design
    }
}

#[cfg(test)]
mod tests;
