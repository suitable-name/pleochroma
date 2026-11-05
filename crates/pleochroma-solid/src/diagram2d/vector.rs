//! The four printed sheet views as line drawings: crisp black facet outlines with their tier
//! labels, for the cutting-instructions template of the HTML and PDF sheets.
//!
//! Each view uses the layout of its raster twin ([`super::render_diagram_single_panel`] and
//! [`super::render_diagram_end_view`]): the same [`compute_single_panel_layout`], projected with
//! [`project_point`], so a vector view of `width` x `height` drawing units lines up with the raster
//! frame of `width` x `height` pixels -- the same centre, scale and index-wheel ring.
//!
//! # Hidden lines
//!
//! The views are orthographic and a faceted stone is convex, so a facet is drawn exactly when its
//! normal points toward the viewer (up for the crown, down for the pavilion, world `+Z` for the
//! profile and the end view, which is the stone turned a quarter turn). An edge two drawn facets
//! share is an [`LineWeight::Edge`]; an edge of only one drawn facet borders a facet turned away,
//! so it is on the silhouette and drawn as [`LineWeight::Outline`]. Corners are matched in the
//! drawing's own coordinates within a small tolerance, so the two copies of a shared edge are
//! found even when their end points differ in the last bits.
//!
//! A concave design keeps the raster's rules for its tool pieces: a crown or pavilion piece is
//! drawn only on the view of its own block, and a seam between two pieces of one curved facet is
//! not drawn unless it is on the silhouette. A non-convex stone can still show an edge that a
//! facet in front of it would hide; the raster's depth test has no line-drawing twin here.
//!
//! # Labels
//!
//! Every drawn facet carries its label ([`DiagramStyle::facet_labels`]) at the centroid of its
//! largest drawn piece, when the label's box fits inside that piece (first at the full size, then
//! smaller); a facet too small to hold it gets none. The crown and pavilion views keep the index
//! wheel: one [`LineWeight::Tick`] per tooth and the tooth numbers at every long tick, tooth 0
//! written as the gear's tooth count, as the template's index lists write it.

use super::{
    DiagramConfig, DiagramStyle, NORMAL_EPS, PanelKind,
    layout::{
        LARGE_FRAME_MIN_HEIGHT, PanelLayout, compute_single_panel_layout, facet_normals,
        project_point, wheel_direction,
    },
    legend::tick_stride,
    render::turned_quarter,
};
use crate::raster::screen_centroid;
use glam::DVec3;
use pleochroma::geometry::{meet_solver::Block, stone_metrics::SolidMesh};
use std::{
    collections::{BTreeMap, HashMap, hash_map::Entry},
    fmt::Write as _,
};

/// A facet label's font size as a fraction of the view's shorter side.
const LABEL_SIZE: f32 = 0.032;
/// The index wheel's tooth numbers, as a fraction of the view's shorter side.
const TOOTH_SIZE: f32 = 0.026;
/// The caption's font size, as a fraction of the view's shorter side.
const CAPTION_SIZE: f32 = 0.03;
/// The smaller size a label is tried at when it does not fit at [`LABEL_SIZE`].
const LABEL_SHRINK: f32 = 0.7;
/// A label's estimated width per character, in ems (Helvetica's digits are 0.556, its capitals
/// 0.611 to 0.778; a little extra keeps a margin).
const LABEL_EM_PER_CHAR: f32 = 0.62;
/// A label's box height in ems: Helvetica's cap height (0.718) plus a margin.
const LABEL_BOX_EMS: f32 = 0.9;
/// How far below a label's centre its baseline sits, in ems: half Helvetica's cap height, so
/// capitals and digits are centred on the label's point.
pub const BASELINE_DROP_EMS: f32 = 0.359;
/// Two projected corners closer than this fraction of the view's longer side are one corner.
const CORNER_TOLERANCE: f32 = 1e-4;

/// One of the four sheet views as a line drawing, in drawing units (`y` grows downward, as on a
/// page or in SVG).
#[derive(Debug, Clone, PartialEq)]
pub struct VectorView {
    /// The drawing's width, in drawing units (the raster twin's pixels).
    pub width: f32,
    /// The drawing's height, in drawing units.
    pub height: f32,
    /// `CROWN`, `PROFILE`, `END` or `PAVILION`.
    pub caption: String,
    /// Every line, in a deterministic order: facet edges first, then the index wheel's ticks.
    pub lines: Vec<VectorLine>,
    /// Facet labels and tooth numbers, each centred on its point.
    pub labels: Vec<VectorLabel>,
}

/// One straight line of a [`VectorView`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VectorLine {
    /// Start point, in drawing units.
    pub from: (f32, f32),
    /// End point, in drawing units.
    pub to: (f32, f32),
    /// How heavy the line is drawn ([`VectorView::stroke_width`]).
    pub weight: LineWeight,
}

/// How heavy a [`VectorLine`] is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LineWeight {
    /// The stone's silhouette: an edge between a drawn facet and one turned away.
    Outline,
    /// An edge between two drawn facets.
    Edge,
    /// An index-wheel tick.
    Tick,
}

/// A piece of text of a [`VectorView`], centred on `(x, y)`.
#[derive(Debug, Clone, PartialEq)]
pub struct VectorLabel {
    /// The text (a tier code, a tooth number, a caption).
    pub text: String,
    /// Horizontal centre, in drawing units.
    pub x: f32,
    /// Vertical centre, in drawing units (`y` down).
    pub y: f32,
    /// Font size (the em), in drawing units.
    pub size: f32,
}

impl VectorView {
    /// The stroke width of `weight` in drawing units: a fixed fraction of the view's shorter
    /// side, so a view scaled into a page cell keeps its proportions.
    #[must_use]
    pub fn stroke_width(&self, weight: LineWeight) -> f32 {
        let base = self.width.min(self.height);
        match weight {
            LineWeight::Outline => base * 0.0055,
            LineWeight::Edge => base * 0.0028,
            LineWeight::Tick => base * 0.0022,
        }
    }

    /// Where the caption is printed: centred under the drawing in a view at least
    /// [`LARGE_FRAME_MIN_HEIGHT`] tall (the band the layout keeps free there), else centred in
    /// the band above it, as the raster twin places its caption.
    #[must_use]
    pub fn caption_label(&self) -> VectorLabel {
        let size = self.width.min(self.height) * CAPTION_SIZE;
        let y = if self.height >= LARGE_FRAME_MIN_HEIGHT as f32 {
            size.mul_add(-0.75, self.height)
        } else {
            (self.height * 0.06).clamp(10.0, 20.0) / 2.0
        };
        VectorLabel {
            text: self.caption.clone(),
            x: self.width / 2.0,
            y,
            size,
        }
    }
}

/// The four sheet views of `mesh` as line drawings, in the template's grid order.
///
/// The order is `+Z` (crown, seen from above), `+X` (the profile), `-Y` (the end view, the
/// stone turned a quarter turn) and `-Z` (the pavilion, seen from below).
///
/// Each view is `config.width` x `config.height` drawing units and is laid out as its raster
/// twin of that many pixels. `style` supplies the facet labels and the concave tool blocks; its
/// colors are not used (a line drawing is black on white). `None` for a zero-sized `config` or
/// a mesh with no facets.
#[must_use]
pub fn render_vector_views(
    mesh: &SolidMesh,
    config: &DiagramConfig,
    style: &DiagramStyle,
) -> Option<[VectorView; 4]> {
    if config.width == 0 || config.height == 0 || mesh.rings.is_empty() {
        return None;
    }
    let wheel = Wheel {
        teeth: config.gear_teeth.max(1),
        reference: config.gear_reference_angle,
    };
    let turned = turned_quarter(mesh);
    Some([
        vector_view(mesh, config, style, PanelKind::Crown, "CROWN", wheel),
        vector_view(mesh, config, style, PanelKind::Profile, "PROFILE", wheel),
        vector_view(&turned, config, style, PanelKind::Profile, "END", wheel),
        vector_view(mesh, config, style, PanelKind::Pavilion, "PAVILION", wheel),
    ])
}

/// `view` as a standalone SVG document: black lines and text on a white ground, the view's own
/// drawing units as the `viewBox`, so the picture scales to whatever box it is placed in.
///
/// The lines of one weight are one `<path>`; every label and the caption is a `<text>` centred on
/// its point. Deterministic: two decimals per number, nothing else varies.
#[must_use]
pub fn view_to_svg(view: &VectorView) -> String {
    let mut out = String::new();
    let _ = write!(
        out,
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {} {}\" role=\"img\" \
         aria-label=\"{}\" font-family=\"Helvetica, Arial, sans-serif\">\
         <rect width=\"{}\" height=\"{}\" fill=\"#ffffff\"/>",
        number(view.width),
        number(view.height),
        xml_escape(&view.caption),
        number(view.width),
        number(view.height),
    );
    for weight in [LineWeight::Tick, LineWeight::Edge, LineWeight::Outline] {
        let mut path = String::new();
        for line in view.lines.iter().filter(|line| line.weight == weight) {
            let _ = write!(
                path,
                "M{} {}L{} {}",
                number(line.from.0),
                number(line.from.1),
                number(line.to.0),
                number(line.to.1)
            );
        }
        if !path.is_empty() {
            let _ = write!(
                out,
                "<path d=\"{path}\" fill=\"none\" stroke=\"#000000\" stroke-width=\"{}\" \
                 stroke-linecap=\"round\"/>",
                number(view.stroke_width(weight))
            );
        }
    }
    for label in view
        .labels
        .iter()
        .chain(std::iter::once(&view.caption_label()))
    {
        let _ = write!(
            out,
            "<text x=\"{}\" y=\"{}\" font-size=\"{}\" text-anchor=\"middle\" \
             fill=\"#000000\">{}</text>",
            number(label.x),
            number(label.size.mul_add(BASELINE_DROP_EMS, label.y)),
            number(label.size),
            xml_escape(&label.text)
        );
    }
    out.push_str("</svg>");
    out
}

/// One number of an SVG: two decimals, and a value that rounds to zero is a plain `0.00`.
fn number(value: f32) -> String {
    if value.abs() < 0.005 {
        "0.00".to_string()
    } else {
        format!("{value:.2}")
    }
}

/// `text` safe inside SVG text and attribute values.
fn xml_escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&#39;"),
            _ => out.push(c),
        }
    }
    out
}

/// The index gear a view's wheel is drawn for.
#[derive(Debug, Clone, Copy)]
struct Wheel {
    /// Number of teeth, never `0`.
    teeth: u32,
    /// The schedule's reference angle, in index units.
    reference: f32,
}

/// One drawn piece of a facet, projected into the view.
struct Piece {
    /// The facet the piece belongs to.
    facet_id: usize,
    /// The ring's corners in drawing units, in ring order.
    points: Vec<(f32, f32)>,
    /// Parallel to `points`: segment `i -> i + 1` is a seam inside one curved facet.
    seam: Vec<bool>,
}

/// One view: the facet lines and labels of the pieces facing the viewer, then the wheel.
fn vector_view(
    mesh: &SolidMesh,
    config: &DiagramConfig,
    style: &DiagramStyle,
    kind: PanelKind,
    caption: &str,
    wheel: Wheel,
) -> VectorView {
    let layout = compute_single_panel_layout(mesh, config.width, config.height, kind);
    let mut view = VectorView {
        width: config.width as f32,
        height: config.height as f32,
        caption: caption.to_string(),
        lines: Vec::new(),
        labels: Vec::new(),
    };
    let pieces = facing_pieces(mesh, &layout, style);
    push_facet_lines(&mut view, &pieces);
    push_facet_labels(&mut view, &pieces, style);
    if kind.has_index_wheel() {
        push_index_wheel(&mut view, &layout, wheel);
    }
    view
}

/// Whether a facet with outward `normal` faces the viewer of a `kind` panel.
fn faces_viewer(kind: PanelKind, normal: DVec3) -> bool {
    let eps = f64::from(NORMAL_EPS);
    match kind {
        PanelKind::Crown => normal.y > eps,
        PanelKind::Pavilion => normal.y < -eps,
        PanelKind::Profile => normal.z > eps,
    }
}

/// Whether a `kind` view may show a concave tool piece of a tier in `block` -- the raster's own
/// rule: crown and pavilion pieces only on their own view, every piece on a side view.
const fn view_shows_block(kind: PanelKind, block: Block) -> bool {
    matches!(
        (kind, block),
        (PanelKind::Crown, Block::Crown)
            | (PanelKind::Pavilion, Block::Pavilion)
            | (PanelKind::Profile, _)
    )
}

/// Every piece of `mesh` that faces the viewer of `layout`'s view, projected.
fn facing_pieces(mesh: &SolidMesh, layout: &PanelLayout, style: &DiagramStyle) -> Vec<Piece> {
    let facet_normal = facet_normals(mesh);
    let mut pieces = Vec::new();
    for (ring_index, (facet_id, ring)) in mesh.rings.iter().enumerate() {
        let normal = mesh.piece_normals.as_ref().map_or_else(
            || facet_normal.get(*facet_id).copied().flatten(),
            |normals| normals.get(ring_index).copied(),
        );
        let Some(normal) = normal else {
            continue;
        };
        if !faces_viewer(layout.kind, normal) {
            continue;
        }
        if let Some(block) = style.tool_facet_block.get(*facet_id).copied().flatten()
            && !view_shows_block(layout.kind, block)
        {
            continue;
        }
        if ring.len() < 3 {
            continue;
        }
        let flags = mesh
            .edge_visible
            .as_ref()
            .and_then(|visible| visible.get(ring_index));
        let points = ring
            .iter()
            .map(|&p| {
                let (x, y, _) = project_point(p, layout);
                (x, y)
            })
            .collect();
        let seam = (0..ring.len())
            .map(|segment| {
                flags
                    .and_then(|flags| flags.get(segment))
                    .is_some_and(|&drawn| !drawn)
            })
            .collect();
        pieces.push(Piece {
            facet_id: *facet_id,
            points,
            seam,
        });
    }
    pieces
}

/// The projected corners of one view, merged within a tolerance: the corner id of a point is the
/// id of the first corner within `tolerance` of it on both axes.
struct Corners {
    tolerance: f32,
    points: Vec<(f32, f32)>,
    cells: HashMap<(i64, i64), Vec<usize>>,
}

impl Corners {
    fn new(tolerance: f32) -> Self {
        Self {
            tolerance: tolerance.max(1e-6),
            points: Vec::new(),
            cells: HashMap::new(),
        }
    }

    /// The grid cell (one tolerance wide) `point` falls in.
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a view's coordinates over its tolerance stay far inside the i64 range"
    )]
    fn cell(&self, point: (f32, f32)) -> (i64, i64) {
        (
            (point.0 / self.tolerance).floor() as i64,
            (point.1 / self.tolerance).floor() as i64,
        )
    }

    /// The id of the corner at `point`, a new one when no known corner is within the tolerance.
    fn id(&mut self, point: (f32, f32)) -> usize {
        let (cx, cy) = self.cell(point);
        for dx in -1..=1 {
            for dy in -1..=1 {
                let Some(ids) = self.cells.get(&(cx + dx, cy + dy)) else {
                    continue;
                };
                for &id in ids {
                    let known = self.points[id];
                    if (known.0 - point.0).abs() <= self.tolerance
                        && (known.1 - point.1).abs() <= self.tolerance
                    {
                        return id;
                    }
                }
            }
        }
        let id = self.points.len();
        self.points.push(point);
        self.cells.entry((cx, cy)).or_default().push(id);
        id
    }
}

/// One edge of the drawn pieces, after the copies of a shared edge are merged.
struct EdgeUse {
    /// The corner ids of its ends, in the order it was first met.
    ends: (usize, usize),
    /// How many drawn pieces have it.
    count: usize,
    /// Whether some copy is a real facet edge rather than a seam of a curved facet.
    drawn: bool,
}

/// The facet lines of `pieces`: each edge once, an edge of one drawn piece as the outline.
fn push_facet_lines(view: &mut VectorView, pieces: &[Piece]) {
    let mut corners = Corners::new(view.width.max(view.height) * CORNER_TOLERANCE);
    let mut edges: Vec<EdgeUse> = Vec::new();
    let mut lookup: HashMap<(usize, usize), usize> = HashMap::new();
    for piece in pieces {
        let ids: Vec<usize> = piece.points.iter().map(|&p| corners.id(p)).collect();
        for (segment, &from) in ids.iter().enumerate() {
            let to = ids[(segment + 1) % ids.len()];
            if from == to {
                continue;
            }
            let seam = piece.seam.get(segment).copied().unwrap_or(false);
            match lookup.entry((from.min(to), from.max(to))) {
                Entry::Occupied(slot) => {
                    let edge = &mut edges[*slot.get()];
                    edge.count += 1;
                    edge.drawn |= !seam;
                }
                Entry::Vacant(slot) => {
                    slot.insert(edges.len());
                    edges.push(EdgeUse {
                        ends: (from, to),
                        count: 1,
                        drawn: !seam,
                    });
                }
            }
        }
    }
    for edge in edges {
        let weight = if edge.count == 1 {
            LineWeight::Outline
        } else if edge.drawn {
            LineWeight::Edge
        } else {
            continue;
        };
        view.lines.push(VectorLine {
            from: corners.points[edge.ends.0],
            to: corners.points[edge.ends.1],
            weight,
        });
    }
}

/// Whether `point` lies inside the convex polygon `polygon` (either winding), boundary included.
fn inside_convex(polygon: &[(f32, f32)], point: (f32, f32)) -> bool {
    let mut sign = 0.0f32;
    for (i, &a) in polygon.iter().enumerate() {
        let b = polygon[(i + 1) % polygon.len()];
        let cross = (b.0 - a.0).mul_add(point.1 - a.1, -((b.1 - a.1) * (point.0 - a.0)));
        if cross.abs() <= 1e-6 {
            continue;
        }
        if sign == 0.0 {
            sign = cross.signum();
        } else if cross.signum() != sign {
            return false;
        }
    }
    true
}

/// Whether a label of `text` at font size `size`, centred on `centre`, fits inside `polygon`.
fn label_fits(polygon: &[(f32, f32)], centre: (f32, f32), text: &str, size: f32) -> bool {
    let half_width = LABEL_EM_PER_CHAR * size * text.chars().count() as f32 / 2.0;
    let half_height = LABEL_BOX_EMS * size / 2.0;
    [(-1.0f32, -1.0f32), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)]
        .iter()
        .all(|&(sx, sy)| {
            inside_convex(
                polygon,
                (
                    sx.mul_add(half_width, centre.0),
                    sy.mul_add(half_height, centre.1),
                ),
            )
        })
}

/// One label per drawn facet, at the centroid of its largest drawn piece, when it fits there.
fn push_facet_labels(view: &mut VectorView, pieces: &[Piece], style: &DiagramStyle) {
    // Largest piece per facet, by area; a `BTreeMap` keeps the labels in facet order.
    let mut largest: BTreeMap<usize, (f32, (f32, f32), usize)> = BTreeMap::new();
    for (index, piece) in pieces.iter().enumerate() {
        let projected: Vec<(f32, f32, f32)> =
            piece.points.iter().map(|&(x, y)| (x, y, 0.0)).collect();
        let (area, cx, cy) = screen_centroid(&projected);
        let entry = largest
            .entry(piece.facet_id)
            .or_insert((area, (cx, cy), index));
        if area > entry.0 {
            *entry = (area, (cx, cy), index);
        }
    }
    let full = view.width.min(view.height) * LABEL_SIZE;
    for (facet_id, (_, centre, index)) in largest {
        let Some(text) = style.facet_labels.get(facet_id) else {
            continue;
        };
        if text.trim().is_empty() {
            continue;
        }
        let polygon = &pieces[index].points;
        let Some(size) = [full, full * LABEL_SHRINK]
            .into_iter()
            .find(|&size| label_fits(polygon, centre, text, size))
        else {
            continue;
        };
        view.labels.push(VectorLabel {
            text: text.clone(),
            x: centre.0,
            y: centre.1,
            size,
        });
    }
}

/// The index wheel of a crown or pavilion view, as the raster draws it (`legend.rs`): a tick per
/// tooth (every k-th past the raster's cap), a longer tick every 8th (every 4th up to 64 teeth)
/// with its tooth number, tooth 0 written as the tooth count.
fn push_index_wheel(view: &mut VectorView, layout: &PanelLayout, wheel: Wheel) {
    let mirrored = matches!(layout.kind, PanelKind::Pavilion);
    let major_every = if wheel.teeth <= 64 { 4 } else { 8 };
    let radius = layout.wheel_radius_px;
    let inner = radius * 0.94;
    let label_radius = radius * 1.2;
    let stride = tick_stride(wheel.teeth);
    let size = view.width.min(view.height) * TOOTH_SIZE;
    for tooth in (0..wheel.teeth).step_by(stride as usize) {
        let phi =
            2.0 * std::f32::consts::PI * (tooth as f32 + wheel.reference) / wheel.teeth as f32;
        let (right, up) = wheel_direction(phi, mirrored);
        let major = (tooth / stride).is_multiple_of(major_every);
        let outer = if major { radius * 1.08 } else { radius };
        let at = |r: f32| {
            (
                right.mul_add(r, layout.center_x),
                up.mul_add(-r, layout.center_y),
            )
        };
        view.lines.push(VectorLine {
            from: at(inner),
            to: at(outer),
            weight: LineWeight::Tick,
        });
        if major {
            let text = if tooth == 0 {
                wheel.teeth.to_string()
            } else {
                tooth.to_string()
            };
            let (x, y) = at(label_radius);
            // Kept inside the view, as the raster keeps its numbers inside the panel.
            let half_width = LABEL_EM_PER_CHAR * size * text.chars().count() as f32 / 2.0;
            let half_height = LABEL_BOX_EMS * size / 2.0;
            view.labels.push(VectorLabel {
                x: x.min(view.width - half_width).max(half_width),
                y: y.min(view.height - half_height).max(half_height),
                text,
                size,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma::geometry::stone_metrics::{SolidStatus, build_solid_mesh};

    /// A regular octahedron: four faces up, four down, every vertex on an axis.
    fn octahedron() -> SolidMesh {
        let k = 1.0 / 3.0f64.sqrt();
        let mut planes: Vec<(DVec3, f64)> = Vec::new();
        for x in [-1.0, 1.0] {
            for y in [-1.0, 1.0] {
                for z in [-1.0, 1.0] {
                    planes.push((DVec3::new(x, y, z) * k, k));
                }
            }
        }
        match build_solid_mesh(&planes) {
            SolidStatus::Closed(mesh) => mesh,
            other => panic!("the octahedron closes: {other:?}"),
        }
    }

    fn config() -> DiagramConfig {
        DiagramConfig {
            width: 560,
            height: 600,
            gear_teeth: 96,
            gear_reference_angle: 0.0,
            symmetry_order: 4,
            mirror: true,
        }
    }

    fn count(view: &VectorView, weight: LineWeight) -> usize {
        view.lines
            .iter()
            .filter(|line| line.weight == weight)
            .count()
    }

    /// From above, the octahedron is a square outline (the four equator edges) split by four
    /// ridges into the four upper faces; the wheel adds one tick per tooth.
    #[test]
    fn the_crown_view_of_an_octahedron_is_a_square_with_four_ridges() {
        let style = DiagramStyle {
            facet_labels: (0..8).map(|i| format!("F{i}")).collect(),
            ..DiagramStyle::default()
        };
        let [crown, profile, end, pavilion] =
            render_vector_views(&octahedron(), &config(), &style).expect("views");
        assert_eq!(crown.caption, "CROWN");
        assert_eq!(profile.caption, "PROFILE");
        assert_eq!(end.caption, "END");
        assert_eq!(pavilion.caption, "PAVILION");
        for view in [&crown, &pavilion] {
            assert_eq!(count(view, LineWeight::Outline), 4, "{}", view.caption);
            assert_eq!(count(view, LineWeight::Edge), 4, "{}", view.caption);
            assert_eq!(count(view, LineWeight::Tick), 96, "{}", view.caption);
            // Twelve tooth numbers on a 96 gear, the top one written as 96.
            assert!(view.labels.iter().any(|label| label.text == "96"));
            assert!(!view.labels.iter().any(|label| label.text == "0"));
        }
        // From the side the four faces toward +Z meet at the front vertex.
        for view in [&profile, &end] {
            assert_eq!(count(view, LineWeight::Outline), 4, "{}", view.caption);
            assert_eq!(count(view, LineWeight::Edge), 4, "{}", view.caption);
            assert_eq!(count(view, LineWeight::Tick), 0, "{}", view.caption);
        }
        // Each upper face is labelled once on the crown view.
        let facet_labels = crown
            .labels
            .iter()
            .filter(|label| label.text.starts_with('F'))
            .count();
        assert_eq!(facet_labels, 4);
    }

    /// The SVG carries one path per weight, the labels as text, and nothing that changes between
    /// two runs.
    #[test]
    fn the_svg_draws_every_line_and_label() {
        let style = DiagramStyle {
            facet_labels: (0..8).map(|i| format!("F{i}")).collect(),
            ..DiagramStyle::default()
        };
        let views = render_vector_views(&octahedron(), &config(), &style).expect("views");
        let svg = view_to_svg(&views[0]);
        assert!(svg.starts_with("<svg xmlns=\"http://www.w3.org/2000/svg\""));
        assert!(svg.ends_with("</svg>"));
        assert_eq!(svg.matches("<path ").count(), 3, "tick, edge and outline");
        // Every line is one `M..L..` pair, and no text of this view has a capital L.
        assert_eq!(svg.matches('L').count(), views[0].lines.len());
        assert!(svg.contains(">CROWN</text>"));
        assert!(svg.contains(">F"));
        assert_eq!(svg, view_to_svg(&views[0]));
    }

    /// A zero-sized frame or an empty mesh gives no views.
    #[test]
    fn nothing_to_draw_gives_no_views() {
        let mut zero = config();
        zero.width = 0;
        assert!(render_vector_views(&octahedron(), &zero, &DiagramStyle::default()).is_none());
    }
}
