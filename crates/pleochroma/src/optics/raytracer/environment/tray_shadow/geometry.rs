//! Bounds and intersections for the shadow's source stone.
use super::TrayShadowOff;
use crate::{
    geometry::tool::StoneGeometry,
    optics::raytracer::{
        camera::Ray,
        environment::tray_plane::{solid_frame, solid_vertices},
        intersect::build_plane_soa,
        intersect_stone::{intersect_stone_soa, intersect_stone_soa_side},
    },
    simd::PlanesSoA32,
};
use glam::Vec3;

/// Tray placement and conservative bounding sphere around the stone.
pub(super) struct StoneFrame {
    /// Height of the receiving plane.
    pub(super) plane_y: f32,
    /// Horizontal centre of the map.
    pub(super) center: [f32; 2],
    /// Half-width of the square map.
    pub(super) half_extent: f32,
    /// Centre of the conservative emission sphere.
    pub(super) sphere_center: Vec3,
    /// Radius of the conservative emission sphere.
    pub(super) sphere_radius: f32,
    /// Offset used for rays launched from the tray or a surface.
    pub(super) lift: f32,
}
impl StoneFrame {
    /// Builds conservative bounds from the vertices of the true solid.
    pub(super) fn new(geom: StoneGeometry<'_>) -> Result<Self, TrayShadowOff> {
        let vertices = solid_vertices(geom).ok_or(TrayShadowOff::DegenerateStone)?;
        // The bounds and the plane come from the helper the Tray Reflection shares, so the
        // two planes cannot drift apart.
        let frame = solid_frame(&vertices).ok_or(TrayShadowOff::DegenerateStone)?;
        Ok(Self {
            plane_y: frame.plane_y,
            center: frame.center,
            half_extent: 3.0 * frame.radius_xz,
            sphere_center: frame.sphere_center,
            sphere_radius: frame.sphere_radius,
            lift: 1e-4 * frame.sphere_radius,
        })
    }
    /// World-space centre of a square map texel.
    pub(super) fn texel_center(&self, (i, j): (u32, u32), n: u32) -> Vec3 {
        let axis = |index: u32, center: f32| {
            let offset = -1.0 + 2.0_f32.mul_add(index as f32, 1.0) / n as f32;
            self.half_extent.mul_add(offset, center)
        };
        Vec3::new(
            axis(i, self.center[0]),
            self.plane_y,
            axis(j, self.center[1]),
        )
    }
    /// Whether the forward ray intersects the conservative sphere.
    pub(super) fn hits_sphere(&self, p: Vec3, d: Vec3) -> bool {
        let offset = p - self.sphere_center;
        let along = offset.dot(d);
        let constant = (-self.sphere_radius).mul_add(self.sphere_radius, offset.length_squared());
        let discriminant = along.mul_add(along, -constant);
        discriminant >= 0.0 && -along + discriminant.sqrt() > 0.0
    }
    /// Landing coordinates of a downward ray that starts above the tray.
    pub(super) fn plane_hit(&self, p: Vec3, d: Vec3) -> Option<[f32; 2]> {
        if d.y >= 0.0 || p.y <= self.plane_y {
            return None;
        }
        let t = (self.plane_y - p.y) / d.y;
        Some([t.mul_add(d.x, p.x), t.mul_add(d.z, p.z)])
    }
}

/// Stone geometry and its acceleration structure shared by gather and photons.
pub(super) struct StoneHit<'a> {
    geom: StoneGeometry<'a>,
    soa: PlanesSoA32,
}
impl<'a> StoneHit<'a> {
    /// Builds the shared plane acceleration structure.
    pub(super) fn new(geom: StoneGeometry<'a>) -> Self {
        Self {
            soa: build_plane_soa(geom.planes),
            geom,
        }
    }
    /// First boundary distance, point and outward normal along a ray.
    pub(super) fn first_hit(&self, p: Vec3, d: Vec3) -> Option<(f32, Vec3, Vec3)> {
        let hit = intersect_stone_soa(
            Ray { origin: p, dir: d },
            &self.soa,
            self.geom.planes.len(),
            self.geom.tools,
        )?;
        Some((hit.t, p + d * hit.t, hit.normal))
    }
    /// Next boundary distance, point, outward normal and entry flag for a ray believed inside.
    ///
    /// The flag is true when the boundary is an entry: a thin gap put a previous exit inside the
    /// intersection's `1e-4` self-hit floor, so the stretch was air and the caller must adopt
    /// that state as the path tracer does.
    pub(super) fn next_hit_inside(&self, p: Vec3, d: Vec3) -> Option<(f32, Vec3, Vec3, bool)> {
        let (hit, entering) = intersect_stone_soa_side(
            Ray { origin: p, dir: d },
            &self.soa,
            self.geom.planes.len(),
            self.geom.tools,
        )?;
        Some((hit.t, p + d * hit.t, hit.normal, entering == Some(true)))
    }
    /// Whether the ray meets any stone boundary.
    pub(super) fn hits(&self, p: Vec3, d: Vec3) -> bool {
        self.first_hit(p, d).is_some()
    }
}
