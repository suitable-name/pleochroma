//! The `PLRAW1` linear image file written by `--raw-out` and `--progress-raw`, and read by
//! `image-compare`.
//!
//! A render's final linear values, before exposure and tone mapping, so two renders can be
//! compared as numbers rather than as 8-bit pictures. Little endian throughout:
//!
//! | Bytes | Content |
//! |---|---|
//! | 6 | the magic `PLRAW1` |
//! | 4 | width, `u32` |
//! | 4 | height, `u32` |
//! | 12 × width × height | per pixel, row-major from the top-left: the mean CIE XYZ, three `f32` |
//! | 4 × width × height | per pixel, the number of samples behind that mean, `u32` |
//!
//! Without adaptive sampling every pixel has the same count; with it the counts differ.

use std::fmt;

/// The six bytes every `PLRAW1` file starts with.
pub const MAGIC: &[u8; 6] = b"PLRAW1";

/// The header length in bytes: the magic, the width and the height.
const HEADER_LEN: usize = 6 + 4 + 4;

/// A linear image: the mean CIE XYZ of every pixel and its sample count.
#[derive(Debug, Clone, PartialEq)]
pub struct RawImage {
    /// Width in pixels.
    pub width: u32,
    /// Height in pixels.
    pub height: u32,
    /// The mean CIE XYZ of each pixel, row-major from the top-left.
    pub xyz: Vec<[f32; 3]>,
    /// The samples behind each pixel's mean, in the same order.
    pub counts: Vec<u32>,
}

/// Why bytes are not a `PLRAW1` image, or an image cannot be written as one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RawImageError {
    /// The file does not start with `PLRAW1`.
    BadMagic,
    /// The file is shorter or longer than its width and height say.
    WrongLength {
        /// The length the header implies.
        expected: usize,
        /// The length found.
        found: usize,
    },
    /// The pixel or count list does not have width × height entries.
    WrongPixelCount {
        /// width × height.
        expected: usize,
        /// Entries in the XYZ list.
        xyz: usize,
        /// Entries in the count list.
        counts: usize,
    },
    /// Width × height does not fit in memory.
    TooLarge,
}

impl fmt::Display for RawImageError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadMagic => write!(
                f,
                "not a PLRAW1 image (the file does not start with PLRAW1)"
            ),
            Self::WrongLength { expected, found } => write!(
                f,
                "a PLRAW1 image of this size has {expected} bytes, the file has {found}"
            ),
            Self::WrongPixelCount {
                expected,
                xyz,
                counts,
            } => write!(
                f,
                "the image needs {expected} pixels, it has {xyz} colors and {counts} counts"
            ),
            Self::TooLarge => write!(f, "the image is too large"),
        }
    }
}

impl std::error::Error for RawImageError {}

/// width × height as a pixel count, `None` when it overflows.
fn pixel_count(width: u32, height: u32) -> Option<usize> {
    usize::try_from(width)
        .ok()?
        .checked_mul(usize::try_from(height).ok()?)
}

/// The file length of an image of `pixels` pixels, `None` when it overflows.
fn file_len(pixels: usize) -> Option<usize> {
    pixels.checked_mul(16)?.checked_add(HEADER_LEN)
}

impl RawImage {
    /// The image as `PLRAW1` bytes.
    ///
    /// # Errors
    ///
    /// [`RawImageError::WrongPixelCount`] when the lists do not have width × height entries,
    /// [`RawImageError::TooLarge`] when the size overflows.
    pub fn to_bytes(&self) -> Result<Vec<u8>, RawImageError> {
        let pixels = pixel_count(self.width, self.height).ok_or(RawImageError::TooLarge)?;
        if self.xyz.len() != pixels || self.counts.len() != pixels {
            return Err(RawImageError::WrongPixelCount {
                expected: pixels,
                xyz: self.xyz.len(),
                counts: self.counts.len(),
            });
        }
        let mut out = Vec::with_capacity(file_len(pixels).ok_or(RawImageError::TooLarge)?);
        out.extend_from_slice(MAGIC);
        out.extend_from_slice(&self.width.to_le_bytes());
        out.extend_from_slice(&self.height.to_le_bytes());
        for value in self.xyz.iter().flatten() {
            out.extend_from_slice(&value.to_le_bytes());
        }
        for count in &self.counts {
            out.extend_from_slice(&count.to_le_bytes());
        }
        Ok(out)
    }

    /// Reads a `PLRAW1` image.
    ///
    /// # Errors
    ///
    /// [`RawImageError::BadMagic`] when the bytes do not start with `PLRAW1`,
    /// [`RawImageError::WrongLength`] when their length does not match the header,
    /// [`RawImageError::TooLarge`] when the header's size overflows.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, RawImageError> {
        let Some((magic, rest)) = bytes.split_at_checked(MAGIC.len()) else {
            return Err(RawImageError::BadMagic);
        };
        if magic != MAGIC {
            return Err(RawImageError::BadMagic);
        }
        let word = |at: usize| -> Option<u32> {
            let chunk: [u8; 4] = rest.get(at..at + 4)?.try_into().ok()?;
            Some(u32::from_le_bytes(chunk))
        };
        let (Some(width), Some(height)) = (word(0), word(4)) else {
            return Err(RawImageError::WrongLength {
                expected: HEADER_LEN,
                found: bytes.len(),
            });
        };
        let pixels = pixel_count(width, height).ok_or(RawImageError::TooLarge)?;
        let expected = file_len(pixels).ok_or(RawImageError::TooLarge)?;
        if bytes.len() != expected {
            return Err(RawImageError::WrongLength {
                expected,
                found: bytes.len(),
            });
        }
        let body = &bytes[HEADER_LEN..];
        let (colors, counts) = body.split_at(pixels * 12);
        let float = |chunk: &[u8]| f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        let xyz = colors
            .as_chunks::<12>()
            .0
            .iter()
            .map(|pixel| {
                [
                    float(&pixel[0..4]),
                    float(&pixel[4..8]),
                    float(&pixel[8..12]),
                ]
            })
            .collect();
        let counts = counts
            .as_chunks::<4>()
            .0
            .iter()
            .map(|chunk| u32::from_le_bytes(*chunk))
            .collect();
        Ok(Self {
            width,
            height,
            xyz,
            counts,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image() -> RawImage {
        RawImage {
            width: 2,
            height: 1,
            xyz: vec![[0.25, 0.5, 1.0], [f32::MIN_POSITIVE, 0.0, 123.456]],
            counts: vec![16, 4096],
        }
    }

    #[test]
    fn an_image_round_trips_bit_for_bit() {
        let bytes = image().to_bytes().expect("writes");
        assert_eq!(&bytes[..6], MAGIC);
        assert_eq!(bytes.len(), HEADER_LEN + 2 * 16);
        assert_eq!(RawImage::from_bytes(&bytes), Ok(image()));
    }

    #[test]
    fn bad_input_is_refused_with_a_reason() {
        assert_eq!(
            RawImage::from_bytes(b"PNG..."),
            Err(RawImageError::BadMagic)
        );
        assert_eq!(RawImage::from_bytes(b""), Err(RawImageError::BadMagic));
        let mut bytes = image().to_bytes().expect("writes");
        bytes.pop();
        assert!(matches!(
            RawImage::from_bytes(&bytes),
            Err(RawImageError::WrongLength { .. })
        ));
        let mut short = image();
        short.counts.pop();
        assert!(matches!(
            short.to_bytes(),
            Err(RawImageError::WrongPixelCount { .. })
        ));
    }
}
