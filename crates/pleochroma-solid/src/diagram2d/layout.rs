//! Panel visibility predicates, the crown/pavilion/profile orthographic
//! projections, and the fixed pixel layout ([`PanelLayout`]) each panel draws
//! into -- see the parent module's doc comment for the panel conventions this
//! implements.

use super::{NORMAL_EPS, PanelKind, fill::to_px};
use glam::{DVec3, Vec3};
use pleochroma::geometry::stone_metrics::SolidMesh;

/// A world-space `f64` narrowed to the `f32` screen/GPU precision.
#[expect(
    clippy::cast_possible_truncation,
    reason = "f32 is the diagram's screen and GPU precision; world coordinates are in the normal float range"
)]
pub const fn narrow(value: f64) -> f32 {
    value as f32
}

/// A facet is drawn top-facing (crown) when its normal points enough toward `+Y`
/// to not be considered edge-on. See the parent module's doc comment.
#[must_use]
pub fn crown_visible(normal: Vec3) -> bool {
    normal.y > NORMAL_EPS
}

/// The pavilion panel's mirror of [`crown_visible`].
#[must_use]
pub fn pavilion_visible(normal: Vec3) -> bool {
    normal.y < -NORMAL_EPS
}

/// A facet is in the profile (side elevation) panel when its normal has any horizontal component,
/// i.e. it is not purely crown or pavilion facing.
///
/// A single backface-culled view direction would show only the one facet facing the camera; see the
/// parent module.
#[must_use]
pub fn profile_visible(normal: Vec3) -> bool {
    normal.x.hypot(normal.z) > NORMAL_EPS
}

/// One panel's fixed layout in pixel space, computed once per [`super::render_diagram`]
/// call from the mesh's own extent so the drawing always fits regardless of the
/// design's physical scale.
///
/// Public through [`super::DiagramLayout`], so an embedding app can place hit tests
/// and drag handles on the very pixels the panel was drawn into.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PanelLayout {
    /// Which panel this layout describes.
    pub kind: PanelKind,
    /// Horizontal panel centre, in pixels.
    pub center_x: f32,
    /// Vertical panel centre, in pixels.
    pub center_y: f32,
    /// World-units -> pixels scale factor for this panel.
    pub scale: f32,
    /// Radius (px) of the index-wheel tick ring. The profile panel draws no ring.
    pub wheel_radius_px: f32,
    /// Clip rectangle (inclusive) this panel is allowed to paint into, so a
    /// panel's fill/edges/labels never bleed into a neighboring column.
    pub clip: (i32, i32, i32, i32),
}

/// Converts a tooth index's azimuth into a unit screen-space direction
/// `(screen_right, screen_up)` for the index wheel and for the crown/pavilion
/// facet-body projections below -- see the parent module's doc comment for the
/// derivation. `mirrored` is `true` for the pavilion panel.
pub fn wheel_direction(phi: f32, mirrored: bool) -> (f32, f32) {
    let (sin_phi, cos_phi) = phi.sin_cos();
    let world_x = cos_phi;
    let world_z = sin_phi;
    let screen_right = if mirrored { -world_z } else { world_z };
    let screen_up = world_x;
    (screen_right, screen_up)
}

/// Projects a world point to `(screen_x, screen_y, depth)` for the crown panel
/// (looking down `-Y`). `depth` is smaller for a point nearer the camera (larger
/// `y`), matching `raster.rs`'s "smaller depth wins" convention.
pub const fn crown_project(p: DVec3, layout: &PanelLayout) -> (f32, f32, f32) {
    let (world_x, world_y, world_z) = (narrow(p.x), narrow(p.y), narrow(p.z));
    let screen_x = layout.center_x + world_z * layout.scale;
    let screen_y = layout.center_y - world_x * layout.scale;
    (screen_x, screen_y, -world_y)
}

/// The pavilion panel's mirror of [`crown_project`] (looking up `+Y`; see the
/// parent module's doc comment for why the horizontal axis flips).
pub const fn pavilion_project(p: DVec3, layout: &PanelLayout) -> (f32, f32, f32) {
    let (world_x, world_y, world_z) = (narrow(p.x), narrow(p.y), narrow(p.z));
    let screen_x = layout.center_x - world_z * layout.scale;
    let screen_y = layout.center_y - world_x * layout.scale;
    (screen_x, screen_y, world_y)
}

/// The profile (side elevation) panel: `world_x` horizontal, `world_y` vertical, `world_z` as
/// depth (near wins). Facets reach it through [`profile_visible`].
pub const fn profile_project(p: DVec3, layout: &PanelLayout) -> (f32, f32, f32) {
    let (world_x, world_y, world_z) = (narrow(p.x), narrow(p.y), narrow(p.z));
    let screen_x = layout.center_x + world_x * layout.scale;
    let screen_y = layout.center_y - world_y * layout.scale;
    (screen_x, screen_y, -world_z)
}

/// Projects a world point with the projection of `layout.kind`: `(screen_x,
/// screen_y, depth)` in the frame's pixel space, exactly as the panel drew it.
#[must_use]
pub const fn project_point(p: DVec3, layout: &PanelLayout) -> (f32, f32, f32) {
    match layout.kind {
        PanelKind::Crown => crown_project(p, layout),
        PanelKind::Pavilion => pavilion_project(p, layout),
        PanelKind::Profile => profile_project(p, layout),
    }
}

/// One flat normal per facet, indexed by `facet_id`. Facets absent from the mesh stay `None`.
pub fn facet_normals(mesh: &SolidMesh) -> Vec<Option<DVec3>> {
    let facet_count = mesh.rings.iter().map(|(id, _)| id + 1).max().unwrap_or(0);
    let mut facet_normal: Vec<Option<DVec3>> = vec![None; facet_count];
    for (&id, &normal) in mesh.facet_id.iter().zip(&mesh.normals) {
        if let Some(slot) = facet_normal.get_mut(id)
            && slot.is_none()
        {
            *slot = Some(normal);
        }
    }
    facet_normal
}

/// A mesh's `(xz_radius, profile_radius, profile_reach)`: the world radii the panels are
/// scaled against (see [`build_panel_layout`]).
///
/// Shared by [`compute_layout`] and [`compute_single_panel_layout`]. `profile_radius` is half
/// the larger bounding-box side. `profile_reach` is the farthest the stone reaches from the
/// profile's centre (world `x = 0`, `y = 0`, the girdle plane), where [`profile_project`] puts
/// that centre. A deep pavilion or crown can reach past `profile_radius`, so large frames scale
/// the profile by the reach, which keeps the picture above its caption.
fn mesh_radii(mesh: &SolidMesh) -> (f32, f32, f32) {
    let (mut min_x, mut max_x, mut min_y, mut max_y) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    let mut xz_radius = 1e-6f64;
    for p in &mesh.positions {
        min_x = min_x.min(p.x);
        max_x = max_x.max(p.x);
        min_y = min_y.min(p.y);
        max_y = max_y.max(p.y);
        xz_radius = xz_radius.max(p.x.hypot(p.z));
    }
    let half_w = ((max_x - min_x) / 2.0).max(1e-6);
    let half_h = ((max_y - min_y) / 2.0).max(1e-6);
    // The extents start at the origin, so each side is already measured from it.
    let reach = (-min_x).max(max_x).max(-min_y).max(max_y).max(1e-6);
    (
        narrow(xz_radius),
        narrow(half_w.max(half_h)),
        narrow(half_w.max(half_h).max(reach)),
    )
}

/// The world radius the profile panel is scaled against in a frame `height` pixels tall: the
/// half-extent in a small frame, so small frames render as before, and the reach from the
/// panel's centre in a [`LARGE_FRAME_MIN_HEIGHT`] frame.
const fn profile_world_radius(radii: (f32, f32, f32), height: u32) -> f32 {
    if height >= LARGE_FRAME_MIN_HEIGHT {
        radii.2
    } else {
        radii.1
    }
}

/// The column/row geometry every [`PanelLayout`] in a frame shares --
/// [`grid_metrics`] computes it once for `columns` equal-width columns spanning
/// the whole frame (`3` for [`compute_layout`], `1` for
/// [`compute_single_panel_layout`]).
struct GridMetrics {
    col_width: f32,
    panel_box: f32,
    center_y: f32,
    /// The index-wheel tick ring's radius as a fraction of half the panel box.
    wheel_frac: f32,
}

/// A frame at least this tall is a viewport or sheet picture, not a thumbnail.
///
/// Its captions and tooth numbers are scaled up ([`text_scale`]), the captions sit under their
/// panel ([`caption_y`]) and the tick ring is smaller so the numbers stay inside the box. Shorter
/// frames keep the existing layout, so thumbnails keep their output.
pub const LARGE_FRAME_MIN_HEIGHT: u32 = 300;

/// The tick ring's radius fraction in a [`LARGE_FRAME_MIN_HEIGHT`] frame: the tooth numbers
/// (`1.2` times the ring) then end inside the panel box, so neighbouring columns' numbers
/// do not run together ("24" + "24") and none is clipped by the frame edge.
const LARGE_WHEEL_FRAC: f32 = 0.74;

/// The tick ring's radius fraction in a frame shorter than [`LARGE_FRAME_MIN_HEIGHT`].
const SMALL_WHEEL_FRAC: f32 = 0.86;

/// Gap, in pixels, around a [`LARGE_FRAME_MIN_HEIGHT`] frame's captions: between the frame
/// edge and the top of the panels, and between a panel and its caption.
const CAPTION_GAP: f32 = 8.0;

/// The pixel multiple the panel captions and tooth numbers are drawn at in a frame `height`
/// pixels tall: `1` below 800 (the glyphs are 5x7 pixels), `2` from 800.
#[must_use]
pub fn text_scale(height: u32) -> u32 {
    if height < LARGE_FRAME_MIN_HEIGHT {
        1
    } else {
        (height / 400).clamp(1, 2)
    }
}

/// The caption's top edge in a frame `frame_height` pixels tall.
///
/// In a small frame it is [`SMALL_CAPTION_Y`]; in a large one it sits under the panel's picture,
/// but never closer than [`CAPTION_GAP`] to the bottom edge.
///
/// The tooth numbers end `1.2 * wheel_radius_px` from the centre. The bottom edge is clear of
/// the viewport border and toolbar, which overlay the top edge.
pub fn caption_y(layout: &PanelLayout, frame_height: u32) -> f32 {
    if frame_height < LARGE_FRAME_MIN_HEIGHT {
        return SMALL_CAPTION_Y;
    }
    let (_, text_h) = super::labels::text_size("A", text_scale(frame_height));
    let panel_half = layout.wheel_radius_px / LARGE_WHEEL_FRAC;
    let under_panel = layout.center_y + panel_half + CAPTION_GAP;
    let lowest = frame_height as f32 - CAPTION_GAP - text_h as f32;
    under_panel.min(lowest).max(0.0)
}

/// The caption's top edge in a frame shorter than [`LARGE_FRAME_MIN_HEIGHT`].
const SMALL_CAPTION_Y: f32 = 2.0;

fn grid_metrics(width: u32, height: u32, columns: u32) -> GridMetrics {
    let (width_f, height_f) = (width as f32, height as f32);
    let col_width = width_f / columns.max(1) as f32;
    if height >= LARGE_FRAME_MIN_HEIGHT {
        // Room for one caption row under the panels, and the same gap above them.
        let (_, text_h) = super::labels::text_size("A", text_scale(height));
        let top = CAPTION_GAP;
        let bottom = 2.0f32.mul_add(CAPTION_GAP, text_h as f32);
        let avail_height = (height_f - top - bottom).max(1.0);
        return GridMetrics {
            col_width,
            panel_box: col_width.min(avail_height),
            center_y: top + avail_height / 2.0,
            wheel_frac: LARGE_WHEEL_FRAC,
        };
    }
    let caption_height = (height_f * 0.06).clamp(10.0, 20.0);
    let avail_height = (height_f - caption_height).max(1.0);
    GridMetrics {
        col_width,
        panel_box: col_width.min(avail_height),
        center_y: caption_height + avail_height / 2.0,
        wheel_frac: SMALL_WHEEL_FRAC,
    }
}

/// Builds one panel's [`PanelLayout`] at column `index` within `metrics`' grid.
fn build_panel_layout(
    metrics: &GridMetrics,
    index: usize,
    kind: PanelKind,
    world_radius: f32,
    body_frac: f32,
    height: u32,
) -> PanelLayout {
    let center_x = (index as f32).mul_add(metrics.col_width, metrics.col_width / 2.0);
    let wheel_radius_px = metrics.panel_box / 2.0 * metrics.wheel_frac;
    let body_radius_px = metrics.panel_box / 2.0 * body_frac;
    let scale = body_radius_px / world_radius.max(1e-6);
    let clip = (
        to_px((index as f32 * metrics.col_width).floor()),
        0,
        to_px(((index as f32 + 1.0) * metrics.col_width).ceil()) - 1,
        height.cast_signed() - 1,
    );
    PanelLayout {
        kind,
        center_x,
        center_y: metrics.center_y,
        scale,
        wheel_radius_px,
        clip,
    }
}

/// Computes the three panels' pixel layout from the mesh's own extent. An empty mesh gets the
/// minimum radius, so a caller needs no special case for "nothing to draw yet".
pub fn compute_layout(mesh: &SolidMesh, width: u32, height: u32) -> [PanelLayout; 3] {
    let radii = mesh_radii(mesh);
    let xz_radius = radii.0;
    let metrics = grid_metrics(width, height, 3);
    [
        build_panel_layout(&metrics, 0, PanelKind::Crown, xz_radius, 0.62, height),
        build_panel_layout(&metrics, 1, PanelKind::Pavilion, xz_radius, 0.62, height),
        build_panel_layout(
            &metrics,
            2,
            PanelKind::Profile,
            profile_world_radius(radii, height),
            0.85,
            height,
        ),
    ]
}

/// The single-panel counterpart to [`compute_layout`]: one column spanning the whole frame width,
/// for [`super::render_diagram_single_panel`].
///
/// It is computed afresh rather than cropped from [`compute_layout`]: a crop would enlarge the
/// pixels but not the `scale`, so the facet body would stay as small as in the three-column layout.
pub fn compute_single_panel_layout(
    mesh: &SolidMesh,
    width: u32,
    height: u32,
    kind: PanelKind,
) -> PanelLayout {
    let radii = mesh_radii(mesh);
    let (world_radius, body_frac) = match kind {
        PanelKind::Crown | PanelKind::Pavilion => (radii.0, 0.62),
        PanelKind::Profile => (profile_world_radius(radii, height), 0.85),
    };
    let metrics = grid_metrics(width, height, 1);
    build_panel_layout(&metrics, 0, kind, world_radius, body_frac, height)
}
