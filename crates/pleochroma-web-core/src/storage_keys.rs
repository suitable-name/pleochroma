//! Pure key and format helpers for the page's browser storage layers.
//!
//! No `web-sys`, so they compile and test on a native host (the page's `storage` module
//! calls them from the wasm-only side).
//!
//! Every key is `pleochroma.<name>.v<version>`. The version is part of the key, so a
//! changed format starts from an empty slot instead of misreading an old entry.
//!
//! # Cutting marks
//!
//! The marks blob of Cutting Mode is filed under one stone key ([`marks_key`]). The stone
//! key of a design is [`mark_keys`]: `id-<design id>` when the design carries a `[meta]` id
//! (a design file saved once), else `sha-<SHA-256 of the design's native text>`. This is
//! the rule of the Android app (`pleochroma-mobile-core`, `cutting/key.rs`), so a design
//! hashes to the same name on both. A design that gets its id on the first save adopts the
//! marks it had under its text hash ([`MarkKeys::legacy`]). The blob is [`StoredMarks`].
//! The marks live in this browser only: nothing leaves it, and another browser or a
//! cleared site starts without them.
//!
//! # Tutorials
//!
//! Two small `localStorage` entries belong to the tutorial runner: the lessons the reader has
//! finished ([`TutorialMarks`], key [`tutorials_key`]) and the place in the running lesson
//! ([`GuidePlace`], key [`guide_place_key`]). Both keep ids only, never lesson text, so a
//! reworded lesson keeps its mark.

use pleochroma_cut_core::{
    Design,
    native::{DesignExtras, design_to_file},
};
use pleochroma_formats::native::design as design_file;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

/// The prefix every key this app writes starts with.
pub const PREFIX: &str = "pleochroma.";
/// The `<name>` of the per-stone cutting-mark blobs (`storage::marks`).
pub const MARKS_NAME: &str = "marks";
/// The format version of a marks blob.
pub const MARKS_VERSION: u32 = 1;
/// The `<name>` of the per-adapter GPU admission records (`storage::admission`).
pub const ADMISSION_NAME: &str = "admission";
/// The format version of an admission record.
pub const ADMISSION_VERSION: u32 = 1;
/// The longest sanitized `<segment>` a caller-supplied key part may contribute, so a
/// long adapter description cannot grow a key without bound.
pub const MAX_SEGMENT_CHARS: usize = 96;

/// `pleochroma.<name>.v<version>`.
#[must_use]
pub fn local_key(name: &str, version: u32) -> String {
    format!("{PREFIX}{name}.v{version}")
}

/// A caller-supplied string made safe as one key segment.
///
/// ASCII letters and digits are lower-cased, `-` and `_` kept, every other character (dots
/// included, so a segment can never forge the `.v<version>` suffix) becomes `_`; at most
/// [`MAX_SEGMENT_CHARS`] characters. The empty string becomes `_`.
#[must_use]
pub fn sanitize_segment(raw: &str) -> String {
    let cleaned: String = raw
        .chars()
        .take(MAX_SEGMENT_CHARS)
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else if c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "_".to_string()
    } else {
        cleaned
    }
}

/// `pleochroma.marks.<stone>.v1`: the cutting-mark blob of one stone.
#[must_use]
pub fn marks_key(stone: &str) -> String {
    local_key(
        &format!("{MARKS_NAME}.{}", sanitize_segment(stone)),
        MARKS_VERSION,
    )
}

/// `pleochroma.admission.<adapter>.v1`: the GPU admission record of one adapter.
#[must_use]
pub fn admission_key(adapter: &str) -> String {
    local_key(
        &format!("{ADMISSION_NAME}.{}", sanitize_segment(adapter)),
        ADMISSION_VERSION,
    )
}

/// Normalizes `adapter_or_key` into a full admission storage key: a value that already begins
/// with [`PREFIX`] is returned unchanged, anything else is an adapter name for
/// [`admission_key`].
#[must_use]
pub fn resolve_admission_key(adapter_or_key: &str) -> String {
    if adapter_or_key.starts_with(PREFIX) {
        adapter_or_key.to_owned()
    } else {
        admission_key(adapter_or_key)
    }
}

/// Whether `key` is one this app wrote (starts with [`PREFIX`]).
#[must_use]
pub fn is_app_key(key: &str) -> bool {
    key.starts_with(PREFIX)
}

/// The stone key a design whose text cannot be written is filed under. Every such design
/// shares it, so it is never something to adopt marks from or to delete on behalf of one
/// of them.
pub const UNREADABLE_TEXT_KEY: &str = "sha-unreadable";
/// The `version` written into every [`StoredMarks`] blob.
pub const STORED_MARKS_VERSION: u32 = 1;

/// The cutting marks of one design as stored: step key (or index key) to the fingerprint
/// of the step's cutting values at the moment it was marked.
///
/// The same shape as the Android app's progress file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredMarks {
    /// [`STORED_MARKS_VERSION`] when written; 0 when the field was missing.
    #[serde(default)]
    pub version: u32,
    /// Step key (or index key) to the step fingerprint it was marked at.
    #[serde(default)]
    pub marks: BTreeMap<String, String>,
}

impl StoredMarks {
    /// The blob that stores `marks`.
    #[must_use]
    pub const fn new(marks: BTreeMap<String, String>) -> Self {
        Self {
            version: STORED_MARKS_VERSION,
            marks,
        }
    }
}

/// The two stone keys of a design's cutting marks. See the module docs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MarkKeys {
    /// The stone key the marks are stored under now.
    pub primary: String,
    /// The stone key the marks had before the design got an id, when [`Self::primary`] is
    /// an id key. Read when the primary slot is empty; removed together with the primary
    /// by a reset.
    pub legacy: Option<String>,
}

impl MarkKeys {
    /// Every stone key a blob of this design can sit under, the current one first. The
    /// shared key of an unwritable design is left out.
    #[must_use]
    pub fn into_stems(self) -> Vec<String> {
        let mut stems = vec![self.primary];
        stems.extend(self.legacy);
        stems.retain(|stem| stem != UNREADABLE_TEXT_KEY);
        stems
    }
}

/// The stone keys of `design`, whose `[meta]` id is `id` (empty when it has none).
///
/// The text hashed is the design's native text without metadata, so the marks of an
/// unsaved design belong to exactly this design and are gone after any edit (an undo
/// brings them back).
#[must_use]
pub fn mark_keys(id: &str, design: &Design) -> MarkKeys {
    mark_keys_from_text(id, native_text(design).as_deref())
}

/// [`mark_keys`] over the design's already written native `text` (`None` when it could not
/// be written). The pure core of the rule.
#[must_use]
pub fn mark_keys_from_text(id: &str, text: Option<&str>) -> MarkKeys {
    let text_key = text.map_or_else(
        || UNREADABLE_TEXT_KEY.to_owned(),
        |text| format!("sha-{}", sha256_hex(text.as_bytes())),
    );
    let id = sanitize_id(id);
    if id.is_empty() {
        MarkKeys {
            primary: text_key,
            legacy: None,
        }
    } else {
        MarkKeys {
            primary: format!("id-{id}"),
            // The shared key of an unwritable design belongs to no design in particular:
            // binding to it would adopt the marks of another design.
            legacy: (text_key != UNREADABLE_TEXT_KEY).then_some(text_key),
        }
    }
}

/// The native design text without metadata, the text [`mark_keys`] hashes; `None` when it
/// cannot be written.
#[must_use]
pub fn native_text(design: &Design) -> Option<String> {
    let file = design_to_file(design, None, &DesignExtras::default());
    design_file::to_string(&file).ok()
}

/// `id` reduced to the characters of a UUID in lower case, so it is safe as a key segment
/// and the same id always reads as the same key.
#[must_use]
pub fn sanitize_id(id: &str) -> String {
    id.trim()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .map(|c| c.to_ascii_lowercase())
        .collect()
}

/// SHA-256 of `data` as 64 lowercase hexadecimal digits.
#[must_use]
pub fn sha256_hex(data: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;

    Sha256::digest(data)
        .iter()
        .fold(String::with_capacity(64), |mut hex, byte| {
            let _ = write!(hex, "{byte:02x}");
            hex
        })
}

/// The `<name>` of the finished-lessons entry (`editor::tutorials`).
pub const TUTORIALS_NAME: &str = "tutorials";
/// The format version of the finished-lessons entry.
pub const TUTORIALS_VERSION: u32 = 1;
/// The `<name>` of the running lesson's place (`editor::guide`).
pub const GUIDE_PLACE_NAME: &str = "guide";
/// The format version of the running lesson's place. Version 1 was the ten-step walkthrough's
/// place ([`crate::guide::GuideProgress`]); [`GuidePlace::from_legacy`] reads it once.
pub const GUIDE_PLACE_VERSION: u32 = 2;
/// The most finished lessons one browser remembers. The built-in learning path has fewer than
/// a hundred; the rest of the room is for the lessons built from library designs.
pub const MAX_TUTORIAL_MARKS: usize = 512;

/// `pleochroma.tutorials.v1`: the lessons the reader has finished.
#[must_use]
pub fn tutorials_key() -> String {
    local_key(TUTORIALS_NAME, TUTORIALS_VERSION)
}

/// `pleochroma.guide.v2`: where the reader is in the running lesson.
#[must_use]
pub fn guide_place_key() -> String {
    local_key(GUIDE_PLACE_NAME, GUIDE_PLACE_VERSION)
}

/// The lessons the reader has finished, as stored: a set of lesson ids.
///
/// The tutorial browser shows a "Done" mark on each of them. An entry that is not a lesson id
/// is dropped when the blob is read, and a full blob (see [`MAX_TUTORIAL_MARKS`]) refuses new
/// ids rather than forgetting old ones.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TutorialMarks {
    /// [`TUTORIALS_VERSION`] when written; 0 when the field was missing.
    #[serde(default)]
    pub version: u32,
    /// The ids of the finished lessons.
    #[serde(default)]
    pub done: BTreeSet<String>,
}

impl TutorialMarks {
    /// Parses a stored blob and repairs it (see [`Self::sanitized`]).
    ///
    /// # Errors
    ///
    /// The JSON parser's message (the caller discards the stored entry).
    pub fn from_json(text: &str) -> Result<Self, String> {
        serde_json::from_str::<Self>(text)
            .map(Self::sanitized)
            .map_err(|e| e.to_string())
    }

    /// The JSON to store.
    ///
    /// # Errors
    ///
    /// The serializer's message (in practice unreachable for these types).
    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|e| e.to_string())
    }

    /// `self` with only valid lesson ids, at most [`MAX_TUTORIAL_MARKS`] of them, and the
    /// current version.
    #[must_use]
    pub fn sanitized(mut self) -> Self {
        self.done
            .retain(|id| pleochroma_editor::guide::is_valid_guide_id(id));
        while self.done.len() > MAX_TUTORIAL_MARKS {
            self.done.pop_last();
        }
        self.version = TUTORIALS_VERSION;
        self
    }

    /// Notes that lesson `id` was finished. Returns whether that changed anything: `false` for
    /// an id seen before, an id that is not a lesson id, and any id once the blob is full.
    pub fn record(&mut self, id: &str) -> bool {
        if !pleochroma_editor::guide::is_valid_guide_id(id) || self.done.len() >= MAX_TUTORIAL_MARKS
        {
            return false;
        }
        self.version = TUTORIALS_VERSION;
        self.done.insert(id.to_owned())
    }

    /// Forgets every mark. Returns whether there was any.
    pub fn clear(&mut self) -> bool {
        let had_any = !self.done.is_empty();
        self.done.clear();
        had_any
    }
}

/// Where the reader is in the running lesson: which lesson, and the panel's state.
///
/// Only the place is stored, never a "done" moment: a step is judged from the design, and the
/// restored design decides again whether the restored step is already complete.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GuidePlace {
    /// The lesson id (`pleochroma_editor::guide::Guide::id`).
    pub guide: String,
    /// The panel is showing (and its locks are in force).
    pub open: bool,
    /// The current step, 0-based into the lesson's web view.
    pub step: usize,
    /// The panel is collapsed to its "Guide - Step N of M" pill.
    pub collapsed: bool,
    /// The floating panel was dragged; [`Self::float_x`] and [`Self::float_y`] hold the spot.
    pub float_placed: bool,
    /// The dragged panel's left edge, logical pixels.
    pub float_x: f32,
    /// The dragged panel's top edge, logical pixels.
    pub float_y: f32,
}

impl Default for GuidePlace {
    fn default() -> Self {
        Self {
            guide: pleochroma_editor::guide::WORKED_EXAMPLE_ID.to_owned(),
            open: false,
            step: 0,
            collapsed: false,
            float_placed: false,
            float_x: 0.0,
            float_y: 0.0,
        }
    }
}

impl GuidePlace {
    /// Parses a stored place and repairs it (see [`Self::sanitized`]).
    ///
    /// # Errors
    ///
    /// The JSON parser's message (the caller discards the stored entry).
    pub fn from_json(text: &str) -> Result<Self, String> {
        serde_json::from_str::<Self>(text)
            .map(Self::sanitized)
            .map_err(|e| e.to_string())
    }

    /// The JSON to store.
    ///
    /// # Errors
    ///
    /// The serializer's message (in practice unreachable for these types).
    pub fn to_json(&self) -> Result<String, String> {
        serde_json::to_string(self).map_err(|e| e.to_string())
    }

    /// The place version 1 stored (the ten-step walkthrough), as a place in that lesson.
    #[must_use]
    pub fn from_legacy(legacy: &crate::guide::GuideProgress) -> Self {
        Self {
            guide: pleochroma_editor::guide::WORKED_EXAMPLE_ID.to_owned(),
            open: legacy.open,
            step: legacy.step,
            collapsed: legacy.collapsed,
            float_placed: legacy.float_placed,
            float_x: legacy.float_x,
            float_y: legacy.float_y,
        }
    }

    /// `self` with every field in range: a position that is not a finite, non-negative pair
    /// counts as never dragged, and a lesson id that is not one is a closed panel on the
    /// default lesson.
    #[must_use]
    pub fn sanitized(mut self) -> Self {
        let sane = |v: f32| v.is_finite() && v >= 0.0;
        if !(sane(self.float_x) && sane(self.float_y)) {
            self.float_placed = false;
            self.float_x = 0.0;
            self.float_y = 0.0;
        }
        if !pleochroma_editor::guide::is_valid_guide_id(&self.guide) {
            self = Self::default();
        }
        self
    }

    /// The step, kept inside a lesson of `steps` steps (the last step when it is past the end).
    #[must_use]
    pub const fn step_within(&self, steps: usize) -> usize {
        let last = steps.saturating_sub(1);
        if self.step > last { last } else { self.step }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_cut_core::templates::TEMPLATES;
    use pleochroma_editor::EditorSession;

    /// The standard round brilliant template as a design.
    fn template_design() -> Design {
        let template = &TEMPLATES[0];
        let mut design = EditorSession::from_template(
            template.fresh_spec(template.default_material_selection()),
            1,
        )
        .design;
        design.meta.headers = vec!["Test Stone".to_owned()];
        design
    }

    #[test]
    fn local_key_has_prefix_and_version() {
        assert_eq!(local_key("settings", 1), "pleochroma.settings.v1");
        assert_eq!(local_key("guide", 12), "pleochroma.guide.v12");
    }

    #[test]
    fn sanitize_replaces_unsafe_characters() {
        assert_eq!(
            sanitize_segment("NVIDIA RTX 4090 (D3D12)"),
            "nvidia_rtx_4090__d3d12_"
        );
        assert_eq!(sanitize_segment("a.b.v9"), "a_b_v9");
        assert_eq!(sanitize_segment("keep-this_one"), "keep-this_one");
    }

    #[test]
    fn sanitize_bounds_length_and_never_returns_empty() {
        assert_eq!(sanitize_segment(""), "_");
        let long = "x".repeat(500);
        assert_eq!(sanitize_segment(&long).chars().count(), MAX_SEGMENT_CHARS);
    }

    #[test]
    fn sanitize_handles_non_ascii_without_panicking() {
        assert_eq!(sanitize_segment("Grüße"), "gr__e");
    }

    #[test]
    fn marks_and_admission_keys_are_distinct_and_versioned() {
        let m = marks_key("Round Brilliant #3");
        let a = admission_key("Round Brilliant #3");
        assert_eq!(m, "pleochroma.marks.round_brilliant__3.v1");
        assert_eq!(a, "pleochroma.admission.round_brilliant__3.v1");
        assert_ne!(m, a);
        assert!(is_app_key(&m) && is_app_key(&a));
        assert!(!is_app_key("other.key"));
    }

    #[test]
    fn sha256_matches_the_published_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        // Two blocks of input (the padding spills into a second block).
        assert_eq!(
            sha256_hex(b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq"),
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1"
        );
    }

    #[test]
    fn sanitize_id_keeps_only_uuid_characters_in_lower_case() {
        assert_eq!(sanitize_id(" 8f2c-AA01/../x "), "8f2c-aa01x");
        assert_eq!(sanitize_id(""), "");
        assert_eq!(sanitize_id("  "), "");
    }

    #[test]
    fn a_design_with_an_id_is_keyed_by_the_id_and_remembers_its_text_key() {
        let keys = mark_keys_from_text("8F2C-aa01", Some("text"));
        assert_eq!(keys.primary, "id-8f2c-aa01");
        assert_eq!(keys.legacy, Some(format!("sha-{}", sha256_hex(b"text"))));
    }

    #[test]
    fn a_design_without_an_id_is_keyed_by_the_hash_of_its_text() {
        let keys = mark_keys_from_text("", Some("text"));
        assert_eq!(keys.primary, format!("sha-{}", sha256_hex(b"text")));
        assert_eq!(keys.legacy, None);
        // A blank id is no id.
        assert_eq!(mark_keys_from_text("   ", Some("text")), keys);
        // Another text is another key.
        assert_ne!(mark_keys_from_text("", Some("other")).primary, keys.primary);
    }

    #[test]
    fn an_unwritable_design_shares_one_key_that_is_never_adopted() {
        let plain = mark_keys_from_text("", None);
        assert_eq!(plain.primary, UNREADABLE_TEXT_KEY);
        assert_eq!(plain.legacy, None);
        let with_id = mark_keys_from_text("abc", None);
        assert_eq!(with_id.primary, "id-abc");
        assert_eq!(with_id.legacy, None, "the shared key is not a legacy key");
    }

    #[test]
    fn stems_list_the_current_key_first_and_skip_the_shared_key() {
        let both = MarkKeys {
            primary: "id-abc".to_owned(),
            legacy: Some("sha-123".to_owned()),
        };
        assert_eq!(both.into_stems(), ["id-abc", "sha-123"]);
        let unreadable = MarkKeys {
            primary: UNREADABLE_TEXT_KEY.to_owned(),
            legacy: None,
        };
        assert!(unreadable.into_stems().is_empty());
    }

    #[test]
    fn a_designs_keys_hash_its_native_text_and_follow_its_edits() {
        let mut design = template_design();
        let keys = mark_keys("", &design);
        assert!(
            keys.primary.starts_with("sha-") && keys.primary.len() == "sha-".len() + 64,
            "{keys:?}"
        );
        assert_eq!(
            mark_keys("", &design),
            keys,
            "the same design, the same key"
        );

        let text = native_text(&design).expect("a template design writes");
        assert_eq!(mark_keys_from_text("", Some(&text)), keys);

        let with_id = mark_keys("8f2c-AA01", &design);
        assert_eq!(with_id.primary, "id-8f2c-aa01");
        assert_eq!(with_id.legacy.as_deref(), Some(keys.primary.as_str()));

        design.meta.headers = vec!["Another Stone".to_owned()];
        assert_ne!(mark_keys("", &design).primary, keys.primary);
    }

    #[test]
    fn resolve_admission_key_handles_both_adapter_and_full_key() {
        assert_eq!(
            resolve_admission_key("intel_arc"),
            "pleochroma.admission.intel_arc.v1"
        );
        assert_eq!(
            resolve_admission_key("pleochroma.admission.custom.v1"),
            "pleochroma.admission.custom.v1"
        );
    }

    #[test]
    fn the_stone_keys_are_safe_blob_slots() {
        let design = template_design();
        let key = marks_key(&mark_keys("", &design).primary);
        assert!(key.starts_with("pleochroma.marks.sha-") && key.strip_suffix(".v1").is_some());
        assert_eq!(
            marks_key("id-8f2c-aa01"),
            "pleochroma.marks.id-8f2c-aa01.v1"
        );
        // A long hash key is not cut short by the segment bound.
        assert_eq!(key.len(), "pleochroma.marks.sha-.v1".len() + 64);
    }

    #[test]
    fn stored_marks_round_trip_and_tolerate_missing_fields() {
        let marks = BTreeMap::from([
            ("t3".to_owned(), "00ff00ff00ff00ff".to_owned()),
            ("t3#12".to_owned(), "00ff00ff00ff00ff".to_owned()),
        ]);
        let stored = StoredMarks::new(marks);
        assert_eq!(stored.version, STORED_MARKS_VERSION);
        let json = serde_json::to_string(&stored).expect("serializes");
        let back: StoredMarks = serde_json::from_str(&json).expect("parses");
        assert_eq!(back, stored);

        let empty: StoredMarks = serde_json::from_str("{}").expect("an empty object parses");
        assert_eq!(empty, StoredMarks::default());
        assert!(serde_json::from_str::<StoredMarks>("not json").is_err());
    }

    #[test]
    fn the_tutorial_keys_are_versioned_and_distinct_from_the_old_guide_key() {
        assert_eq!(tutorials_key(), "pleochroma.tutorials.v1");
        assert_eq!(guide_place_key(), "pleochroma.guide.v2");
        assert_ne!(guide_place_key(), crate::guide::GUIDE_KEY);
        assert!(is_app_key(&tutorials_key()) && is_app_key(&guide_place_key()));
    }

    #[test]
    fn a_finished_lesson_is_recorded_once_and_a_non_id_never() {
        let mut marks = TutorialMarks::default();
        assert!(marks.record("tiers-add-a-tier"));
        assert!(
            !marks.record("tiers-add-a-tier"),
            "the second save is a no-op"
        );
        assert!(
            marks.record("build:0a1b2c"),
            "a lesson built from a library design"
        );
        assert!(!marks.record(""));
        assert!(!marks.record("Not An Id"));
        assert!(!marks.record("a--b"));
        assert_eq!(marks.done.len(), 2);
        assert_eq!(marks.version, TUTORIALS_VERSION);
    }

    #[test]
    fn the_finished_lessons_round_trip_and_clear() {
        let mut marks = TutorialMarks::default();
        marks.record("new-design-walkthrough");
        marks.record("solving-solve");
        let json = marks.to_json().expect("writes");
        assert_eq!(TutorialMarks::from_json(&json), Ok(marks.clone()));
        assert!(marks.clear());
        assert!(!marks.clear(), "nothing left to forget");
        assert!(marks.done.is_empty());
    }

    #[test]
    fn a_stored_blob_loses_what_is_not_a_lesson_id_and_what_does_not_fit() {
        let dirty = TutorialMarks::from_json(
            r#"{"version":9,"done":["solving-solve","Bad Id","","x--y","output-save"]}"#,
        )
        .expect("parses");
        let ids: Vec<&str> = dirty.done.iter().map(String::as_str).collect();
        assert_eq!(ids, ["output-save", "solving-solve"]);
        assert_eq!(dirty.version, TUTORIALS_VERSION);

        let many: BTreeSet<String> = (0..MAX_TUTORIAL_MARKS + 40)
            .map(|n| format!("lesson-{n:04}"))
            .collect();
        let capped = TutorialMarks {
            version: 1,
            done: many,
        }
        .sanitized();
        assert_eq!(capped.done.len(), MAX_TUTORIAL_MARKS);
        let mut full = capped;
        assert!(
            !full.record("one-more-lesson"),
            "a full blob refuses new ids"
        );

        assert_eq!(
            TutorialMarks::from_json("{}"),
            Ok(TutorialMarks {
                version: TUTORIALS_VERSION,
                done: BTreeSet::new(),
            })
        );
        assert!(TutorialMarks::from_json("not json").is_err());
    }

    #[test]
    fn a_fresh_place_is_a_closed_panel_on_the_walkthrough() {
        let fresh = GuidePlace::default();
        assert_eq!(fresh.guide, pleochroma_editor::guide::WORKED_EXAMPLE_ID);
        assert!(!fresh.open && !fresh.collapsed && !fresh.float_placed);
        assert_eq!(GuidePlace::from_json("{}"), Ok(fresh));
    }

    #[test]
    fn a_place_round_trips_through_json() {
        let place = GuidePlace {
            guide: "solving-solve".to_owned(),
            open: true,
            step: 3,
            collapsed: true,
            float_placed: true,
            float_x: 512.5,
            float_y: 96.0,
        };
        let json = place.to_json().expect("writes");
        assert_eq!(GuidePlace::from_json(&json), Ok(place));
    }

    #[test]
    fn a_bad_position_or_lesson_id_in_a_stored_place_is_repaired() {
        let lost = GuidePlace::from_json(
            r#"{"guide":"tiers-move","float_placed":true,"float_x":-5.0,"float_y":10.0}"#,
        )
        .expect("parses");
        assert!(!lost.float_placed);
        assert_eq!((lost.float_x, lost.float_y), (0.0, 0.0));
        assert_eq!(lost.guide, "tiers-move");

        let bad_id = GuidePlace::from_json(r#"{"guide":"No Such Lesson!","open":true,"step":4}"#)
            .expect("parses");
        assert_eq!(
            bad_id,
            GuidePlace::default(),
            "a bad id is a closed default place"
        );
        assert!(GuidePlace::from_json("not json").is_err());
        assert!(GuidePlace::from_json(r#"{"step":-1}"#).is_err());
    }

    #[test]
    fn a_step_past_the_end_of_a_lesson_is_the_last_step() {
        let place = GuidePlace {
            step: 999,
            ..GuidePlace::default()
        };
        assert_eq!(place.step_within(10), 9);
        assert_eq!(place.step_within(1), 0);
        assert_eq!(place.step_within(0), 0);
        let early = GuidePlace {
            step: 2,
            ..GuidePlace::default()
        };
        assert_eq!(early.step_within(10), 2);
    }

    #[test]
    fn the_old_walkthrough_place_becomes_a_place_in_the_walkthrough() {
        let legacy = crate::guide::GuideProgress {
            open: true,
            step: 4,
            collapsed: true,
            float_placed: true,
            float_x: 12.0,
            float_y: 34.0,
        };
        let place = GuidePlace::from_legacy(&legacy);
        assert_eq!(place.guide, pleochroma_editor::guide::WORKED_EXAMPLE_ID);
        assert_eq!(
            (place.open, place.step, place.collapsed, place.float_placed),
            (true, 4, true, true)
        );
        assert_eq!((place.float_x, place.float_y), (12.0, 34.0));
    }
}
