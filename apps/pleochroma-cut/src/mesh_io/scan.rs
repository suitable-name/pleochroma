//! Imports a scan for the planner: the rough, the status-line note, the [`ImportReport`] and the
//! geometry the view draws (repaired triangles, crossings, the outline the grid uses).
//!
//! Like the rest of `mesh_io` this has no window and no file access: the caller reads the file,
//! parses it and scales it to millimetres.

use super::report::{FindingKind, ImportNotes, ImportReport, crossing_place};
use glam::DVec3;
use pleochroma_cut_core::rough_plan::{
    RoughBase, RoughMesh, import_mesh,
    shape::hull::{self, SourceFrame},
};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::atomic::{AtomicUsize, Ordering},
};

/// The note of a file that has no faces and nothing else to say.
pub const NO_FACES_NOTE: &str = "The file has no faces, so its convex hull is the rough.";

/// The most outline edges kept for the view; a rough with a bigger outline draws none.
const MAX_OUTLINE_EDGES: usize = 6000;

/// The most planes and corners the outline's edge search handles.
const MAX_OUTLINE_PLANES: usize = 4000;

/// The most corners the outline's edge search handles.
const MAX_OUTLINE_CORNERS: usize = 8000;

/// How close two vertices are to be one in the search for repaired triangles, as a fraction of
/// the mesh's size: a little over the coarsest weld the repair tries (`1e-5`).
const VERTEX_TOLERANCE: f64 = 2e-5;

/// How many triangles of a file have been read, and how many it has: the progress of the read.
///
/// The counts are atomics: the worker thread that parses the file adds to them while the window
/// reads them on a timer.
#[derive(Debug, Default)]
pub struct ReadProgress {
    /// The triangles the file has, `0` while that is not known.
    expected: AtomicUsize,
    /// The triangles read so far.
    parsed: AtomicUsize,
}

impl ReadProgress {
    /// Records that the file has `total` triangles.
    pub fn expect_triangles(&self, total: usize) {
        self.expected.store(total, Ordering::Relaxed);
    }

    /// Records that `count` more triangles were read.
    pub fn add_triangles(&self, count: usize) {
        self.parsed.fetch_add(count, Ordering::Relaxed);
    }

    /// How far the read is, from 0 to 1; `None` while the number of triangles is not known (a
    /// format whose parser does not count, or a file not looked at yet).
    #[must_use]
    pub fn fraction(&self) -> Option<f32> {
        let expected = self.expected.load(Ordering::Relaxed);
        if expected == 0 {
            return None;
        }
        let parsed = self.parsed.load(Ordering::Relaxed).min(expected);
        Some(parsed as f32 / expected as f32)
    }
}

/// The geometry the view draws for a scan, in the rough's frame in millimetres at the size the
/// rough had when it was imported; the view scales it by the rough's size now over `extents_mm`
/// (Fit to Weight scales the rough about its origin).
#[derive(Debug, Clone, PartialEq)]
pub struct ScanLayers {
    /// The extents of the rough's bounding box at the import.
    pub extents_mm: [f64; 3],
    /// The edges of the outline the grid uses: the hull, or the simplified outline of a smooth
    /// scan.
    pub outline: Vec<[DVec3; 2]>,
    /// The triangles the repair added (the fill of a hole), as corner triples.
    pub repaired: Vec<[DVec3; 3]>,
    /// The places where the mesh crosses itself.
    pub crossings: Vec<DVec3>,
}

impl ScanLayers {
    /// The layers of the rough `base` with nothing repaired and nothing crossing (a rough that
    /// came back from a saved plan).
    #[must_use]
    pub fn outline_only(base: &RoughBase) -> Self {
        Self {
            extents_mm: base.bounding_box_extents(),
            outline: outline_segments(base),
            repaired: Vec::new(),
            crossings: Vec::new(),
        }
    }

    /// The factor by which a rough with the bounding box `extents` is bigger than the one the
    /// layers were made for, `None` when it is a different shape: the three sides must have
    /// grown by the same factor.
    #[must_use]
    pub fn scale_for(&self, extents: [f64; 3]) -> Option<f64> {
        let ratios = [0, 1, 2].map(|i| extents[i] / self.extents_mm[i]);
        let first = ratios[0];
        (first.is_finite() && first > 0.0 && ratios.iter().all(|r| (r / first - 1.0).abs() < 1e-6))
            .then_some(first)
    }
}

/// A scan imported: the rough, the note the status line shows, what the import told as a
/// report, and the geometry for the view.
#[derive(Debug)]
pub struct ScanImport {
    /// The rough: the mesh (or, failing that, its convex outline).
    pub base: RoughBase,
    /// The one-line note for the status line, `None` when there is nothing to tell.
    pub note: Option<String>,
    /// The structured report.
    pub report: ImportReport,
    /// What the view draws.
    pub layers: ScanLayers,
}

/// Imports the mesh `points` (already in millimetres) and `triangles` as a rough.
///
/// `split_note` is the parser's note (see [`super::parse_mesh`]); `refuse` looks at the rough and
/// says why it cannot be used (a size that is not believable in the file's unit), which turns the
/// import into an error. The note is the first of: the parser's note, the import's note, the
/// repair and outline notes, or the no-faces note.
///
/// # Errors
///
/// Returns the message of an import the planner cannot take: a hull that cannot be built, a
/// mesh too complex to plan on without its outline, or the reason `refuse` gives.
pub fn import_scan(
    points: &[DVec3],
    triangles: &[[u32; 3]],
    split_note: Option<String>,
    refuse: impl FnOnce(&RoughBase) -> Option<String>,
) -> Result<ScanImport, String> {
    let (base, import_note) = import_mesh(points, triangles).map_err(|e| e.to_string())?;
    if let Some(reason) = refuse(&base) {
        return Err(reason);
    }
    // The outline and repair notes come from the mesh built here, not from `import_mesh`: a
    // saved plan stores the repaired mesh and must come back without them.
    let (mesh, outline) = match base {
        RoughBase::Hull { id, .. } => (hull::mesh(id), hull::outline_note(id)),
        _ => (None, None),
    };
    let repair = mesh
        .as_ref()
        .and_then(|m| m.repair_note().map(str::to_owned));
    let told = match (&repair, &outline) {
        (Some(repair), Some(outline)) => Some(format!("{repair} {outline}")),
        (repair, outline) => repair.clone().or_else(|| outline.clone()),
    };
    let mut report = ImportReport::from_notes(&ImportNotes {
        split_note: split_note.as_deref(),
        import_note: import_note.as_deref(),
        repair_note: repair.as_deref(),
        outline_note: outline.as_deref(),
        no_faces: triangles.is_empty(),
    });
    let note = split_note
        .or_else(|| import_note.clone())
        .or(told)
        .or_else(|| triangles.is_empty().then(|| NO_FACES_NOTE.to_string()));
    let frame = match base {
        RoughBase::Hull { id, .. } => hull::source_frame(id).unwrap_or(SourceFrame::IDENTITY),
        _ => SourceFrame::IDENTITY,
    };
    let mut layers = ScanLayers {
        extents_mm: base.bounding_box_extents(),
        outline: outline_segments(&base),
        repaired: Vec::new(),
        crossings: Vec::new(),
    };
    report.triangles = triangles.len();
    report.outline_planes = match base {
        RoughBase::Hull { .. } => base.to_halfspaces(false).map_or(0, |planes| planes.len()),
        _ => 0,
    };
    if let Some(mesh) = &mesh {
        report.cavities = count_cavities(mesh.vertices(), mesh.triangles());
        if repair.is_some() {
            let moved: Vec<DVec3> = points.iter().map(|&p| frame.apply(p)).collect();
            let size = layers.extents_mm.into_iter().fold(0.0_f64, f64::max);
            report.repaired_triangles = repaired_triangles(
                &moved,
                triangles,
                mesh.vertices(),
                mesh.triangles(),
                VERTEX_TOLERANCE * size,
            );
            layers.repaired = triangle_corners(mesh, &report.repaired_triangles);
        }
    }
    locate_findings(&mut report, &mut layers, import_note.as_deref(), frame);
    Ok(ScanImport {
        base,
        note,
        report,
        layers,
    })
}

/// Sets the place of the findings that have one, and records the crossing in the report and
/// the layers.
///
/// A hole's fill is placed at the middle of its first triangle. A crossing is placed where its
/// note names it, moved from the file's coordinates into the rough's frame by `frame`.
fn locate_findings(
    report: &mut ImportReport,
    layers: &mut ScanLayers,
    import_note: Option<&str>,
    frame: SourceFrame,
) {
    if let Some(first) = layers.repaired.first() {
        let place = ImportReport::centroid(*first);
        for finding in &mut report.findings {
            if finding.kind == FindingKind::Filled {
                finding.place = Some(place);
            }
        }
    }
    if let Some(place) = import_note.and_then(crossing_place) {
        let moved = frame.apply(DVec3::from_array(place));
        report.crossings.push(moved.to_array());
        layers.crossings.push(moved);
        for finding in &mut report.findings {
            if finding.kind == FindingKind::Crossing {
                finding.place = Some(moved.to_array());
            }
        }
    }
}

/// The corners of the triangles `ids` of `mesh`.
fn triangle_corners(mesh: &RoughMesh, ids: &[u32]) -> Vec<[DVec3; 3]> {
    ids.iter()
        .filter_map(|&id| {
            let tri = mesh.triangles().get(id as usize)?;
            let corner = |k: usize| mesh.vertices().get(tri[k] as usize).copied();
            Some([corner(0)?, corner(1)?, corner(2)?])
        })
        .collect()
}

/// A grid that finds the vertex of a set that lies within a tolerance of a point.
struct VertexGrid {
    /// The side of a cell, at least the tolerance.
    cell: f64,
    /// The tolerance.
    tolerance: f64,
    /// The vertices, by id.
    points: Vec<DVec3>,
    /// The ids of the vertices in each cell.
    cells: BTreeMap<[i64; 3], Vec<u32>>,
}

/// The index of the grid cell holding the coordinate `value` for cells of side `cell`.
#[expect(
    clippy::cast_possible_truncation,
    reason = "a coordinate over the tolerance is far inside the i64 range, and the cast saturates"
)]
fn cell_index(value: f64, cell: f64) -> i64 {
    (value / cell).floor() as i64
}

impl VertexGrid {
    /// An empty grid for vertices `tolerance` apart.
    const fn new(tolerance: f64) -> Self {
        let tolerance = tolerance.max(1e-12);
        Self {
            cell: tolerance,
            tolerance,
            points: Vec::new(),
            cells: BTreeMap::new(),
        }
    }

    /// The cell of `p`.
    fn cell_of(&self, p: DVec3) -> [i64; 3] {
        [
            cell_index(p.x, self.cell),
            cell_index(p.y, self.cell),
            cell_index(p.z, self.cell),
        ]
    }

    /// The nearest vertex within the tolerance of `p`.
    fn find(&self, p: DVec3) -> Option<u32> {
        let [cx, cy, cz] = self.cell_of(p);
        let mut best: Option<(f64, u32)> = None;
        for dx in -1..=1 {
            for dy in -1..=1 {
                for dz in -1..=1 {
                    let Some(ids) = self.cells.get(&[cx + dx, cy + dy, cz + dz]) else {
                        continue;
                    };
                    for &id in ids {
                        let distance = self.points[id as usize].distance(p);
                        if distance <= self.tolerance && best.is_none_or(|(d, _)| distance < d) {
                            best = Some((distance, id));
                        }
                    }
                }
            }
        }
        best.map(|(_, id)| id)
    }

    /// The vertex within the tolerance of `p`, or a new one at `p`; `None` past `u32` ids.
    fn find_or_insert(&mut self, p: DVec3) -> Option<u32> {
        if let Some(id) = self.find(p) {
            return Some(id);
        }
        let id = u32::try_from(self.points.len()).ok()?;
        self.points.push(p);
        let cell = self.cell_of(p);
        self.cells.entry(cell).or_default().push(id);
        Some(id)
    }
}

/// The triangles of the repaired mesh (`mesh_verts`, `mesh_tris`) that are not a triangle of the
/// file (`points`, `triangles`, both in the mesh's frame): what the repair added. Vertices
/// within `tolerance` of each other are one, which absorbs the welds of the import and of the
/// repair; a triangle with a vertex that is none of the file's (the middle of a filled hole) is
/// new as well.
///
/// A triangle the repair only turned is not listed: it is the file's triangle the other way
/// round.
#[must_use]
pub fn repaired_triangles(
    points: &[DVec3],
    triangles: &[[u32; 3]],
    mesh_verts: &[DVec3],
    mesh_tris: &[[u32; 3]],
    tolerance: f64,
) -> Vec<u32> {
    let mut grid = VertexGrid::new(tolerance);
    let canonical: Vec<Option<u32>> = points.iter().map(|&p| grid.find_or_insert(p)).collect();
    let sorted = |ids: [Option<u32>; 3]| -> Option<[u32; 3]> {
        let mut ids = [ids[0]?, ids[1]?, ids[2]?];
        ids.sort_unstable();
        Some(ids)
    };
    let original: BTreeSet<[u32; 3]> = triangles
        .iter()
        .filter_map(|tri| sorted(tri.map(|v| canonical.get(v as usize).copied().flatten())))
        .collect();
    mesh_tris
        .iter()
        .enumerate()
        .filter(|(_, tri)| {
            let ids = tri.map(|v| mesh_verts.get(v as usize).and_then(|&p| grid.find(p)));
            sorted(ids).is_none_or(|found| !original.contains(&found))
        })
        .filter_map(|(index, _)| u32::try_from(index).ok())
        .collect()
}

/// How many cavities the closed mesh `verts`, `tris` has: the edge-connected shells whose signed
/// volume is negative (a cavity's faces look into the void).
#[must_use]
pub fn count_cavities(verts: &[DVec3], tris: &[[u32; 3]]) -> usize {
    let mut parent: Vec<usize> = (0..verts.len()).collect();
    for tri in tris {
        let [a, b, c] = tri.map(|v| v as usize);
        if a >= verts.len() || b >= verts.len() || c >= verts.len() {
            continue;
        }
        let ra = shell_root(&mut parent, a);
        let rb = shell_root(&mut parent, b);
        parent[rb] = ra;
        let rc = shell_root(&mut parent, c);
        let ra = shell_root(&mut parent, a);
        parent[rc] = ra;
    }
    let mut volumes: BTreeMap<usize, f64> = BTreeMap::new();
    for tri in tris {
        let [a, b, c] = tri.map(|v| v as usize);
        let (Some(&pa), Some(&pb), Some(&pc)) = (verts.get(a), verts.get(b), verts.get(c)) else {
            continue;
        };
        let shell = shell_root(&mut parent, a);
        *volumes.entry(shell).or_insert(0.0) += pa.dot(pb.cross(pc)) / 6.0;
    }
    volumes.values().filter(|&&volume| volume < 0.0).count()
}

/// The representative of the shell vertex `at` belongs to, in the union-find `parent`.
const fn shell_root(parent: &mut [usize], mut at: usize) -> usize {
    while parent[at] != at {
        parent[at] = parent[parent[at]];
        at = parent[at];
    }
    at
}

/// The edges of the convex outline of `base` (a hull; any other base has none): the corner
/// pairs that lie on two planes of the outline at once.
///
/// An outline with more planes or corners than the view can draw gives no edges.
#[must_use]
pub fn outline_segments(base: &RoughBase) -> Vec<[DVec3; 2]> {
    if !matches!(base, RoughBase::Hull { .. }) {
        return Vec::new();
    }
    let (Ok(planes), Ok(corners)) = (base.to_halfspaces(false), base.vertices(false)) else {
        return Vec::new();
    };
    if planes.len() > MAX_OUTLINE_PLANES || corners.len() > MAX_OUTLINE_CORNERS {
        return Vec::new();
    }
    let size = base
        .bounding_box_extents()
        .into_iter()
        .fold(0.0_f64, f64::max);
    let eps = 1e-6 * size.max(1e-9);
    // The planes each corner lies on, ascending, and the corners each plane holds.
    let mut on_planes: Vec<Vec<usize>> = vec![Vec::new(); corners.len()];
    let mut of_plane: Vec<Vec<usize>> = vec![Vec::new(); planes.len()];
    for (p, &(normal, offset)) in planes.iter().enumerate() {
        for (c, corner) in corners.iter().enumerate() {
            if (normal.dot(*corner) - offset).abs() <= eps {
                on_planes[c].push(p);
                of_plane[p].push(c);
            }
        }
    }
    let mut edges: BTreeSet<(usize, usize)> = BTreeSet::new();
    for (p, face) in of_plane.iter().enumerate() {
        for (i, &a) in face.iter().enumerate() {
            for &b in &face[i + 1..] {
                let shared_turn = on_planes[a]
                    .iter()
                    .filter(|q| **q != p && on_planes[b].contains(q))
                    .any(|&q| planes[p].0.cross(planes[q].0).length() > 1e-6);
                if shared_turn {
                    edges.insert((a.min(b), a.max(b)));
                }
            }
        }
        if edges.len() > MAX_OUTLINE_EDGES {
            return Vec::new();
        }
    }
    edges
        .into_iter()
        .map(|(a, b)| [corners[a], corners[b]])
        .collect()
}
