//! The two standard PDF fonts the PDF sheet uses, Helvetica and Helvetica-Bold: their advance
//! widths and the encoding of text to `WinAnsiEncoding` bytes.
//!
//! The fonts are two of the standard 14 every PDF reader has, so nothing is embedded. Measuring
//! text (to centre it, align it right and wrap the comments) needs their widths: the tables are
//! the standard Adobe font metrics (AFM) of Helvetica and Helvetica-Bold, in thousandths of an
//! em, for the `WinAnsiEncoding` codes 32 to 255. A character `WinAnsiEncoding` cannot hold is
//! written as `?`.

/// One of the sheet's two fonts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Font {
    /// Helvetica.
    Regular,
    /// Helvetica-Bold.
    Bold,
}

impl Font {
    /// The font's resource name on every page (`/F1`, `/F2`).
    #[must_use]
    pub const fn resource(self) -> &'static [u8] {
        match self {
            Self::Regular => b"F1",
            Self::Bold => b"F2",
        }
    }

    /// The font's `BaseFont`, a standard-14 name.
    #[must_use]
    pub const fn base_font(self) -> &'static [u8] {
        match self {
            Self::Regular => b"Helvetica",
            Self::Bold => b"Helvetica-Bold",
        }
    }

    /// The advance width of `code` in thousandths of an em; `0` below the space (never
    /// written: [`encode`] turns control characters into `?`).
    #[must_use]
    pub fn advance(self, code: u8) -> u16 {
        let table = match self {
            Self::Regular => &HELVETICA,
            Self::Bold => &HELVETICA_BOLD,
        };
        code.checked_sub(32)
            .map_or(0, |index| table[usize::from(index)])
    }
}

/// `text` as `WinAnsiEncoding` bytes, one per character; a character the encoding cannot hold
/// (and any control character) becomes `?`.
#[must_use]
pub fn encode(text: &str) -> Vec<u8> {
    text.chars().map(win_ansi).collect()
}

/// The width of `text` in `font` at `size` points, in points.
#[must_use]
pub fn text_width(text: &str, font: Font, size: f32) -> f32 {
    let thousandths: u32 = encode(text)
        .into_iter()
        .map(|code| u32::from(font.advance(code)))
        .sum();
    thousandths as f32 * size / 1000.0
}

/// One character's `WinAnsiEncoding` code, `?` when it has none.
fn win_ansi(c: char) -> u8 {
    let code = u32::from(c);
    // Printable ASCII and the Latin-1 block are the same codes in WinAnsiEncoding.
    if (0x20..=0x7e).contains(&code) || (0xa0..=0xff).contains(&code) {
        return u8::try_from(code).unwrap_or(b'?');
    }
    match c {
        '\u{20ac}' => 0x80,
        '\u{201a}' => 0x82,
        '\u{0192}' => 0x83,
        '\u{201e}' => 0x84,
        '\u{2026}' => 0x85,
        '\u{2020}' => 0x86,
        '\u{2021}' => 0x87,
        '\u{02c6}' => 0x88,
        '\u{2030}' => 0x89,
        '\u{0160}' => 0x8a,
        '\u{2039}' => 0x8b,
        '\u{0152}' => 0x8c,
        '\u{017d}' => 0x8e,
        '\u{2018}' => 0x91,
        '\u{2019}' => 0x92,
        '\u{201c}' => 0x93,
        '\u{201d}' => 0x94,
        '\u{2022}' => 0x95,
        '\u{2013}' => 0x96,
        '\u{2014}' => 0x97,
        '\u{02dc}' => 0x98,
        '\u{2122}' => 0x99,
        '\u{0161}' => 0x9a,
        '\u{203a}' => 0x9b,
        '\u{0153}' => 0x9c,
        '\u{017e}' => 0x9e,
        '\u{0178}' => 0x9f,
        _ => b'?',
    }
}

/// Helvetica's advance widths for the `WinAnsiEncoding` codes 32 to 255 (AFM, thousandths of an
/// em); `0` for the five codes the encoding leaves undefined and for DEL.
#[rustfmt::skip]
const HELVETICA: [u16; 224] = [
    // 32..=47: space ! " # $ % & ' ( ) * + , - . /
    278, 278, 355, 556, 556, 889, 667, 191, 333, 333, 389, 584, 278, 333, 278, 278,
    // 48..=63: 0-9 : ; < = > ?
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 278, 278, 584, 584, 584, 556,
    // 64..=79: @ A-O
    1015, 667, 667, 722, 722, 667, 611, 778, 722, 278, 500, 667, 556, 833, 722, 778,
    // 80..=95: P-Z [ \ ] ^ _
    667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 278, 278, 278, 469, 556,
    // 96..=111: ` a-o
    333, 556, 556, 500, 556, 556, 278, 556, 556, 222, 222, 500, 222, 833, 556, 556,
    // 112..=127: p-z { | } ~ DEL
    556, 556, 333, 500, 278, 556, 500, 722, 500, 500, 500, 334, 260, 334, 584, 0,
    // 128..=143
    556, 0, 222, 556, 333, 1000, 556, 556, 333, 1000, 667, 333, 1000, 0, 611, 0,
    // 144..=159
    0, 222, 222, 333, 333, 350, 556, 1000, 333, 1000, 500, 333, 944, 0, 500, 667,
    // 160..=175
    278, 333, 556, 556, 556, 556, 260, 556, 333, 737, 370, 556, 584, 333, 737, 333,
    // 176..=191 (176 is the degree sign)
    400, 584, 333, 333, 333, 556, 537, 278, 333, 333, 365, 556, 834, 834, 834, 611,
    // 192..=207
    667, 667, 667, 667, 667, 667, 1000, 722, 667, 667, 667, 667, 278, 278, 278, 278,
    // 208..=223
    722, 722, 778, 778, 778, 778, 778, 584, 778, 722, 722, 722, 722, 667, 667, 611,
    // 224..=239
    556, 556, 556, 556, 556, 556, 889, 500, 556, 556, 556, 556, 278, 278, 278, 278,
    // 240..=255
    556, 556, 556, 556, 556, 556, 556, 584, 611, 556, 556, 556, 556, 500, 556, 500,
];

/// Helvetica-Bold's advance widths, laid out as [`HELVETICA`].
#[rustfmt::skip]
const HELVETICA_BOLD: [u16; 224] = [
    // 32..=47
    278, 333, 474, 556, 556, 889, 722, 238, 333, 333, 389, 584, 278, 333, 278, 278,
    // 48..=63
    556, 556, 556, 556, 556, 556, 556, 556, 556, 556, 333, 333, 584, 584, 584, 611,
    // 64..=79
    975, 722, 722, 722, 722, 667, 611, 778, 722, 278, 556, 722, 611, 833, 722, 778,
    // 80..=95
    667, 778, 722, 667, 611, 722, 667, 944, 667, 667, 611, 333, 278, 333, 584, 556,
    // 96..=111
    333, 556, 611, 556, 611, 556, 333, 611, 611, 278, 278, 556, 278, 889, 611, 611,
    // 112..=127
    611, 611, 389, 556, 333, 611, 556, 778, 556, 556, 500, 389, 280, 389, 584, 0,
    // 128..=143
    556, 0, 278, 556, 500, 1000, 556, 556, 333, 1000, 667, 333, 1000, 0, 611, 0,
    // 144..=159
    0, 278, 278, 500, 500, 350, 556, 1000, 333, 1000, 556, 333, 944, 0, 500, 667,
    // 160..=175
    278, 333, 556, 556, 556, 556, 280, 556, 333, 737, 370, 556, 584, 333, 737, 333,
    // 176..=191
    400, 584, 333, 333, 333, 611, 556, 278, 333, 333, 365, 556, 834, 834, 834, 611,
    // 192..=207
    722, 722, 722, 722, 722, 722, 1000, 722, 667, 667, 667, 667, 278, 278, 278, 278,
    // 208..=223
    722, 722, 778, 778, 778, 778, 778, 584, 778, 722, 722, 722, 722, 667, 667, 611,
    // 224..=239
    556, 556, 556, 556, 556, 556, 889, 556, 556, 556, 556, 556, 278, 278, 278, 278,
    // 240..=255
    611, 611, 611, 611, 611, 611, 611, 584, 611, 611, 611, 611, 611, 556, 611, 556,
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_is_encoded_to_win_ansi_and_the_rest_is_a_question_mark() {
        assert_eq!(encode("P1 40.00\u{b0}"), b"P1 40.00\xb0");
        assert_eq!(encode("\u{2013}\u{20ac}"), [0x96u8, 0x80]);
        assert_eq!(encode("\u{3b8}\t"), b"??");
    }

    #[test]
    fn widths_are_the_standard_metrics() {
        // Every digit is 556 in both fonts, so "96" at 10 pt is 11.12 pt wide.
        assert!((text_width("96", Font::Regular, 10.0) - 11.12).abs() < 1e-4);
        assert!((text_width("96", Font::Bold, 10.0) - 11.12).abs() < 1e-4);
        assert_eq!(Font::Regular.advance(b'i'), 222);
        assert_eq!(Font::Bold.advance(b'i'), 278);
        assert_eq!(Font::Regular.advance(0xb0), 400, "the degree sign");
        assert_eq!(Font::Regular.advance(b'~'), 584);
        assert_eq!(Font::Bold.advance(0xff), 556);
    }
}
