//! Resolves a [`Design`] from a catalogue entry's full record, preferring an attached
//! self-contained `.psdd` design file.
//!
//! A real attached schedule comes next, and the angle-table placeholder reconstruction last.
//! The bare-`.asc` half (`design_from_asc_text`, [`LoadedDesign`]) lives in
//! `pleochroma_editor::loading`, shared with the web app.

use pleochroma::geometry::stone_metrics::ExternalProportions;
use pleochroma_cut_core::{
    Design,
    native::{
        CustomMaterialSnapshot, DesignLoadError, DesignMetadata, RiRangeChoice, design_from_str,
    },
};
use pleochroma_editor::loading::{
    LoadedDesign, default_preform_for_schedule, design_from_asc_text,
};
use tracing::warn;

/// Why [`design_from_attachment`] could not produce a design.
///
/// `concave` marks a failure of the concave (fantasy-cut) tiers a file carries. It
/// must never be answered with a fallback to the planar schedule or the angle
/// table: those drop the concave tiers, and the next Save would then destroy them
/// for good. The caller surfaces it as a load failure instead.
#[derive(Debug)]
pub(super) struct AttachmentError {
    /// A ready, human-readable message.
    pub message: String,
    /// Whether the cause is an unusable concave tier (no fallback is acceptable).
    pub concave: bool,
}

impl AttachmentError {
    const fn plain(message: String) -> Self {
        Self {
            message,
            concave: false,
        }
    }
}

/// A design read from a catalogue attachment, with the custom-material snapshot its file
/// carries.
///
/// `custom_material` is the file's own snapshot of a material no built-in preset answers
/// to (`pleochroma_cut_core::native::LoadedDesign::restorable_custom_material`): `Some` only
/// when the file names such a material and carried a snapshot of it.
/// The editor's [`LoadedDesign`]
/// has no field for it -- the web app and the CLI build that type by literal -- so it
/// travels beside it. The consumers that draw a library design in its own material
/// (`super::catalogue_planes`) rebuild the material from it when the vault's custom list
/// lacks the name.
pub(super) struct AttachedDesign {
    /// The design and where its schedule came from.
    pub loaded: LoadedDesign,
    /// The file's snapshot of its custom material, when it has one to restore.
    pub custom_material: Option<CustomMaterialSnapshot>,
}

impl AttachedDesign {
    /// `loaded` for a source that carries no material snapshot (a bare `.asc`, a converted
    /// `.gem`/`.gcs`).
    const fn without_snapshot(loaded: LoadedDesign) -> Self {
        Self {
            loaded,
            custom_material: None,
        }
    }
}

/// The refractive-index range a `.gcs` file states in its `<info>` element, as the designer's own
/// statement ([`RiRangeChoice::Designer`]), or [`RiRangeChoice::Off`] when the bytes do not read as
/// a `.gcs` or state no range.
///
/// Both the library import and the file open use it, so a `.gcs` that states a range opens with
/// that range in the editor state (the choice "From File"); an `.asc` schedule has no place for it.
/// Reading runs inside the importer's own per-file guard.
pub(in crate::gui::editor) fn gcs_bytes_ri_range(bytes: &[u8]) -> RiRangeChoice {
    crate::gui::library::local::catch_file_panic(std::panic::AssertUnwindSafe(|| {
        pleochroma_formats::gcs::parse_gcs_bytes(bytes).map_or(RiRangeChoice::Off, |design| {
            pleochroma_formats::gcs::gcs_ri_range_choice(&design)
        })
    }))
    .unwrap_or(RiRangeChoice::Off)
}

/// The range the first `.gcs` attached to `full` states, or [`RiRangeChoice::Off`].
///
/// A library import of a `.gcs` attaches the converted `.asc` first and the original `.gcs` beside
/// it, and the `.asc` is what opens, so the range is looked for in the `.gcs` beside it.
fn attached_gcs_ri_range(
    full: &pleochroma_vault::model::entry::FullDiagramRecord,
) -> RiRangeChoice {
    full.attached_files
        .iter()
        .filter(|file| {
            pleochroma_vault::local::DesignFileKind::from_file_name(&file.name)
                == Some(pleochroma_vault::local::DesignFileKind::Gcs)
        })
        .map(|file| gcs_bytes_ri_range(&file.content))
        .find(|choice| *choice != RiRangeChoice::Off)
        .unwrap_or(RiRangeChoice::Off)
}

/// The design built from `full`'s self-contained `.psdd` attachment, if it has one that reads.
///
/// The file carries the whole design, so it wins over any `.asc`, `.gem` or `.gcs` attached
/// beside it (an import attaches a design file found next to its schedule; a catalogue save
/// attaches the saved design beside the `.asc` it wrote).
///
/// The design is recorded under the attached `.asc`'s name when there is one -- with
/// that text kept, so a later Save can preserve it byte for byte -- else under
/// `<stem>.asc` of the converted `.gem`/`.gcs` or of the design file itself. `None`
/// (logged) when the attachment is not UTF-8 or does not parse, so the schedule beside
/// it still loads. A file whose concave tiers are unusable is the exception: it is an
/// `Err` (see [`AttachmentError`]), since the schedule beside it has no concave tiers.
fn design_from_design_attachment(
    full: &pleochroma_vault::model::entry::FullDiagramRecord,
) -> Option<Result<AttachedDesign, AttachmentError>> {
    let names = || full.attached_files.iter().map(|f| f.name.as_str());
    let attached =
        &full.attached_files[pleochroma_vault::local::native_design_attachment_position(names())?];
    let parsed = match std::str::from_utf8(&attached.content) {
        Ok(text) => design_from_str(text),
        Err(e) => {
            warn!(
                "Attached design file '{}' on diagram #{} could not be read ({e}); using the \
                 schedule beside it.",
                attached.name, full.entry_id
            );
            return None;
        }
    };
    let loaded = match parsed {
        Ok(loaded) => loaded,
        Err(e @ DesignLoadError::ConcaveTier { .. }) => {
            return Some(Err(AttachmentError {
                message: format!(
                    "'{}': its concave tiers cannot be loaded, and the design was not opened \
                     so they are not lost on the next save: {e}",
                    attached.name
                ),
                concave: true,
            }));
        }
        Err(e) => {
            warn!(
                "Attached design file '{}' on diagram #{} could not be read ({e}); using the \
                 schedule beside it.",
                attached.name, full.entry_id
            );
            return None;
        }
    };
    let schedule = pleochroma_vault::local::design_attachment_position(names())
        .map(|(position, kind)| (&full.attached_files[position], kind));
    let (asc_filename, original_asc_text) = match schedule {
        Some((file, pleochroma_vault::local::DesignFileKind::Asc)) => (
            file.name.clone(),
            Some(pleochroma_formats::asc::decode_asc_bytes(&file.content).into_owned()),
        ),
        Some((file, _)) => (
            pleochroma_vault::local::converted_asc_file_name(&file.name),
            None,
        ),
        None => (
            pleochroma_vault::local::converted_asc_file_name(&attached.name),
            None,
        ),
    };
    Some(Ok(AttachedDesign {
        loaded: LoadedDesign {
            design: loaded.design,
            used_placeholder: false,
            asc_filename: Some(asc_filename),
            original_asc_text,
            // Kept so a Save from this design writes the file's own `[meta]` and
            // attachments back instead of dropping them.
            metadata: loaded.metadata,
            attachments: loaded.attachments,
        },
        custom_material: loaded.restorable_custom_material,
    }))
}

/// The design built from `full`'s design-file attachment.
///
/// The first `.asc` is used, else the first `.gem`, else the first `.gcs` (extension
/// case-insensitive, picked by `pleochroma_vault::local::design_attachment_position`). Each is
/// read as `.asc` text by `pleochroma_vault::local::design_file_to_asc_text`: a `.gem` or `.gcs`
/// is converted to cutting instructions, so the design loads with its real masts instead of the
/// angle-table placeholder. Reading runs inside `gui::library::local::catch_file_panic`, the
/// importer's own per-file guard.
///
/// `None` when the record has no design-file attachment; `Some(Err)` (a ready
/// message) when it has one that does not read, convert or parse, or whose concave
/// tiers are unusable ([`AttachmentError::concave`]).
///
/// A self-contained `.psdd` attachment comes first
/// ([`design_from_design_attachment`]).
///
/// The design comes with the custom-material snapshot its file carries, if any
/// ([`AttachedDesign::custom_material`]): a design file's own; a bare or converted
/// schedule has none.
///
/// Also the design-file half of `super::catalogue_planes::resolve_catalogue_planes`,
/// so the editor's Load Selected and every plane consumer read the same file.
pub(super) fn design_from_attachment(
    full: &pleochroma_vault::model::entry::FullDiagramRecord,
) -> Option<Result<AttachedDesign, AttachmentError>> {
    if let Some(loaded) = design_from_design_attachment(full) {
        return Some(loaded);
    }
    let (position, kind) = pleochroma_vault::local::design_attachment_position(
        full.attached_files.iter().map(|f| f.name.as_str()),
    )?;
    let attached = &full.attached_files[position];
    let read = crate::gui::library::local::catch_file_panic(std::panic::AssertUnwindSafe(|| {
        pleochroma_vault::local::design_file_to_asc_text(&attached.name, kind, &attached.content)
    }));
    let file = match read {
        Ok(Ok(file)) => file,
        Ok(Err(e)) => {
            return Some(Err(AttachmentError::plain(format!(
                "'{}': {e}",
                attached.name
            ))));
        }
        Err(panic_msg) => {
            return Some(Err(AttachmentError::plain(format!(
                "'{}': internal error: {panic_msg}",
                attached.name
            ))));
        }
    };
    if !file.warnings.is_empty() {
        warn!(
            "Attached '{}' on diagram #{} converted with notes: {:?}",
            attached.name, full.entry_id, file.warnings
        );
    }
    let lw_ratio = full.lw_ratio.as_deref();
    // `.asc` has no notation for frosting. The rows the library stored with the entry (what a
    // `.gcs` import or a saved design file recorded) win; a `.gcs` attachment that is read
    // here states its own.
    let frosting = if full.frosting.is_empty() {
        file.frosting
    } else {
        pleochroma_vault::model::frosting::StoredTierFrosting::to_gcs(&full.frosting)
    };
    let stated_range = attached_gcs_ri_range(full);
    let loaded = design_from_asc_text(&file.asc_file_name, &file.asc_text, lw_ratio)
        .map(|mut loaded| {
            loaded.design.apply_imported_frosting(&frosting);
            // A schedule has no RI range of its own, so the range a `.gcs` states travels in the
            // design's `[meta]` (the choice "From File").
            if stated_range != RiRangeChoice::Off {
                loaded.metadata.set_ri_range_choice(stated_range);
            }
            AttachedDesign::without_snapshot(loaded)
        })
        .map_err(AttachmentError::plain);
    Some(loaded.map_err(|e| {
        if e.concave {
            e
        } else {
            AttachmentError::plain(format!("'{}': {}", attached.name, e.message))
        }
    }))
}

/// Builds the design that should be loaded for one catalogue entry's full record.
///
/// Prefers a real attached design file's own schedule ([`design_from_attachment`]: a `.asc`,
/// else a `.gem`, else a `.gcs`), whose masts are the file's recorded depths. The fallback is
/// `pleochroma_vault::local::reconstruct_asc_schedule` over the angle/index table alone. It has
/// no depth data and sets every tier's `mast` to `0.0`, so it is used only when no usable design
/// file is attached. [`LoadedDesign::used_placeholder`] tells the caller which path was
/// taken, so it can warn the cutter that the masts are placeholders.
///
/// # Errors
///
/// Returns `Err` (a human-readable message) only when there is no readable attached
/// `.asc`/`.gem`/`.gcs` AND no angle-settings table to reconstruct from at all --
/// nothing in this diagram's record describes cutting instructions.
pub(in crate::gui::editor) fn design_from_full_record(
    full: &pleochroma_vault::model::entry::FullDiagramRecord,
) -> Result<LoadedDesign, String> {
    match design_from_attachment(full) {
        Some(Ok(attached)) => return Ok(attached.loaded),
        // Concave tiers that cannot be loaded are a load failure, never a fallback:
        // the angle table has none, and the next Save would destroy them.
        Some(Err(e)) if e.concave => return Err(e.message),
        Some(Err(e)) => warn!(
            "Attached design file on diagram #{} could not be loaded ({}); falling back to \
             the angle-table reconstruction.",
            full.entry_id, e.message
        ),
        None => {}
    }

    let schedule = pleochroma_vault::local::reconstruct_asc_schedule(
        &full.title,
        full.refractive_index.as_deref(),
        full.index_gear.as_deref(),
        &full.angle_settings,
    )
    .map_err(|e| format!("This diagram's cutting-instructions data could not be read: {e}"))?
    .ok_or_else(|| "This diagram has no cutting-instructions data to load.".to_string())?;
    let preform = default_preform_for_schedule(&schedule, full.lw_ratio.as_deref());
    Ok(LoadedDesign {
        design: Design::from_asc_schedule(preform, &schedule),
        used_placeholder: true,
        asc_filename: None,
        original_asc_text: None,
        metadata: DesignMetadata::default(),
        attachments: Vec::new(),
    })
}

/// Builds Deep Solve's external verification targets from a catalogue entry's printed
/// proportion columns (see `solve_meet_points_verified` for why they are used). `volume`,
/// `lw_ratio`, `cw_ratio`, `pw_ratio` and `hw_ratio` map onto `ExternalProportions`' `vol_w3`,
/// `lw`, `cw`, `pw` and `hw`.
///
/// Returns `None` when none of the five columns holds a usable positive, finite number: with
/// nothing printed there is no signal to score against, so Deep Solve must be disabled (see
/// `super::view::deep_solve_hint`).
pub(in crate::gui::editor) fn external_proportions_from_full_record(
    full: &pleochroma_vault::model::entry::FullDiagramRecord,
) -> Option<ExternalProportions> {
    fn parse(value: Option<&String>) -> Option<f64> {
        value
            .map(String::as_str)?
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|v: &f64| v.is_finite() && *v > 0.0)
    }
    let props = ExternalProportions {
        vol_w3: parse(full.volume.as_ref()),
        lw: parse(full.lw_ratio.as_ref()),
        cw: parse(full.cw_ratio.as_ref()),
        pw: parse(full.pw_ratio.as_ref()),
        hw: parse(full.hw_ratio.as_ref()),
    };
    (props.vol_w3.is_some()
        || props.lw.is_some()
        || props.cw.is_some()
        || props.pw.is_some()
        || props.hw.is_some())
    .then_some(props)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma::geometry::meet_solver::MeetConstraint;
    use pleochroma_formats::native::design::DESIGN_EXTENSION_DOTTED;
    use pleochroma_vault::model::{entry::FullDiagramRecord, file::AttachedFile};

    fn empty_full_record() -> FullDiagramRecord {
        FullDiagramRecord {
            entry_id: 1,
            title: "Test Design".to_string(),
            url: "local://test.asc".to_string(),
            design_id: None,
            page_url: String::new(),
            diagram_image_name: None,
            diagram_image_data: None,
            competition_diagram: None,
            lw_ratio: None,
            refractive_index: None,
            index_gear: None,
            volume: None,
            facets_count: None,
            shape: None,
            designer_info: None,
            hw_ratio: None,
            tw_ratio: None,
            uw_ratio: None,
            pw_ratio: None,
            cw_ratio: None,
            symmetry_order: None,
            mirror_symmetry: None,
            designer: None,
            source_citation: None,
            pdf_file: None,
            gem_file: None,
            shape_category: None,
            angle_settings: Vec::new(),
            attached_files: Vec::new(),
            frosting: Vec::new(),
        }
    }

    /// A frosted standard round brilliant as `.gcs` bytes: tier 1 whole, tier 3 at its facet
    /// slots 1 and 3.
    fn frosted_gcs() -> (
        Vec<u8>,
        Vec<(usize, pleochroma_formats::gcs::GcsTierFrosting)>,
    ) {
        use pleochroma_cut_core::{ConstraintTier, PreformSpec, ScheduleMeta};
        use pleochroma_formats::gcs::GcsTierFrosting;
        let design = Design::new(
            PreformSpec::block(2.0, 1.0, 2.0),
            ScheduleMeta::standard_round_brilliant(),
            ConstraintTier::standard_round_brilliant(),
        );
        let schedule = design.to_asc_schedule().expect("the design exports");
        let frosting = vec![
            (1, GcsTierFrosting::Whole),
            (3, GcsTierFrosting::Facets(vec![1, 3])),
        ];
        let text = pleochroma_formats::gcs::to_gcs_string_with_frosting(&schedule, &frosting)
            .expect("the schedule writes as .gcs");
        (text.into_bytes(), frosting)
    }

    /// The frosting the editor's design must carry for [`frosted_gcs`].
    fn assert_frosted_like_the_gcs(design: &Design) {
        use pleochroma_cut_core::TierFrosting;
        assert_eq!(design.tier_frosting(1), Some(&TierFrosting::Tier));
        let slots: Vec<f64> = [1_usize, 3]
            .iter()
            .map(|&slot| design.tiers[3].indices[slot])
            .collect();
        assert_eq!(design.tier_frosting(3), Some(&TierFrosting::Facets(slots)));
        assert_eq!(design.tier_frosting.len(), 2, "no other tier is frosted");
    }

    /// The library import of a `.gcs` opens in the editor with its tier and facets frosted; the
    /// polished twin opens polished.
    ///
    /// The converted `.asc` comes first, the original `.gcs` beside it, and the frosting is in
    /// the side table.
    #[test]
    fn a_library_design_imported_from_a_frosted_gcs_opens_with_its_frosting() {
        use crate::gui::library::local::{ForeignFormat, convert_foreign_design};
        use pleochroma_vault::{
            db::sqlite::Database,
            local::{LOCAL_SOURCE_ID, apply_imported_extras, import_asc},
        };
        let (gcs, stated) = frosted_gcs();
        let converted = convert_foreign_design(ForeignFormat::Gcs, &gcs).expect("converts");
        assert_eq!(converted.frosting, stated);

        // What `gui::library::local::import::pipeline` does with it.
        let mut parsed = import_asc("rb.gcs", &converted.asc_text, None)
            .expect("the converted text parses")
            .with_frosting(&converted.frosting);
        parsed.detail.attached_files[0].name = "rb.asc".to_string();
        parsed.detail.attached_files.push(AttachedFile {
            name: "rb.gcs".to_string(),
            url: String::new(),
            content: gcs,
        });
        let db = Database::new(Some(":memory:")).expect("an in-memory database");
        let id = db
            .save_design(&parsed.entry, &parsed.detail, LOCAL_SOURCE_ID)
            .expect("saves");
        apply_imported_extras(&db, id, &parsed.extras).expect("applies the frosting");

        let full = db.get_diagram_full(id).expect("reads").expect("a row");
        assert!(!full.frosting.is_empty(), "the row carries the frosting");
        // The first `.asc` is what opens (and cannot state frosting): the stored rows apply.
        let names: Vec<&str> = full
            .attached_files
            .iter()
            .map(|f| f.name.as_str())
            .collect();
        assert_eq!(names, ["rb.asc", "rb.gcs"]);
        let loaded = design_from_full_record(&full).expect("opens");
        assert_frosted_like_the_gcs(&loaded.design);

        // The same record with no stored rows (a library from before the side table) opens
        // polished from the `.asc`, as it always did.
        let mut old = full.clone();
        old.frosting.clear();
        let loaded = design_from_full_record(&old).expect("opens");
        assert!(loaded.design.tier_frosting.is_empty());
    }

    /// A record whose only design file is a `.gcs` states its frosting itself: it applies when
    /// no stored rows exist, and the stored rows win when they do.
    #[test]
    fn a_gcs_only_record_applies_its_own_frosting_unless_rows_are_stored() {
        use pleochroma_vault::model::frosting::StoredTierFrosting;
        let (gcs, _) = frosted_gcs();
        let mut full = empty_full_record();
        full.attached_files.push(AttachedFile {
            name: "rb.gcs".to_string(),
            url: String::new(),
            content: gcs,
        });
        let loaded = design_from_full_record(&full).expect("opens");
        assert_frosted_like_the_gcs(&loaded.design);

        full.frosting = vec![StoredTierFrosting::whole(0)];
        let loaded = design_from_full_record(&full).expect("opens");
        assert_eq!(
            loaded.design.tier_frosting.len(),
            1,
            "the stored rows replace it"
        );
        assert!(loaded.design.tier_is_frosted(0));
    }

    #[test]
    fn design_from_full_record_prefers_a_real_attached_asc_file() {
        let mut full = empty_full_record();
        full.attached_files
            .push(pleochroma_vault::model::file::AttachedFile {
                name: "design.asc".to_string(),
                url: String::new(),
                content:
                    b"GemCad 5.0\ng 96 0.0\ny 4 y\nI 1.54\na -41.000000 0.64991234 92 n 1 84\n"
                        .to_vec(),
            });
        let loaded = design_from_full_record(&full).unwrap();
        assert!(!loaded.used_placeholder);
        assert_eq!(loaded.asc_filename.as_deref(), Some("design.asc"));
        assert!(loaded.original_asc_text.is_some());
        assert_eq!(loaded.design.tiers.len(), 1);
        // A lone tier with no explicit anchor gets its own real recorded mast
        // borrowed as a `ScaleReference` anchor by `Design::from_asc_schedule`
        // (see that function's doc comment) -- not a placeholder zero.
        match loaded.design.tiers[0].constraint {
            MeetConstraint::ScaleReference(v) => assert!((v - 0.649_912_34).abs() < 1e-9),
            ref other => panic!("expected a borrowed ScaleReference anchor, got {other:?}"),
        }
    }

    /// An attached `.psdd` file with unusable concave tiers is an error: the
    /// `.asc` beside it has none, so using it would lose them on the next Save.
    #[test]
    fn an_attached_design_file_with_unusable_concave_tiers_is_not_skipped() {
        let text = pleochroma_cut_core::native::design_to_string(
            &Design::concave_fixture(),
            None,
            &pleochroma_cut_core::native::DesignExtras::default(),
        )
        .expect("serializes");
        assert!(text.contains("tool = \"CYL\""));
        let forged = text.replace("tool = \"CYL\"", "tool = \"XYZ\"");
        let mut full = empty_full_record();
        full.attached_files.push(AttachedFile {
            name: "design.asc".to_string(),
            url: String::new(),
            content: b"GemCad 5.0\ng 96 0.0\ny 4 y\nI 1.54\na -41.000000 0.64991234 92 n 1 84\n"
                .to_vec(),
        });
        full.attached_files.push(AttachedFile {
            name: format!("design{DESIGN_EXTENSION_DOTTED}"),
            url: String::new(),
            content: forged.into_bytes(),
        });
        let message = design_from_full_record(&full)
            .err()
            .expect("a load failure");
        assert!(message.contains("concave"), "{message}");
        // The intact file opens with its tiers.
        full.attached_files[1].content = text.into_bytes();
        let loaded = design_from_full_record(&full).expect("opens");
        assert_eq!(loaded.design.concave_tiers.len(), 2);
    }

    /// A record whose attachments hold a `.asc` AND a self-contained design file opens
    /// from the design file (every tier of it), under the `.asc`'s name.
    #[test]
    fn design_from_full_record_prefers_an_attached_design_file() {
        attached_design_file_is_preferred_over_the_asc(&format!("round{DESIGN_EXTENSION_DOTTED}"));
    }

    fn attached_design_file_is_preferred_over_the_asc(design_name: &str) {
        use pleochroma_cut_core::{
            ConstraintTier, PreformSpec, ScheduleMeta, native::DesignExtras,
        };
        let design = Design::new(
            PreformSpec::block(2.0, 1.0, 2.0),
            ScheduleMeta::standard_round_brilliant(),
            ConstraintTier::standard_round_brilliant(),
        );
        let text =
            pleochroma_cut_core::native::design_to_string(&design, None, &DesignExtras::default())
                .expect("serializes");
        let asc = b"GemCad 5.0\ng 96 0.0\ny 4 y\nI 1.54\na -41.000000 0.64991234 92 n 1 84\n";
        let mut full = empty_full_record();
        full.attached_files.push(AttachedFile {
            name: "round.asc".to_string(),
            url: String::new(),
            content: asc.to_vec(),
        });
        full.attached_files.push(AttachedFile {
            name: design_name.to_string(),
            url: String::new(),
            content: text.into_bytes(),
        });
        let loaded = design_from_full_record(&full).unwrap();
        assert!(!loaded.used_placeholder);
        assert_eq!(loaded.design.tiers.len(), design.tiers.len());
        assert_eq!(loaded.asc_filename.as_deref(), Some("round.asc"));
        assert!(loaded.original_asc_text.is_some());

        // Alone, the design file names the design after itself and has no `.asc` text.
        full.attached_files.remove(0);
        let lone = design_from_full_record(&full).unwrap();
        assert_eq!(lone.asc_filename.as_deref(), Some("round.asc"));
        assert_eq!(lone.original_asc_text, None);
        assert_eq!(lone.design.tiers.len(), design.tiers.len());
    }

    /// An attached design file that does not parse leaves the `.asc` beside it
    /// to load, and is not used on its own.
    #[test]
    fn an_unreadable_design_attachment_is_not_used() {
        let mut full = empty_full_record();
        full.attached_files.push(AttachedFile {
            name: "design.asc".to_string(),
            url: String::new(),
            content: b"GemCad 5.0\ng 96 0.0\ny 4 y\nI 1.54\na -41.000000 0.64991234 92 n 1 84\n"
                .to_vec(),
        });
        full.attached_files.push(AttachedFile {
            name: format!("design{DESIGN_EXTENSION_DOTTED}"),
            url: String::new(),
            content: b"not a design".to_vec(),
        });
        let loaded = design_from_full_record(&full).unwrap();
        assert_eq!(loaded.design.tiers.len(), 1, "the .asc still loads");

        full.attached_files.remove(0);
        full.attached_files[0].content = Vec::new();
        assert!(
            design_from_design_attachment(&full).is_none(),
            "an unreadable lone design file is not used"
        );
    }

    #[test]
    fn design_from_full_record_falls_back_to_the_angle_table_when_no_asc_is_attached() {
        let mut full = empty_full_record();
        full.angle_settings
            .push(pleochroma_vault::model::angle::AngleSetting {
                order_index: 0,
                facet: "T".to_string(),
                angle: "0".to_string(),
                index: String::new(),
                notes: String::new(),
                ..Default::default()
            });
        let loaded = design_from_full_record(&full).unwrap();
        assert!(loaded.used_placeholder);
        assert_eq!(loaded.asc_filename, None);
        assert_eq!(loaded.original_asc_text, None);
        assert_eq!(loaded.design.tiers.len(), 1);
        // The reconstruction's own documented placeholder mast (0.0) is what gets
        // borrowed as this lone tier's `ScaleReference` anchor.
        assert_eq!(
            loaded.design.tiers[0].constraint,
            MeetConstraint::ScaleReference(0.0)
        );
    }

    /// A record whose only design file is a `.gem` opens with the `.gem`'s real masts, not the
    /// zero-mast angle-table placeholder.
    #[test]
    fn design_from_full_record_opens_a_gem_only_record_with_real_masts() {
        use crate::gui::library::local::test_gem::{SYNTHETIC_GEM_TIERS, encode_gem};
        let mut full = empty_full_record();
        full.attached_files.push(AttachedFile {
            name: "Synthetic.GEM".to_string(),
            url: String::new(),
            content: encode_gem(SYNTHETIC_GEM_TIERS, 96, "Synthetic Gem"),
        });
        // An angle-table row too, as a scraped catalogue detail has: the `.gem`
        // must win over it.
        full.angle_settings
            .push(pleochroma_vault::model::angle::AngleSetting {
                order_index: 0,
                facet: "T".to_string(),
                angle: "0".to_string(),
                index: String::new(),
                notes: String::new(),
                ..Default::default()
            });
        let loaded = design_from_full_record(&full).expect("a .gem-only record loads");
        assert!(!loaded.used_placeholder);
        assert_eq!(loaded.asc_filename.as_deref(), Some("Synthetic.asc"));
        assert_eq!(loaded.design.tiers.len(), SYNTHETIC_GEM_TIERS.len());
        let schedule = loaded
            .design
            .to_asc_schedule()
            .expect("the .gem's masts anchor every tier");
        assert_eq!(schedule.tiers.len(), SYNTHETIC_GEM_TIERS.len());
        for (tier, spec) in schedule.tiers.iter().zip(SYNTHETIC_GEM_TIERS) {
            assert!(
                (tier.mast.abs() - spec.2).abs() < 1e-6,
                "tier {} must keep the .gem's real mast {}, got {}",
                spec.0,
                spec.2,
                tier.mast
            );
        }
    }

    /// A corrupt `.gem` falls back to the angle table, exactly like an `.asc`
    /// that fails to parse.
    #[test]
    fn design_from_full_record_falls_back_when_the_gem_does_not_parse() {
        let mut full = empty_full_record();
        full.attached_files.push(AttachedFile {
            name: "broken.gem".to_string(),
            url: String::new(),
            content: vec![1, 2, 3],
        });
        full.angle_settings
            .push(pleochroma_vault::model::angle::AngleSetting {
                order_index: 0,
                facet: "T".to_string(),
                angle: "0".to_string(),
                index: String::new(),
                notes: String::new(),
                ..Default::default()
            });
        let loaded = design_from_full_record(&full).expect("the angle table still loads");
        assert!(loaded.used_placeholder);
    }

    #[test]
    fn design_from_full_record_errors_with_no_schedule_data_at_all() {
        let full = empty_full_record();
        assert!(design_from_full_record(&full).is_err());
    }

    /// A `.gcs` that states an RI range, as the importer attaches it.
    fn gcs_stating(choice: RiRangeChoice) -> Vec<u8> {
        use pleochroma_cut_core::{ConstraintTier, PreformSpec, ScheduleMeta};
        let design = Design::new(
            PreformSpec::block(2.0, 1.0, 2.0),
            ScheduleMeta::standard_round_brilliant(),
            ConstraintTier::standard_round_brilliant(),
        );
        let schedule = design.to_asc_schedule().expect("the design exports");
        pleochroma_formats::gcs::to_gcs_string_with_ri_range(&schedule, &[], choice)
            .expect("the schedule writes as .gcs")
            .into_bytes()
    }

    /// The range a `.gcs` states, with its tolerance for the text round trip.
    fn assert_designer_range(choice: RiRangeChoice, low: f64, high: Option<f64>) {
        match choice {
            RiRangeChoice::Designer {
                low: got_low,
                high: got_high,
            } => {
                assert!((got_low - low).abs() < 1e-9, "{choice:?}");
                assert_eq!(got_high.is_some(), high.is_some(), "{choice:?}");
                if let (Some(got), Some(want)) = (got_high, high) {
                    assert!((got - want).abs() < 1e-9, "{choice:?}");
                }
            }
            other => panic!("expected the designer's range, got {other:?}"),
        }
    }

    /// A library import of a `.gcs` attaches the converted `.asc` first and the `.gcs` beside it:
    /// the design that opens carries the range the `.gcs` states.
    #[test]
    fn a_library_design_from_a_gcs_with_an_ri_range_opens_with_that_range() {
        let mut full = empty_full_record();
        full.attached_files.push(AttachedFile {
            name: "rb.asc".to_string(),
            url: String::new(),
            content: b"GemCad 5.0\ng 96 0.0\ny 4 y\nI 1.54\na -41.000000 0.64991234 92 n 1 84\n"
                .to_vec(),
        });
        full.attached_files.push(AttachedFile {
            name: "rb.gcs".to_string(),
            url: String::new(),
            content: gcs_stating(RiRangeChoice::Designer {
                low: 1.7,
                high: Some(2.15),
            }),
        });
        let loaded = design_from_full_record(&full).expect("opens");
        assert_designer_range(loaded.metadata.ri_range_choice(), 1.7, Some(2.15));

        // A `.gcs` that states only the low end is an open-ended range.
        full.attached_files[1].content = gcs_stating(RiRangeChoice::Designer {
            low: 1.63,
            high: None,
        });
        let loaded = design_from_full_record(&full).expect("opens");
        assert_designer_range(loaded.metadata.ri_range_choice(), 1.63, None);
    }

    /// A `.gcs` alone (no converted `.asc` beside it) carries its range in too.
    #[test]
    fn a_gcs_only_record_carries_its_ri_range() {
        let mut full = empty_full_record();
        full.attached_files.push(AttachedFile {
            name: "rb.gcs".to_string(),
            url: String::new(),
            content: gcs_stating(RiRangeChoice::Designer {
                low: 1.7,
                high: Some(2.15),
            }),
        });
        let loaded = design_from_full_record(&full).expect("opens");
        assert_designer_range(loaded.metadata.ri_range_choice(), 1.7, Some(2.15));
    }

    /// A `.gcs` that states no range, and a record with no `.gcs`, leave the choice Off; the
    /// estimate is never written by an import.
    #[test]
    fn no_stated_range_leaves_the_choice_off() {
        let mut full = empty_full_record();
        full.attached_files.push(AttachedFile {
            name: "rb.gcs".to_string(),
            url: String::new(),
            content: gcs_stating(RiRangeChoice::Off),
        });
        let loaded = design_from_full_record(&full).expect("opens");
        assert_eq!(loaded.metadata.ri_range_choice(), RiRangeChoice::Off);

        let mut plain = empty_full_record();
        plain.attached_files.push(AttachedFile {
            name: "design.asc".to_string(),
            url: String::new(),
            content: b"GemCad 5.0\ng 96 0.0\ny 4 y\nI 1.54\na -41.000000 0.64991234 92 n 1 84\n"
                .to_vec(),
        });
        let loaded = design_from_full_record(&plain).expect("opens");
        assert_eq!(loaded.metadata.ri_range_choice(), RiRangeChoice::Off);
    }

    /// Bytes that are no `.gcs` state no range and never panic.
    #[test]
    fn gcs_bytes_that_do_not_read_state_no_range() {
        assert_eq!(gcs_bytes_ri_range(b"not a gcs"), RiRangeChoice::Off);
        assert_eq!(gcs_bytes_ri_range(&[]), RiRangeChoice::Off);
    }
}
