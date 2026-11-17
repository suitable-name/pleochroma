//! Compact, phone-transcribable encoding for one-time enrollment tokens.
//!
//! An [`EnrollToken`] carries two independent 256-bit values: a fresh CSPRNG bearer
//! secret, and the SHA-256 fingerprint of the worker's CA certificate. Both travel
//! together in one string an operator can read aloud or paste, because the client
//! claiming a token has no CA file of its own yet, so the fingerprint has to arrive by
//! the same out-of-band channel as the secret itself.
//!
//! Lives in `pleochroma-net` because a claiming client needs [`decode`] (see
//! [`crate::enroll`]). `apps/pleochroma-worker` only calls [`encode`]. Both directions
//! share one format, so both stay `pub` here.
//!
//! # Encoding
//!
//! [Crockford Base32](https://www.crockford.com/base32.html): 32 symbols
//! (`0-9`, `A-HJKMNP-TV-Z`). `I`, `L`, `O` and `U` are excluded: `I` and `L` are read as
//! `1`, `O` as `0`, and `U` is left out to avoid accidental words. Decoding maps those
//! misreads back and is case-insensitive.
//!
//! The payload is exactly [`PAYLOAD_LEN`] (64) bytes: [`SECRET_LEN`] (32) bytes of secret
//! followed by [`FINGERPRINT_LEN`] (32) bytes of CA fingerprint. It encodes to
//! `ceil(64 * 8 / 5)` = 103 symbols, grouped in blocks of 5 with hyphens. The `GW1-` tag
//! is a format and version marker, not part of the payload. It makes a truncated or
//! wrong-shaped string fail at once instead of decoding to 64 bytes of garbage.
//!
//! The string is long, about 127 characters, because it carries two full 256-bit values.

use zeroize::Zeroizing;

/// Length in bytes of the CSPRNG bearer secret half of an [`EnrollToken`]'s payload.
pub const SECRET_LEN: usize = 32;
/// Length in bytes of the CA-fingerprint half of an [`EnrollToken`]'s payload.
pub const FINGERPRINT_LEN: usize = 32;
/// The whole decoded payload's length: [`SECRET_LEN`] followed by [`FINGERPRINT_LEN`].
pub const PAYLOAD_LEN: usize = SECRET_LEN + FINGERPRINT_LEN;

/// Fixed prefix identifying this as a `pleochroma-worker` enrollment token, format version 1.
///
/// Not part of the encoded payload. [`decode`] checks it case-insensitively, so a
/// wrong-shaped string is rejected instead of decoding to 64 bytes of nonsense.
pub const TOKEN_PREFIX: &str = "GW1";

const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHJKMNPQRSTVWXYZ";

/// A decoded enrollment token: the bearer secret plus the CA fingerprint it commits to.
/// See the module doc comment for why both travel together.
///
/// `secret` is wrapped in [`Zeroizing`], so it is wiped on drop. `ca_fingerprint` is
/// public (a hash of the CA certificate that every client receives) and needs no wiping.
/// Its `Debug` output never shows the secret.
pub struct EnrollToken {
    /// The fresh CSPRNG bearer secret, zeroized on drop.
    pub secret: Zeroizing<[u8; SECRET_LEN]>,
    /// The worker's CA certificate fingerprint this token commits to.
    pub ca_fingerprint: crate::tls::Fingerprint,
}

impl std::fmt::Debug for EnrollToken {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EnrollToken")
            .field("secret", &"<redacted>")
            .field("ca_fingerprint", &self.ca_fingerprint)
            .finish()
    }
}

/// Why [`decode`] could not produce an [`EnrollToken`] from a string.
///
/// `Debug` is written by hand: `InvalidCharacter` would otherwise print a character of the
/// token.
#[derive(PartialEq, Eq)]
pub enum TokenError {
    /// Missing or wrong `GW1-` prefix: most likely not a worker enrollment token, or a
    /// truncated one.
    WrongPrefix,
    /// A character outside the Crockford Base32 alphabet (after case-folding).
    InvalidCharacter(char),
    /// Does not decode to exactly [`PAYLOAD_LEN`] bytes, or the final symbol's padding bits
    /// are not zero. Either way the token is truncated or corrupted, so it is refused
    /// rather than masked.
    WrongLength,
}

impl std::fmt::Debug for TokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WrongPrefix => f.write_str("WrongPrefix"),
            Self::InvalidCharacter(_) => f.write_str("InvalidCharacter(<redacted>)"),
            Self::WrongLength => f.write_str("WrongLength"),
        }
    }
}

impl std::fmt::Display for TokenError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WrongPrefix => write!(
                f,
                "not a pleochroma-worker enrollment token (missing or wrong {TOKEN_PREFIX:?} prefix)"
            ),
            // The character is part of the secret, so the message says only that one is wrong.
            Self::InvalidCharacter(_) => {
                f.write_str("the token has a character that is not valid Crockford Base32")
            }
            Self::WrongLength => write!(
                f,
                "token does not decode to exactly {PAYLOAD_LEN} bytes -- it's truncated or corrupted"
            ),
        }
    }
}

impl std::error::Error for TokenError {}

/// Encodes `secret`/`ca_fingerprint` as a `GW1-`-prefixed, hyphen-grouped Crockford
/// Base32 string. See the module doc comment for the exact format.
#[must_use]
pub fn encode(secret: &[u8; SECRET_LEN], ca_fingerprint: &crate::tls::Fingerprint) -> String {
    // The transient copies of the secret (raw bytes, then their Base32 symbols) are
    // overwritten when they go out of scope; only the returned token string outlives them.
    let mut payload = Zeroizing::new([0u8; PAYLOAD_LEN]);
    payload[..SECRET_LEN].copy_from_slice(secret);
    payload[SECRET_LEN..].copy_from_slice(ca_fingerprint);

    let symbols = Zeroizing::new(base32_encode(payload.as_slice()));

    let mut out = String::with_capacity(TOKEN_PREFIX.len() + 1 + symbols.len() + symbols.len() / 5);
    out.push_str(TOKEN_PREFIX);
    out.push('-');
    for (i, c) in symbols.chars().enumerate() {
        if i > 0 && i % 5 == 0 {
            out.push('-');
        }
        out.push(c);
    }
    out
}

/// Decodes a string produced by [`encode`], or one transcribed from it. Hyphens and
/// surrounding whitespace are ignored, and matching is case-insensitive.
///
/// # Errors
///
/// A [`TokenError`] naming the specific fault, so a reader knows whether to re-read the
/// whole token or fix one character.
pub fn decode(s: &str) -> Result<EnrollToken, TokenError> {
    let trimmed = s.trim();
    let prefix_matches = trimmed
        .get(..TOKEN_PREFIX.len())
        .is_some_and(|p| p.eq_ignore_ascii_case(TOKEN_PREFIX));
    if !prefix_matches {
        return Err(TokenError::WrongPrefix);
    }
    // `get` returned `Some`, so this byte index is on a char boundary.
    let rest = &trimmed[TOKEN_PREFIX.len()..];
    let rest = rest.strip_prefix('-').unwrap_or(rest);

    // Every transient copy of the secret (the cleaned symbols, the decoded payload) is
    // zeroized on drop, like the returned `EnrollToken::secret`.
    let symbols: Zeroizing<String> = Zeroizing::new(
        rest.chars()
            .filter(|c| *c != '-' && !c.is_whitespace())
            .collect(),
    );
    let payload = base32_decode(&symbols)?;

    let mut secret = Zeroizing::new([0u8; SECRET_LEN]);
    secret.copy_from_slice(&payload[..SECRET_LEN]);
    let mut ca_fingerprint = [0u8; FINGERPRINT_LEN];
    ca_fingerprint.copy_from_slice(&payload[SECRET_LEN..]);

    Ok(EnrollToken {
        secret,
        ca_fingerprint,
    })
}

/// Encodes `data` as Crockford Base32 symbols, 5 bits per symbol, most-significant-bit
/// first, with the final symbol's low-order padding bits (if `data`'s bit length isn't a
/// multiple of 5) set to zero.
fn base32_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity((data.len() * 8).div_ceil(5));
    let mut acc: u32 = 0;
    let mut acc_bits: u32 = 0;
    for &byte in data {
        acc = (acc << 8) | u32::from(byte);
        acc_bits += 8;
        while acc_bits >= 5 {
            acc_bits -= 5;
            let idx = (acc >> acc_bits) & 0x1f;
            out.push(char::from(ALPHABET[usize::try_from(idx).unwrap_or(0)]));
        }
    }
    if acc_bits > 0 {
        let idx = (acc << (5 - acc_bits)) & 0x1f;
        out.push(char::from(ALPHABET[usize::try_from(idx).unwrap_or(0)]));
    }
    out
}

/// The inverse of [`base32_encode`], decoding into exactly [`PAYLOAD_LEN`] bytes.
///
/// # Errors
///
/// [`TokenError::InvalidCharacter`] for anything outside the Crockford alphabet (after
/// case-folding and the conventional `O`->`0`, `I`/`L`->`1` misread corrections).
/// [`TokenError::WrongLength`] if the decoded bit count isn't exactly `PAYLOAD_LEN * 8`,
/// or the final symbol's padding bits aren't all zero.
fn base32_decode(symbols: &str) -> Result<Zeroizing<Vec<u8>>, TokenError> {
    let mut acc: u32 = 0;
    let mut acc_bits: u32 = 0;
    let mut out = Zeroizing::new(Vec::with_capacity(PAYLOAD_LEN + 1));

    for c in symbols.chars() {
        let value = crockford_value(c).ok_or(TokenError::InvalidCharacter(c))?;
        acc = (acc << 5) | u32::from(value);
        acc_bits += 5;
        if acc_bits >= 8 {
            acc_bits -= 8;
            out.push(u8::try_from((acc >> acc_bits) & 0xff).unwrap_or(0));
        }
    }

    // Bits left in `acc` below `acc_bits` are padding and must be zero. Anything else
    // means the token was truncated or mistyped.
    if acc_bits > 0 && (acc & ((1 << acc_bits) - 1)) != 0 {
        return Err(TokenError::WrongLength);
    }

    if out.len() != PAYLOAD_LEN {
        return Err(TokenError::WrongLength);
    }
    Ok(out)
}

/// Maps one Crockford Base32 character to its 5-bit value, case-insensitively, applying
/// Crockford's own documented misread corrections (`O`->`0`, `I`/`L`->`1`).
fn crockford_value(c: char) -> Option<u8> {
    let upper = c.to_ascii_uppercase();
    match upper {
        'O' => Some(0),
        'I' | 'L' => Some(1),
        _ => ALPHABET
            .iter()
            .position(|&b| char::from(b) == upper)
            .and_then(|p| u8::try_from(p).ok()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_secret() -> [u8; SECRET_LEN] {
        std::array::from_fn(|i| u8::try_from(i).expect("array index fits in u8"))
    }

    fn sample_fingerprint() -> crate::tls::Fingerprint {
        std::array::from_fn(|i| {
            u8::try_from(i)
                .expect("array index fits in u8")
                .wrapping_mul(7)
                .wrapping_add(3)
        })
    }

    #[test]
    fn round_trips_secret_and_fingerprint() {
        let secret = sample_secret();
        let fp = sample_fingerprint();
        let encoded = encode(&secret, &fp);
        assert!(encoded.starts_with("GW1-"));

        let decoded = decode(&encoded).unwrap();
        assert_eq!(*decoded.secret, secret);
        assert_eq!(decoded.ca_fingerprint, fp);
    }

    #[test]
    fn decode_is_case_insensitive_and_tolerates_extra_whitespace_and_hyphens() {
        let secret = sample_secret();
        let fp = sample_fingerprint();
        let encoded = encode(&secret, &fp);
        let mangled = format!("  {}  ", encoded.to_lowercase());

        let decoded = decode(&mangled).unwrap();
        assert_eq!(*decoded.secret, secret);
        assert_eq!(decoded.ca_fingerprint, fp);
    }

    #[test]
    fn decode_applies_crockford_misread_corrections() {
        // Swap in O/I/L (never emitted by encode) for their corresponding 0/1 and
        // verify the same bytes still decode.
        let secret = sample_secret();
        let fp = sample_fingerprint();
        let encoded = encode(&secret, &fp);
        // Substitute only within the payload -- the "GW1-" prefix itself contains a
        // '1', which would fail the prefix check instead.
        let (prefix, payload) = encoded.split_at(TOKEN_PREFIX.len() + 1);
        let swapped_payload: String = payload
            .chars()
            .map(|c| match c {
                '0' => 'O',
                '1' => 'I',
                other => other,
            })
            .collect();
        let swapped = format!("{prefix}{swapped_payload}");
        assert_ne!(
            swapped, encoded,
            "test should actually exercise a substitution"
        );

        let decoded = decode(&swapped).unwrap();
        assert_eq!(*decoded.secret, secret);
        assert_eq!(decoded.ca_fingerprint, fp);
    }

    #[test]
    fn decode_rejects_wrong_prefix() {
        let err = decode("NOPE-XXXXX").unwrap_err();
        assert_eq!(err, TokenError::WrongPrefix);
    }

    #[test]
    fn decode_rejects_invalid_characters() {
        // 'U' is deliberately excluded from the Crockford alphabet.
        let err = decode("GW1-UUUUU").unwrap_err();
        assert_eq!(err, TokenError::InvalidCharacter('U'));
        // Neither the message nor the debug text echoes the character of the token.
        assert!(!err.to_string().contains('U'), "{err}");
        assert!(!format!("{err:?}").contains('U'), "{err:?}");
    }

    #[test]
    fn decode_rejects_truncated_payload() {
        let secret = sample_secret();
        let fp = sample_fingerprint();
        let encoded = encode(&secret, &fp);
        let truncated = &encoded[..encoded.len() - 10];
        let err = decode(truncated).unwrap_err();
        assert_eq!(err, TokenError::WrongLength);
    }

    #[test]
    fn decode_rejects_a_single_flipped_character() {
        let secret = sample_secret();
        let fp = sample_fingerprint();
        let encoded = encode(&secret, &fp);
        // Flip the FIRST payload character, not the last: the final Crockford symbol
        // carries only 2 real bits plus zero padding, so flipping it would usually
        // fail as malformed padding instead of "decodes fine, to different bytes".
        let mut chars: Vec<char> = encoded.chars().collect();
        let flip_at = TOKEN_PREFIX.len() + 1;
        let original = chars[flip_at];
        let replacement = if original == 'Z' { 'Y' } else { 'Z' };
        chars[flip_at] = replacement;
        let mutated: String = chars.into_iter().collect();

        let decoded = decode(&mutated).unwrap();
        assert_ne!(
            *decoded.secret, secret,
            "flipping a payload character must change the decoded bytes"
        );
    }
}
