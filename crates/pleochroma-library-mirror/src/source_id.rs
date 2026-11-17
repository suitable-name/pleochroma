//! The `source_id` a mirror sync attributes its designs to.

/// What every mirror `source_id` starts with.
///
/// It marks a row (`diagram_entries.source_id`,
/// `library_mirror_state.source_id`) as copied from a remote library, as opposed to the user's
/// own `local` rows.
pub const SOURCE_ID_PREFIX: &str = "remote-library:";

/// The `source_id` for the remote identified by `remote_key`: [`SOURCE_ID_PREFIX`] plus the
/// key. Cut keys a remote by its worker address; the phone keys it by the pairing's CA tag,
/// because its saved addresses are rewritten when the computer is found again.
#[must_use]
pub fn source_id(remote_key: &str) -> String {
    format!("{SOURCE_ID_PREFIX}{remote_key}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_id_is_prefixed() {
        assert_eq!(
            source_id("10.0.0.5:9443"),
            "remote-library:10.0.0.5:9443".to_string()
        );
        assert!(source_id("ca-1234").starts_with(SOURCE_ID_PREFIX));
        assert_ne!(source_id("a"), source_id("b"));
    }
}
