//! Stable recovery identity for the parsed, sampling-relevant corpus.
//!
//! Preserve the original Debug wire representation so adding reporting metadata cannot strand
//! an unchanged ASC run or its validation journal. Actual augmented sizes and authored-material
//! eligibility remain in this identity; changing either must refuse old episode/replay reuse.
//!
//! [`DatasetSplit::semantic_identity_fields`] is the field-level form recovery bundles store
//! beside it: explicit field names, no skip-reason text, and a named field per difference.

use std::{
    collections::BTreeMap,
    fmt::{self, Write as _},
};

use super::{DatasetSplit, DesignEntry};

/// Revision of [`DatasetSplit::semantic_identity_fields`]: bump it when a listed field changes
/// meaning (adding a field with a recorded legacy value does not need a bump).
pub const CORPUS_IDENTITY_REVISION: u32 = 1;

impl DatasetSplit {
    /// Exact legacy-shaped identity shared by recovery and completed evaluation journals.
    #[must_use]
    pub fn recovery_identity(&self) -> String {
        format!("{:?}", CorpusIdentity(self))
    }

    /// Field-level corpus identity for recovery: every sampling-relevant input under a stable
    /// field name. Skipped records are identified by position and title only. Why a record was
    /// skipped is message text, so a reworded or reclassified reason cannot strand a run, while a
    /// record that starts or stops loading still changes `records`. The parsed design and the
    /// authored material are compared through digests of their complete parsed values; the
    /// provenance flag `physical_size_augmented` is reporting metadata and is left out.
    #[must_use]
    pub fn semantic_identity_fields(&self) -> BTreeMap<String, String> {
        let list = |indices: &[usize]| {
            indices
                .iter()
                .map(usize::to_string)
                .collect::<Vec<_>>()
                .join(",")
        };
        let records = self
            .by_record
            .iter()
            .map(|entry| entry.map_or_else(|| "-".to_owned(), |index| index.to_string()))
            .collect::<Vec<_>>()
            .join(",");
        let mut fields = BTreeMap::from([
            ("split_seed".to_owned(), self.split_seed.to_string()),
            ("max_tiers".to_owned(), self.max_tiers.to_string()),
            ("train".to_owned(), list(&self.train)),
            ("val".to_owned(), list(&self.val)),
            ("test".to_owned(), list(&self.test)),
            ("records".to_owned(), records),
            ("entries".to_owned(), self.entries.len().to_string()),
            ("skipped".to_owned(), self.skipped.len().to_string()),
        ]);
        for (index, skipped) in self.skipped.iter().enumerate() {
            fields.insert(format!("skipped[{index}].title"), skipped.title.clone());
        }
        for (index, entry) in self.entries.iter().enumerate() {
            let DesignEntry {
                id,
                title,
                filename,
                design,
                authored_material,
                physical_size_augmented: _,
            } = entry;
            let material = authored_material
                .as_ref()
                .map_or_else(|| "none".to_owned(), debug_digest);
            fields.extend([
                (format!("entries[{index}].id"), id.to_string()),
                (format!("entries[{index}].title"), title.clone()),
                (format!("entries[{index}].filename"), filename.clone()),
                (format!("entries[{index}].design"), debug_digest(design)),
                (format!("entries[{index}].authored_material"), material),
            ]);
        }
        fields
    }
}

/// 64-bit FNV-1a over everything written to it, without keeping the text.
struct DigestWriter {
    hash: u64,
    bytes: usize,
}

impl fmt::Write for DigestWriter {
    fn write_str(&mut self, text: &str) -> fmt::Result {
        for byte in text.bytes() {
            self.hash ^= u64::from(byte);
            self.hash = self.hash.wrapping_mul(0x0100_0000_01b3);
        }
        self.bytes += text.len();
        Ok(())
    }
}

/// A compact, deterministic digest of `value`'s `Debug` text (the text the legacy identity
/// embeds in full): equal parsed values give equal digests.
fn debug_digest(value: &impl fmt::Debug) -> String {
    let mut digest = DigestWriter {
        hash: 0xcbf2_9ce4_8422_2325,
        bytes: 0,
    };
    // The writer itself never fails; an error could only come from a faulty `Debug` impl, and
    // the partial digest then still differs from any complete one.
    let _ = write!(digest, "{value:?}");
    format!("fnv1a64:{:016x}:{}", digest.hash, digest.bytes)
}

struct CorpusIdentity<'a>(&'a DatasetSplit);

impl fmt::Debug for CorpusIdentity<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let split = self.0;
        f.debug_struct("DatasetSplit")
            .field("entries", &EntryIdentities(&split.entries))
            .field("train", &split.train)
            .field("val", &split.val)
            .field("test", &split.test)
            .field("by_record", &split.by_record)
            .field("skipped", &split.skipped)
            .field("max_tiers", &split.max_tiers)
            .field("split_seed", &split.split_seed)
            .finish()
    }
}

struct EntryIdentities<'a>(&'a [DesignEntry]);

impl fmt::Debug for EntryIdentities<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list()
            .entries(self.0.iter().map(EntryIdentity))
            .finish()
    }
}

struct EntryIdentity<'a>(&'a DesignEntry);

impl fmt::Debug for EntryIdentity<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let entry = self.0;
        // physical_size_augmented describes provenance only. The effective physical size is
        // already part of design; authored_material determines source-sampling eligibility.
        f.debug_struct("DesignEntry")
            .field("id", &entry.id)
            .field("title", &entry.title)
            .field("filename", &entry.filename)
            .field("design", &entry.design)
            .field("authored_material", &entry.authored_material)
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn corpus_identity_keeps_the_legacy_debug_wire_shape() {
        let split = DatasetSplit::fixture();
        let entry = &split.entries[0];
        // Spell out the pre-provenance layout rather than deriving it from the new entry.
        let legacy_entry = format!(
            "DesignEntry {{ id: {:?}, title: {:?}, filename: {:?}, design: {:?}, authored_material: {:?} }}",
            entry.id, entry.title, entry.filename, entry.design, entry.authored_material,
        );
        let legacy_corpus = format!(
            "DatasetSplit {{ entries: [{legacy_entry}], train: {:?}, val: {:?}, test: {:?}, by_record: {:?}, skipped: {:?}, max_tiers: {:?}, split_seed: {:?} }}",
            split.train,
            split.val,
            split.test,
            split.by_record,
            split.skipped,
            split.max_tiers,
            split.split_seed,
        );
        assert_eq!(split.recovery_identity(), legacy_corpus);
    }

    #[test]
    fn reporting_provenance_does_not_change_identity_but_sampling_inputs_do() {
        let mut split = DatasetSplit::fixture();
        let identity = split.recovery_identity();
        split.entries[0].physical_size_augmented = !split.entries[0].physical_size_augmented;
        assert_eq!(split.recovery_identity(), identity);

        let old_width = split.entries[0].design.girdle_diameter_mm;
        split.entries[0].design.girdle_diameter_mm = Some(old_width.unwrap_or(7.0) + 1.0);
        assert_ne!(split.recovery_identity(), identity);
        split.entries[0].design.girdle_diameter_mm = old_width;
        split.entries[0].authored_material = Some(pleochroma_ai::env::design_source_material(
            &split.entries[0].design,
        ));
        assert_ne!(split.recovery_identity(), identity);
    }

    #[test]
    fn semantic_identity_ignores_skip_text_and_provenance_but_names_changed_inputs() {
        let mut split = DatasetSplit::fixture();
        split.by_record.push(None);
        split.skipped.push(crate::dataset::SkippedRecord {
            title: "Broken attachment".to_owned(),
            reason: ".asc parse error: line 3".to_owned(),
        });
        let fields = split.semantic_identity_fields();
        let legacy = split.recovery_identity();
        split.skipped[0].reason = "unsupported design representation: reworded".to_owned();
        split.entries[0].physical_size_augmented = !split.entries[0].physical_size_augmented;
        assert_eq!(split.semantic_identity_fields(), fields);
        assert_ne!(
            split.recovery_identity(),
            legacy,
            "the legacy form embeds the reason text"
        );

        split.entries[0].design.girdle_diameter_mm =
            Some(split.entries[0].design.girdle_diameter_mm.unwrap_or(7.0) + 1.0);
        let changed = split.semantic_identity_fields();
        assert_ne!(changed["entries[0].design"], fields["entries[0].design"]);
        assert_eq!(changed["entries[0].title"], fields["entries[0].title"]);
        assert_eq!(changed["entries[0].authored_material"], "none");
        assert_eq!(changed["records"], "0,-");
    }
}
