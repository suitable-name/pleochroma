//! The parser of `image-compare`: two image files and a handful of options.

use super::{
    types::{Command, ImageCompareArgs},
    values::Cursor,
};
use std::path::PathBuf;

/// Parses the words after `image-compare`.
///
/// Two positional words are the reference image and the image under test, in that order. The
/// options may stand anywhere among them.
pub(super) fn parse_image_compare(args: &[String]) -> Result<Command, String> {
    let mut cursor = Cursor::new("image-compare", args);
    let mut images: Vec<PathBuf> = Vec::new();
    let (mut ref2, mut tonemap_from, mut error_png, mut out) = (None, None, None, None);
    let mut json = false;
    while let Some(word) = cursor.token() {
        match word {
            "--ref2" => ref2 = Some(cursor.path("--ref2")?),
            "--tonemap-from" => tonemap_from = Some(cursor.path("--tonemap-from")?),
            "--error-png" => error_png = Some(cursor.path("--error-png")?),
            "--json" => json = true,
            "--out" => out = Some(cursor.path("--out")?),
            flag if flag.starts_with('-') => {
                return Err(format!(
                    "unknown flag {flag:?} for \"image-compare\" (see --help)"
                ));
            }
            image => images.push(PathBuf::from(image)),
        }
    }
    let mut images = images.into_iter();
    let (Some(reference), Some(run), None) = (images.next(), images.next(), images.next()) else {
        return Err(
            "\"image-compare\" needs exactly two image files, the reference and the run \
             (see --help)"
                .to_string(),
        );
    };
    Ok(Command::ImageCompare(ImageCompareArgs {
        reference,
        run,
        ref2,
        tonemap_from,
        error_png,
        json,
        out,
    }))
}
