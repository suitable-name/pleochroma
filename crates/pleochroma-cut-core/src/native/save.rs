//! [`catalogue_asc`] builds the catalogue `.asc` text that a "Save" action writes, with its
//! [`SaveError`] and [`CatalogueAsc`] types.
//!
//! The design file itself is built by [`super::design_to_string`]. The parent module describes
//! when the original `.asc` text is kept.

use super::convert::SaveExtras;
use crate::design::{Design, DesignSolveError};
use pleochroma::{
    geometry::meet_solver::{MeetConstraint, SolveStrategy, SolvedTier},
    optics::materials::GemMaterial,
};
use pleochroma_formats::asc::{AscSchedule, AscTier};
use std::fmt;

/// Why [`catalogue_asc`] could not produce the `.asc` text.
///
/// Not raised for `design` not currently solving -- see [`catalogue_asc`]'s doc
/// comment: that case falls through to a draft ([`CatalogueAsc::draft_reason`])
/// instead of an error.
#[derive(Debug)]
pub enum SaveError {
    /// The `.asc` text could not be written: a header, footnote, or notes
    /// string would not survive a `.asc` write/re-parse round trip unchanged (a
    /// tier name never fails this way -- see
    /// [`pleochroma_formats::asc::asc_safe_tier_name`]). See
    /// [`pleochroma_formats::asc::AscWriteError`].
    AscWrite(pleochroma_formats::asc::AscWriteError),
}

impl fmt::Display for SaveError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AscWrite(e) => write!(f, "cannot save: {e}"),
        }
    }
}

impl std::error::Error for SaveError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::AscWrite(e) => Some(e),
        }
    }
}

/// Why [`catalogue_asc`] wrote a draft instead of an ordinary `.asc` -- see
/// [`CatalogueAsc::draft_reason`].
#[derive(Debug)]
pub enum DraftReason {
    /// `design.solve()` failed: at least one crown/pavilion/girdle block has no
    /// explicit scale-reference tier (so its masts are genuinely undetermined), or
    /// a [`crate::design::TierTarget`] could not be resolved. See
    /// [`DesignSolveError`].
    Unsolved(DesignSolveError),
    /// `design.tiers` is empty. `to_asc_schedule` would succeed with zero facet records, but
    /// `pleochroma_formats::asc::parse_asc` rejects an `.asc` with no facet (`a`) records. The
    /// `.asc` is therefore a placeholder with one dummy facet. The design file holds the empty
    /// tier list.
    NoTiers,
    /// `design.solve()` returned `Ok`, but a tier that is not a
    /// [`pleochroma::geometry::meet_solver::MeetConstraint::ScaleReference`] came back
    /// [`SolveStrategy::Failed`]. The over-`MAX_PLANES` fallback in `Design::solve` returns `Ok`
    /// with fabricated masts instead of an error, so the solved masts are checked here. Treated
    /// as a draft, like [`Self::Unsolved`].
    TooManyPlanes,
}

impl fmt::Display for DraftReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Unsolved(e) => write!(f, "unsolved: {e}"),
            Self::NoTiers => write!(
                f,
                "no tiers yet -- add at least one (with a scale-reference anchor) before this \
                 file can be reopened"
            ),
            Self::TooManyPlanes => write!(
                f,
                "too many planes for the solver to verify -- the masts below are unsolved \
                 placeholders, not real cutting instructions"
            ),
        }
    }
}

impl std::error::Error for DraftReason {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Unsolved(e) => Some(e),
            Self::NoTiers | Self::TooManyPlanes => None,
        }
    }
}

/// `true` iff `solved` (a [`Design::solve`] result already known to be `Ok`) has a tier that
/// is not a [`pleochroma::geometry::meet_solver::MeetConstraint::ScaleReference`] but came
/// back [`SolveStrategy::Failed`] (see [`DraftReason::TooManyPlanes`]). A `ScaleReference`
/// tier is excluded, because its mast is authored, not solved.
#[must_use]
fn is_too_many_planes(design: &Design, solved: &[SolvedTier]) -> bool {
    design.tiers.iter().zip(solved).any(|(tier, solved_tier)| {
        !matches!(tier.constraint, MeetConstraint::ScaleReference(_))
            && matches!(solved_tier.strategy, SolveStrategy::Failed)
    })
}

/// Everything [`catalogue_asc`] produced: the cutting-instructions text of the design
/// and whether it is a draft.
///
/// The `.asc` text is the catalogue copy a "Save" stores next to the design file (the
/// design file itself, see [`super::design_to_string`], is the source of truth and
/// carries the whole design).
#[derive(Debug)]
pub struct CatalogueAsc {
    /// Text of the `.asc` file.
    pub asc_text: String,
    /// `true` iff `asc_text` is `original_asc_text` unchanged, `false` iff it was regenerated
    /// (see [`catalogue_asc`]). Lets a caller report whether the schedule changed. Always
    /// `false` for a draft, whose placeholder text is never the caller's own.
    pub asc_preserved: bool,
    /// `Some(reason)` iff a draft was written instead of an ordinary `.asc`, because `design`
    /// does not solve or has no tiers. `asc_text` is then placeholder text, and the design
    /// file's `draft` flag ([`pleochroma_formats::native::design::DesignFile::draft`]) should
    /// be set. `None` for an ordinary `.asc`.
    pub draft_reason: Option<DraftReason>,
}

/// Builds the catalogue `.asc` text a "Save" action should store.
///
/// `original_asc_text`, when given, is kept verbatim in `asc_text` while `design`'s schedule
/// still equals what that text parses to (see the parent module). Text that fails to parse,
/// or is absent, gets a fresh export.
///
/// `placeholder_note`, when `Some`, marks the `.asc` as derived via
/// [`pleochroma_formats::asc::mark_reconstructed`]. Use it for a design built from a
/// placeholder, such as a catalogue entry with only an angle table and fabricated `0.0`
/// masts, so the output is never mistaken for verified cutting instructions. It also skips
/// the preserve path, since such masts are never worth keeping.
///
/// # Unsolved or tier-less designs: drafts
///
/// A design that does not solve (for example a block without its
/// [`pleochroma::geometry::meet_solver::MeetConstraint::ScaleReference`] tier) or has no tiers
/// gives a draft, not an error. [`CatalogueAsc::draft_reason`] is then `Some`, and `asc_text`
/// is a placeholder: real angles and indices with unsolved masts ([`draft_asc_schedule`]), or
/// one dummy facet for a tier-less design ([`draft_asc_schedule_for_no_tiers`]). Neither is a
/// cut instruction.
///
/// # Errors
///
/// [`SaveError::AscWrite`] if the schedule cannot be written as `.asc` text.
pub fn catalogue_asc(
    design: &Design,
    original_asc_text: Option<&str>,
    placeholder_note: Option<&str>,
) -> Result<CatalogueAsc, SaveError> {
    catalogue_asc_extended(
        design,
        original_asc_text,
        placeholder_note,
        &SaveExtras::default(),
    )
}

/// Like [`catalogue_asc`], but also takes [`SaveExtras`].
///
/// Only `extras.custom_catalogue` matters here: it is consulted for the written `I` line.
/// With `SaveExtras::default()` the output equals [`catalogue_asc`]'s built-ins-only output.
///
/// # Errors
///
/// Same as [`catalogue_asc`].
pub fn catalogue_asc_extended(
    design: &Design,
    original_asc_text: Option<&str>,
    placeholder_note: Option<&str>,
    extras: &SaveExtras<'_>,
) -> Result<CatalogueAsc, SaveError> {
    if design.tiers.is_empty() {
        return draft_asc(
            &draft_asc_schedule_for_no_tiers(design, extras.custom_catalogue),
            DraftReason::NoTiers,
        );
    }

    match design.solve() {
        Ok(solved) if is_too_many_planes(design, &solved) => draft_asc(
            &draft_asc_schedule(design, original_asc_text, extras.custom_catalogue),
            DraftReason::TooManyPlanes,
        ),
        Ok(solved) => catalogue_asc_extended_from_solved(
            design,
            &solved,
            original_asc_text,
            placeholder_note,
            extras,
        ),
        Err(missing_anchor) => draft_asc(
            &draft_asc_schedule(design, original_asc_text, extras.custom_catalogue),
            DraftReason::Unsolved(missing_anchor),
        ),
    }
}

/// [`catalogue_asc_extended`]'s "design solves" branch.
///
/// Takes an already solved mast list (from [`Design::solve`] or [`Design::resolve_dirty`]),
/// so a caller that has solved already does not pay for a second solve. Writes the schedule
/// with cheater offsets folded in ([`Design::to_asc_schedule_from_solved_with_cheater_offsets`])
/// and the custom catalogue for the `I` line. The placeholder marking and the preserve test
/// are the same as in [`catalogue_asc_extended`]. The draft branches stay there.
///
/// # Panics
///
/// `solved` must have one entry per tier `design` currently has, in the same order
/// -- see [`Design::to_asc_schedule_from_solved`]'s own panic contract, which this
/// inherits unchanged.
///
/// # Errors
///
/// Same as [`catalogue_asc_extended`].
pub fn catalogue_asc_extended_from_solved(
    design: &Design,
    solved: &[SolvedTier],
    original_asc_text: Option<&str>,
    placeholder_note: Option<&str>,
    extras: &SaveExtras<'_>,
) -> Result<CatalogueAsc, SaveError> {
    // Only this file-writing path folds an authored cheater offset into the exported indices.
    // The plain `_with` variant used for rendering and measurement does not.
    let mut current_schedule =
        design.to_asc_schedule_from_solved_with_cheater_offsets(solved, extras.custom_catalogue);
    // `.asc` cannot hold a concave tier: it is carried as two footnotes per tier
    // (and, losslessly, in the design file) instead.
    design.append_concave_footnotes(&mut current_schedule);

    if let Some(note) = placeholder_note {
        pleochroma_formats::asc::mark_reconstructed(&mut current_schedule, note);
        let asc_text = pleochroma_formats::asc::to_asc_string(&current_schedule)
            .map_err(SaveError::AscWrite)?;
        return Ok(CatalogueAsc {
            asc_text,
            asc_preserved: false,
            draft_reason: None,
        });
    }

    let preserved = original_asc_text.and_then(|original| {
        let original_schedule = pleochroma_formats::asc::parse_asc(original).ok()?;
        schedules_equal_ignoring_refractive_index(&original_schedule, &current_schedule)
            .then(|| original.to_string())
    });

    let (asc_text, asc_preserved) = match preserved {
        Some(original) => (original, true),
        None => (
            pleochroma_formats::asc::to_asc_string(&current_schedule)
                .map_err(SaveError::AscWrite)?,
            false,
        ),
    };

    Ok(CatalogueAsc {
        asc_text,
        asc_preserved,
        draft_reason: None,
    })
}

/// `true` iff `original` and `current` are equal once `current`'s `refractive_index` is
/// replaced by `original`'s. This is the preserve test of [`catalogue_asc_extended`]. Picking
/// another material changes the effective `I` line alone, and must not force a rewrite of a
/// hand-authored file. The design file's `[material]` table keeps the real selection.
#[must_use]
fn schedules_equal_ignoring_refractive_index(
    original: &AscSchedule,
    current: &AscSchedule,
) -> bool {
    // The `.asc` text holds the writer's sanitised tier names (`asc_safe_tier_name`: "Crown
    // Main" is written as `Crown_Main`), and the parser's `warnings` describe that text. Both
    // are normalised to the original's form. So are `line_ending` (an untouched LF file must
    // still count as untouched) and each tier's `index_names`, which an export leaves empty.
    let mut masked_current = AscSchedule {
        refractive_index: original.refractive_index,
        warnings: original.warnings.clone(),
        line_ending: original.line_ending,
        ..current.clone()
    };
    for (tier, orig) in masked_current.tiers.iter_mut().zip(&original.tiers) {
        let safe = pleochroma_formats::asc::asc_safe_tier_name(&tier.name);
        if safe != tier.name {
            tier.name = safe.into_owned();
        }
        if tier.index_names.is_empty() {
            tier.index_names.clone_from(&orig.index_names);
        }
    }
    *original == masked_current
}

/// The shared tail of [`catalogue_asc_extended`]'s draft paths (unsolved, tier-less,
/// too-many-planes). Builds `asc_text` from `draft_schedule`, marked with `reason`'s message
/// via [`pleochroma_formats::asc::mark_reconstructed`], so the `.asc` reads as a placeholder
/// even to software that does not know the design file's `draft` flag.
fn draft_asc(draft_schedule: &AscSchedule, reason: DraftReason) -> Result<CatalogueAsc, SaveError> {
    let mut schedule = draft_schedule.clone();
    pleochroma_formats::asc::mark_reconstructed(&mut schedule, &reason.to_string());
    let asc_text =
        pleochroma_formats::asc::to_asc_string(&schedule).map_err(SaveError::AscWrite)?;
    Ok(CatalogueAsc {
        asc_text,
        asc_preserved: false,
        draft_reason: Some(reason),
    })
}

/// The placeholder mast a draft writes for a tier with no better number (see
/// [`draft_asc_schedule`]). It is not a real dimension. It is always paired with
/// [`SolveStrategy::Failed`], whose mast should not be trusted.
const DRAFT_PLACEHOLDER_MAST: f64 = 1.0;

/// Builds the placeholder [`AscSchedule`] a draft writes as `asc_text`. Angles, names,
/// indices and index-wheel positions are real. Only the masts, and the notes derived from
/// them, are not to be trusted. A tier takes its mast from `original_asc_text` at the same
/// position, when that text parses and has `design`'s current tier count
/// ([`Design::to_asc_schedule_from_solved`] needs the same alignment). Otherwise it gets
/// [`DRAFT_PLACEHOLDER_MAST`], for example a tier added after its block lost its anchor.
///
/// `custom` is consulted for the `I` line as in [`Design::to_asc_schedule_from_solved_with`].
fn draft_asc_schedule(
    design: &Design,
    original_asc_text: Option<&str>,
    custom: &[GemMaterial],
) -> AscSchedule {
    let original_masts = original_asc_text
        .and_then(|text| pleochroma_formats::asc::parse_asc(text).ok())
        .filter(|schedule| schedule.tiers.len() == design.tiers.len())
        .map(|schedule| {
            schedule
                .tiers
                .into_iter()
                .map(|tier| tier.mast)
                .collect::<Vec<_>>()
        });

    let placeholder_solved: Vec<SolvedTier> = (0..design.tiers.len())
        .map(|i| SolvedTier {
            mast: original_masts
                .as_ref()
                .and_then(|masts| masts.get(i).copied())
                .unwrap_or(DRAFT_PLACEHOLDER_MAST),
            strategy: SolveStrategy::Failed,
            detail: "draft save: design does not currently solve".to_string(),
        })
        .collect();

    let mut schedule = design.to_asc_schedule_from_solved_with(&placeholder_solved, custom);
    design.append_concave_footnotes(&mut schedule);
    schedule
}

/// The name of the one dummy facet record [`draft_asc_schedule_for_no_tiers`] writes.
const NO_TIERS_PLACEHOLDER_NAME: &str = "placeholder";

/// Builds the placeholder [`AscSchedule`] a tier-less draft writes as `asc_text`. There are
/// no tiers to carry over. The one dummy facet exists only so that
/// `pleochroma_formats::asc::parse_asc`, which refuses a file with no facet records, can read
/// the text.
///
/// `custom` is consulted for the `I` line as in [`Design::effective_refractive_index_with`].
fn draft_asc_schedule_for_no_tiers(design: &Design, custom: &[GemMaterial]) -> AscSchedule {
    let mut schedule = AscSchedule {
        gemcad_version: design.meta.gemcad_version.clone(),
        gear_teeth: design.meta.gear_teeth,
        gear_reference_angle: design.meta.gear_reference_angle,
        symmetry_order: design.meta.symmetry_order,
        mirror: design.meta.mirror,
        refractive_index: design.effective_refractive_index_with(custom),
        headers: design.meta.headers.clone(),
        footnotes: design.meta.footnotes.clone(),
        tiers: vec![AscTier {
            angle_deg: 0.0,
            mast: DRAFT_PLACEHOLDER_MAST,
            name: NO_TIERS_PLACEHOLDER_NAME.to_string(),
            indices: Vec::new(),
            index_names: Vec::new(),
            notes: "draft save: design has no tiers yet".to_string(),
        }],
        warnings: Vec::new(),
        line_ending: pleochroma_formats::asc::AscLineEnding::default(),
    };
    design.append_concave_footnotes(&mut schedule);
    schedule
}
