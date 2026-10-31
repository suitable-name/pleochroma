//! [`environment_from_hdr_bytes`]: the ONE function that turns Radiance `.hdr` bytes
//! into an [`EnvironmentMap`] (texels plus its importance-sampling `Distribution2D`).
//!
//! # Why one function
//!
//! A remote render worker lights a scene with an HDR panorama it received as raw file
//! bytes (protocol v14, `pleochroma_net`'s content-addressed assets), while the viewer
//! lights the same scene from the file it loaded. Their samples are only mergeable if
//! both sides build the identical map -- the same texels AND the same marginal and
//! conditional CDFs, bit for bit. Every node therefore decodes through this one
//! function: the same pinned `image` HDR decoder (`Cargo.lock`), the same RGBE-to-float
//! conversion, the same `EnvironmentMap::from_rgb` importance build. The viewer reads its
//! file and calls this; a worker calls it on the bytes it was sent.
//!
//! # Header metadata
//!
//! The decoder applies the Radiance header the way the format defines physical radiance:
//! every texel is divided by the product of all `EXPOSURE=` lines and, per channel, by the
//! product of all `COLORCORR=` lines. A product that is not finite and strictly positive
//! is an [`EnvMapError::Decode`]. A file WITHOUT those lines (or with all-ones values) is
//! bit-identical to the previous loader, so no existing render moves for it.
//!
//! `PRIMARIES=` is not converted: the renderer takes texels as linear Rec. 709. A file
//! that carries the line is reported through [`HdrHeaderInfo::primaries_ignored`] (see
//! [`environment_from_hdr_bytes_with_info`]) so an app can show a note one day.
//! `PIXASPECT=` is a display hint and is ignored.
//!
//! Because every node runs this one function, viewer and worker stay mergeable -- provided
//! every node runs the same version: an older worker would light a file that carries
//! `EXPOSURE` or `COLORCORR` differently, which is why the remote protocol version is
//! bumped together with this policy.
//!
//! # Untrusted input
//!
//! A worker decodes bytes a peer sent it. [`HdrLimits`] bounds the declared dimensions
//! and the decoded size, checked against the HEADER before the pixel buffer is
//! allocated; malformed data surfaces as [`EnvMapError::Decode`], never a panic.

use super::{EnvMapError, EnvironmentMap};
use image::ImageDecoder;

/// Bytes one decoded texel occupies in the `image` crate's `Rgb32F` buffer.
const DECODED_BYTES_PER_TEXEL: u64 = 12;

/// Bounds for [`environment_from_hdr_bytes`], checked against the file's header before
/// any pixel buffer is allocated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HdrLimits {
    /// Largest accepted width in texels.
    pub max_width: u32,
    /// Largest accepted height in texels.
    pub max_height: u32,
    /// Largest accepted decoded size, `width * height * 12` bytes (`Rgb32F`).
    pub max_decoded_bytes: u64,
}

impl HdrLimits {
    /// The limits every node applies, viewer and worker alike, so a map the viewer can
    /// load is always one a worker accepts: at most 16384 x 8192 texels and at most
    /// 512 MiB decoded -- the decoded cap the `image` crate's default `Limits` already
    /// imposed on the viewer's loader before this function existed.
    pub const DEFAULT: Self = Self {
        max_width: 16_384,
        max_height: 8_192,
        max_decoded_bytes: 512 * 1024 * 1024,
    };

    /// Whether a `width x height` image fits these limits.
    #[must_use]
    pub fn admits(&self, width: u32, height: u32) -> bool {
        width <= self.max_width
            && height <= self.max_height
            && u64::from(width) * u64::from(height) * DECODED_BYTES_PER_TEXEL
                <= self.max_decoded_bytes
    }
}

/// What the Radiance header asked for and this decoder applied.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HdrHeaderInfo {
    /// The product of every `EXPOSURE=` line (1.0 when absent); texels were divided by it.
    pub exposure: f32,
    /// The per-channel product of every `COLORCORR=` line ((1,1,1) when absent); divided out.
    pub color_correction: [f32; 3],
    /// A `PRIMARIES=` line was present; it is not applied (texels are taken as linear Rec. 709).
    pub primaries_ignored: bool,
}

/// Decodes Radiance `.hdr` `bytes` into an [`EnvironmentMap`] under `limits`.
///
/// See the module doc comment for why every node must build its map through this function,
/// and for the header metadata it applies.
///
/// # Errors
///
/// See [`environment_from_hdr_bytes_with_info`].
pub fn environment_from_hdr_bytes(
    bytes: &[u8],
    limits: HdrLimits,
) -> Result<EnvironmentMap, EnvMapError> {
    environment_from_hdr_bytes_with_info(bytes, limits).map(|(map, _)| map)
}

/// [`environment_from_hdr_bytes`] plus what the Radiance header asked for
/// ([`HdrHeaderInfo`]): the same map, and the header values that were applied to it.
///
/// # Errors
///
/// - [`EnvMapError::TooLarge`] when the header declares an image outside `limits`
///   (decided from the header alone; nothing is allocated for the pixels).
/// - [`EnvMapError::Decode`] when the bytes are not a well-formed Radiance HDR image
///   (bad signature, truncated header or scanlines, corrupt run-length data), or when
///   the product of the `EXPOSURE=` lines or any channel of the `COLORCORR=` product is
///   not finite and strictly positive.
/// - [`EnvMapError::ZeroSized`] for a `0 x n` image.
pub fn environment_from_hdr_bytes_with_info(
    bytes: &[u8],
    limits: HdrLimits,
) -> Result<(EnvironmentMap, HdrHeaderInfo), EnvMapError> {
    let decoder = image::codecs::hdr::HdrDecoder::new(std::io::Cursor::new(bytes))
        .map_err(|e| EnvMapError::Decode(e.to_string()))?;
    let (width, height) = decoder.dimensions();
    if !limits.admits(width, height) {
        return Err(EnvMapError::TooLarge {
            width,
            height,
            limits,
        });
    }
    if width == 0 || height == 0 {
        return Err(EnvMapError::ZeroSized);
    }
    // `from_decoder` consumes the decoder, so the header is read first.
    let meta = decoder.metadata();
    // `from_decoder` is exactly what `image::load_from_memory_with_format(.., Hdr)` (the
    // loader this replaced) runs after its own header parse: one `Rgb32F` buffer filled
    // scanline by scanline. `into_rgb32f` is a no-op move for an `Rgb32F` image.
    let rgb = image::DynamicImage::from_decoder(decoder)
        .map_err(|e| EnvMapError::Decode(e.to_string()))?
        .into_rgb32f();
    let (w, h) = (rgb.width() as usize, rgb.height() as usize);
    let mut pixels: Vec<[f32; 3]> = rgb.pixels().map(|p| p.0).collect();

    let exposure = meta.exposure.unwrap_or(1.0);
    let corr = meta.color_correction.map_or([1.0; 3], Into::into);
    let positive_finite = |v: f32| v.is_finite() && v > 0.0;
    // The combined divisor per channel is checked too: two valid factors can still
    // overflow to infinity or underflow to zero when multiplied.
    let divisor = corr.map(|c| exposure * c);
    if !positive_finite(exposure)
        || !corr.into_iter().all(positive_finite)
        || !divisor.into_iter().all(positive_finite)
    {
        return Err(EnvMapError::Decode(format!(
            "EXPOSURE/COLORCORR must be finite and positive, got {exposure} / {corr:?}"
        )));
    }
    // Radiance: stored value / (EXPOSURE * COLORCORR) is the radiance. A division (not a
    // reciprocal multiply) so `EXPOSURE=4` is exact. A file without the lines (or with
    // all-ones values) is left untouched, bit for bit.
    if exposure != 1.0 || corr != [1.0; 3] {
        for p in &mut pixels {
            for (value, d) in p.iter_mut().zip(divisor) {
                *value /= d;
            }
        }
    }
    let primaries_ignored = meta.custom_attributes.iter().any(|(k, _)| k == "PRIMARIES");
    let map = EnvironmentMap::from_rgb(w, h, pixels)?;
    Ok((
        map,
        HdrHeaderInfo {
            exposure,
            color_correction: corr,
            primaries_ignored,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small, genuinely non-uniform Radiance `.hdr` file, encoded by the `image`
    /// crate's own HDR encoder (no fixture file needed).
    fn synthetic_hdr(width: u32, height: u32) -> Vec<u8> {
        let pixels: Vec<image::Rgb<f32>> = (0..height)
            .flat_map(|y| {
                (0..width).map(move |x| {
                    let u = x as f32 / width as f32;
                    let v = y as f32 / height as f32;
                    image::Rgb([
                        (4.0 * u).mul_add(u, 0.05),
                        v.mul_add(2.0, 0.1),
                        (u * 7.0).sin().abs(),
                    ])
                })
            })
            .collect();
        let mut out = Vec::new();
        image::codecs::hdr::HdrEncoder::new(&mut out)
            .encode(&pixels, width as usize, height as usize)
            .expect("encoding a synthetic HDR image into a Vec cannot fail");
        out
    }

    /// Inserts `lines` (each followed by `\n`) into the header of a file the `image`
    /// encoder wrote, right before the blank line that ends the header
    /// (`#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y h +X w\n`).
    fn with_header_lines(hdr: &[u8], lines: &[&str]) -> Vec<u8> {
        let marker = hdr
            .windows(2)
            .position(|w| w == b"\n\n")
            .expect("the encoder's header ends with a blank line");
        let insert_at = marker + 1;
        let mut out = hdr[..insert_at].to_vec();
        for line in lines {
            out.extend_from_slice(line.as_bytes());
            out.push(b'\n');
        }
        out.extend_from_slice(&hdr[insert_at..]);
        out
    }

    /// `map` with every texel channel divided by `divisor[channel]`, rebuilt through
    /// `from_rgb` (the texels AND the importance distribution).
    fn divided(map: &EnvironmentMap, divisor: [f32; 3]) -> EnvironmentMap {
        let pixels: Vec<[f32; 3]> = map
            .texels()
            .iter()
            .map(|p| [p[0] / divisor[0], p[1] / divisor[1], p[2] / divisor[2]])
            .collect();
        EnvironmentMap::from_rgb(map.width(), map.height(), pixels).unwrap()
    }

    fn decode_with_info(bytes: &[u8]) -> (EnvironmentMap, HdrHeaderInfo) {
        environment_from_hdr_bytes_with_info(bytes, HdrLimits::DEFAULT).unwrap()
    }

    /// The map built from a transferred COPY of the bytes equals the locally built one
    /// bit for bit -- texels, marginal and every conditional CDF.
    #[test]
    fn transferred_bytes_build_a_bitwise_identical_map_and_distribution() {
        let bytes = synthetic_hdr(64, 32);
        let transferred = bytes.clone();
        let local = environment_from_hdr_bytes(&bytes, HdrLimits::DEFAULT).unwrap();
        let remote = environment_from_hdr_bytes(&transferred, HdrLimits::DEFAULT).unwrap();
        assert!(local.bitwise_eq(&remote));
        assert_eq!((local.width(), local.height()), (64, 32));
    }

    /// The shared builder reproduces the pre-E loader (`load_from_memory_with_format`
    /// then `from_rgb`) bit for bit, so a viewer upgraded to it renders exactly as before.
    #[test]
    fn the_shared_builder_matches_the_previous_loader_bit_for_bit() {
        let bytes = synthetic_hdr(48, 24);
        let legacy = {
            let rgb = image::load_from_memory_with_format(&bytes, image::ImageFormat::Hdr)
                .unwrap()
                .into_rgb32f();
            let pixels: Vec<[f32; 3]> = rgb.pixels().map(|p| p.0).collect();
            EnvironmentMap::from_rgb(48, 24, pixels).unwrap()
        };
        let shared = environment_from_hdr_bytes(&bytes, HdrLimits::DEFAULT).unwrap();
        assert!(legacy.bitwise_eq(&shared));
        // A different image is detected as different.
        let other = environment_from_hdr_bytes(&synthetic_hdr(48, 23), HdrLimits::DEFAULT);
        assert!(!legacy.bitwise_eq(&other.unwrap()));
    }

    /// A header declaring a huge image is refused from the header alone: no pixel data
    /// follows, so a decoder that allocated and started reading scanlines first would
    /// report a truncation instead of `TooLarge`.
    #[test]
    fn an_oversized_header_is_rejected_before_any_pixel_allocation() {
        let header = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 8193 +X 16384\n";
        let err = environment_from_hdr_bytes(header, HdrLimits::DEFAULT).unwrap_err();
        assert!(
            matches!(
                err,
                EnvMapError::TooLarge {
                    width: 16_384,
                    height: 8_193,
                    ..
                }
            ),
            "{err:?}"
        );
        // Within both axis caps but over the decoded-size cap.
        let tight = HdrLimits {
            max_decoded_bytes: 1024,
            ..HdrLimits::DEFAULT
        };
        let err = environment_from_hdr_bytes(&synthetic_hdr(16, 16), tight).unwrap_err();
        assert!(matches!(err, EnvMapError::TooLarge { .. }), "{err:?}");
        assert!(err.to_string().contains("exceeds the decode limits"));
    }

    /// Malformed bytes are a clean `Decode` error, never a panic.
    #[test]
    fn malformed_bytes_are_a_decode_error() {
        for bad in [
            &b""[..],
            b"not an hdr file at all",
            b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n",
            b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 4 +X 4\n\x02\x02",
        ] {
            let err = environment_from_hdr_bytes(bad, HdrLimits::DEFAULT).unwrap_err();
            assert!(matches!(err, EnvMapError::Decode(_)), "{bad:?}: {err:?}");
        }
        // A valid file cut short mid-scanline.
        let mut truncated = synthetic_hdr(32, 16);
        truncated.truncate(truncated.len() - 40);
        assert!(matches!(
            environment_from_hdr_bytes(&truncated, HdrLimits::DEFAULT),
            Err(EnvMapError::Decode(_))
        ));
    }

    /// A file without header lines reports the neutral info and builds the same map as
    /// the previous loader, bit for bit.
    #[test]
    fn default_headers_stay_bit_identical() {
        let bytes = synthetic_hdr(32, 16);
        let (map, info) = decode_with_info(&bytes);
        assert_eq!(
            info,
            HdrHeaderInfo {
                exposure: 1.0,
                color_correction: [1.0; 3],
                primaries_ignored: false,
            }
        );
        let legacy = {
            let rgb = image::load_from_memory_with_format(&bytes, image::ImageFormat::Hdr)
                .unwrap()
                .into_rgb32f();
            let pixels: Vec<[f32; 3]> = rgb.pixels().map(|p| p.0).collect();
            EnvironmentMap::from_rgb(32, 16, pixels).unwrap()
        };
        assert!(map.bitwise_eq(&legacy));
    }

    /// `EXPOSURE=4.0` divides every texel by 4 exactly, and the importance distribution
    /// is rebuilt from the divided texels.
    #[test]
    fn exposure_divides_every_texel_and_rebuilds_the_distribution() {
        let plain_bytes = synthetic_hdr(32, 16);
        let plain = environment_from_hdr_bytes(&plain_bytes, HdrLimits::DEFAULT).unwrap();
        let bytes = with_header_lines(&plain_bytes, &["EXPOSURE=4.0"]);
        let (map, info) = decode_with_info(&bytes);
        assert_eq!(info.exposure, 4.0);
        assert_eq!(info.color_correction, [1.0; 3]);
        assert!(!map.bitwise_eq(&plain));
        for (got, base) in map.texels().iter().zip(plain.texels()) {
            for (g, b) in got.iter().zip(base) {
                assert_eq!(g.to_bits(), (b / 4.0).to_bits());
            }
        }
        assert!(map.bitwise_eq(&divided(&plain, [4.0; 3])));
    }

    /// Every `EXPOSURE=` line counts: the factors multiply.
    #[test]
    fn several_exposure_lines_multiply() {
        let plain_bytes = synthetic_hdr(32, 16);
        let plain = environment_from_hdr_bytes(&plain_bytes, HdrLimits::DEFAULT).unwrap();
        let bytes = with_header_lines(&plain_bytes, &["EXPOSURE=2.0", "EXPOSURE=4.0"]);
        let (map, info) = decode_with_info(&bytes);
        assert_eq!(info.exposure, 8.0);
        assert!(map.bitwise_eq(&divided(&plain, [8.0; 3])));
    }

    /// `COLORCORR=1.0 0.5 2.0`: red unchanged, green doubled, blue halved.
    #[test]
    fn colorcorr_divides_per_channel() {
        let plain_bytes = synthetic_hdr(32, 16);
        let plain = environment_from_hdr_bytes(&plain_bytes, HdrLimits::DEFAULT).unwrap();
        let bytes = with_header_lines(&plain_bytes, &["COLORCORR=1.0 0.5 2.0"]);
        let (map, info) = decode_with_info(&bytes);
        assert_eq!(info.exposure, 1.0);
        assert_eq!(info.color_correction, [1.0, 0.5, 2.0]);
        for (got, base) in map.texels().iter().zip(plain.texels()) {
            assert_eq!(got[0].to_bits(), base[0].to_bits());
            assert_eq!(got[1].to_bits(), (base[1] * 2.0).to_bits());
            assert_eq!(got[2].to_bits(), (base[2] * 0.5).to_bits());
        }
        assert!(map.bitwise_eq(&divided(&plain, [1.0, 0.5, 2.0])));
    }

    /// Neutral header values leave the map bit-identical to the plain file's.
    #[test]
    fn exposure_one_is_the_identity() {
        let plain_bytes = synthetic_hdr(32, 16);
        let plain = environment_from_hdr_bytes(&plain_bytes, HdrLimits::DEFAULT).unwrap();
        let bytes = with_header_lines(&plain_bytes, &["EXPOSURE=1.0", "COLORCORR=1 1 1"]);
        let (map, info) = decode_with_info(&bytes);
        assert_eq!(info.exposure, 1.0);
        assert_eq!(info.color_correction, [1.0; 3]);
        assert!(map.bitwise_eq(&plain));
    }

    /// A zero, negative, infinite or NaN factor is a decode error naming the header.
    #[test]
    fn non_positive_or_non_finite_exposure_is_a_decode_error() {
        let plain_bytes = synthetic_hdr(32, 16);
        for line in [
            "EXPOSURE=0",
            "EXPOSURE=-2",
            "EXPOSURE=inf",
            "EXPOSURE=nan",
            "COLORCORR=1 0 1",
        ] {
            let bytes = with_header_lines(&plain_bytes, &[line]);
            let err = environment_from_hdr_bytes(&bytes, HdrLimits::DEFAULT).unwrap_err();
            assert!(matches!(err, EnvMapError::Decode(_)), "{line}: {err:?}");
            assert!(err.to_string().contains("EXPOSURE"), "{line}: {err}");
        }
    }

    /// `PRIMARIES=` is reported but not applied.
    #[test]
    fn primaries_is_reported_and_not_applied() {
        let plain_bytes = synthetic_hdr(32, 16);
        let plain = environment_from_hdr_bytes(&plain_bytes, HdrLimits::DEFAULT).unwrap();
        let bytes = with_header_lines(
            &plain_bytes,
            &["PRIMARIES=0.64 0.33 0.29 0.60 0.15 0.06 0.3333 0.3333"],
        );
        let (map, info) = decode_with_info(&bytes);
        assert!(info.primaries_ignored);
        assert!(map.bitwise_eq(&plain));
    }

    /// `PIXASPECT=` is a display hint: the map is untouched.
    #[test]
    fn pixaspect_is_ignored() {
        let plain_bytes = synthetic_hdr(32, 16);
        let plain = environment_from_hdr_bytes(&plain_bytes, HdrLimits::DEFAULT).unwrap();
        let bytes = with_header_lines(&plain_bytes, &["PIXASPECT=2.0"]);
        let (map, info) = decode_with_info(&bytes);
        assert!(!info.primaries_ignored);
        assert!(map.bitwise_eq(&plain));
    }

    /// The transfer test again on a file that carries `EXPOSURE=0.5`: both nodes apply
    /// the header and build the identical map.
    #[test]
    fn a_transferred_copy_with_headers_builds_the_same_map() {
        let plain_bytes = synthetic_hdr(64, 32);
        let plain = environment_from_hdr_bytes(&plain_bytes, HdrLimits::DEFAULT).unwrap();
        let bytes = with_header_lines(&plain_bytes, &["EXPOSURE=0.5"]);
        let transferred = bytes.clone();
        let local = environment_from_hdr_bytes(&bytes, HdrLimits::DEFAULT).unwrap();
        let remote = environment_from_hdr_bytes(&transferred, HdrLimits::DEFAULT).unwrap();
        assert!(local.bitwise_eq(&remote));
        assert!(!local.bitwise_eq(&plain));
        assert!(local.bitwise_eq(&divided(&plain, [0.5; 3])));
    }

    #[test]
    fn limits_admit_exactly_their_bounds() {
        let limits = HdrLimits::DEFAULT;
        assert!(limits.admits(8_192, 4_096));
        assert!(!limits.admits(16_385, 1));
        assert!(!limits.admits(1, 8_193));
        // 16384 x 8192 fits both axes but is 1.5 GiB decoded.
        assert!(!limits.admits(16_384, 8_192));
    }
}
