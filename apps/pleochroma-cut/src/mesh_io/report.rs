//! The structured report of a mesh import: what the planner plans on, and one finding per
//! thing the import did or found.
//!
//! The verdict is the scan itself, its convex outline, or refused. The findings reuse the
//! words of the import's notes (the mesh repair, the reason a mesh could not be used, the
//! simplified outline of a smooth scan), so the report and the manual agree. This module has
//! no window, no file access and no rough.

use glam::DVec3;

/// A finding that is good news (the weight matches the scale).
pub const LEVEL_OK: i32 = 0;

/// A finding that only informs.
pub const LEVEL_INFO: i32 = 1;

/// A finding that says what the import repaired.
pub const LEVEL_REPAIRED: i32 = 2;

/// A finding that is a problem: the mesh could not be used as it is.
pub const LEVEL_PROBLEM: i32 = 3;

/// The start of the note the core gives (and the parsers give for a mesh with too many
/// triangles) when a mesh could not be used and the convex outline stands in for it.
const UNUSABLE_PREFIX: &str = "The mesh could not be used as it is (";

/// The end of that note.
const UNUSABLE_SUFFIX: &str = "); its convex outline is used instead.";

/// The start of the note the mesh repair gives.
const REPAIRED_PREFIX: &str = "The mesh was repaired: ";

/// What the planner plans on for a scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanVerdict {
    /// The scan's own surface decides where the material is.
    ScanItself,
    /// The convex outline of the scan's vertices stands in for the mesh.
    ConvexOutline,
    /// The file was not used at all.
    Refused,
}

impl ScanVerdict {
    /// The number the window's `scan_verdict` holds: 0 the scan itself, 1 its convex outline,
    /// 2 refused.
    #[must_use]
    pub const fn index(self) -> i32 {
        match self {
            Self::ScanItself => 0,
            Self::ConvexOutline => 1,
            Self::Refused => 2,
        }
    }

    /// The words of the "Planned on" badge.
    #[must_use]
    pub const fn text(self) -> &'static str {
        match self {
            Self::ScanItself => "the scan itself",
            Self::ConvexOutline => "its convex outline",
            Self::Refused => "refused",
        }
    }
}

/// What a finding is about; the window shows the level and the text, the code that locates a
/// finding in the view reads the kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FindingKind {
    /// Vertices closer than a tolerance were joined.
    Joined,
    /// Faces that ran the wrong way round were turned.
    Turned,
    /// Small holes were filled.
    Filled,
    /// The mesh crosses itself.
    Crossing,
    /// The mesh could not be used for another reason, so its convex outline is used.
    Fallback,
    /// The file has no faces at all (a point cloud).
    NoFaces,
    /// A smooth scan's outline was simplified for the grid.
    Outline,
    /// The plan was saved with the repaired mesh and the import report is gone.
    Saved,
}

/// One thing the import did or found.
#[derive(Debug, Clone, PartialEq)]
pub struct ScanFinding {
    /// 0 ok, 1 info, 2 repaired, 3 problem (see the `LEVEL_*` constants).
    pub level: i32,
    /// What happened, in the words of the import's notes.
    pub text: String,
    /// What it means and what to do about it; may be empty.
    pub detail: String,
    /// Which kind of finding this is.
    pub kind: FindingKind,
    /// Where the view should turn to show it, in the rough's frame in millimetres at the scale
    /// of the import; `None` for a finding that has no place.
    pub place: Option<[f64; 3]>,
}

/// What the import of one mesh file did, found and decided.
#[derive(Debug, Clone, PartialEq)]
pub struct ImportReport {
    /// What the planner plans on.
    pub verdict: ScanVerdict,
    /// The words of the "Planned on" badge; for a refused file the reason and what to do.
    pub verdict_text: String,
    /// One entry per thing done or found, in the order the import met them.
    pub findings: Vec<ScanFinding>,
    /// Indices into the repaired mesh's triangles that the repair added (the fill of a hole);
    /// empty without a repair.
    pub repaired_triangles: Vec<u32>,
    /// Where the mesh crosses itself, in the rough's frame in millimetres at the scale of the
    /// import. The core's search reports the first place only, so there is at most one.
    pub crossings: Vec<[f64; 3]>,
    /// How many planes the outline the grid uses has (0 when the base is not a hull).
    pub outline_planes: usize,
    /// How many triangles the file had.
    pub triangles: usize,
    /// How many cavities the used mesh has (0 for a convex outline).
    pub cavities: usize,
}

/// The notes of one import, as the import produced them.
#[derive(Debug, Clone, Copy, Default)]
pub struct ImportNotes<'a> {
    /// The parser's note: a polygon that could not be split, or too many triangles.
    pub split_note: Option<&'a str>,
    /// The core's note for a mesh that could not be used.
    pub import_note: Option<&'a str>,
    /// What the mesh repair did.
    pub repair_note: Option<&'a str>,
    /// What was told of a smooth scan's simplified outline.
    pub outline_note: Option<&'a str>,
    /// The file has no faces and nothing else explains it.
    pub no_faces: bool,
}

/// `text` with its first letter in capitals.
fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// The reason inside a "could not be used as it is" note, `None` for any other text.
fn unusable_reason(note: &str) -> Option<&str> {
    note.strip_prefix(UNUSABLE_PREFIX)?
        .strip_suffix(UNUSABLE_SUFFIX)
}

/// The place a self-crossing reason names, in the file's coordinates: the reason reads "the
/// mesh crosses itself in 3 places, the first near x 1.00, y 2.00, z 3.00 mm" (and "at least 16"
/// when the search stopped).
#[must_use]
pub fn crossing_place(note: &str) -> Option<[f64; 3]> {
    let tail = note.split("the first near x ").nth(1)?;
    let numbers = tail.split(" mm").next()?;
    let mut parts = numbers.split(", ");
    let x = parts.next()?.trim().parse().ok()?;
    let y = parts.next()?.strip_prefix("y ")?.trim().parse().ok()?;
    let z = parts.next()?.strip_prefix("z ")?.trim().parse().ok()?;
    Some([x, y, z])
}

/// The finding for a mesh that could not be used, from the reason the note names.
fn unusable_finding(reason: &str) -> ScanFinding {
    if let Some((head, tail)) = reason.split_once(", the first near ") {
        return ScanFinding {
            level: LEVEL_PROBLEM,
            text: capitalised(head),
            detail: format!(
                "The first is near {tail}. Fix it in a mesh tool to plan on the scan itself; \
                 its convex outline is used instead."
            ),
            kind: FindingKind::Crossing,
            place: None,
        };
    }
    ScanFinding {
        level: LEVEL_PROBLEM,
        text: capitalised(reason),
        detail: "Its convex outline is used instead. Repair the mesh in a mesh tool to plan on \
                 the scan itself."
            .to_string(),
        kind: FindingKind::Fallback,
        place: None,
    }
}

/// The finding for a parser note: the same sentence as the import's note when the mesh had too
/// many triangles, the polygon sentence otherwise.
fn split_finding(note: &str) -> ScanFinding {
    unusable_reason(note).map_or_else(
        || ScanFinding {
            level: LEVEL_PROBLEM,
            text: note.to_string(),
            detail: "Triangulate the faces in a mesh tool to plan on the scan itself.".to_string(),
            kind: FindingKind::Fallback,
            place: None,
        },
        unusable_finding,
    )
}

/// The findings for the clauses of a repair note ("The mesh was repaired: closed gaps by joining
/// 12 vertices closer than 0.010 mm; turned 3 faces to wind the surface consistently.").
fn repair_findings(note: &str) -> Vec<ScanFinding> {
    let body = note
        .strip_prefix(REPAIRED_PREFIX)
        .unwrap_or(note)
        .trim_end_matches('.');
    body.split("; ")
        .filter(|clause| !clause.is_empty())
        .map(|clause| {
            let (kind, detail) = if clause.starts_with("closed gaps") {
                (
                    FindingKind::Joined,
                    "Vertices closer than this were treated as one.",
                )
            } else if clause.starts_with("turned") {
                (
                    FindingKind::Turned,
                    "Faces that ran the wrong way round their neighbours were turned.",
                )
            } else {
                (
                    FindingKind::Filled,
                    "The new triangles are shown in amber when Problems is on.",
                )
            };
            ScanFinding {
                level: LEVEL_REPAIRED,
                text: capitalised(clause),
                detail: detail.to_string(),
                kind,
                place: None,
            }
        })
        .collect()
}

impl ImportReport {
    /// The report of an import with these `notes`. The numbers (triangles, cavities, planes) and
    /// the places are filled by the caller, which knows the mesh and its frame.
    #[must_use]
    pub fn from_notes(notes: &ImportNotes<'_>) -> Self {
        let mut findings = Vec::new();
        if let Some(note) = notes.split_note {
            findings.push(split_finding(note));
        }
        if let Some(reason) = notes.import_note.and_then(unusable_reason) {
            findings.push(unusable_finding(reason));
        } else if let Some(note) = notes.import_note {
            findings.push(ScanFinding {
                level: LEVEL_PROBLEM,
                text: note.to_string(),
                detail: String::new(),
                kind: FindingKind::Fallback,
                place: None,
            });
        }
        if let Some(note) = notes.repair_note {
            findings.extend(repair_findings(note));
        }
        if let Some(note) = notes.outline_note {
            findings.push(ScanFinding {
                level: LEVEL_INFO,
                text: note.to_string(),
                detail: "Every stone is checked against the scan itself, not only against the \
                         outline."
                    .to_string(),
                kind: FindingKind::Outline,
                place: None,
            });
        }
        if notes.no_faces && notes.split_note.is_none() && notes.import_note.is_none() {
            findings.push(ScanFinding {
                level: LEVEL_INFO,
                text: "The file has no faces, so its convex hull is the rough.".to_string(),
                detail: "A point cloud has no surface to plan on; its convex outline is used."
                    .to_string(),
                kind: FindingKind::NoFaces,
                place: None,
            });
        }
        let fallback = notes.split_note.is_some() || notes.import_note.is_some() || notes.no_faces;
        let verdict = if fallback {
            ScanVerdict::ConvexOutline
        } else {
            ScanVerdict::ScanItself
        };
        Self {
            verdict,
            verdict_text: verdict.text().to_string(),
            findings,
            repaired_triangles: Vec::new(),
            crossings: Vec::new(),
            outline_planes: 0,
            triangles: 0,
            cavities: 0,
        }
    }

    /// The report of a file that was refused: `reason` says why and what to do about it.
    #[must_use]
    pub fn refused(reason: &str) -> Self {
        Self {
            verdict: ScanVerdict::Refused,
            verdict_text: reason.to_string(),
            findings: Vec::new(),
            repaired_triangles: Vec::new(),
            crossings: Vec::new(),
            outline_planes: 0,
            triangles: 0,
            cavities: 0,
        }
    }

    /// The report of a plan that was opened from a saved plan: the rough's repaired mesh (or its
    /// convex outline) without what the import said. `has_mesh` tells which of the two it is.
    #[must_use]
    pub fn saved(has_mesh: bool, outline_planes: usize, triangles: usize, cavities: usize) -> Self {
        let verdict = if has_mesh {
            ScanVerdict::ScanItself
        } else {
            ScanVerdict::ConvexOutline
        };
        Self {
            verdict,
            verdict_text: verdict.text().to_string(),
            findings: vec![ScanFinding {
                level: LEVEL_INFO,
                text: "Saved with the plan, repaired on import".to_string(),
                detail: "The plan holds the mesh as it was planned on; what the import found is \
                         shown when the file is read again."
                    .to_string(),
                kind: FindingKind::Saved,
                place: None,
            }],
            repaired_triangles: Vec::new(),
            crossings: Vec::new(),
            outline_planes,
            triangles,
            cavities,
        }
    }

    /// The highest level among the findings, 0 without any.
    #[must_use]
    pub fn highest_level(&self) -> i32 {
        self.findings.iter().map(|f| f.level).max().unwrap_or(0)
    }

    /// The first finding of `kind` (the import tests look findings up by kind).
    #[cfg(test)]
    #[must_use]
    pub fn finding_of(&self, kind: FindingKind) -> Option<&ScanFinding> {
        self.findings.iter().find(|f| f.kind == kind)
    }

    /// The centroid of `triangle`, for the place of a finding.
    #[must_use]
    pub fn centroid(triangle: [DVec3; 3]) -> [f64; 3] {
        ((triangle[0] + triangle[1] + triangle[2]) / 3.0).to_array()
    }
}
