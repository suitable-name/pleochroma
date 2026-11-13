//! A minimal built-in 5x7 bitmap font.
//!
//! A fixed glyph lookup for code that stamps text straight into an RGBA8 buffer without
//! a GUI toolkit's text layout: [`crate::diagram2d`]'s panel labels, and the desktop's
//! tilt video-export overlay (re-exported there as `gui::pixel_font`). Each caller keeps
//! its own scaling, drawing and color code; only the glyph data lives here, so the
//! callers cannot drift apart one character at a time.
//!
//! Covers every character either caller actually draws: space, `-`, `.`, `'`,
//! `/`, `_`, `%`, `:`, the digits, and every uppercase letter A-Z (case-folded --
//! see [`glyph`]). An unsupported character renders as a blank cell rather than a
//! placeholder glyph.

/// Glyph width in pixels.
pub const GLYPH_WIDTH: usize = 5;
/// Glyph height in pixels.
pub const GLYPH_HEIGHT: usize = 7;

/// Converts a 5-character `'X'`/`'.'` row into a bitmask (bit 4 = leftmost column).
fn row_bits(row: &str) -> u8 {
    let mut bits = 0u8;
    for (i, ch) in row.chars().take(GLYPH_WIDTH).enumerate() {
        if ch != '.' {
            bits |= 1 << (GLYPH_WIDTH - 1 - i);
        }
    }
    bits
}

/// The punctuation and whitespace arms of [`glyph_rows`], split out for clippy's
/// `too_many_lines` limit. `c` is already uppercased by the caller.
const fn glyph_rows_symbols(c: char) -> Option<[&'static str; GLYPH_HEIGHT]> {
    Some(match c {
        ' ' => [
            ".....", ".....", ".....", ".....", ".....", ".....", ".....",
        ],
        '-' => [
            ".....", ".....", ".....", "XXXXX", ".....", ".....", ".....",
        ],
        '.' => [
            ".....", ".....", ".....", ".....", ".....", ".XX..", ".XX..",
        ],
        '\'' => [
            ".X...", ".X...", ".....", ".....", ".....", ".....", ".....",
        ],
        // Used by the multi-name tier join (`design/tier.rs` in pleochroma-cut-core)
        // and by underscores in tier names. Unknown characters draw blank, so each
        // one needs its own entry here.
        '/' => [
            "....X", "...X.", "..X..", "..X..", ".X...", "X....", ".....",
        ],
        '_' => [
            ".....", ".....", ".....", ".....", ".....", ".....", "XXXXX",
        ],
        // Used by the tilt video-export overlay (percentages and "HH:MM" times). The
        // diagram labels do not use either.
        '%' => [
            "X...X", "...X.", "..X..", "..X..", ".X...", "X...X", ".....",
        ],
        ':' => [
            ".....", ".XX..", ".XX..", ".....", ".XX..", ".XX..", ".....",
        ],
        _ => return None,
    })
}

/// The digit arms of [`glyph_rows`], split out for clippy's `too_many_lines` limit.
/// `c` is already uppercased by the caller.
const fn glyph_rows_digits(c: char) -> Option<[&'static str; GLYPH_HEIGHT]> {
    Some(match c {
        '0' => [
            "XXXXX", "X...X", "X..XX", "X.X.X", "XX..X", "X...X", "XXXXX",
        ],
        '1' => [
            "..X..", ".XX..", "..X..", "..X..", "..X..", "..X..", ".XXX.",
        ],
        '2' => [
            ".XXX.", "X...X", "....X", "...X.", "..X..", ".X...", "XXXXX",
        ],
        '3' => [
            "XXXXX", "...X.", "..X..", "...X.", "....X", "X...X", ".XXX.",
        ],
        '4' => [
            "...X.", "..XX.", ".X.X.", "X..X.", "XXXXX", "...X.", "...X.",
        ],
        '5' => [
            "XXXXX", "X....", "XXXX.", "....X", "....X", "X...X", ".XXX.",
        ],
        '6' => [
            "..XX.", ".X...", "X....", "XXXX.", "X...X", "X...X", ".XXX.",
        ],
        '7' => [
            "XXXXX", "....X", "...X.", "..X..", ".X...", ".X...", ".X...",
        ],
        '8' => [
            ".XXX.", "X...X", "X...X", ".XXX.", "X...X", "X...X", ".XXX.",
        ],
        '9' => [
            ".XXX.", "X...X", "X...X", ".XXXX", "....X", "...X.", ".XX..",
        ],
        _ => return None,
    })
}

/// The `A`-`M` letter arms of [`glyph_rows`], split out for clippy's `too_many_lines`
/// limit. `c` is already uppercased by the caller.
const fn glyph_rows_letters_a_to_m(c: char) -> Option<[&'static str; GLYPH_HEIGHT]> {
    Some(match c {
        'A' => [
            "..X..", ".X.X.", "X...X", "X...X", "XXXXX", "X...X", "X...X",
        ],
        'B' => [
            "XXXX.", "X...X", "X...X", "XXXX.", "X...X", "X...X", "XXXX.",
        ],
        'C' => [
            ".XXXX", "X....", "X....", "X....", "X....", "X....", ".XXXX",
        ],
        'D' => [
            "XXXX.", "X...X", "X...X", "X...X", "X...X", "X...X", "XXXX.",
        ],
        'E' => [
            "XXXXX", "X....", "X....", "XXXX.", "X....", "X....", "XXXXX",
        ],
        'F' => [
            "XXXXX", "X....", "X....", "XXXX.", "X....", "X....", "X....",
        ],
        'G' => [
            ".XXXX", "X....", "X....", "X.XXX", "X...X", "X...X", ".XXXX",
        ],
        'H' => [
            "X...X", "X...X", "X...X", "XXXXX", "X...X", "X...X", "X...X",
        ],
        'I' => [
            "XXXXX", "..X..", "..X..", "..X..", "..X..", "..X..", "XXXXX",
        ],
        'J' => [
            "....X", "....X", "....X", "....X", "X...X", "X...X", ".XXX.",
        ],
        'K' => [
            "X...X", "X..X.", "X.X..", "XX...", "X.X..", "X..X.", "X...X",
        ],
        'L' => [
            "X....", "X....", "X....", "X....", "X....", "X....", "XXXXX",
        ],
        'M' => [
            "X...X", "XX.XX", "X.X.X", "X...X", "X...X", "X...X", "X...X",
        ],
        _ => return None,
    })
}

/// The `N`-`Z` letter arms of [`glyph_rows`], split out for clippy's `too_many_lines`
/// limit. `c` is already uppercased by the caller.
const fn glyph_rows_letters_n_to_z(c: char) -> Option<[&'static str; GLYPH_HEIGHT]> {
    Some(match c {
        'N' => [
            "X...X", "XX..X", "X.X.X", "X..XX", "X...X", "X...X", "X...X",
        ],
        'O' => [
            ".XXX.", "X...X", "X...X", "X...X", "X...X", "X...X", ".XXX.",
        ],
        'P' => [
            "XXXX.", "X...X", "X...X", "XXXX.", "X....", "X....", "X....",
        ],
        'Q' => [
            ".XXX.", "X...X", "X...X", "X...X", "X.X.X", "X..X.", ".XX.X",
        ],
        'R' => [
            "XXXX.", "X...X", "X...X", "XXXX.", "X.X..", "X..X.", "X...X",
        ],
        'S' => [
            ".XXXX", "X....", "X....", ".XXX.", "....X", "....X", "XXXX.",
        ],
        'T' => [
            "XXXXX", "..X..", "..X..", "..X..", "..X..", "..X..", "..X..",
        ],
        'U' => [
            "X...X", "X...X", "X...X", "X...X", "X...X", "X...X", ".XXX.",
        ],
        'V' => [
            "X...X", "X...X", "X...X", "X...X", "X...X", ".X.X.", "..X..",
        ],
        'W' => [
            "X...X", "X...X", "X...X", "X.X.X", "X.X.X", "XX.XX", "X...X",
        ],
        'X' => [
            "X...X", "X...X", ".X.X.", "..X..", ".X.X.", "X...X", "X...X",
        ],
        'Y' => [
            "X...X", "X...X", ".X.X.", "..X..", "..X..", "..X..", "..X..",
        ],
        'Z' => [
            "XXXXX", "....X", "...X.", "..X..", ".X...", "X....", "XXXXX",
        ],
        _ => return None,
    })
}

/// Returns the glyph rows for `c` (case-insensitive letters), or `None` for an
/// unsupported character; callers skip it rather than draw a placeholder. Tries the
/// [`glyph_rows_symbols`], [`glyph_rows_digits`] and letter tables in turn.
const fn glyph_rows(c: char) -> Option<[&'static str; GLYPH_HEIGHT]> {
    let c = c.to_ascii_uppercase();
    if let Some(rows) = glyph_rows_symbols(c) {
        return Some(rows);
    }
    if let Some(rows) = glyph_rows_digits(c) {
        return Some(rows);
    }
    if let Some(rows) = glyph_rows_letters_a_to_m(c) {
        return Some(rows);
    }
    glyph_rows_letters_n_to_z(c)
}

/// The bitmap for `c` (case-insensitive), one packed row per byte (bit 4 =
/// leftmost column) -- a blank cell for any character [`glyph_rows`] does not
/// cover.
#[must_use]
pub fn glyph(c: char) -> [u8; GLYPH_HEIGHT] {
    let rows = glyph_rows(c).unwrap_or([
        ".....", ".....", ".....", ".....", ".....", ".....", ".....",
    ]);
    std::array::from_fn(|i| row_bits(rows[i]))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn row_bits_packs_x_as_1_and_dot_as_0_leftmost_bit_first() {
        assert_eq!(row_bits("X...."), 0b1_0000);
        assert_eq!(row_bits("....X"), 0b0_0001);
        assert_eq!(row_bits("....."), 0);
        assert_eq!(row_bits("XXXXX"), 0b1_1111);
    }

    #[test]
    fn glyph_is_blank_for_an_unsupported_character() {
        assert_eq!(glyph('@'), [0u8; GLYPH_HEIGHT]);
    }

    #[test]
    fn glyph_is_case_insensitive() {
        assert_eq!(glyph('a'), glyph('A'));
        assert_eq!(glyph('z'), glyph('Z'));
    }

    /// The tilt video-export overlay draws `%` and `:`, so the table must cover them.
    #[test]
    fn glyph_covers_the_percent_and_colon_symbols_the_overlay_needs() {
        assert_ne!(glyph('%'), [0u8; GLYPH_HEIGHT]);
        assert_ne!(glyph(':'), [0u8; GLYPH_HEIGHT]);
    }

    /// Every character the two callers draw must render a real glyph, never the blank
    /// fallback.
    #[test]
    fn every_character_either_caller_draws_renders_a_non_blank_glyph() {
        for c in "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789-.'/_%:".chars() {
            assert_ne!(
                glyph(c),
                [0u8; GLYPH_HEIGHT],
                "expected a real glyph for {c:?}"
            );
        }
        // Space is the one deliberately blank glyph in that set.
        assert_eq!(glyph(' '), [0u8; GLYPH_HEIGHT]);
    }
}
