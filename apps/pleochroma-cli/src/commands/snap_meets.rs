//! `snap-meets`: moves the depth of every tier that reaches no meet point onto the nearest
//! point already on the stone, so a cutter has a point to cut each tier to.
//!
//! The cutting sheet's meet column names what a tier's facets reach at the moment the tier is
//! cut, on the stone cut so far (`Design::cut_meets`). A tier set to a free depth (by the
//! optimizer, or by hand) often reaches no such point: it floats, and the sheet gives only its
//! depth (`Cut to depth 2.81 mm.`). This command walks the cutting order and gives each floating
//! explicit-depth tier the mast that puts its plane through the nearest vertex of the stone cut
//! so far, when that is within the limit (`Design::snap_depths_to_meets`). The anchors never
//! move. The design is then solved again and its meets worked out once more.
//!
//! # The limit
//!
//! By default a tier moves at most `DEFAULT_SNAP_MAX_MOVE_FRACTION` of the finished stone's size
//! (its largest extent); `--max-move-fraction F` sets another share. `--max-move MM` gives it in
//! millimeters, which only a design with a girdle diameter can turn into its own units: on any
//! other design it is refused (exit code 1) with a pointer to `--max-move-fraction`.
//!
//! # Output
//!
//! A header (the design, the limit, how many tiers float before and after), then one line per
//! moved tier, the facets the new depths cut away and the tiers that still float:
//!
//! ```text
//! P2  0.7662 -> 0.7598  (moved 0.03 mm, now meets P1, G1)
//! Cut away by the snap: P3 (all 10 facets) and C3 (3 of 10 facets)
//! Still no meet point: T (0.83 mm off)
//! ```
//!
//! Distances are in millimeters when the design has a girdle diameter, else a share of the
//! stone's size (`1.2 % of the stone's size`). Tiers are named as the tier table and the
//! verdict's reasons name them.
//!
//! # The stone is checked by the editor's own rule
//!
//! Moving a tier onto a point can move it onto another point than the designer meant (on
//! Pentalume, P2 goes down to the culet), and the new depths can cut small tiers away. The
//! snapped design goes through `pleochroma_editor::verdict::judge_snap`, the one rule the
//! verdict's Snap to Meets fix applies too: the snap is refused when the stone does not solve
//! or close, loses or much thins its girdle, or puts a table or culet on the wrong side of the
//! girdle. Facets cut away do not refuse it: the rule reports them tier by tier, and the
//! "Cut away by the snap" line is its `SnapOutcome::cut_away_list`.
//!
//! # Exit codes and files
//!
//! 0 when every tier but the anchors meets a point after the snap and nothing was cut away; 4,
//! as for a `validate` Problem, when some tier still floats or the snap cut facets away: the
//! run worked and found something left to look at, so the snapped design is still written.
//! `--out` gets the snapped design as a `.psdd` file (with everything else the source file
//! held); `--dry-run` writes nothing. A design that does not solve and close, before or after
//! the snap, or that the snap would break, is refused with exit code 2 and nothing is written.

use super::{key_values, list_block, open};
use crate::{
    args::{SnapLimit, SnapMeetsArgs},
    format::fixed,
    load::Loaded,
    materials::Catalogue,
    outcome::{CliError, CommandResult, EXIT_OK, EXIT_PROBLEM, Outcome},
    stone::analyze,
};
use pleochroma::geometry::meet_solver::SolvedTier;
use pleochroma_cut_core::{CutMeet, CutMeets, DEFAULT_SNAP_MAX_MOVE_FRACTION, Design, SnapMove};
use pleochroma_editor::{
    retarget::plan::tier_display_names,
    verdict::{SnapRefusal, judge_snap},
};

/// The refusal when a stone that closes still gives no meets (the analysis and the solve
/// disagree on the tiers).
const NO_MEETS: &str = "the meet points of the stone cut so far cannot be worked out";

/// How the report writes a distance given in design units.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Scale {
    /// Millimeters per design unit, when the design has a girdle diameter.
    mm_per_unit: Option<f64>,
    /// The finished stone's size (its largest extent), in design units. Positive.
    size: f64,
}

impl Scale {
    /// The scale of `design` solved as `solved`, whose finished stone is `size` design units.
    ///
    /// # Errors
    ///
    /// [`CliError::design`] when `size` is not a positive length.
    fn of(design: &Design, solved: &[SolvedTier], size: f64) -> Result<Self, CliError> {
        if size.is_finite() && size > 0.0 {
            Ok(Self {
                mm_per_unit: design.yield_report(solved).mm_per_unit,
                size,
            })
        } else {
            Err(CliError::design(format!(
                "{NO_MEETS}: the stone has no size to measure them against"
            )))
        }
    }

    /// `distance` (design units) as a percentage of the stone's size.
    const fn percent(self, distance: f64) -> f64 {
        100.0 * distance / self.size
    }

    /// `distance` (design units) as the report writes it: `0.03 mm` when the design has a
    /// girdle diameter, else `0.3 % of the stone's size`.
    fn distance(self, distance: f64) -> String {
        self.mm_per_unit.map_or_else(
            || format!("{} % of the stone's size", fixed(self.percent(distance), 1)),
            |per_unit| format!("{} mm", fixed(distance * per_unit, 2)),
        )
    }
}

/// The most one tier may move, in design units.
///
/// # Errors
///
/// [`CliError::usage`] for `--max-move` on a design without a girdle diameter: it has no
/// millimeters to turn into its own units.
fn limit_in_units(limit: SnapLimit, scale: Scale) -> Result<f64, CliError> {
    match limit {
        SnapLimit::Fraction(fraction) => Ok(fraction * scale.size),
        SnapLimit::Millimeters(mm) => {
            scale
                .mm_per_unit
                .map(|per_unit| mm / per_unit)
                .ok_or_else(|| {
                    CliError::usage(format!(
                        "--max-move is in millimeters, and this design has no girdle diameter (the \
                     stone's size) to turn them into its own units: give --max-move-fraction F \
                     instead, a share of the stone's size (without either the limit is \
                     {DEFAULT_SNAP_MAX_MOVE_FRACTION})"
                    ))
                })
        }
    }
}

/// The limit as the report's header gives it: `0.20 mm (2.0 % of the stone's size)`, or the
/// share alone without a girdle diameter.
fn limit_text(max_move: f64, scale: Scale) -> String {
    let share = fixed(scale.percent(max_move), 1);
    scale.mm_per_unit.map_or_else(
        || format!("{share} % of the stone's size"),
        |per_unit| {
            format!(
                "{} mm ({share} % of the stone's size)",
                fixed(max_move * per_unit, 2)
            )
        },
    )
}

/// What the report calls every tier, by tier index: the name the tier table shows (its own
/// name, or its standard code), or `Tier N` for a tier without one, as the verdict's reasons
/// name them.
fn tier_names(design: &Design) -> Vec<String> {
    tier_display_names(design)
        .into_iter()
        .enumerate()
        .map(|(index, name)| {
            let name = name.trim();
            if name.is_empty() {
                format!("Tier {}", index + 1)
            } else {
                name.to_string()
            }
        })
        .collect()
}

/// The name of tier `tier` in `names`, `Tier N` when `names` has none.
fn name_of(names: &[String], tier: usize) -> String {
    names
        .get(tier)
        .cloned()
        .unwrap_or_else(|| format!("Tier {}", tier + 1))
}

/// `items` as a list in a sentence: `a`, `a and b`, `a, b and c`.
fn sentence_list(items: &[String]) -> String {
    match items {
        [] => String::new(),
        [only] => only.clone(),
        [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
    }
}

/// `1 tier has no meet point`, `3 tiers have no meet point`, or that every tier meets.
fn floating_count(count: usize) -> String {
    match count {
        0 => "every tier but the anchors meets a point".to_string(),
        1 => "1 tier has no meet point".to_string(),
        many => format!("{many} tiers have no meet point"),
    }
}

/// One line per tier the snap moved, in cutting order, the names in one padded column:
/// `P2  0.7662 -> 0.7598  (moved 0.03 mm, now meets P1, G1)`. The tiers it meets are the ones
/// `after` (the meets of the solved, snapped design) gives it; a moved tier that `after` does
/// not show meeting a point gets no "now meets", and is listed among the tiers that still float.
fn moved_lines(
    names: &[String],
    moved: &[SnapMove],
    after: &CutMeets,
    scale: Scale,
) -> Vec<String> {
    let labels: Vec<String> = moved.iter().map(|step| name_of(names, step.tier)).collect();
    let width = labels
        .iter()
        .map(|label| label.chars().count())
        .max()
        .unwrap_or(0);
    moved
        .iter()
        .zip(&labels)
        .map(|(step, label)| {
            let pad = " ".repeat(width.saturating_sub(label.chars().count()));
            let mut why = format!(
                "moved {}",
                scale.distance((step.new_mast - step.old_mast).abs())
            );
            if let Some(CutMeet::Meets(tiers)) = after.tiers.get(step.tier) {
                let met: Vec<String> = tiers.iter().map(|&tier| name_of(names, tier)).collect();
                why.push_str(", now meets ");
                why.push_str(&met.join(", "));
            }
            format!(
                "{label}{pad}  {} -> {}  ({why})",
                fixed(step.old_mast, 4),
                fixed(step.new_mast, 4)
            )
        })
        .collect()
}

/// `Still no meet point: P3 (0.26 mm off) and T (0.83 mm off)` for the tiers `floating`
/// lists as (tier, miss in design units), in cutting order; `None` when there are none.
fn still_floating_line(
    names: &[String],
    floating: &[(usize, f64)],
    scale: Scale,
) -> Option<String> {
    if floating.is_empty() {
        return None;
    }
    let items: Vec<String> = floating
        .iter()
        .map(|&(tier, miss)| format!("{} ({} off)", name_of(names, tier), scale.distance(miss)))
        .collect();
    Some(format!("Still no meet point: {}", sentence_list(&items)))
}

/// `Cut away by the snap: P3 (all 10 facets) and C3 (3 of 10 facets)` for the list
/// `SnapOutcome::cut_away_list` gives.
fn cut_away_line(list: &str) -> String {
    format!("Cut away by the snap: {list}")
}

/// The refusal `judge_snap` gives a snap that would break the stone, as this command's error:
/// exit code 2, and nothing is written.
fn refused(refusal: &SnapRefusal) -> CliError {
    CliError::design(format!("{refusal} Nothing was written."))
}

/// What the snap did, for the report.
struct Facts<'a> {
    /// What the report calls every tier ([`tier_names`] of the design as opened).
    names: Vec<String>,
    /// The limit, in design units.
    max_move: f64,
    /// The scale of the design as opened: the limit and the moves are given in it.
    before_scale: Scale,
    /// The scale of the snapped design: the misses that are left are given in it.
    after_scale: Scale,
    /// How many tiers floated before the snap.
    floating_before: usize,
    /// The tiers the snap moved, in cutting order.
    moved: &'a [SnapMove],
    /// The meets of the snapped design, solved again.
    after: &'a CutMeets,
    /// The tiers that still float after the snap, in cutting order, with their miss.
    floating_after: Vec<(usize, f64)>,
    /// The tiers the new depths cut facets away from, as `judge_snap` lists them
    /// (`P3 (all 10 facets) and C3 (3 of 10 facets)`); `None` when nothing is cut away.
    cut_away: Option<String>,
}

/// The text report.
fn text_report(loaded: &Loaded, facts: &Facts<'_>, dry_run: bool) -> String {
    let mut text = key_values(&[
        ("Design", format!("{} ({})", loaded.name, loaded.file_name)),
        ("Limit", limit_text(facts.max_move, facts.before_scale)),
        ("Before", floating_count(facts.floating_before)),
        ("After", floating_count(facts.floating_after.len())),
    ]);
    text.push('\n');
    let moved = moved_lines(&facts.names, facts.moved, facts.after, facts.before_scale);
    if moved.is_empty() {
        text.push_str(if facts.floating_before == 0 {
            "Nothing to move: every tier but the anchors meets a point already on the stone.\n"
        } else {
            "No tier was moved (a tier moves only when it is set by an explicit depth and a \
             point is within the limit).\n"
        });
    }
    for line in &moved {
        text.push_str(line);
        text.push('\n');
    }
    if let Some(list) = &facts.cut_away {
        text.push_str(&cut_away_line(list));
        text.push('\n');
    }
    let still = still_floating_line(&facts.names, &facts.floating_after, facts.after_scale);
    if let Some(line) = &still {
        text.push_str(line);
        text.push('\n');
    }
    if still.is_none() && !moved.is_empty() {
        text.push_str("Every tier but the anchors now meets a point already on the stone.\n");
    }
    if dry_run {
        text.push_str("Dry run: nothing was written.\n");
    }
    let notes = list_block("Notes", &loaded.notes);
    if !notes.is_empty() {
        text.push('\n');
        text.push_str(&notes);
    }
    text
}

/// Runs `snap-meets` on an opened design.
///
/// # Errors
///
/// [`CliError::design`] when the design does not solve and close, before or after the snap, when
/// the snap would break the stone (`judge_snap` refuses it), or when its meets cannot be worked
/// out;
/// [`CliError::usage`] for `--max-move` on a design without a girdle diameter. Tiers that still
/// float, or facets the snap cut away, are not an error: exit code 4, and the file is written.
fn execute(loaded: &Loaded, catalogue: &Catalogue, args: &SnapMeetsArgs) -> CommandResult {
    let design = &loaded.design;
    let analysis = analyze(design);
    let solved = analysis.require_stone(design)?;
    let before = design
        .cut_meets(solved)
        .ok_or_else(|| CliError::design(NO_MEETS))?;
    let before_scale = Scale::of(design, solved, before.size)?;
    let max_move = limit_in_units(args.limit, before_scale)?;

    let mut snapped = design.clone();
    let report = snapped
        .snap_depths_to_meets(solved, max_move)
        .ok_or_else(|| CliError::design(NO_MEETS))?;
    let after_analysis = analyze(&snapped);
    let after_solved = after_analysis
        .require_stone(&snapped)
        .map_err(|error| CliError::design(format!("after the snap, {}", error.message)))?;
    // The editor's own rule for a snapped design: refused when the stone breaks, facets cut away
    // reported.
    let judged = judge_snap(design, &snapped).map_err(|refusal| refused(&refusal))?;
    let after = snapped
        .cut_meets(after_solved)
        .ok_or_else(|| CliError::design(NO_MEETS))?;
    let after_scale = Scale::of(&snapped, after_solved, after.size)?;

    let facts = Facts {
        names: tier_names(design),
        max_move,
        before_scale,
        after_scale,
        floating_before: before.floating(design).len(),
        moved: &report.moved,
        after: &after,
        floating_after: after.floating(&snapped),
        cut_away: judged.cut_away_list(design),
    };
    let exit = if facts.floating_after.is_empty() && facts.cut_away.is_none() {
        EXIT_OK
    } else {
        EXIT_PROBLEM
    };
    let mut outcome = Outcome::text(text_report(loaded, &facts, args.dry_run)).with_exit(exit);
    if let Some(path) = &args.out
        && !args.dry_run
    {
        outcome = outcome.with_file(path.clone(), loaded.design_text(&snapped, catalogue)?);
    }
    Ok(outcome)
}

/// Runs `snap-meets`.
///
/// # Errors
///
/// [`CliError`] when the design cannot be opened, does not solve and close (before or after the
/// snap), the snap would break the stone, or `--max-move` is given for a design without a
/// girdle diameter. Tiers that still float, or facets the snap cut away, are an outcome with
/// exit code 4, not an error: see the module documentation.
pub fn run(args: &SnapMeetsArgs) -> CommandResult {
    let (loaded, catalogue) = open(&args.design, args.db.as_deref())?;
    execute(&loaded, &catalogue, args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        load::from_bytes,
        outcome::{EXIT_USAGE, OutFile},
        testing,
    };
    use pleochroma_cut_core::CutMeetAnchor;
    use pleochroma_editor::verdict::{CutAway, SnapOutcome};
    use std::path::PathBuf;

    /// Five millimeters per design unit on a stone two units across (10 mm).
    const IN_MM: Scale = Scale {
        mm_per_unit: Some(5.0),
        size: 2.0,
    };

    /// The same stone without a girdle diameter.
    const AS_SHARE: Scale = Scale {
        mm_per_unit: None,
        size: 2.0,
    };

    fn args(limit: SnapLimit, dry_run: bool, out: Option<&str>) -> SnapMeetsArgs {
        SnapMeetsArgs {
            design: PathBuf::new(),
            limit,
            dry_run,
            out: out.map(PathBuf::from),
            db: None,
        }
    }

    const fn default_limit() -> SnapLimit {
        SnapLimit::Fraction(DEFAULT_SNAP_MAX_MOVE_FRACTION)
    }

    fn names(list: &[&str]) -> Vec<String> {
        list.iter().map(|name| (*name).to_string()).collect()
    }

    const fn close(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-12
    }

    #[test]
    fn a_moved_tier_is_one_line_with_its_masts_its_move_and_its_meets() {
        let after = CutMeets {
            tiers: vec![
                CutMeet::Anchor(CutMeetAnchor::CutToCenterpoint),
                CutMeet::Anchor(CutMeetAnchor::SetStoneSize),
                CutMeet::Meets(vec![0, 1]),
            ],
            size: 2.0,
        };
        let moved = [SnapMove {
            tier: 2,
            old_mast: 0.7662,
            new_mast: 0.7598,
            meets: vec![0, 1],
        }];
        let names = names(&["P1", "G1", "P2"]);
        assert_eq!(
            moved_lines(&names, &moved, &after, IN_MM),
            ["P2  0.7662 -> 0.7598  (moved 0.03 mm, now meets P1, G1)"]
        );
        assert_eq!(
            moved_lines(&names, &moved, &after, AS_SHARE),
            ["P2  0.7662 -> 0.7598  (moved 0.3 % of the stone's size, now meets P1, G1)"]
        );
    }

    #[test]
    fn the_names_of_several_moved_tiers_line_up_and_one_that_floats_names_no_meets() {
        let after = CutMeets {
            tiers: vec![
                CutMeet::Meets(vec![1]),
                CutMeet::Anchor(CutMeetAnchor::NothingToMeet),
                CutMeet::Floats {
                    miss: 0.01,
                    nearest: vec![1],
                },
            ],
            size: 2.0,
        };
        let moved = [
            SnapMove {
                tier: 0,
                old_mast: 1.0,
                new_mast: 0.99,
                meets: vec![1],
            },
            SnapMove {
                tier: 2,
                old_mast: 0.5,
                new_mast: 0.52,
                meets: vec![1],
            },
        ];
        assert_eq!(
            moved_lines(&names(&["P2", "G1", "C10"]), &moved, &after, IN_MM),
            [
                "P2   1.0000 -> 0.9900  (moved 0.05 mm, now meets G1)",
                "C10  0.5000 -> 0.5200  (moved 0.10 mm)",
            ]
        );
        // A tier the names do not cover is still named.
        assert_eq!(
            moved_lines(&[], &moved[..1], &after, IN_MM),
            ["Tier 1  1.0000 -> 0.9900  (moved 0.05 mm, now meets Tier 2)"]
        );
    }

    #[test]
    fn the_tiers_that_still_float_are_one_sentence() {
        let names = names(&["P1", "P2", "P3", "T"]);
        assert_eq!(still_floating_line(&names, &[], IN_MM), None);
        assert_eq!(
            still_floating_line(&names, &[(3, 0.166)], IN_MM).as_deref(),
            Some("Still no meet point: T (0.83 mm off)")
        );
        assert_eq!(
            still_floating_line(&names, &[(1, 0.024), (2, 0.0528), (3, 0.166)], IN_MM).as_deref(),
            Some("Still no meet point: P2 (0.12 mm off), P3 (0.26 mm off) and T (0.83 mm off)")
        );
        assert_eq!(
            still_floating_line(&names, &[(3, 0.024)], AS_SHARE).as_deref(),
            Some("Still no meet point: T (1.2 % of the stone's size off)")
        );
        assert_eq!(
            sentence_list(&names[..2]),
            "P1 and P2",
            "two items take no comma"
        );
    }

    #[test]
    fn the_limit_is_a_share_of_the_stone_or_millimeters_where_the_design_has_them() {
        assert_eq!(
            limit_text(0.04, IN_MM),
            "0.20 mm (2.0 % of the stone's size)"
        );
        assert_eq!(limit_text(0.04, AS_SHARE), "2.0 % of the stone's size");
        let share = limit_in_units(SnapLimit::Fraction(0.02), AS_SHARE).expect("a share");
        assert!(close(share, 0.04), "{share}");
        let mm = limit_in_units(SnapLimit::Millimeters(0.1), IN_MM).expect("millimeters");
        assert!(close(mm, 0.02), "{mm}");
        let error = limit_in_units(SnapLimit::Millimeters(0.1), AS_SHARE)
            .expect_err("no millimeters without a girdle diameter");
        assert_eq!(error.code, EXIT_USAGE);
        assert!(
            error.message.contains("--max-move-fraction"),
            "{}",
            error.message
        );
        assert_eq!(
            floating_count(0),
            "every tier but the anchors meets a point"
        );
        assert_eq!(floating_count(1), "1 tier has no meet point");
        assert_eq!(floating_count(3), "3 tiers have no meet point");
    }

    fn cams() -> Loaded {
        testing::asc("cams.asc", testing::CAMS_ASC)
    }

    #[test]
    fn a_dry_run_on_a_true_meet_point_design_moves_nothing_writes_nothing_and_exits_0() {
        let outcome = execute(
            &cams(),
            &Catalogue::default(),
            &args(default_limit(), true, Some("cams-snapped.psdd")),
        )
        .expect("Cam's Portuguese is a usable stone");
        assert!(outcome.files.is_empty(), "a dry run writes nothing");
        assert!(outcome.stdout.starts_with("Design:"), "{}", outcome.stdout);
        for line in [
            "Before:  every tier but the anchors meets a point\n",
            "After:   every tier but the anchors meets a point\n",
            "Nothing to move: every tier but the anchors meets a point already on the stone.\n",
            "Dry run: nothing was written.\n",
        ] {
            assert!(
                outcome.stdout.contains(line),
                "{line:?}: {}",
                outcome.stdout
            );
        }
        assert!(
            !outcome.stdout.contains("Cut away by the snap:"),
            "{}",
            outcome.stdout
        );
        assert!(
            !outcome.stdout.contains("Still no meet point:"),
            "{}",
            outcome.stdout
        );
        assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stdout);
        assert_eq!(outcome.stderr, "");
    }

    #[test]
    fn the_snapped_design_is_saved_keeps_its_tiers_and_the_same_run_gives_the_same_bytes() {
        let loaded = testing::asc("pentalume.asc", testing::PENTALUME_ASC);
        let wanted = args(default_limit(), false, Some("pentalume-snapped.psdd"));
        let outcome = execute(&loaded, &Catalogue::default(), &wanted).expect("snaps");
        // Tiers still float (T) and facets are cut away (P3, C3): 4, and the file is written.
        assert_eq!(outcome.exit, EXIT_PROBLEM, "{}", outcome.stdout);
        let saved =
            testing::file_text(&outcome, "pentalume-snapped.psdd").expect("the design is saved");
        let again = from_bytes("pentalume-snapped.psdd", saved.as_bytes()).expect("opens");
        // Only depths move: every tier keeps its name, angle and index positions.
        assert_eq!(
            testing::shape(&again.design),
            testing::shape(&loaded.design)
        );
        assert_ne!(
            again.design.tiers[1].constraint, loaded.design.tiers[1].constraint,
            "P2 is saved at its new depth"
        );
        assert!(analyze(&again.design).is_closed());
        let second = execute(&loaded, &Catalogue::default(), &wanted).expect("snaps");
        assert_eq!(outcome, second);
    }

    #[test]
    fn millimeters_need_a_girdle_diameter_and_a_share_does_not() {
        let mut without_girdle = cams();
        without_girdle.design.girdle_diameter_mm = None;
        let error = execute(
            &without_girdle,
            &Catalogue::default(),
            &args(SnapLimit::Millimeters(0.2), true, None),
        )
        .expect_err("no millimeters without a girdle diameter");
        assert_eq!(error.code, EXIT_USAGE, "{}", error.message);
        assert!(
            error.message.contains("--max-move-fraction"),
            "{}",
            error.message
        );
        let share = execute(
            &without_girdle,
            &Catalogue::default(),
            &args(default_limit(), true, None),
        )
        .expect("a share needs no girdle diameter");
        assert!(
            share
                .stdout
                .contains("Limit:   2.0 % of the stone's size\n"),
            "{}",
            share.stdout
        );

        let mut sized = cams();
        sized.design.girdle_diameter_mm = Some(6.5);
        let outcome = execute(
            &sized,
            &Catalogue::default(),
            &args(SnapLimit::Millimeters(0.2), true, None),
        )
        .expect("millimeters with a girdle diameter");
        assert!(
            outcome.stdout.contains("Limit:   0.20 mm ("),
            "{}",
            outcome.stdout
        );
    }

    #[test]
    fn a_design_that_is_not_a_stone_is_refused() {
        let empty = Design::new(
            pleochroma_cut_core::PreformSpec::block(2.0, 1.0, 4.0),
            pleochroma_cut_core::ScheduleMeta::standard_round_brilliant(),
            Vec::new(),
        );
        let error = execute(
            &Loaded::in_memory(empty, "empty.psdd"),
            &Catalogue::default(),
            &args(default_limit(), false, Some("empty-out.psdd")),
        )
        .expect_err("no stone");
        assert_eq!(error.code, crate::outcome::EXIT_DESIGN, "{}", error.message);
    }

    /// On Pentalume the walk moves P2 down onto the culet (and G2 and C2 after it), which cuts
    /// P3 and C3 away entirely. The girdle and the flats survive, so the snap is not refused:
    /// the report names what is lost and the run ends with exit code 4.
    #[test]
    fn snapping_pentalume_names_the_tiers_it_cuts_away() {
        let loaded = testing::asc("pentalume.asc", testing::PENTALUME_ASC);
        let outcome = execute(
            &loaded,
            &Catalogue::default(),
            &args(default_limit(), true, None),
        )
        .expect("the snap keeps a usable stone");
        assert!(
            outcome
                .stdout
                .contains("Cut away by the snap: P3 (all 10 facets) and C3 (all 10 facets)\n"),
            "{}",
            outcome.stdout
        );
        assert!(
            outcome.stdout.lines().any(|line| line.starts_with("P2  ")),
            "P2 is moved: {}",
            outcome.stdout
        );
        assert_eq!(outcome.exit, EXIT_PROBLEM, "{}", outcome.stdout);
        assert!(outcome.files.is_empty(), "a dry run writes nothing");
    }

    /// The line is the shared rule's own list behind the CLI's words, whatever the outcome
    /// holds: partly cut tiers, whole ones and a lone facet, in the outcome's order.
    #[test]
    fn the_cut_away_line_is_the_shared_rules_list() {
        let pentalume = testing::asc("pentalume.asc", testing::PENTALUME_ASC);
        let outcome = SnapOutcome {
            moved: vec![1],
            cut_away: vec![
                CutAway {
                    tier: 2,
                    lost: 10,
                    total: 10,
                },
                CutAway {
                    tier: 7,
                    lost: 3,
                    total: 10,
                },
            ],
        };
        let list = outcome
            .cut_away_list(&pentalume.design)
            .expect("something is cut away");
        assert_eq!(
            cut_away_line(&list),
            "Cut away by the snap: P3 (all 10 facets) and C3 (3 of 10 facets)"
        );
        assert_eq!(
            SnapOutcome::default().cut_away_list(&pentalume.design),
            None,
            "no line when nothing is cut away"
        );
    }

    /// A snapped design the shared rule refuses is this command's exit-2 refusal, and it says
    /// that nothing was written.
    #[test]
    fn a_snap_the_shared_rule_refuses_is_exit_2_and_writes_nothing() {
        let pentalume = testing::asc("pentalume.asc", testing::PENTALUME_ASC);
        let mut short = pentalume.design.clone();
        short.tiers.pop();
        let refusal = judge_snap(&pentalume.design, &short).expect_err("not the same tiers");
        let error = refused(&refusal);
        assert_eq!(error.code, crate::outcome::EXIT_DESIGN, "{}", error.message);
        assert!(
            error.message.starts_with(&refusal.sentence),
            "{}",
            error.message
        );
        assert!(
            error.message.ends_with(" Nothing was written."),
            "{}",
            error.message
        );
        // The unchanged design passes with nothing moved or cut away.
        assert_eq!(
            judge_snap(&pentalume.design, &pentalume.design),
            Ok(SnapOutcome::default())
        );
    }

    #[test]
    fn end_to_end_a_dry_run_from_the_command_line_writes_nothing() {
        let input = testing::write_design("snap-meets-e2e.psdd", &cams().design);
        let outcome = testing::run(&["snap-meets", &input, "--dry-run"]);
        assert_eq!(outcome.exit, EXIT_OK, "{}", outcome.stderr);
        assert_eq!(outcome.files, [] as [OutFile; 0]);
        assert!(outcome.stdout.contains("Dry run"), "{}", outcome.stdout);
    }
}
