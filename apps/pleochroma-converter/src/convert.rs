//! Converts recorded cutting instructions without changing their pinned geometry.

use anyhow::{Context, Result, anyhow, ensure};
use pleochroma::geometry::{
    GpuFacetPlane,
    cuts::StandardGemCuts,
    meet_solver::SolveStrategy,
    stone_metrics::{SolidStatus, build_solid_mesh},
};
use pleochroma_cut_core::{
    Design, PreformSpec,
    native::{AttachmentBlob, AttachmentRole, DesignExtras, design_from_str, design_to_file},
};
use pleochroma_editor::{
    files::{InputFileKind, convert_foreign_design},
    loading::default_preform_for_schedule,
};
use pleochroma_formats::{asc, native::design as native_format};

use crate::database::{Entry, Source};

pub struct Converted {
    pub text: String,
    pub warnings: Vec<String>,
    pub draft: bool,
    pub adopted_meets: usize,
}

pub fn convert(entry: &Entry, source: &Source, adopt: bool) -> Result<Converted> {
    ensure!(!source.bytes.is_empty(), "the design attachment is empty");
    let kind = InputFileKind::classify(&source.name, &source.bytes)
        .ok_or_else(|| anyhow!("unsupported design attachment: {}", source.name))?;
    if kind == InputFileKind::Design {
        return convert_native(source, adopt);
    }
    convert_schedule(entry, source, kind, adopt)
}

fn convert_native(source: &Source, adopt: bool) -> Result<Converted> {
    let original_text = std::str::from_utf8(&source.bytes).context("native design is not UTF-8")?;
    let mut file = native_format::parse(original_text).context("invalid native design")?;
    let mut loaded = design_from_str(original_text).context("native design cannot be restored")?;
    let mut warnings = Vec::new();
    let mut draft = loaded.draft;
    if draft {
        warnings.push("The source native document is already marked as a draft.".to_owned());
    } else if let Err(error) = verify_solved(&loaded.design) {
        draft = true;
        warnings.push(format!(
            "The source native design does not solve; saved as a draft: {error:#}"
        ));
    }
    let adopted_meets = if adopt && !draft {
        let enrichment = crate::enrich::adopt_meets(&mut loaded.design);
        warnings.extend(enrichment.notes);
        enrichment.adopted
    } else {
        0
    };
    let text = if adopted_meets == 0 && draft == loaded.draft {
        // This also preserves comments, future keys and the original history length.
        original_text.to_owned()
    } else {
        // Change only adopted tiers and/or the draft flag. Keep every other table,
        // including future fields and an unbounded original history.
        if adopted_meets > 0 {
            file.tiers = design_to_file(&loaded.design, None, &DesignExtras::default()).tiers;
        }
        file.draft = draft;
        file.version = file.version.max(file.needed_version());
        checked_text(&file, &loaded.design)?
    };
    Ok(Converted {
        text,
        warnings,
        draft,
        adopted_meets,
    })
}

fn convert_schedule(
    entry: &Entry,
    source: &Source,
    kind: InputFileKind,
    adopt: bool,
) -> Result<Converted> {
    let (text, mut warnings, frosting) = match kind {
        InputFileKind::Asc => (
            asc::decode_asc_bytes(&source.bytes).into_owned(),
            Vec::new(),
            Vec::new(),
        ),
        InputFileKind::Gem | InputFileKind::Gcs => {
            let converted = convert_foreign_design(&source.name, kind, &source.bytes)
                .map_err(|error| anyhow!(error))?;
            (converted.asc_text, converted.warnings, converted.frosting)
        }
        InputFileKind::Design | InputFileKind::Hdr => {
            return Err(anyhow!("expected ASC, GEM or GCS cutting instructions"));
        }
    };
    let schedule = asc::parse_asc(&text).context("invalid cutting instructions")?;
    ensure!(
        !schedule.tiers.is_empty(),
        "the schedule has no facet tiers"
    );
    if kind == InputFileKind::Gcs {
        warnings.push(
            "GCS preview material and dispersion settings are not fully translated; the original GCS attachment preserves them. Supported facet frosting is imported."
                .to_owned(),
        );
    }
    warnings.extend(schedule.warnings.iter().cloned());
    let mut design =
        Design::from_asc_schedule(default_preform_for_schedule(&schedule, None), &schedule);
    design.apply_imported_frosting(&frosting);
    let mut draft = infer_bounding_preform(&mut design, &schedule, &mut warnings);
    if let Err(error) = verify_pinned_masts(&design, &schedule) {
        draft = true;
        warnings.push(format!("The recorded tiers could not be solved: {error:#}"));
    }
    let adopted_meets = if adopt && !draft {
        let enrichment = crate::enrich::adopt_meets(&mut design);
        warnings.extend(enrichment.notes);
        enrichment.adopted
    } else {
        0
    };
    let attachment = original_attachment(source, kind);
    let mut metadata = entry.metadata.clone();
    if !metadata.notes.is_empty() {
        metadata.notes.push_str("\n\n");
    }
    metadata.notes.push_str(if draft {
        "Converter provenance: rough is inferred for display, not measured. This draft is not a certified closed cut. The original source attachment retains the recorded geometry. Physical dimensions and material composition were not inferred."
    } else {
        "Converter provenance: the enclosing block was inferred from the recorded cut, with 5% clearance on each axis. It is not measured rough and must not be used as a real yield target. Physical dimensions and material composition were not inferred."
    });
    let extras = DesignExtras {
        metadata: Some(&metadata),
        attachments: std::slice::from_ref(&attachment),
        ..DesignExtras::default()
    };
    let file = design_to_file(&design, None, &extras).with_draft(draft);
    let text = checked_text(&file, &design)?;
    Ok(Converted {
        text,
        warnings,
        draft,
        adopted_meets,
    })
}

/// Returns whether the source lacks a complete solid independent of guessed rough.
fn infer_bounding_preform(
    design: &mut Design,
    schedule: &asc::AscSchedule,
    warnings: &mut Vec<String>,
) -> bool {
    let planes: Vec<_> = StandardGemCuts::from_asc_schedule(schedule)
        .into_iter()
        .map(GpuFacetPlane::to_halfspace_f64)
        .collect();
    let mesh = match build_solid_mesh(&planes) {
        SolidStatus::Closed(mesh) => mesh,
        SolidStatus::Unbounded { .. } => {
            warnings.push(
                "Source facets do not enclose a finite solid; saved as a draft with the editor's default rough."
                    .to_owned(),
            );
            return true;
        }
        SolidStatus::Degenerate { .. } => {
            warnings.push(
                "Source facets form a degenerate solid; saved as a draft with the editor's default rough."
                    .to_owned(),
            );
            return true;
        }
    };
    let mut extent = [0.0_f64; 3];
    for position in &mesh.positions {
        for (axis, maximum) in extent.iter_mut().enumerate() {
            *maximum = maximum.max(position[axis].abs());
        }
    }
    let half_width = extent[0] * 1.05;
    let length_over_width = extent[2] / extent[0];
    let depth = extent[1] * 2.1;
    if ![half_width, length_over_width, depth]
        .into_iter()
        .all(|value| value.is_finite() && value > 0.0)
    {
        warnings.push("Could not derive a finite enclosing rough; saved as a draft.".to_owned());
        return true;
    }
    design.preform = PreformSpec::block(half_width, length_over_width, depth);
    warnings.push(
        "A bounding block with 5% clearance was inferred from the cut; actual rough, physical size and material composition are unknown."
            .to_owned(),
    );
    false
}

fn verify_pinned_masts(design: &Design, schedule: &asc::AscSchedule) -> Result<()> {
    let solved = design
        .solve()
        .context("the imported constraints do not solve")?;
    ensure!(solved.len() == schedule.tiers.len(), "tier count changed");
    for (index, (solved, original)) in solved.iter().zip(&schedule.tiers).enumerate() {
        ensure!(
            solved.strategy == SolveStrategy::ScaleReference
                && solved.mast.to_bits() == original.mast.to_bits(),
            "tier {} did not retain its exact original mast",
            index + 1
        );
    }
    Ok(())
}

fn verify_solved(design: &Design) -> Result<()> {
    let solved = design
        .solve()
        .context("the native constraints do not solve")?;
    ensure!(
        !solved.is_empty(),
        "the native document has no planar facet tiers"
    );
    ensure!(solved.len() == design.tiers.len(), "tier count changed");
    ensure!(
        solved
            .iter()
            .all(|tier| { tier.strategy != SolveStrategy::Failed && tier.mast.is_finite() }),
        "one or more native tiers failed to solve"
    );
    Ok(())
}

fn original_attachment(source: &Source, kind: InputFileKind) -> AttachmentBlob {
    let (extension, role) = match kind {
        InputFileKind::Asc => ("asc", AttachmentRole::Asc),
        InputFileKind::Gem => ("gem", AttachmentRole::Gem),
        InputFileKind::Gcs => ("gcs", AttachmentRole::Other),
        InputFileKind::Design | InputFileKind::Hdr => ("bin", AttachmentRole::Other),
    };
    // Database names may contain path separators, control characters or duplicate
    // basenames. An id-based attachment name is always safe and unambiguous.
    let mut attachment = AttachmentBlob::new(
        format!("source-{}.{extension}", source.id),
        role,
        source.bytes.clone(),
    );
    if source.url.starts_with("https://") || source.url.starts_with("http://") {
        attachment.source_url.clone_from(&source.url);
    }
    attachment
}

fn checked_text(file: &native_format::DesignFile, expected: &Design) -> Result<String> {
    let text = native_format::to_string(file).context("cannot write native design")?;
    let parsed = native_format::parse(&text).context("written native document cannot be parsed")?;
    ensure!(parsed == *file, "native serialization changed the document");
    let restored = design_from_str(&text).context("written native design cannot be restored")?;
    ensure!(
        restored.design == *expected && restored.design.tier_ids_eq(expected),
        "native round-trip changed the design's authored content or tier identities"
    );
    for (before, after) in expected.tiers.iter().zip(&restored.design.tiers) {
        ensure!(
            before.angle_deg.to_bits() == after.angle_deg.to_bits(),
            "native round-trip changed a facet angle's sign or precision"
        );
    }
    Ok(text)
}

#[cfg(test)]
#[path = "convert_tests.rs"]
mod tests;
