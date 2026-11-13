//! Parses the tier-edit form's text/enum fields into a
//! [`pleochroma_cut_core::ConstraintTier`] and a [`TierTarget`].
//!
//! It also reads the shorthand index notations: colon-separated arithmetic sequences and
//! the orbit "xN" fold-count suffix.

use super::number_expr::{NameValues, eval_number};
use pleochroma::geometry::meet_solver::MeetConstraint;
use pleochroma_cut_core::{ConstraintTier, TierTarget};

/// Parses just the angle text an inline tier-list cell commits (`inline_set_angle`).
///
/// It applies the same validation as the angle field of [`parse_tier_form`], without the
/// other fields a whole form would need.
///
/// # Errors
///
/// A message when `angle` is not a finite number, or its magnitude exceeds 90
/// degrees. The text may be arithmetic over constants (`41 + 0.5`); see
/// [`eval_number`].
pub fn parse_angle_only(angle: &str) -> Result<f64, String> {
    parse_angle_only_with_names(angle, None)
}

/// [`parse_angle_only`], where the text may also name tiers through `names` (`P1 - 2`
/// with the angle magnitude of the tier called `P1`).
///
/// # Errors
///
/// As [`parse_angle_only`]; a name `names` does not know is reported as "there is no
/// tier called 'X'".
pub fn parse_angle_only_with_names(
    angle: &str,
    names: Option<NameValues<'_>>,
) -> Result<f64, String> {
    let angle_deg = eval_number(angle, names).map_err(|error| error.message("Angle", angle))?;
    if !angle_deg.is_finite() {
        return Err("Angle must be a finite number.".to_string());
    }
    reject_angle_over_90(angle_deg)?;
    Ok(angle_deg)
}

/// Parses `token` as the `start:step:stop` sequence shorthand (e.g. `"0:12:96"`).
///
/// It generates every `start + k*step` strictly before `stop`, wrapped modulo
/// `gear_teeth_abs` so a sequence running past the gear still lands on real positions.
/// The caller's finite, range and duplicate checks still run on each value.
///
/// Returns `None` when `token` is not three colon-separated finite numbers, or `step` is
/// zero (the sequence would never end). The caller then reads `token` as an ordinary index
/// and reports its own "not a number" error.
fn parse_colon_sequence(token: &str, gear_teeth_abs_f64: f64) -> Option<Vec<f64>> {
    let parts: Vec<&str> = token.split(':').collect();
    let [start, step, end] = parts.as_slice() else {
        return None;
    };
    let start: f64 = start.trim().parse().ok()?;
    let step: f64 = step.trim().parse().ok()?;
    let end: f64 = end.trim().parse().ok()?;
    if step == 0.0 || !(start.is_finite() && step.is_finite() && end.is_finite()) {
        return None;
    }
    let mut values = Vec::new();
    let mut current = start;
    // Bounded rather than a `while` run to `stop`: a pathological step (say
    // `1e-9`) must still terminate rather than hang the UI thread. No real
    // index gear has anywhere near this many teeth, so the bound is never
    // reached by a legitimate sequence.
    for _ in 0..10_000 {
        if (step > 0.0 && current >= end) || (step < 0.0 && current <= end) {
            break;
        }
        values.push(if gear_teeth_abs_f64 > 0.0 {
            current.rem_euclid(gear_teeth_abs_f64)
        } else {
            current
        });
        current += step;
    }
    Some(values)
}

/// Parses `suffix` as the "xN" fold-count half of the "N xM" orbit shorthand.
///
/// `"12 x8"` is the notation the ORBIT column shows for a complete family (see the "Orbits"
/// section of `docs/manual/03-loading-and-tier-list.md`). Returns the fold count when
/// `suffix` is `x`/`X` followed by digits naming a positive count, `None` otherwise. The
/// caller then reads both tokens as ordinary indices, which reports a normal "not a number"
/// error.
fn parse_orbit_suffix(suffix: &str) -> Option<u32> {
    let digits = suffix.strip_prefix(['x', 'X'])?;
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u32>().ok().filter(|&n| n > 0)
}

/// Expands `base` into `fold_count` evenly spaced index-wheel positions.
///
/// The positions are `base + k * (gear_teeth_abs / fold_count)` for `k` in `0..fold_count`,
/// wrapped modulo `gear_teeth_abs`: the other half of [`parse_orbit_suffix`]'s "12 x8"
/// shorthand. A `fold_count` that does not divide the gear evenly gives fractional
/// positions, which the non-integral index warning flags.
fn expand_orbit_shorthand(base: f64, fold_count: u32, gear_teeth_abs_f64: f64) -> Vec<f64> {
    if gear_teeth_abs_f64 <= 0.0 {
        return vec![base];
    }
    let step = gear_teeth_abs_f64 / f64::from(fold_count);
    (0..fold_count)
        .map(|k| f64::mul_add(f64::from(k), step, base).rem_euclid(gear_teeth_abs_f64))
        .collect()
}

/// Folds an index onto the ring `0..gear_teeth_abs_f64`, so the gear's own tooth count
/// (96 on a 96-tooth gear) and `0` name the same position.
///
/// A gear of zero teeth (nonsensical, but not this function's business to reject)
/// leaves the value as it is.
fn wheel_position(value: f64, gear_teeth_abs_f64: f64) -> f64 {
    if gear_teeth_abs_f64 > 0.0 {
        value.rem_euclid(gear_teeth_abs_f64)
    } else {
        value
    }
}

/// Refuses a magnitude beyond 90 degrees.
///
/// No crown or pavilion facet is steeper than a right angle, since angles are measured from
/// the girdle plane. Checking here refuses a mistyped digit (`410` for `41.0`) at entry.
fn reject_angle_over_90(angle_deg: f64) -> Result<(), String> {
    if angle_deg.abs() > 90.0 {
        // The message reads the magnitude (`-91` and `91` both read `91.00`): the side of the
        // girdle is the block's, not a sign.
        return Err(format!(
            "Angle {:.2}\u{b0} exceeds 90\u{b0} -- angles are measured from the girdle \
             plane, so no crown or pavilion facet can be steeper than that.",
            angle_deg.abs()
        ));
    }
    Ok(())
}

/// [`parse_tier_form`]'s bundled input.
///
/// The form's text and enum fields (`angle`, `constraint_kind`, `constraint_text`, `name`,
/// `indices`), the design's `gear_teeth_abs` that `indices` is checked against, the carried
/// `imported_meet` and `original_notes` an existing row keeps across a save, and the other
/// tiers' names. A struct keeps [`parse_tier_form`] under clippy's argument limit.
pub struct TierFormFields<'a> {
    /// The angle text field; must parse as a finite `f64`.
    pub angle: &'a str,
    /// `EditorTierItem`'s three-way constraint-kind encoding -- see
    /// [`parse_tier_form`]'s own doc comment for the `0`/`1`/`2` mapping.
    pub constraint_kind: i32,
    /// The constraint text field, meaning depending on `constraint_kind`.
    pub constraint_text: &'a str,
    /// The tier's name field.
    pub name: &'a str,
    /// The comma/space/semicolon-separated index list text field.
    pub indices: &'a str,
    /// The design's own index-wheel tooth count, used to validate `indices`.
    pub gear_teeth_abs: u32,
    /// The tier's current `imported_meet`, threaded straight through unchanged.
    pub imported_meet: Option<MeetConstraint>,
    /// The tier's current `original_notes`, threaded straight through unchanged.
    pub original_notes: Option<String>,
    /// The `/`-split name tokens of every OTHER tier (see `ConstraintTier::names()`), with
    /// the tier being saved excluded. The new name is checked against them case-insensitively:
    /// `MeetNameResolver::name_match` binds a `MeetNamed` reference to the FIRST tier bearing
    /// a name, so an undetected clash silently redirects another tier's constraint.
    ///
    /// This checks only the tier being saved. Other tiers that name a RENAMED tier are
    /// rewritten by `crate::tier_save::tier_save_edit`, and
    /// `crate::tier_save::unnaming_blocked_message` refuses to leave a met tier unnamed.
    pub other_tier_names: Vec<String>,
}

/// Parses a tier form's index field into validated index-wheel positions.
///
/// Accepts a comma/space/semicolon separated list whose tokens may each be a plain number,
/// a `start:step:stop` arithmetic sequence, or a `base xN` orbit shorthand (see
/// [`parse_colon_sequence`] and [`expand_orbit_shorthand`]). Every produced value is checked
/// for finiteness, for being on the design's gear (`0` up to and including `gear_teeth_abs`),
/// and for not repeating.
///
/// The index wheel is a ring: `gear_teeth_abs` (96 on a 96-tooth gear) is the same position
/// as `0`. A typed `96` is valid and kept as typed (never rewritten to `0`, so a `.asc` file
/// that carries `96` round-trips), while the duplicate check compares ring positions, so
/// `0, 96` is reported as a repeat.
///
/// # Errors
///
/// A message naming the offending token, ready to show the cutter. A value produced by a
/// shorthand names the shorthand as well as the value, so the error traces back to what was
/// typed. A value off the gear reports the valid range ("Indices run from 0 to 96 on this
/// gear.").
///
/// Public so the desktop's `setup_generate_step_series_callback` reads its indices field
/// with the same convention as [`parse_tier_form`].
pub fn parse_index_list(indices: &str, gear_teeth_abs: u32) -> Result<Vec<f64>, String> {
    let gear_teeth_abs_f64 = f64::from(gear_teeth_abs);
    let mut parsed_indices: Vec<f64> = Vec::new();
    let push_index =
        |value: f64, label: &str, parsed_indices: &mut Vec<f64>| -> Result<(), String> {
            if !value.is_finite() {
                return Err(format!("Index '{label}' must be a finite number."));
            }
            if value < 0.0 || value > gear_teeth_abs_f64 {
                return Err(format!(
                    "Index '{label}' is off the gear. \
                     Indices run from 0 to {gear_teeth_abs} on this gear."
                ));
            }
            let position = wheel_position(value, gear_teeth_abs_f64);
            if parsed_indices.iter().any(|&earlier| {
                (wheel_position(earlier, gear_teeth_abs_f64) - position).abs() < 1e-9
            }) {
                return Err(format!("Index '{label}' is listed more than once."));
            }
            parsed_indices.push(value);
            Ok(())
        };

    let raw_tokens: Vec<&str> = indices
        .split([',', ' ', ';'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let mut i = 0;
    while i < raw_tokens.len() {
        let token = raw_tokens[i];
        // "0:12:96" -- a colon-separated arithmetic sequence.
        if let Some(values) = parse_colon_sequence(token, gear_teeth_abs_f64) {
            for value in values {
                push_index(
                    value,
                    &format!("{value} (from '{token}')"),
                    &mut parsed_indices,
                )?;
            }
            i += 1;
            continue;
        }
        // "12 x8" -- a base index followed by an "xN" fold-count token.
        if let Ok(base) = token.parse::<f64>()
            && let Some(next) = raw_tokens.get(i + 1)
            && let Some(fold_count) = parse_orbit_suffix(next)
        {
            let shorthand = format!("{token} {next}");
            for value in expand_orbit_shorthand(base, fold_count, gear_teeth_abs_f64) {
                push_index(
                    value,
                    &format!("{value} (from '{shorthand}')"),
                    &mut parsed_indices,
                )?;
            }
            i += 2;
            continue;
        }
        let value: f64 = token
            .parse()
            .map_err(|_| format!("Index '{token}' is not a number."))?;
        push_index(value, token, &mut parsed_indices)?;
        i += 1;
    }
    Ok(parsed_indices)
}

/// Parses the tier-edit form's five text/enum fields into a [`ConstraintTier`].
///
/// Every validation failure is an `Err` message shown to the cutter, never a panic. The form
/// has no `mast` field: a mast is [`pleochroma_cut_core::Design::solve`]'s output, never
/// authored state (see `pleochroma_cut_core::design`'s module docs).
///
/// `constraint_kind` encodes the Meets choice, the inverse of `constraint_kind_and_text` in
/// the view model:
/// - `0` -- [`MeetConstraint::MeetExisting`] (`constraint_text` ignored).
/// - `1` -- [`MeetConstraint::MeetNamed`], `constraint_text` a comma-separated name list.
/// - `2` -- [`MeetConstraint::ScaleReference`], `constraint_text` a single real number.
/// - `3`/`4`/`5` -- an authored `TierTarget` (depth, girdle thickness, table width; see
///   [`parse_tier_target`]). The returned constraint is a `ScaleReference(0.0)` placeholder:
///   `Design::resolved_meet_tier_inputs` overwrites it with the resolved mast before
///   solving, and this function has no `Design` to resolve it against. The caller calls
///   [`parse_tier_target`] with the same fields to get the `TierTarget`.
/// - anything else -- rejected as an `Err`, never coerced to a default kind.
///
/// Indices split on comma, space or semicolon, as
/// `pleochroma_vault::local::reconstruct_asc_schedule` splits the same field, so a value
/// copied from its indices column round-trips. The two
/// shorthands ([`parse_colon_sequence`], [`expand_orbit_shorthand`]) are expanded first, and
/// their values go through the same per-value checks as a typed index: each must be finite,
/// within `0.0..=gear_teeth_abs` (`ScheduleMeta::gear_teeth_abs`), and not repeat an earlier
/// value. `0` and `gear_teeth_abs` count as the same position, and the closing value is kept
/// as typed. The first failing entry is reported by its own text.
///
/// A non-integral value is accepted: real `GemCad` files carry fractional positions (see
/// `pleochroma_formats::asc`). The caller warns about it instead of rejecting it
/// (`callbacks::tier_actions::setup_save_tier_callback`).
///
/// `imported_meet` is passed through to the built [`ConstraintTier`] unchanged, so editing
/// another field does not discard the file's stated meet instruction. The caller passes the
/// current tier's value for an existing row and `None` for a new tier. Only the one-click
/// "Adopt" action (`setup_adopt_meet_callback`) builds a constraint from it.
///
/// # Errors
///
/// A message naming the offending field: an angle that is not a finite number or
/// exceeds 90 degrees, a name another tier already holds, an invalid index list
/// (see [`parse_index_list`]), an empty "meet named" list, a scale reference that
/// is not a finite number, or an unknown constraint kind.
pub fn parse_tier_form(form: TierFormFields<'_>) -> Result<ConstraintTier, String> {
    let TierFormFields {
        angle,
        constraint_kind,
        constraint_text,
        name,
        indices,
        gear_teeth_abs,
        imported_meet,
        original_notes,
        other_tier_names,
    } = form;
    let angle_deg = parse_angle_only(angle)?;

    let name = name.trim();
    // Reject a name (or, for a `/`-joined multi-name tier, any ONE of its names)
    // another tier already holds -- see `TierFormFields::other_tier_names`'s own
    // doc comment for why an undetected collision is worse than merely confusing.
    for token in name.split('/').map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(existing) = other_tier_names
            .iter()
            .find(|other| other.eq_ignore_ascii_case(token))
        {
            return Err(format!(
                "Another tier is already named '{existing}' -- facet names must be unique so \
                 meet constraints resolve to the right tier."
            ));
        }
    }

    let parsed_indices = parse_index_list(indices, gear_teeth_abs)?;

    let constraint = match constraint_kind {
        0 => MeetConstraint::MeetExisting,
        1 => {
            let names: Vec<String> = constraint_text
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string)
                .collect();
            if names.is_empty() {
                return Err("\"Meet named\" needs at least one facet name.".to_string());
            }
            MeetConstraint::MeetNamed(names)
        }
        2 => {
            let value = eval_number(constraint_text, None)
                .map_err(|error| error.message("Scale reference", constraint_text))?;
            if !value.is_finite() {
                return Err("Scale reference must be a finite number.".to_string());
            }
            MeetConstraint::ScaleReference(value)
        }
        // 3-5: an authored `TierTarget` ("cut to depth"/"girdle thickness"/
        // "table width") -- see this function's own doc comment and
        // `parse_tier_target` below. The tier's own `constraint` is a
        // `ScaleReference` placeholder that `Design::resolved_meet_tier_inputs`
        // always overwrites before solving; the real millimetre value is
        // validated and returned separately, by `parse_tier_target`, since this
        // function only ever returns a `ConstraintTier`.
        3..=5 => MeetConstraint::ScaleReference(0.0),
        other => return Err(format!("Unknown constraint kind {other}.")),
    };

    Ok(ConstraintTier {
        angle_deg,
        name: name.to_string(),
        indices: parsed_indices,
        constraint,
        imported_meet,
        original_notes,
        detached: Vec::new(),
    })
}

/// [`parse_tier_form`] for a form whose angle field may hold a relation.
///
/// Text starting with `=` (`=C1-4`, `= (P1 + P3) / 2`) drives the tier's angle from other
/// tiers instead of fixing it.
///
/// Returns the tier and the relation text exactly as typed after the `=` (trimmed;
/// the caller turns it into a relation with `EditorSession::set_tier_relation`, which
/// reads the tier names against the design). The tier carries `placeholder_angle_deg`
/// -- the tier's current angle when editing, any valid angle for a new tier -- because
/// the relation, not this form, decides the real angle once it is applied. An angle
/// field that does not start with `=` behaves exactly as [`parse_tier_form`] and
/// returns no relation text.
///
/// # Errors
///
/// As [`parse_tier_form`]; an angle field that is only `=` is an "Angle ..." error.
pub fn parse_tier_form_with_relation(
    form: TierFormFields<'_>,
    placeholder_angle_deg: f64,
) -> Result<(ConstraintTier, Option<String>), String> {
    let Some(relation) = form.angle.trim().strip_prefix('=') else {
        return parse_tier_form(form).map(|tier| (tier, None));
    };
    let relation = relation.trim().to_owned();
    if relation.is_empty() {
        return Err(
            "Angle relation is empty -- type a calculation after '=', for example =C1-4."
                .to_string(),
        );
    }
    let placeholder = placeholder_angle_deg.to_string();
    let tier = parse_tier_form(TierFormFields {
        angle: &placeholder,
        ..form
    })?;
    Ok((tier, Some(relation)))
}

/// Parses the tier form's Meets combo into a [`TierTarget`] when `constraint_kind` names
/// one of the three target kinds.
///
/// `3` [`TierTarget::DepthMm`], `4` [`TierTarget::GirdleThicknessMm`], `5`
/// [`TierTarget::TableWidthMm`] -- the counterpart to [`parse_tier_form`]'s own
/// `0`/`1`/`2` handling for a plain [`MeetConstraint`].
///
/// `constraint_text` is the SAME single numeric field
/// [`parse_tier_form`] reads for kind `2` (`ScaleReference`); here it is
/// always a millimetre figure, matching the unit every [`TierTarget`] variant
/// itself declares.
///
/// `Ok(None)` for every other kind (`0`/`1`/`2`, or anything
/// [`parse_tier_form`] would itself reject) -- "no target," not an error, so a
/// plain meet/scale-reference save that never authored a target is a no-op
/// through `callbacks::tier_actions::setup_save_tier_callback`'s own
/// target-clearing logic, which calls this unconditionally alongside
/// [`parse_tier_form`].
///
/// # Errors
///
/// The same "name the offending value" wording [`parse_tier_form`]'s own
/// `ScaleReference` arm uses, prefixed with the target's own cutter-facing
/// label ("Depth"/"Girdle thickness"/"Table width") instead of "Scale
/// reference" -- [`tier_form_error_field`] recognizes
/// all three prefixes and classifies them the same `"constraint"` field a
/// bad Meets value always has. A non-finite or non-positive value is
/// rejected the same way a non-positive preform dimension is
/// ([`super::preform_and_new_design::parse_preform_form`]): a depth, girdle
/// thickness, or table width of zero or less is never a real target.
pub fn parse_tier_target(
    constraint_kind: i32,
    constraint_text: &str,
) -> Result<Option<TierTarget>, String> {
    let (label, build): (&str, fn(f64) -> TierTarget) = match constraint_kind {
        3 => ("Depth", TierTarget::DepthMm as fn(f64) -> TierTarget),
        4 => (
            "Girdle thickness",
            TierTarget::GirdleThicknessMm as fn(f64) -> TierTarget,
        ),
        5 => (
            "Table width",
            TierTarget::TableWidthMm as fn(f64) -> TierTarget,
        ),
        _ => return Ok(None),
    };
    let text = constraint_text.trim();
    let value = eval_number(text, None).map_err(|error| error.message(label, text))?;
    if !value.is_finite() {
        return Err(format!("{label} must be a finite number."));
    }
    if value <= 0.0 {
        return Err(format!("{label} must be a positive number."));
    }
    Ok(Some(build(value)))
}

/// Classifies a [`parse_tier_form`] (or `Edit`-apply) error message by tier-form field.
///
/// Names the form field a message is about, for marking its control.
///
/// The answer is `"angle"`, `"name"`, `"indices"` or `"constraint"` (and, for the concave
/// form, `"tool"`, `"theta"`, `"x"`, `"y"`, `"z"`, `"diameter"` or `"tool_angle"`), or `""`
/// for a general error. It matches the exact wording this module's error branches build,
/// since [`parse_tier_form`] returns a plain `String`.
#[must_use]
pub fn tier_form_error_field(message: &str) -> &'static str {
    // The concave form's messages name their field up front (`"theta: ..."`, see
    // `loading::concave_form`); none of the flat form's messages has that lowercase
    // `field:` shape, so this cannot shadow them.
    if let Some(field) = concave_form_error_field(message) {
        return field;
    }
    if message.starts_with("Angle") {
        "angle"
    } else if message.starts_with("Another tier is already named") {
        "name"
    } else if message.starts_with("Index '") {
        "indices"
    } else if message.starts_with("\"Meet named\"")
        || message.starts_with("Scale reference")
        || message.starts_with("No facet named")
        || message.starts_with("Unknown constraint kind")
        // `parse_tier_target`'s own three target labels -- same "constraint" bucket
        // as `Scale reference`'s wording, since these are all failures of the same
        // Meets-combo numeric field.
        || message.starts_with("Depth")
        || message.starts_with("Girdle thickness")
        || message.starts_with("Table width")
    {
        "constraint"
    } else {
        ""
    }
}

/// The field a concave form message names by prefix, or `None` for any other message.
///
/// `"tool angle:"` is listed before `"tool:"`; the answers are `"name"`, `"angle"`,
/// `"indices"`, `"tool"`, `"theta"`, `"x"`, `"y"`, `"z"`, `"diameter"` and `"tool_angle"`.
fn concave_form_error_field(message: &str) -> Option<&'static str> {
    const PREFIXES: [(&str, &str); 10] = [
        ("tool angle:", "tool_angle"),
        ("tool:", "tool"),
        ("name:", "name"),
        ("angle:", "angle"),
        ("indices:", "indices"),
        ("theta:", "theta"),
        ("x:", "x"),
        ("y:", "y"),
        ("z:", "z"),
        ("diameter:", "diameter"),
    ];
    PREFIXES
        .iter()
        .find(|(prefix, _)| message.starts_with(prefix))
        .map(|&(_, field)| field)
}

/// The warning shown for a non-integral index-wheel position, which is accepted anyway.
///
/// The warning shown for a non-integral index-wheel position, which is accepted anyway.
///
/// [`parse_tier_form`] deliberately ACCEPTS such a position, since real `.asc` files carry
/// some and a hand-typed fraction is sometimes exactly what was meant. Without this warning a
/// stray extra digit ("12.5" for "12") would only surface later as an obscure solver oddity.
/// A caller shows it after a successful parse, so a save is only flagged, never blocked.
/// `1e-3` is `orbit::model::INDEX_TOLERANCE`, the tolerance for "close enough to call it a
/// whole tooth".
#[must_use]
pub fn non_integral_index_warning(indices: &[f64]) -> Option<String> {
    let mut fractional: Vec<String> = indices
        .iter()
        .filter(|v| (**v - v.round()).abs() > 1e-3)
        .map(|v| format!("{v:.3}"))
        .collect();
    fractional.dedup();
    if fractional.is_empty() {
        return None;
    }
    Some(format!(
        "Note: non-integral index position(s) {} -- check this was intentional.",
        fractional.join(", ")
    ))
}

#[cfg(test)]
mod arithmetic_tests;
#[cfg(test)]
mod tests;
