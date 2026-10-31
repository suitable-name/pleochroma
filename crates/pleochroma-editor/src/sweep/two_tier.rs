//! The two-tier sweep: two tiers' angles over a grid, every cell solved and scored.
//!
//! The dialog's "Two Tiers" mode. [`plan_sweep_2d`] turns two [`SweepRange`]s into the
//! grid (columns are the angles of the first tier, rows the angles of the second) and
//! refuses what cannot be swept; [`sweep_two_tier_angles`] runs it on the same scoring
//! path as the one-tier sweep (`score_clone`), cancellable and parallel over the cells, the
//! cells coming back in row-major order whatever the number of workers;
//! [`refine_around`] makes a finer grid around one cell; [`apply_sweep_angles`] makes a
//! cell's pair of angles real as one undo step; [`sweep2d_csv`] writes the cells as CSV.
//!
//! Angles are signed as the tiers store them (a pavilion tier's are negative) and shown as
//! magnitudes, exactly as in the one-tier sweep.

use super::{
    MAX_SWEEP_ANGLE_DEG, MIN_SWEEP_ANGLE_DEG, RowMetrics, SweepError, SweepMetric, SweepOptions,
    SweepRange, SweepScene,
    csv::{LINE_END, escape, number, yes_no},
    estimate_seconds, plan_sweep,
    score::{Cancelled, score_clone},
};
use crate::{
    EditChange, EditorSession, session::SessionEditError, solve_policy::SolveCostEstimate,
};
use pleochroma::geometry::stone_metrics::measure_solid;
use pleochroma_cut_core::{Design, Edit, EditError, design::snap_noise};
use std::{
    fmt,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};

/// The most cells one two-tier sweep takes (30 by 30).
pub const MAX_SWEEP_CELLS: usize = 900;

/// Why a two-tier sweep cannot start.
#[derive(Debug, Clone, PartialEq)]
pub enum Sweep2dError {
    /// One of the two tiers cannot be swept, or its range is not valid. `across` says
    /// which one: the columns' tier (`true`) or the rows' tier (`false`). A tier whose
    /// angle follows a relation (one tier driving the other included) is refused here
    /// as [`SweepError::Driven`].
    Axis {
        /// Whether it is the first (across) tier.
        across: bool,
        /// What is wrong with it.
        error: SweepError,
    },
    /// The same tier was named for both directions.
    SameTier {
        /// The tier's name for messages.
        tier: String,
    },
    /// The grid holds more cells than a two-tier sweep takes.
    TooManyCells {
        /// How many cells the grid holds (the current pair included).
        cells: usize,
        /// The most a two-tier sweep takes.
        max: usize,
    },
}

impl fmt::Display for Sweep2dError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Axis { error, .. } => fmt::Display::fmt(error, f),
            Self::SameTier { tier } => write!(
                f,
                "{tier} is named twice. Pick two different tiers to sweep together."
            ),
            Self::TooManyCells { cells, max } => write!(
                f,
                "That is {cells} angle pairs, and a two-tier sweep takes at most {max}. Use a \
                 larger step or a shorter range."
            ),
        }
    }
}

impl std::error::Error for Sweep2dError {}

/// The grid of a two-tier sweep, ready to run: see [`plan_sweep_2d`].
///
/// The columns are the angles of the first (across) tier, the rows the angles of the second
/// (down) tier, each from the flattest to the steepest, the current angle of each among
/// them. A cell is `(column, row)`; its position in the row-major order is
/// `row * columns + column`.
#[derive(Debug, Clone, PartialEq)]
pub struct Sweep2dPlan {
    /// The across tier.
    pub tier_a: usize,
    /// Its name for messages and files.
    pub tier_a_name: String,
    /// Its angle now, signed as the tier stores it.
    pub current_a_deg: f64,
    /// Its angles, signed as stored, ascending magnitude: the columns.
    pub angles_a: Vec<f64>,
    /// The position of the current angle in `angles_a`.
    pub current_col: usize,
    /// The down tier.
    pub tier_b: usize,
    /// Its name for messages and files.
    pub tier_b_name: String,
    /// Its angle now, signed as the tier stores it.
    pub current_b_deg: f64,
    /// Its angles, signed as stored, ascending magnitude: the rows.
    pub angles_b: Vec<f64>,
    /// The position of the current angle in `angles_b`.
    pub current_row: usize,
}

impl Sweep2dPlan {
    /// How many columns (angles of the across tier).
    #[must_use]
    pub const fn columns(&self) -> usize {
        self.angles_a.len()
    }

    /// How many rows (angles of the down tier).
    #[must_use]
    pub const fn rows(&self) -> usize {
        self.angles_b.len()
    }

    /// How many cells the grid holds.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.angles_a.len() * self.angles_b.len()
    }

    /// Whether the grid holds no cell.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The position of the cell `(column, row)` in the row-major order.
    #[must_use]
    pub const fn index_of(&self, column: usize, row: usize) -> usize {
        row * self.angles_a.len() + column
    }
}

/// One cell of a two-tier sweep: a pair of angles and its figures.
#[derive(Debug, Clone, PartialEq)]
pub struct Sweep2dCell {
    /// The column of the cell (its position in [`Sweep2dPlan::angles_a`]).
    pub col: usize,
    /// The row of the cell (its position in [`Sweep2dPlan::angles_b`]).
    pub row: usize,
    /// The across tier's angle, signed as the tier stores it.
    pub angle_a: f64,
    /// The down tier's angle, signed as the tier stores it.
    pub angle_b: f64,
    /// The figures; `None` when the pair does not give a stone (see `notes`).
    pub metrics: Option<RowMetrics>,
    /// Plain-English remarks: why the cell is invalid, or what to watch at this pair.
    pub notes: Vec<String>,
    /// Whether this is the design's own pair of angles.
    pub is_current: bool,
}

impl Sweep2dCell {
    /// Whether the pair gives a stone that was scored.
    #[must_use]
    pub const fn is_valid(&self) -> bool {
        self.metrics.is_some()
    }

    /// The notes as one line.
    #[must_use]
    pub fn notes_text(&self) -> String {
        self.notes.join("; ")
    }
}

/// The cells of a two-tier sweep, finished or stopped.
#[derive(Debug, Clone, PartialEq)]
pub struct Sweep2dOutcome {
    /// The grid that was run.
    pub plan: Sweep2dPlan,
    /// How many cells the plan held.
    pub requested: usize,
    /// Whether the sweep was stopped before every cell was done.
    pub cancelled: bool,
    /// Whether the cells carry tilt averages.
    pub tilt_average: bool,
    /// The finished cells in row-major order (a row of the down tier at a time, the
    /// flattest first). After a cancel it holds only the finished ones, still in order.
    pub cells: Vec<Sweep2dCell>,
}

impl Sweep2dOutcome {
    /// The cell at `(column, row)`, when it finished.
    #[must_use]
    pub fn cell_at(&self, column: usize, row: usize) -> Option<&Sweep2dCell> {
        let index = self.plan.index_of(column, row);
        // A complete outcome has the cell at its index; a cancelled one may have gaps.
        self.cells
            .get(index)
            .filter(|cell| cell.col == column && cell.row == row)
            .or_else(|| {
                self.cells
                    .iter()
                    .find(|cell| cell.col == column && cell.row == row)
            })
    }

    /// The cell of the design's own pair of angles, when it finished.
    #[must_use]
    pub fn current_cell(&self) -> Option<&Sweep2dCell> {
        self.cells.iter().find(|cell| cell.is_current)
    }

    /// How many finished cells are valid.
    #[must_use]
    pub fn valid_count(&self) -> usize {
        self.cells.iter().filter(|cell| cell.is_valid()).count()
    }

    /// The figures of every finished cell, in order, for [`super::best_flags_of`].
    #[must_use]
    pub fn metrics(&self) -> Vec<Option<RowMetrics>> {
        self.cells.iter().map(|cell| cell.metrics).collect()
    }
}

/// Works out the grid of a sweep of `tier_a` over `range_a` (the columns) and `tier_b` over
/// `range_b` (the rows). Never solves.
///
/// Each axis is [`plan_sweep`]'s, so the current pair of angles is always a cell. On top
/// of an axis's own refusals it refuses the same tier twice, and a grid of more than
/// [`MAX_SWEEP_CELLS`] cells. A tier whose angle follows a relation is refused as an axis
/// ([`SweepError::Driven`]), which covers one tier driving the other.
///
/// # Errors
///
/// [`Sweep2dError::SameTier`], [`Sweep2dError::Axis`] (carrying the axis's
/// [`SweepError`]) and [`Sweep2dError::TooManyCells`].
pub fn plan_sweep_2d(
    design: &Design,
    tier_a: usize,
    range_a: &SweepRange,
    tier_b: usize,
    range_b: &SweepRange,
) -> Result<Sweep2dPlan, Sweep2dError> {
    if tier_a == tier_b && design.tiers.get(tier_a).is_some() {
        return Err(Sweep2dError::SameTier {
            tier: crate::retarget::plan::tier_display_name(design, tier_a),
        });
    }
    let plan_a = plan_sweep(design, tier_a, *range_a).map_err(|error| Sweep2dError::Axis {
        across: true,
        error,
    })?;
    let plan_b = plan_sweep(design, tier_b, *range_b).map_err(|error| Sweep2dError::Axis {
        across: false,
        error,
    })?;
    let cells = plan_a.angles.len() * plan_b.angles.len();
    if cells > MAX_SWEEP_CELLS {
        return Err(Sweep2dError::TooManyCells {
            cells,
            max: MAX_SWEEP_CELLS,
        });
    }
    Ok(Sweep2dPlan {
        tier_a,
        tier_a_name: plan_a.tier_name,
        current_a_deg: plan_a.current_deg,
        angles_a: plan_a.angles,
        current_col: plan_a.current_row,
        tier_b,
        tier_b_name: plan_b.tier_name,
        current_b_deg: plan_b.current_deg,
        angles_b: plan_b.angles,
        current_row: plan_b.current_row,
    })
}

/// A finer grid around the cell `(column, row)` of `plan`, with about the same number of cells.
///
/// Each axis is centered on the cell's angle, its step is the local step of the plan (the
/// distance to the nearer neighbour) divided by `factor`, and the range is kept between the
/// flattest and the steepest angle a sweep takes. The design's own pair stays a cell (an axis
/// may gain one angle for it).
///
/// An axis with a single angle stays a single angle.
///
/// # Errors
///
/// [`Sweep2dError::Axis`] with [`SweepError::StepNotPositive`] for a `factor` that is not
/// more than 0, the axis's own refusals if the tier cannot be swept any more, and
/// [`Sweep2dError::TooManyCells`] when the extra current angles push the grid over
/// [`MAX_SWEEP_CELLS`].
pub fn refine_around(
    plan: &Sweep2dPlan,
    design: &Design,
    cell: (usize, usize),
    factor: f64,
) -> Result<Sweep2dPlan, Sweep2dError> {
    if !(factor.is_finite() && factor > 0.0) {
        return Err(Sweep2dError::Axis {
            across: true,
            error: SweepError::StepNotPositive,
        });
    }
    let range_a = refined_range(&plan.angles_a, cell.0, factor);
    let range_b = refined_range(&plan.angles_b, cell.1, factor);
    plan_sweep_2d(design, plan.tier_a, &range_a, plan.tier_b, &range_b)
}

/// The range of one axis of a finer grid around `angles[at]` (see [`refine_around`]).
fn refined_range(angles: &[f64], at: usize, factor: f64) -> SweepRange {
    let at = at.min(angles.len().saturating_sub(1));
    let centre = angles
        .get(at)
        .map_or(MIN_SWEEP_ANGLE_DEG, |angle| angle.abs());
    let count = angles.len();
    if count < 2 {
        return SweepRange {
            from_deg: centre,
            to_deg: centre,
            step_deg: 1.0,
        };
    }
    let before = (at > 0).then(|| (angles[at].abs() - angles[at - 1].abs()).abs());
    let after = (at + 1 < count).then(|| (angles[at + 1].abs() - angles[at].abs()).abs());
    let local = match (before, after) {
        (Some(b), Some(a)) => b.min(a),
        (Some(only), None) | (None, Some(only)) => only,
        (None, None) => 1.0,
    };
    let step = snap_noise(local / factor);
    let span = step * (count - 1) as f64;
    let mut low = centre - span / 2.0;
    let mut high = centre + span / 2.0;
    // Keep the whole range inside what a sweep takes, sliding it rather than cutting it.
    if low < MIN_SWEEP_ANGLE_DEG {
        high += MIN_SWEEP_ANGLE_DEG - low;
        low = MIN_SWEEP_ANGLE_DEG;
    }
    if high > MAX_SWEEP_ANGLE_DEG {
        low -= high - MAX_SWEEP_ANGLE_DEG;
        high = MAX_SWEEP_ANGLE_DEG;
    }
    SweepRange {
        from_deg: snap_noise(low.max(MIN_SWEEP_ANGLE_DEG)),
        to_deg: snap_noise(high.min(MAX_SWEEP_ANGLE_DEG)),
        step_deg: step,
    }
}

/// What the workers share while a grid runs.
struct Shared<'a> {
    design: &'a Design,
    plan: &'a Sweep2dPlan,
    scene: &'a SweepScene<'a>,
    options: &'a SweepOptions,
    /// The rough's volume, for the yield.
    preform_volume: Option<f64>,
    cancel: &'a AtomicBool,
    progress: &'a (dyn Fn(usize, usize) + Sync),
    /// The next cell (row-major index) to claim.
    next: AtomicUsize,
    /// How many cells are finished.
    done: AtomicUsize,
}

/// Sweeps the grid of `plan`: every cell is solved and scored on its own clone of
/// `design`.
///
/// The cells are in row-major order (rows are the down tier's angles, columns the across
/// tier's) whatever `options.workers` is, and a cell is a pure function of its pair of
/// angles, so the result does not depend on the number of workers. `progress(done, total)`
/// is called after each finished cell, from whichever worker finished it. Setting `cancel`
/// stops the sweep: the cells finished so far come back with
/// [`Sweep2dOutcome::cancelled`] set. With `options.tilt_average` each cell's tilt average
/// runs on its worker's thread, so the cell workers are the only pool.
#[must_use]
pub fn sweep_two_tier_angles(
    design: &Design,
    plan: &Sweep2dPlan,
    options: &SweepOptions,
    scene: &SweepScene<'_>,
    cancel: &AtomicBool,
    progress: &(dyn Fn(usize, usize) + Sync),
) -> Sweep2dOutcome {
    let shared = Shared {
        design,
        plan,
        scene,
        options,
        preform_volume: measure_solid(&design.preform.planes()).map(|solid| solid.volume),
        cancel,
        progress,
        next: AtomicUsize::new(0),
        done: AtomicUsize::new(0),
    };
    let workers = options.workers.clamp(1, plan.len().max(1));
    let mut found = run_workers(&shared, workers);
    found.sort_by_key(|(index, _)| *index);
    let mut cells: Vec<Sweep2dCell> = found.into_iter().map(|(_, cell)| cell).collect();
    note_lost_girdles(&mut cells);
    Sweep2dOutcome {
        plan: plan.clone(),
        requested: plan.len(),
        cancelled: cells.len() < plan.len(),
        tilt_average: options.tilt_average,
        cells,
    }
}

/// Runs the claim loop on `workers` scoped threads and gathers what they finished.
#[cfg(not(target_arch = "wasm32"))]
fn run_workers(shared: &Shared<'_>, workers: usize) -> Vec<(usize, Sweep2dCell)> {
    if workers <= 1 {
        return claim_cells(shared);
    }
    std::thread::scope(|scope| {
        let mut handles = Vec::with_capacity(workers);
        for _ in 0..workers {
            handles.push(scope.spawn(|| claim_cells(shared)));
        }
        let mut found = Vec::new();
        for handle in handles {
            found.extend(handle.join().expect("a sweep worker must not panic"));
        }
        found
    })
}

/// The `wasm32` arm of [`run_workers`]: no thread to spawn, the same loop inline.
#[cfg(target_arch = "wasm32")]
fn run_workers(shared: &Shared<'_>, _workers: usize) -> Vec<(usize, Sweep2dCell)> {
    claim_cells(shared)
}

/// One worker: claims the next cell until none is left or the sweep is cancelled.
fn claim_cells(shared: &Shared<'_>) -> Vec<(usize, Sweep2dCell)> {
    let total = shared.plan.len();
    let mut finished = Vec::new();
    loop {
        if shared.cancel.load(Ordering::Relaxed) {
            break;
        }
        let index = shared.next.fetch_add(1, Ordering::Relaxed);
        if index >= total {
            break;
        }
        let Some(cell) = evaluate_cell(shared, index) else {
            break;
        };
        finished.push((index, cell));
        let done = shared.done.fetch_add(1, Ordering::Relaxed) + 1;
        (shared.progress)(done, total);
    }
    finished
}

/// Solves and scores one cell on a clone of the design; `None` when cancelled.
fn evaluate_cell(shared: &Shared<'_>, index: usize) -> Option<Sweep2dCell> {
    let plan = shared.plan;
    let col = index % plan.columns();
    let row = index / plan.columns();
    let mut cell = Sweep2dCell {
        col,
        row,
        angle_a: plan.angles_a[col],
        angle_b: plan.angles_b[row],
        metrics: None,
        notes: Vec::new(),
        is_current: col == plan.current_col && row == plan.current_row,
    };
    let mut scratch = shared.design.clone();
    // Both angles are written before the relations are evaluated, so a relation that
    // reads both tiers never sees a half-moved pair.
    if let Err(message) = set_angles(
        &mut scratch,
        &[(plan.tier_a, cell.angle_a), (plan.tier_b, cell.angle_b)],
    ) {
        cell.notes.push(message);
        return Some(cell);
    }
    match score_clone(
        &scratch,
        shared.scene,
        shared.options,
        shared.preform_volume,
        Some(1),
        shared.cancel,
        &mut cell.notes,
    ) {
        Ok(metrics) => {
            cell.metrics = metrics;
            Some(cell)
        }
        Err(Cancelled) => None,
    }
}

/// Sets the swept tiers' angles on the clone and lets every tier whose angle follows a
/// relation follow, as a real edit does: `rows::set_swept_angle` for several
/// tiers at once (the relations are evaluated once, after every angle is written). The
/// error is the relation's plain-English message.
fn set_angles(scratch: &mut Design, angles: &[(usize, f64)]) -> Result<(), String> {
    for &(tier, angle_deg) in angles {
        if let Some(swept) = scratch.tiers.get_mut(tier) {
            swept.angle_deg = angle_deg;
        }
    }
    let updates = scratch
        .evaluate_relations()
        .map_err(|error| error.to_string())?;
    for (position, deg) in updates {
        if let Some(driven) = scratch.tiers.get_mut(position) {
            driven.angle_deg = deg;
        }
    }
    Ok(())
}

/// Says "the girdle band is gone" on every valid cell that lost the band the current
/// cell has (the one-tier sweep's rule).
fn note_lost_girdles(cells: &mut [Sweep2dCell]) {
    let had_girdle = cells
        .iter()
        .find(|cell| cell.is_current)
        .and_then(|cell| cell.metrics)
        .is_some_and(|metrics| metrics.has_girdle);
    if !had_girdle {
        return;
    }
    for cell in cells.iter_mut().filter(|cell| !cell.is_current) {
        if cell.metrics.is_some_and(|metrics| !metrics.has_girdle) {
            cell.notes
                .push("The girdle band is gone at this angle.".to_owned());
        }
    }
}

/// Makes the two angles the real angles of their tiers, as ONE undo step.
///
/// The tiers whose angles follow a relation to them follow in the same step. `a` and `b` are
/// `(tier, new_angle)`. `Ok(None)` when both tiers already have the angles.
///
/// # Errors
///
/// [`SessionEditError::Edit`] for a tier the design does not have, or the same tier twice;
/// [`SessionEditError::Driven`] if a tier has come to follow a relation since the sweep
/// ran; [`SessionEditError::Relation`] when a relation cannot be satisfied at the angles.
/// The design is untouched on `Err`.
pub fn apply_sweep_angles(
    session: &mut EditorSession,
    a: (usize, f64),
    b: (usize, f64),
) -> Result<Option<EditChange>, SessionEditError> {
    let tier_count = session.design.tiers.len();
    let mut changes = Vec::with_capacity(2);
    for (tier, new_deg) in [a, b] {
        let Some(old_deg) = session.design.tiers.get(tier).map(|t| t.angle_deg) else {
            return Err(SessionEditError::Edit(EditError {
                index: tier,
                tier_count,
            }));
        };
        if (old_deg - new_deg).abs() >= 1e-9 {
            changes.push((tier, old_deg, new_deg));
        }
    }
    if a.0 == b.0 {
        return Err(SessionEditError::Edit(EditError {
            index: a.0,
            tier_count,
        }));
    }
    if changes.is_empty() {
        return Ok(None);
    }
    session
        .try_apply(Edit::RetargetAngles { changes })
        .map(Some)
}

/// A rough guess of how long a two-tier sweep of `cells` cells takes, in seconds: the
/// one-tier estimate with a cell for a row, and the workers credited the same way.
#[must_use]
pub fn estimate_seconds_2d(
    cells: usize,
    tilt_average: bool,
    workers: usize,
    cost: SolveCostEstimate,
) -> f64 {
    estimate_seconds(cells, tilt_average, workers, cost)
}

/// The cells of `outcome` as CSV text.
///
/// A header, then one line per finished cell in row-major order (the down tier's angle changes
/// slowest). Angles are magnitudes, like the table; the format is the one-tier CSV's (point
/// decimals, four decimals, empty field for a missing figure, CRLF line ends). The tilt-average
/// columns are there only when the sweep asked for them.
#[must_use]
pub fn sweep2d_csv(outcome: &Sweep2dOutcome) -> String {
    let metrics: Vec<SweepMetric> = SweepMetric::ALL
        .into_iter()
        .filter(|metric| outcome.tilt_average || !metric.is_tilt())
        .collect();
    let mut header: Vec<&str> = vec![
        "tier_a",
        "tier_b",
        "angle_a_deg",
        "angle_b_deg",
        "current",
        "valid",
    ];
    header.extend(metrics.iter().map(|metric| metric.csv_name()));
    header.push("facet_warnings");
    header.push("notes");
    let mut text = header.join(",");
    text.push_str(LINE_END);
    for cell in &outcome.cells {
        let mut fields = vec![
            escape(&outcome.plan.tier_a_name),
            escape(&outcome.plan.tier_b_name),
            format!("{:.4}", cell.angle_a.abs()),
            format!("{:.4}", cell.angle_b.abs()),
            yes_no(cell.is_current),
            yes_no(cell.is_valid()),
        ];
        fields.extend(
            metrics
                .iter()
                .map(|metric| number(metric.value_of(cell.metrics.as_ref()))),
        );
        fields.push(
            cell.metrics
                .map_or_else(String::new, |figures| figures.warning_count.to_string()),
        );
        fields.push(escape(&cell.notes_text()));
        text.push_str(&fields.join(","));
        text.push_str(LINE_END);
    }
    text
}
