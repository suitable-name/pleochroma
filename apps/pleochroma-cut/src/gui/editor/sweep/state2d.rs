//! What the Angle Sweep dialog's Two Tiers mode remembers between callbacks, and what its
//! form comes to: the sweep that is running, the cells being shown, the figure on the map,
//! the selected and hovered pairs, and the question about a long sweep with tilt averages.
//!
//! Plain data and decisions, no Slint types, so they are tested without a window.

use super::{form::Fields, run2d::RunHandle2d};
use pleochroma_cut_core::{Design, design::snap_noise};
use pleochroma_editor::{
    solve_policy::SolveCostEstimate,
    sweep::{
        DEFAULT_SWEEP_STEP_DEG, MAX_SWEEP_ANGLE_DEG, Sweep2dError, Sweep2dOutcome, Sweep2dPlan,
        SweepMetric, estimate_seconds_2d, format_angle_input, format_duration_estimate,
        parse_sweep_range, plan_sweep_2d,
    },
};

/// How far either side of the current angle a tier's range starts out in Two Tiers, degrees:
/// a grid costs the product of its two sides, so it starts smaller than a one-tier range.
const HALF_RANGE_2D_DEG: f64 = 2.0;

/// The flattest end the starting range reaches, degrees from flat.
const RANGE_FLOOR_DEG: f64 = 0.5;

/// A tilt sweep of more cells than this asks before it runs.
pub(super) const TILT_CONFIRM_CELLS: usize = 100;

/// What the Two Tiers form comes to.
#[derive(Debug, Clone, PartialEq)]
pub(super) enum Checked2d {
    /// A grid that can run, with the line that tells the cutter what it will do.
    Ready { plan: Sweep2dPlan, summary: String },
    /// Why it cannot run, in a sentence for the dialog.
    Refused(String),
}

/// The range a tier starts with in Two Tiers: two degrees either side of its angle in
/// half-degree steps, as magnitudes, flattest first.
pub(super) fn default_fields_2d(angle_deg: f64) -> Fields {
    let magnitude = angle_deg.abs();
    let low = snap_noise((magnitude - HALF_RANGE_2D_DEG).max(RANGE_FLOOR_DEG));
    let high = snap_noise((magnitude + HALF_RANGE_2D_DEG).min(MAX_SWEEP_ANGLE_DEG - 0.4));
    Fields {
        from: format_angle_input(low),
        to: format_angle_input(high),
        step: format_angle_input(DEFAULT_SWEEP_STEP_DEG),
    }
}

/// The entry of `tiers` (design positions, as the combo lists them) the down tier starts
/// on: the first tier on the other side of the girdle from the across tier `a_choice`, else
/// the first other tier, else the across tier itself (the form then says why it cannot run).
pub(super) fn default_b_choice(design: &Design, tiers: &[usize], a_choice: usize) -> usize {
    let Some(&a_tier) = tiers.get(a_choice) else {
        return 0;
    };
    let below_girdle = |tier: usize| {
        design
            .tiers
            .get(tier)
            .map(|found| found.angle_deg.is_sign_negative())
    };
    let a_side = below_girdle(a_tier);
    let others = || {
        tiers
            .iter()
            .enumerate()
            .filter(move |&(_, &tier)| tier != a_tier)
    };
    others()
        .find(|&(_, &tier)| below_girdle(tier) != a_side)
        .or_else(|| others().next())
        .map_or(a_choice, |(index, _)| index)
}

/// `Across tier: ...` or `Down tier: ...` before a sentence about one axis.
fn axis_message(across: bool, message: &str) -> String {
    let which = if across { "Across tier" } else { "Down tier" };
    format!("{which}: {message}")
}

/// The sentence for a grid that cannot run.
pub(super) fn refusal_text(error: &Sweep2dError) -> String {
    match error {
        Sweep2dError::Axis { across, error } => axis_message(*across, &error.to_string()),
        other => other.to_string(),
    }
}

/// `21 × 17 = 357 angles · about 12 seconds`.
fn summary_line(plan: &Sweep2dPlan, seconds: f64) -> String {
    format!(
        "{} \u{d7} {} = {} angle{} \u{b7} {}",
        plan.columns(),
        plan.rows(),
        plan.len(),
        if plan.len() == 1 { "" } else { "s" },
        format_duration_estimate(seconds)
    )
}

/// Reads the form: each tier's three fields, then the grid, then the line that says how many
/// angle pairs it tries and how long that takes. Cheap (it never solves), so the dialog
/// calls it on every keystroke. `across` and `down` are `(tier, fields)`.
pub(super) fn check_2d(
    design: &Design,
    across: (usize, &Fields),
    down: (usize, &Fields),
    tilt_average: bool,
    workers: usize,
) -> Checked2d {
    let read = |is_across: bool, fields: &Fields| {
        parse_sweep_range(&fields.from, &fields.to, &fields.step)
            .map_err(|error| axis_message(is_across, &error.to_string()))
    };
    let (range_a, range_b) = match (read(true, across.1), read(false, down.1)) {
        (Ok(a), Ok(b)) => (a, b),
        (Err(message), _) | (_, Err(message)) => return Checked2d::Refused(message),
    };
    match plan_sweep_2d(design, across.0, &range_a, down.0, &range_b) {
        Ok(plan) => {
            let seconds = estimate_seconds_2d(
                plan.len(),
                tilt_average,
                workers,
                SolveCostEstimate::of(design),
            );
            let summary = summary_line(&plan, seconds);
            Checked2d::Ready { plan, summary }
        }
        Err(error) => Checked2d::Refused(refusal_text(&error)),
    }
}

/// Whether a sweep of `cells` angle pairs asks before it runs: it uses tilt averages and is
/// long.
pub(super) const fn needs_tilt_confirm(cells: usize, tilt_average: bool) -> bool {
    tilt_average && cells > TILT_CONFIRM_CELLS
}

/// The question the form asks before a long sweep with tilt averages.
pub(super) fn tilt_question(seconds: f64) -> String {
    format!(
        "This sweep takes {} with Tilt Averages. Run it?",
        format_duration_estimate(seconds)
    )
}

/// The three number fields that describe `angles` (signed as stored, ascending magnitude):
/// the first and last as magnitudes and the widest gap as the step. A single angle keeps
/// `previous_step`.
pub(super) fn axis_fields(angles: &[f64], previous_step: &str) -> Fields {
    let first = angles.first().map_or(0.0, |angle| angle.abs());
    let last = angles.last().map_or(0.0, |angle| angle.abs());
    let step = angles
        .windows(2)
        .map(|pair| (pair[1].abs() - pair[0].abs()).abs())
        .filter(|gap| *gap > 1e-9)
        .reduce(f64::max);
    Fields {
        from: format_angle_input(first),
        to: format_angle_input(last),
        step: step.map_or_else(|| previous_step.to_owned(), format_angle_input),
    }
}

/// The figures the map can show for `outcome`: the fast ones, the yield when the rough has a
/// volume, and the tilt averages when the sweep asked for them.
pub(super) fn figures_of(outcome: &Sweep2dOutcome) -> Vec<SweepMetric> {
    SweepMetric::ALL
        .into_iter()
        .filter(|metric| match metric {
            SweepMetric::TiltBrilliance
            | SweepMetric::TiltWindowing
            | SweepMetric::TiltExtinction => outcome.tilt_average,
            SweepMetric::Yield => outcome
                .cells
                .iter()
                .any(|cell| metric.value_of(cell.metrics.as_ref()).is_some()),
            _ => true,
        })
        .collect()
}

/// The pair of angles "Use These Angles" would set.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) struct Pick2d {
    /// The cell in the grid.
    pub col: usize,
    pub row: usize,
    /// The across tier and the angle to set, degrees, signed as the tier stores it.
    pub tier_a: usize,
    pub angle_a: f64,
    /// The down tier and its angle.
    pub tier_b: usize,
    pub angle_b: f64,
}

/// The Two Tiers form as it was when the dialog went back to One Tier, so coming back finds
/// it as it was left.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct SavedForm {
    /// The entries of the two tier combos.
    pub a_index: i32,
    pub b_index: i32,
    /// The two tiers' number fields.
    pub a: Fields,
    pub b: Fields,
}

/// The Two Tiers mode's state.
pub(super) struct TwoTier {
    /// The sweep that is running, if one is.
    pub run: Option<RunHandle2d>,
    /// The design's generation when the shown or running sweep started; "Use These Angles"
    /// refuses once the design has moved on.
    pub generation: u64,
    /// The finished or stopped sweep being shown.
    pub outcome: Option<Sweep2dOutcome>,
    /// The figure on the map.
    pub figure: SweepMetric,
    /// Whether the plateau is outlined.
    pub contour: bool,
    /// The selected pair, `(column, row)`.
    pub selected: Option<(usize, usize)>,
    /// The pair under the pointer.
    pub hover: Option<(usize, usize)>,
    /// Whether the question about a long tilt sweep was answered with "Run Anyway" in this
    /// dialog session.
    pub tilt_confirmed: bool,
    /// The grid waiting for that answer.
    pub pending: Option<Sweep2dPlan>,
    /// The form as it was left when the dialog went back to One Tier; `None` before it has
    /// been filled in this dialog session.
    pub saved: Option<SavedForm>,
}

impl TwoTier {
    pub(super) const fn new() -> Self {
        Self {
            run: None,
            generation: 0,
            outcome: None,
            figure: SweepMetric::Brilliance,
            contour: true,
            selected: None,
            hover: None,
            tilt_confirmed: false,
            pending: None,
            saved: None,
        }
    }

    /// Everything a new dialog session starts without: the run (dropping its handle stops
    /// it), the cells, the question and the form.
    pub(super) fn reset(&mut self) {
        *self = Self::new();
    }

    /// Forgets the cells being shown, and the selection and hover with them.
    pub(super) fn clear_results(&mut self) {
        self.outcome = None;
        self.selected = None;
        self.hover = None;
    }

    /// The figures the map offers; none before a sweep has finished.
    pub(super) fn figures(&self) -> Vec<SweepMetric> {
        self.outcome.as_ref().map_or_else(Vec::new, figures_of)
    }

    /// The position of the figure on the map among [`Self::figures`] (the first when the
    /// figure is not offered by this sweep).
    pub(super) fn figure_position(&self) -> usize {
        self.figures()
            .iter()
            .position(|metric| *metric == self.figure)
            .unwrap_or(0)
    }

    /// The figure on the map, which is the first one offered when the held one is not.
    pub(super) fn metric(&self) -> SweepMetric {
        self.figures()
            .get(self.figure_position())
            .copied()
            .unwrap_or(self.figure)
    }

    /// Puts the figure at `index` of [`Self::figures`] on the map; `false` for no such one.
    pub(super) fn set_figure_index(&mut self, index: usize) -> bool {
        match self.figures().get(index) {
            Some(&metric) => {
                self.figure = metric;
                true
            }
            None => false,
        }
    }

    /// Whether the cell `(col, row)` finished.
    fn has_cell(&self, col: usize, row: usize) -> bool {
        self.outcome
            .as_ref()
            .is_some_and(|outcome| outcome.cell_at(col, row).is_some())
    }

    /// Selects the cell `(col, row)`; `false` (and nothing changes) when it did not finish.
    pub(super) fn select(&mut self, col: usize, row: usize) -> bool {
        if self.has_cell(col, row) {
            self.selected = Some((col, row));
            true
        } else {
            false
        }
    }

    /// Points at a cell (`None` for a pointer that has left the map). Returns whether the
    /// hovered cell changed.
    pub(super) fn hover_at(&mut self, cell: Option<(usize, usize)>) -> bool {
        let cell = cell.filter(|&(col, row)| self.has_cell(col, row));
        let changed = cell != self.hover;
        self.hover = cell;
        changed
    }

    /// Moves the selection by `(dcol, drow)` cells, starting from the selected cell, else
    /// from the design's own pair, and stopping at the edge of the grid. `false` when it did
    /// not move.
    pub(super) fn step_selection(&mut self, dcol: isize, drow: isize) -> bool {
        let Some(outcome) = &self.outcome else {
            return false;
        };
        let (columns, rows) = (outcome.plan.columns(), outcome.plan.rows());
        if columns == 0 || rows == 0 {
            return false;
        }
        let from = self
            .selected
            .or_else(|| outcome.current_cell().map(|cell| (cell.col, cell.row)))
            .unwrap_or((0, 0));
        let col = from.0.saturating_add_signed(dcol).min(columns - 1);
        let row = from.1.saturating_add_signed(drow).min(rows - 1);
        if Some((col, row)) == self.selected {
            return false;
        }
        self.select(col, row)
    }

    /// The pair "Use These Angles" sets: the selected cell, when it is a valid pair other
    /// than the one the design already has.
    pub(super) fn pick(&self) -> Option<Pick2d> {
        let outcome = self.outcome.as_ref()?;
        let (col, row) = self.selected?;
        let cell = outcome.cell_at(col, row)?;
        (cell.is_valid() && !cell.is_current).then_some(Pick2d {
            col,
            row,
            tier_a: outcome.plan.tier_a,
            angle_a: cell.angle_a,
            tier_b: outcome.plan.tier_b,
            angle_b: cell.angle_b,
        })
    }

    /// Records that the cell `(col, row)` was made the design's pair: it is now the current
    /// cell, and `generation` is the design's generation after that edit (the other cells
    /// stay true, because each is the design with only these two tiers' angles changed).
    pub(super) fn mark_applied(&mut self, col: usize, row: usize, generation: u64) {
        if let Some(outcome) = &mut self.outcome {
            for cell in &mut outcome.cells {
                cell.is_current = cell.col == col && cell.row == row;
            }
            let angles = outcome
                .cell_at(col, row)
                .map(|cell| (cell.angle_a, cell.angle_b));
            if let Some((angle_a, angle_b)) = angles {
                outcome.plan.current_col = col;
                outcome.plan.current_row = row;
                outcome.plan.current_a_deg = angle_a;
                outcome.plan.current_b_deg = angle_b;
            }
        }
        self.generation = generation;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_cut_core::{ConstraintTier, PreformSpec, ScheduleMeta};
    use pleochroma_editor::sweep::{RowMetrics, Sweep2dCell};

    const CROWN_MAIN: usize = 2;
    const PAVILION_MAIN: usize = 5;

    fn round_brilliant() -> Design {
        let mut design = Design::new(
            PreformSpec::block(2.0, 1.0, 2.0),
            ScheduleMeta::standard_round_brilliant(),
            ConstraintTier::standard_round_brilliant(),
        );
        design.ensure_tier_ids();
        design
    }

    fn fields(from: &str, to: &str, step: &str) -> Fields {
        Fields {
            from: from.to_owned(),
            to: to.to_owned(),
            step: step.to_owned(),
        }
    }

    fn figures(brilliance_pct: f32) -> RowMetrics {
        RowMetrics {
            brilliance_pct,
            windowing_pct: 10.0,
            extinction_pct: 5.0,
            fire_index: 1.0,
            scintillation_pct: 20.0,
            yield_pct: None,
            warning_count: 0,
            has_girdle: true,
            tilt: None,
        }
    }

    /// A 3 by 2 grid: columns are crown angles 34.0, 34.5, 35.0, rows are pavilion angles
    /// -40.0 and -41.0; the design's own pair is column 1, row 1; the pair at column 0,
    /// row 0 gives no stone.
    fn outcome() -> Sweep2dOutcome {
        let plan = Sweep2dPlan {
            tier_a: CROWN_MAIN,
            tier_a_name: "Crown Main".to_owned(),
            current_a_deg: 34.5,
            angles_a: vec![34.0, 34.5, 35.0],
            current_col: 1,
            tier_b: PAVILION_MAIN,
            tier_b_name: "Pavilion Main".to_owned(),
            current_b_deg: -41.0,
            angles_b: vec![-40.0, -41.0],
            current_row: 1,
        };
        let mut cells = Vec::new();
        for row in 0..2 {
            for col in 0..3 {
                cells.push(Sweep2dCell {
                    col,
                    row,
                    angle_a: plan.angles_a[col],
                    angle_b: plan.angles_b[row],
                    metrics: (col + row > 0).then(|| figures(70.0 + (col + row) as f32)),
                    notes: Vec::new(),
                    is_current: col == 1 && row == 1,
                });
            }
        }
        Sweep2dOutcome {
            plan,
            requested: 6,
            cancelled: false,
            tilt_average: false,
            cells,
        }
    }

    fn two_tier() -> TwoTier {
        let mut state = TwoTier::new();
        state.outcome = Some(outcome());
        state
    }

    #[test]
    fn a_tier_starts_with_two_degrees_either_side_as_positive_numbers() {
        assert_eq!(default_fields_2d(34.5), fields("32.5", "36.5", "0.5"));
        assert_eq!(default_fields_2d(-41.0), fields("39", "43", "0.5"));
        assert_eq!(default_fields_2d(1.0).from, "0.5", "kept off flat");
    }

    #[test]
    fn the_down_tier_starts_on_the_other_side_of_the_girdle() {
        let design = round_brilliant();
        let tiers = [1, CROWN_MAIN, 3, PAVILION_MAIN, 6];
        // The crown main (entry 1) pairs with the first tier below the girdle.
        assert_eq!(tiers[default_b_choice(&design, &tiers, 1)], PAVILION_MAIN);
        // A pavilion tier pairs with the first crown-side tier.
        assert_eq!(tiers[default_b_choice(&design, &tiers, 3)], 1);
        // A single tier has nothing to pair with: it names itself, and the form says why not.
        assert_eq!(default_b_choice(&design, &[CROWN_MAIN], 0), 0);
        assert_eq!(default_b_choice(&design, &[], 4), 0);
    }

    #[test]
    fn a_good_form_gives_the_grid_and_a_summary_line() {
        let design = round_brilliant();
        let ready = check_2d(
            &design,
            (CROWN_MAIN, &fields("33.5", "35.5", "0.5")),
            (PAVILION_MAIN, &fields("40", "42", "1")),
            false,
            4,
        );
        let Checked2d::Ready { plan, summary, .. } = ready else {
            panic!("the form is fine");
        };
        assert_eq!((plan.columns(), plan.rows()), (5, 3));
        assert!(
            summary.starts_with("5 \u{d7} 3 = 15 angles \u{b7} "),
            "{summary}"
        );
    }

    #[test]
    fn a_form_that_cannot_run_says_which_tier_and_why() {
        let design = round_brilliant();
        let refusal = |a: usize, fa: Fields, b: usize, fb: Fields| match check_2d(
            &design,
            (a, &fa),
            (b, &fb),
            false,
            4,
        ) {
            Checked2d::Refused(message) => message,
            Checked2d::Ready { .. } => panic!("the form should be refused"),
        };
        let good_a = fields("33.5", "35.5", "0.5");
        let good_b = fields("40", "42", "1");
        assert!(
            refusal(
                CROWN_MAIN,
                fields("abc", "35", "1"),
                PAVILION_MAIN,
                good_b.clone()
            )
            .starts_with("Across tier: From")
        );
        assert!(
            refusal(
                CROWN_MAIN,
                good_a.clone(),
                PAVILION_MAIN,
                fields("40", "x", "1")
            )
            .starts_with("Down tier: To")
        );
        assert!(
            refusal(CROWN_MAIN, good_a.clone(), CROWN_MAIN, good_a.clone()).contains("named twice")
        );
        assert!(refusal(CROWN_MAIN, good_a, 99, good_b).starts_with("Down tier: "));
        // 31 by 31 is over the 900 pairs a two-tier sweep takes.
        assert!(
            refusal(
                CROWN_MAIN,
                fields("30", "45", "0.5"),
                PAVILION_MAIN,
                fields("30", "45", "0.5")
            )
            .contains("at most 900")
        );
    }

    #[test]
    fn a_long_tilt_sweep_asks_once_and_a_short_or_plain_one_does_not() {
        assert!(needs_tilt_confirm(101, true));
        assert!(!needs_tilt_confirm(100, true));
        assert!(!needs_tilt_confirm(900, false));
        assert_eq!(
            tilt_question(200.0),
            "This sweep takes about 3 minutes with Tilt Averages. Run it?"
        );
    }

    #[test]
    fn a_grid_gives_back_its_fields() {
        let fields = axis_fields(&[-40.0, -40.5, -41.0, -41.5], "9");
        assert_eq!((fields.from.as_str(), fields.to.as_str()), ("40", "41.5"));
        assert_eq!(fields.step, "0.5");
        assert_eq!(axis_fields(&[34.5], "0.25").step, "0.25");
    }

    #[test]
    fn the_map_offers_the_fast_figures_and_a_yield_or_tilt_only_with_a_value() {
        let plain = two_tier();
        assert_eq!(plain.figures().len(), 5);
        assert_eq!(plain.metric(), SweepMetric::Brilliance);
        let mut tilted = two_tier();
        if let Some(outcome) = &mut tilted.outcome {
            outcome.tilt_average = true;
            outcome.cells[1].metrics = outcome.cells[1].metrics.map(|mut m| {
                m.yield_pct = Some(30.0);
                m
            });
        }
        assert_eq!(tilted.figures().len(), 9);
        assert!(tilted.set_figure_index(5));
        assert_eq!(tilted.metric(), SweepMetric::Yield);
        assert!(!tilted.set_figure_index(40));
        assert_eq!(TwoTier::new().figures(), Vec::<SweepMetric>::new());
    }

    #[test]
    fn a_figure_the_sweep_lacks_falls_back_to_the_first() {
        let mut state = two_tier();
        state.figure = SweepMetric::TiltWindowing;
        assert_eq!(state.figure_position(), 0);
        assert_eq!(state.metric(), SweepMetric::Brilliance);
    }

    #[test]
    fn the_arrow_keys_start_at_the_current_pair_and_stop_at_the_edge() {
        let mut state = two_tier();
        assert!(state.step_selection(1, 0));
        assert_eq!(
            state.selected,
            Some((2, 1)),
            "one step right of the current pair"
        );
        assert!(!state.step_selection(1, 0), "the edge");
        assert!(state.step_selection(0, -1));
        assert_eq!(state.selected, Some((2, 0)));
        assert!(state.step_selection(-5, 0));
        assert_eq!(state.selected, Some((0, 0)));
        assert!(!state.step_selection(0, -1));
        assert!(!TwoTier::new().step_selection(1, 0));
    }

    #[test]
    fn the_pointer_reports_a_change_only_once() {
        let mut state = two_tier();
        assert!(state.hover_at(Some((1, 0))));
        assert!(!state.hover_at(Some((1, 0))));
        assert!(
            state.hover_at(Some((9, 9))),
            "no such cell: the pointer is off the map"
        );
        assert!(!state.hover_at(None));
        assert_eq!(state.hover, None);
    }

    #[test]
    fn only_a_valid_pair_other_than_the_current_one_can_be_used() {
        let mut state = two_tier();
        assert_eq!(state.pick(), None, "nothing is selected");
        assert!(state.select(0, 0));
        assert_eq!(state.pick(), None, "the pair gives no stone");
        assert!(state.select(1, 1));
        assert_eq!(
            state.pick(),
            None,
            "the current pair is already the design's"
        );
        assert!(state.select(2, 0));
        assert_eq!(
            state.pick(),
            Some(Pick2d {
                col: 2,
                row: 0,
                tier_a: CROWN_MAIN,
                angle_a: 35.0,
                tier_b: PAVILION_MAIN,
                angle_b: -40.0,
            }),
            "the pick keeps the stored signs, which is what the edit takes"
        );
        assert!(!state.select(3, 0), "there is no fourth column");
    }

    #[test]
    fn using_a_pair_moves_the_current_mark_and_the_generation() {
        let mut state = two_tier();
        state.generation = 3;
        state.mark_applied(2, 0, 4);
        let outcome = state.outcome.as_ref().expect("cells");
        assert_eq!(
            outcome.cells.iter().filter(|cell| cell.is_current).count(),
            1
        );
        assert!(outcome.cell_at(2, 0).is_some_and(|cell| cell.is_current));
        assert_eq!((outcome.plan.current_col, outcome.plan.current_row), (2, 0));
        assert!((outcome.plan.current_b_deg + 40.0).abs() < 1e-12);
        assert_eq!(state.generation, 4);
    }

    #[test]
    fn clearing_the_results_forgets_the_selection_too() {
        let mut state = two_tier();
        assert!(state.select(2, 1));
        state.hover_at(Some((0, 1)));
        state.clear_results();
        assert!(state.outcome.is_none());
        assert_eq!((state.selected, state.hover), (None, None));
    }
}
