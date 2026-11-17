//! The pairing link: one `pleochroma://pair?...` string with everything a client needs.
//!
//! It carries what a client needs to claim a certificate from a coordinator -- who it is,
//! where it listens, which access the code grants and the single-use enrollment token. It
//! is what the QR code on the coordinator shows, what "Copy pairing link" puts on the
//! clipboard and what a phone's deep link receives.
//!
//! ```text
//! pleochroma://pair?v=1&r=viewer&n=Studio+PC&a=192.168.178.20:7878&a=10.0.0.5:7878&e=7879&t=GW1-...
//! ```
//!
//! | Key | Meaning |
//! |---|---|
//! | `v` | Link version. This build reads `1`; any other value is [`PairingLinkError::NeedsNewerVersion`]. |
//! | `r` | `viewer`, `worker`, `monitor` or `admin` (the [`PeerRole`] the code grants). |
//! | `n` | Display name of the coordinator, UTF-8, percent-encoded (a space is written `+`). |
//! | `a` | One to three IPv4 `host:port` addresses, default-route interface first. The key may repeat (what [`PairingLink::build`] writes) or hold a comma-separated list (accepted when parsing). |
//! | `e` | The enrollment port, on the same host as whichever `a` answered. |
//! | `t` | The `GW1-` enrollment token (see [`crate::token`]). A secret. |
//!
//! A link is about 217 characters, which fits a version-11 QR code at error correction M.
//!
//! The token is a bearer secret. [`PairingLink`] keeps it in a zeroizing string, its
//! `Debug` output redacts it, and no [`PairingLinkError`] message ever contains any part of
//! the link. Callers must keep the string [`PairingLink::build`] returns out of logs too.
//!
//! Parsing never panics and never allocates more than a few times the input length: input
//! over [`MAX_LINK_LEN`] characters is refused before it is looked at.

use std::{collections::HashSet, fmt, net::SocketAddr};

use zeroize::Zeroizing;

use crate::messages::PeerRole;

#[cfg(test)]
mod tests;

/// The scheme and host every pairing link starts with, up to and including the `?`.
pub const LINK_PREFIX: &str = "pleochroma://pair?";

/// The link version this build reads and writes.
pub const LINK_VERSION: u32 = 1;

/// The most addresses one link may carry.
pub const MAX_ADDRS: usize = 3;

/// The longest display name, in UTF-8 bytes.
pub const MAX_NAME_BYTES: usize = 64;

/// The longest token, in characters. A real `GW1-` token is about 127.
pub const MAX_TOKEN_LEN: usize = 256;

/// The longest link [`PairingLink::parse`] looks at, in characters.
pub const MAX_LINK_LEN: usize = 2048;

/// The token tag every link's token starts with.
const TOKEN_TAG: &str = "GW1-";

/// Why a pairing link could not be built or read. Every message is a plain sentence and
/// none of them contains any text of the link, so they are safe to show and to log.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingLinkError {
    /// The text is not a `pleochroma://pair?...` link at all.
    NotAPairingLink,
    /// The link is longer than [`MAX_LINK_LEN`] characters.
    TooLong,
    /// The link's version is not one this build reads.
    NeedsNewerVersion,
    /// A required part (version, role, address list or enrollment port) is missing.
    MissingPart,
    /// A part appears twice where it may appear once.
    RepeatedPart,
    /// The role is not viewer, worker, monitor or admin.
    UnknownRole,
    /// The name is too long, contains control characters or is not valid text.
    BadName,
    /// An address is not written as `host:port`.
    BadAddress,
    /// An address is not an IPv4 address.
    NotIpv4,
    /// The link carries no address.
    NoAddress,
    /// The link carries more than [`MAX_ADDRS`] addresses.
    TooManyAddresses,
    /// The enrollment port is not a number from 1 to 65535.
    BadPort,
    /// The link carries no token.
    MissingToken,
    /// The token is not shaped like a `GW1-` enrollment token.
    BadToken,
    /// Percent-encoding in the link is broken.
    BadEncoding,
}

impl fmt::Display for PairingLinkError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::NotAPairingLink => "This is not a Pleochroma pairing link.",
            Self::TooLong => "This pairing link is too long to be real.",
            Self::NeedsNewerVersion => "This link needs a newer Pleochroma; update the app.",
            Self::MissingPart => "This pairing link is incomplete.",
            Self::RepeatedPart => "This pairing link repeats a part that should appear once.",
            Self::UnknownRole => "This pairing link names an access level this app does not know.",
            Self::BadName => "This pairing link has an unusable computer name.",
            Self::BadAddress => "This pairing link has an address that is not written host:port.",
            Self::NotIpv4 => "This pairing link has an address that is not IPv4.",
            Self::NoAddress => "This pairing link has no address to connect to.",
            Self::TooManyAddresses => "This pairing link has more than three addresses.",
            Self::BadPort => "This pairing link has an unusable enrollment port.",
            Self::MissingToken => "This pairing link has no code in it.",
            Self::BadToken => "This pairing link has a code that is not shaped right.",
            Self::BadEncoding => "This pairing link is damaged.",
        })
    }
}

impl std::error::Error for PairingLinkError {}

/// The decoded contents of a pairing link.
///
/// Fields are public so a caller can read them; build a new one with
/// [`PairingLink::new`] and write it with [`PairingLink::build`]. The token is a secret:
/// `Debug` redacts it and the string is wiped when the value is dropped.
#[derive(Clone, PartialEq, Eq)]
pub struct PairingLink {
    /// The link version (`v`). Always [`LINK_VERSION`] for a link this build can read.
    pub v: u32,
    /// The access the code grants (`r`).
    pub role: PeerRole,
    /// The coordinator's display name (`n`).
    pub name: String,
    /// One to three IPv4 addresses to try, default-route interface first (`a`).
    pub addrs: Vec<SocketAddr>,
    /// The enrollment port (`e`), on the same host as whichever address answered.
    pub enroll_port: u16,
    /// The `GW1-` enrollment token (`t`). A secret.
    pub token: Zeroizing<String>,
}

impl fmt::Debug for PairingLink {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PairingLink")
            .field("v", &self.v)
            .field("role", &self.role)
            .field("name", &self.name)
            .field("addrs", &self.addrs)
            .field("enroll_port", &self.enroll_port)
            .field("token", &"<redacted>")
            .finish()
    }
}

impl PairingLink {
    /// A version-1 link for `role`. Nothing is checked until [`PairingLink::build`].
    #[must_use]
    pub fn new(
        role: PeerRole,
        name: impl Into<String>,
        addrs: Vec<SocketAddr>,
        enroll_port: u16,
        token: impl Into<String>,
    ) -> Self {
        Self {
            v: LINK_VERSION,
            role,
            name: name.into(),
            addrs,
            enroll_port,
            token: Zeroizing::new(token.into()),
        }
    }

    /// The token as text. A secret: keep it out of logs and error messages.
    #[must_use]
    pub fn token(&self) -> &str {
        self.token.as_str()
    }

    /// Writes the link as `pleochroma://pair?...`. The result contains the token; treat it
    /// as a secret.
    ///
    /// # Errors
    /// The same checks [`PairingLink::parse`] applies: an unknown version, a bad name, no
    /// address, more than three, a non-IPv4 address, port 0, or a malformed token.
    pub fn build(&self) -> Result<String, PairingLinkError> {
        if self.v != LINK_VERSION {
            return Err(PairingLinkError::NeedsNewerVersion);
        }
        check_name(&self.name)?;
        check_addrs(&self.addrs)?;
        if self.enroll_port == 0 {
            return Err(PairingLinkError::BadPort);
        }
        check_token(&self.token)?;
        let mut out = String::with_capacity(LINK_PREFIX.len() + 96 + self.token.len());
        out.push_str(LINK_PREFIX);
        out.push_str("v=");
        out.push_str(&self.v.to_string());
        out.push_str("&r=");
        out.push_str(role_word(self.role));
        out.push_str("&n=");
        percent_encode_into(&mut out, &self.name);
        for addr in &self.addrs {
            out.push_str("&a=");
            out.push_str(&addr.to_string());
        }
        out.push_str("&e=");
        out.push_str(&self.enroll_port.to_string());
        out.push_str("&t=");
        out.push_str(&self.token);
        Ok(out)
    }

    /// Reads a pairing link.
    ///
    /// Surrounding whitespace is ignored, the scheme is matched without regard to case, and
    /// anything after a `#` is dropped. Unknown keys are skipped so a later link version can
    /// add parts.
    ///
    /// The version is checked first, so a link from a newer Pleochroma reports
    /// [`PairingLinkError::NeedsNewerVersion`] even if it is otherwise shaped differently.
    ///
    /// # Errors
    /// A [`PairingLinkError`] naming the first problem found.
    pub fn parse(text: &str) -> Result<Self, PairingLinkError> {
        let text = text.trim();
        if text.chars().count() > MAX_LINK_LEN {
            return Err(PairingLinkError::TooLong);
        }
        let query =
            strip_prefix_ignore_case(text, LINK_PREFIX).ok_or(PairingLinkError::NotAPairingLink)?;
        let query = query.split('#').next().unwrap_or("");

        let mut version = None;
        let mut role = None;
        let mut name = None;
        let mut addr_texts: Vec<&str> = Vec::new();
        let mut port = None;
        let mut token = None;
        for pair in query.split('&').filter(|p| !p.is_empty()) {
            let (key, value) = pair.split_once('=').unwrap_or((pair, ""));
            match key {
                "v" => set_once(&mut version, value)?,
                "r" => set_once(&mut role, value)?,
                "n" => set_once(&mut name, value)?,
                "e" => set_once(&mut port, value)?,
                "t" => set_once(&mut token, value)?,
                "a" => addr_texts.push(value),
                _ => {}
            }
        }

        // The version decides whether the rest means anything, so it is read first.
        let version = version.ok_or(PairingLinkError::MissingPart)?;
        let version: u32 = version.parse().map_err(|_| {
            // A number too large for `u32` is certainly from the future.
            if !version.is_empty() && version.bytes().all(|b| b.is_ascii_digit()) {
                PairingLinkError::NeedsNewerVersion
            } else {
                PairingLinkError::MissingPart
            }
        })?;
        if version != LINK_VERSION {
            return Err(PairingLinkError::NeedsNewerVersion);
        }

        let role = role_from_word(role.ok_or(PairingLinkError::MissingPart)?)?;
        let name = match name {
            Some(raw) => percent_decode(raw)?,
            None => String::new(),
        };
        check_name(&name)?;

        let mut addrs = Vec::new();
        for text in addr_texts {
            for piece in text.split(',').filter(|p| !p.is_empty()) {
                addrs.push(parse_addr(&percent_decode(piece)?)?);
            }
        }
        check_addrs(&addrs)?;

        let enroll_port: u16 = port
            .ok_or(PairingLinkError::MissingPart)?
            .parse()
            .map_err(|_| PairingLinkError::BadPort)?;
        if enroll_port == 0 {
            return Err(PairingLinkError::BadPort);
        }

        let token = token.ok_or(PairingLinkError::MissingToken)?;
        check_token(token)?;

        Ok(Self {
            v: version,
            role,
            name,
            addrs,
            enroll_port,
            token: Zeroizing::new(token.to_owned()),
        })
    }
}

/// The link word for a role.
const fn role_word(role: PeerRole) -> &'static str {
    match role {
        PeerRole::Viewer => "viewer",
        PeerRole::Worker => "worker",
        PeerRole::Monitor => "monitor",
        PeerRole::Admin => "admin",
    }
}

/// The role a link word names.
fn role_from_word(word: &str) -> Result<PeerRole, PairingLinkError> {
    match word {
        "viewer" => Ok(PeerRole::Viewer),
        "worker" => Ok(PeerRole::Worker),
        "monitor" => Ok(PeerRole::Monitor),
        "admin" => Ok(PeerRole::Admin),
        _ => Err(PairingLinkError::UnknownRole),
    }
}

/// Stores `value` in `slot`, or fails if the key was already seen.
const fn set_once<'a>(slot: &mut Option<&'a str>, value: &'a str) -> Result<(), PairingLinkError> {
    if slot.is_some() {
        return Err(PairingLinkError::RepeatedPart);
    }
    *slot = Some(value);
    Ok(())
}

/// `text` without `prefix`, matching `prefix` without regard to ASCII case.
fn strip_prefix_ignore_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    let head = text.get(..prefix.len())?;
    head.eq_ignore_ascii_case(prefix)
        .then(|| &text[prefix.len()..])
}

/// A name must fit [`MAX_NAME_BYTES`] and hold no control characters.
fn check_name(name: &str) -> Result<(), PairingLinkError> {
    if name.len() > MAX_NAME_BYTES || name.chars().any(char::is_control) {
        return Err(PairingLinkError::BadName);
    }
    Ok(())
}

/// One to three distinct IPv4 addresses, none with port 0.
fn check_addrs(addrs: &[SocketAddr]) -> Result<(), PairingLinkError> {
    if addrs.is_empty() {
        return Err(PairingLinkError::NoAddress);
    }
    if addrs.len() > MAX_ADDRS {
        return Err(PairingLinkError::TooManyAddresses);
    }
    let mut seen = HashSet::new();
    for addr in addrs {
        match addr {
            SocketAddr::V4(v4) if v4.port() != 0 => {
                // A repeated address adds nothing; refusing it keeps the list honest.
                if !seen.insert(*v4) {
                    return Err(PairingLinkError::BadAddress);
                }
            }
            SocketAddr::V4(_) => return Err(PairingLinkError::BadAddress),
            SocketAddr::V6(_) => return Err(PairingLinkError::NotIpv4),
        }
    }
    Ok(())
}

/// Reads one `host:port` IPv4 address.
fn parse_addr(text: &str) -> Result<SocketAddr, PairingLinkError> {
    text.parse::<SocketAddr>().map_err(|_| {
        // Distinguish "an IPv6 literal" from "not an address" for a clearer message.
        if text.starts_with('[') {
            PairingLinkError::NotIpv4
        } else {
            PairingLinkError::BadAddress
        }
    })
}

/// A token is `GW1-` followed by letters, digits and hyphens, at most [`MAX_TOKEN_LEN`] long.
fn check_token(token: &str) -> Result<(), PairingLinkError> {
    if token.is_empty() {
        return Err(PairingLinkError::MissingToken);
    }
    let body = token
        .strip_prefix(TOKEN_TAG)
        .ok_or(PairingLinkError::BadToken)?;
    if body.is_empty()
        || token.len() > MAX_TOKEN_LEN
        || !body.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
    {
        return Err(PairingLinkError::BadToken);
    }
    Ok(())
}

/// Appends `text` percent-encoded: unreserved characters stay, a space becomes `+`, and
/// every other byte of its UTF-8 becomes `%XX`.
fn percent_encode_into(out: &mut String, text: &str) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for &byte in text.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(char::from(byte));
            }
            b' ' => out.push('+'),
            _ => {
                out.push('%');
                out.push(char::from(HEX[usize::from(byte >> 4)]));
                out.push(char::from(HEX[usize::from(byte & 0x0F)]));
            }
        }
    }
}

/// Reverses [`percent_encode_into`]: `+` is a space, `%XX` is a byte, and the bytes must
/// be valid UTF-8.
fn percent_decode(text: &str) -> Result<String, PairingLinkError> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' => {
                let hi = bytes.get(i + 1).and_then(|&b| hex_value(b));
                let lo = bytes.get(i + 2).and_then(|&b| hex_value(b));
                match (hi, lo) {
                    (Some(hi), Some(lo)) => out.push((hi << 4) | lo),
                    _ => return Err(PairingLinkError::BadEncoding),
                }
                i += 3;
            }
            other => {
                out.push(other);
                i += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| PairingLinkError::BadName)
}

/// The value of one hex digit.
const fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}
