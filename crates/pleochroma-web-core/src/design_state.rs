//! [`DesignState`]: the loaded design and what a save needs to write it back.
//!
//! That is everything the desktop would write -- its editor session, where it came from
//! ([`DesignSource`]), the printed proportions, the `[meta]` table and attachments of a design
//! file, and the snapshots of the custom materials the design has used.
//!
//! Plain data over `EditorSession`, no window and no DOM, so the snapshot bookkeeping (a snapshot
//! belongs to its NAME) is tested natively. The page's `app::state::WebApp` holds one.

use pleochroma::{geometry::stone_metrics::ExternalProportions, optics::materials::GemMaterial};
use pleochroma_cut_core::native::{AttachmentBlob, CustomMaterialSnapshot, DesignMetadata};
use pleochroma_editor::EditorSession;
use std::collections::BTreeMap;

/// The longest value, in characters, the dock's design-info text shows for one field.
const INFO_VALUE_CHARS: usize = 200;

/// Where the current design came from, for the status strip and the dock.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DesignSource {
    /// A New Design from the gallery; carries the template's card name.
    Template(String),
    /// A bare `.asc` file.
    Asc,
    /// A self-contained `.psdd` design file, no `.asc`.
    DesignFile,
    /// A `.gem` or `.gcs` design converted to `.asc`; carries `".gem"`/`".gcs"`.
    Converted(&'static str),
    /// Restored after a reload (this tab's `sessionStorage`) or after a restart (the
    /// `IndexedDB` autosave).
    Restored,
}

impl DesignSource {
    /// The short label the status strip and the dock show.
    #[must_use]
    pub fn label(&self) -> String {
        match self {
            Self::Template(name) => format!("New design: {name}"),
            Self::Asc => "Opened .asc".to_string(),
            Self::DesignFile => "Opened design file".to_string(),
            Self::Converted(ext) => format!("Converted from {ext}"),
            Self::Restored => "Restored from autosave".to_string(),
        }
    }
}

/// The loaded design and everything a save needs to write it back the way the
/// desktop would.
pub struct DesignState {
    /// The design, its undo/redo history, selection and dirty tracking.
    pub session: EditorSession,
    /// The design's `.asc` name (`round.asc`), `None` for a New design never saved.
    pub asc_filename: Option<String>,
    /// Where it came from.
    pub source: DesignSource,
    /// The printed proportions a design file carried in its `[source]` table.
    pub printed_proportions: Option<ExternalProportions>,
    /// The snapshot of every custom material this design has used, keyed by the
    /// material's name in ASCII lower case; a save writes the one the design names now
    /// ([`Self::custom_snapshot`]).
    ///
    /// A snapshot belongs to its NAME: the design's material name selects which one a
    /// save carries; a name with no snapshot saves none (the file then names a material
    /// this build cannot restore, exactly as before). Fill it through
    /// [`Self::remember_custom_snapshot`], which does the keying.
    pub custom_snapshots: BTreeMap<String, CustomMaterialSnapshot>,
    /// The `[meta]` table a design file carried (empty for any other source), written
    /// back on the next save with a fresh `modified_at`; unknown keys included.
    pub metadata: DesignMetadata,
    /// The attachments a design file carried, kept byte for byte and written back on
    /// the next save.
    pub attachments: Vec<AttachmentBlob>,
}

impl DesignState {
    /// A design with no file behind it yet.
    #[must_use]
    pub fn new(session: EditorSession, source: DesignSource) -> Self {
        Self {
            session,
            asc_filename: None,
            source,
            printed_proportions: None,
            custom_snapshots: BTreeMap::new(),
            metadata: DesignMetadata::default(),
            attachments: Vec::new(),
        }
    }

    /// The read-only "Design info" text for the dock: title, designer, source, notes,
    /// tags and licence, one line each for the fields that are set; empty when none is.
    #[must_use]
    pub fn info_text(&self) -> String {
        let meta = &self.metadata;
        let source = if meta.source_citation.is_empty() {
            &meta.source_url
        } else {
            &meta.source_citation
        };
        let tags = meta.tags.join(", ");
        [
            ("Title", meta.title.as_str()),
            ("Designer", meta.designer.as_str()),
            ("Source", source.as_str()),
            ("Notes", meta.notes.as_str()),
            ("Tags", tags.as_str()),
            ("License", meta.license.as_str()),
        ]
        .iter()
        .filter(|(_, value)| !value.is_empty())
        .map(|(label, value)| {
            if value.chars().count() > INFO_VALUE_CHARS {
                let head: String = value.chars().take(INFO_VALUE_CHARS).collect();
                format!("{label}: {head}...")
            } else {
                format!("{label}: {value}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
    }

    /// Remembers `snapshot` as the snapshot of the custom material called `name` (any
    /// earlier one of that name, ignoring ASCII case, is replaced).
    pub fn remember_custom_snapshot(&mut self, name: &str, snapshot: CustomMaterialSnapshot) {
        self.custom_snapshots
            .insert(name.to_ascii_lowercase(), snapshot);
    }

    /// Forgets the snapshot of the custom material called `name`, if one is kept.
    pub fn forget_custom_snapshot(&mut self, name: &str) {
        self.custom_snapshots.remove(&name.to_ascii_lowercase());
    }

    /// The custom-material snapshot a native save should carry: the one kept under the
    /// name the design's material has NOW ([`Self::custom_snapshots`]), `None` when that
    /// name is a built-in's or no snapshot is kept for it (a snapshot of another
    /// material the design used earlier must not be written under this name).
    #[must_use]
    pub fn custom_snapshot(&self) -> Option<&CustomMaterialSnapshot> {
        let name = self.session.design.material.name.as_deref()?;
        let builtin = GemMaterial::all_materials()
            .iter()
            .any(|m| m.name.eq_ignore_ascii_case(name));
        if builtin {
            return None;
        }
        self.custom_snapshots.get(&name.to_ascii_lowercase())
    }

    /// The name the status strip shows: the `.asc` name, else "Untitled design".
    #[must_use]
    pub fn display_name(&self) -> String {
        self.asc_filename
            .clone()
            .unwrap_or_else(|| "Untitled design".to_string())
    }

    /// The `.asc` name a save proposes (`pleochroma_editor::files::suggested_asc_file_name`).
    #[must_use]
    pub fn save_asc_name(&self) -> String {
        pleochroma_editor::files::suggested_asc_file_name(
            self.asc_filename.as_deref(),
            &self.session.design.meta.headers,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_editor::loading::{parse_new_design_form, parse_preform_form};

    /// An empty design with no custom snapshot, built the way New Design builds one.
    fn design_state() -> DesignState {
        let preform = parse_preform_form(1, "1.50", "1.00", "1.50", 96).expect("preform");
        let spec = parse_new_design_form(96, preform, "8", true, 0).expect("spec");
        DesignState::new(
            EditorSession::from_template(spec, 0),
            DesignSource::Template("Empty".to_string()),
        )
    }

    /// A snapshot whose `mean_ri` tells it apart from the others.
    fn snapshot(mean_ri: f64) -> CustomMaterialSnapshot {
        CustomMaterialSnapshot::new(mean_ri, 0.01, 0.0, None, "Cubic", "Isotropic")
    }

    fn name_the_material(state: &mut DesignState, name: &str) {
        state.session.design.material.name = Some(name.to_string());
    }

    #[test]
    fn the_saved_snapshot_is_the_named_materials() {
        let mut state = design_state();
        state.remember_custom_snapshot("A", snapshot(1.5));
        state.remember_custom_snapshot("B", snapshot(1.9));

        name_the_material(&mut state, "A");
        assert!((state.custom_snapshot().expect("A's").mean_ri - 1.5).abs() < 1e-12);

        name_the_material(&mut state, "B");
        assert!((state.custom_snapshot().expect("B's").mean_ri - 1.9).abs() < 1e-12);

        name_the_material(&mut state, "Sapphire");
        assert!(state.custom_snapshot().is_none(), "a built-in saves none");

        name_the_material(&mut state, "c");
        assert!(
            state.custom_snapshot().is_none(),
            "an unknown custom saves none"
        );

        state.session.design.material.name = None;
        assert!(state.custom_snapshot().is_none(), "no material saves none");
    }

    #[test]
    fn names_match_ignoring_ascii_case() {
        let mut state = design_state();
        state.remember_custom_snapshot("My Glass", snapshot(1.6));

        name_the_material(&mut state, "my glass");
        assert!((state.custom_snapshot().expect("lower case").mean_ri - 1.6).abs() < 1e-12);
        name_the_material(&mut state, "MY GLASS");
        assert!((state.custom_snapshot().expect("upper case").mean_ri - 1.6).abs() < 1e-12);

        // Remembering the same name in another case replaces, not duplicates.
        state.remember_custom_snapshot("MY glass", snapshot(1.7));
        assert_eq!(state.custom_snapshots.len(), 1);
        assert!((state.custom_snapshot().expect("replaced").mean_ri - 1.7).abs() < 1e-12);

        state.forget_custom_snapshot("My GLASS");
        assert!(state.custom_snapshot().is_none());
    }
}
