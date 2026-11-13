//! The design settings panel's "Printed Proportions" form: the five figures (Vol/W^3,
//! L/W, C/W, P/W, H/W) that a printed sheet lists, parsed from their text fields.
//!
//! Both editors store the result in the design's `printed_proportions`. It is the external
//! verification target of Deep Solve and is kept in a design file's `[source]` table.

use pleochroma::geometry::stone_metrics::ExternalProportions;

/// Parses one field: `None` for blank text, `Some(value)` for a finite positive number.
///
/// # Errors
///
/// A ready-to-show message naming `label` when `text` is non-blank but does not
/// parse as a finite positive number.
pub fn parse_printed_proportions_field(label: &str, text: &str) -> Result<Option<f64>, String> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let value: f64 = trimmed
        .parse()
        .map_err(|_| format!("{label} '{trimmed}' is not a number."))?;
    if !value.is_finite() || value <= 0.0 {
        return Err(format!("{label} must be a positive number."));
    }
    Ok(Some(value))
}

/// Parses the panel's five text fields into [`ExternalProportions`], the same shape a
/// catalogue row's measured columns produce.
///
/// # Errors
///
/// The first field (in `vol_w3, lw, cw, pw, hw` order) that fails to parse, via
/// [`parse_printed_proportions_field`].
pub fn parse_printed_proportions_form(
    vol_w3: &str,
    lw: &str,
    cw: &str,
    pw: &str,
    hw: &str,
) -> Result<ExternalProportions, String> {
    Ok(ExternalProportions {
        vol_w3: parse_printed_proportions_field("Vol/W\u{b3}", vol_w3)?,
        lw: parse_printed_proportions_field("L/W", lw)?,
        cw: parse_printed_proportions_field("C/W", cw)?,
        pw: parse_printed_proportions_field("P/W", pw)?,
        hw: parse_printed_proportions_field("H/W", hw)?,
    })
}
