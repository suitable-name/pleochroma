//! Rasterisation, the half-open pixel rule and the GPU leaf packing.

use glam::{DVec2, Vec3};

use super::{
    beam::{
        BranchPrefix, GPU_LEAF_MAX_VERTICES, LEAF_FLAG_DELTAS, LEAF_FLAG_GLARE,
        LEAF_FLAG_REFLECTION, Leaf, SpectralInterval,
    },
    geom::polygon::ConvexPolygon,
    leaf::{exit_direction, exit_weight, pack_leaves, pack_leaves_counted},
    primary::TileRect,
    raster::{Axis, clip_interval, rasterise, rasterise_polygon},
    stats::BeamStats,
};
use crate::renderer::path_cache::{record::f16_bits_to_f32, signature::BRANCH_SIG_SEED};

/// The relative tolerance of the fragment areas against the polygon area (guide §13.6 asks 1e-9).
const AREA_RELATIVE_TOLERANCE: f64 = 1.0e-12;

/// The relative tolerance of `f16` weights (half precision keeps 11 bits).
const F16_RELATIVE_TOLERANCE: f32 = 1.0e-3;

/// The absolute tolerance of `f16` direction deltas of magnitude below 1.
const F16_DELTA_TOLERANCE: f32 = 2.0e-4;

/// The tile of the random polygon tests: 32 by 32 pixels.
const TILE: TileRect = TileRect {
    x0: 0,
    y0: 0,
    x1: 32,
    y1: 32,
};

/// A small deterministic generator (64-bit LCG, high bits) for the random polygons.
struct Lcg(u64);

impl Lcg {
    /// The next value in `[0, 1)`.
    fn next(&mut self) -> f64 {
        self.next_bits() as f64 / (1_u64 << 53) as f64
    }

    /// The next 53 random bits.
    fn next_bits(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 11
    }

    /// The next integer in `[0, n)`.
    fn below(&mut self, n: u64) -> usize {
        let scaled = (u128::from(self.next_bits()) * u128::from(n)) >> 53;
        usize::try_from(scaled).expect("an index below n")
    }

    /// The next value in `[lo, hi)`.
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        (hi - lo).mul_add(self.next(), lo)
    }
}

/// A random convex polygon with 3 to 12 vertices inscribed in an ellipse inside `[3, 29]^2`.
fn random_polygon(rng: &mut Lcg) -> ConvexPolygon {
    let centre = DVec2::new(rng.range(9.0, 23.0), rng.range(9.0, 23.0));
    let (semi_major, semi_minor) = (rng.range(2.0, 6.0), rng.range(2.0, 6.0));
    let turn = rng.range(0.0, std::f64::consts::TAU);
    let count = 3 + rng.below(10);
    let points: Vec<DVec2> = (0..count)
        .map(|i| {
            let angle =
                std::f64::consts::TAU * 0.8_f64.mul_add(rng.next(), i as f64) / count as f64;
            let (sin_a, cos_a) = angle.sin_cos();
            let (ex, ey) = (semi_major * cos_a, semi_minor * sin_a);
            let (st, ct) = turn.sin_cos();
            centre + DVec2::new(ey.mul_add(-st, ex * ct), ey.mul_add(ct, ex * st))
        })
        .collect();
    ConvexPolygon::from_image_points(&points).expect("an inscribed polygon is convex")
}

/// A leaf over `image` with `nodes` nodes, constant directions and unit weights.
fn flat_leaf(image: &ConvexPolygon, nodes: u16) -> Leaf {
    let count = image.len() * usize::from(nodes);
    Leaf {
        tile: 0,
        image: *image,
        dirs: vec![Vec3::Y; count],
        weights: vec![1.0; count],
        spectrum: SpectralInterval {
            first: 0,
            last: nodes - 1,
        },
        sig: BRANCH_SIG_SEED,
        prefix: BranchPrefix::EMPTY,
        glare: true,
        reflection: true,
        tau_omega: 1.0e-4,
        pre_clip: None,
    }
}

/// The polygon with the corners `(x0, y0)` to `(x1, y1)` in image space.
fn rectangle(x0: f64, y0: f64, x1: f64, y1: f64) -> ConvexPolygon {
    ConvexPolygon::from_image_points(&[
        DVec2::new(x0, y0),
        DVec2::new(x1, y0),
        DVec2::new(x1, y1),
        DVec2::new(x0, y1),
    ])
    .expect("a rectangle")
}

/// The sum of the fragment areas.
fn total_area(leaf: &Leaf, tile: TileRect) -> f64 {
    rasterise(leaf, tile)
        .iter()
        .map(|fragment| fragment.polygon.area())
        .sum()
}

#[test]
fn the_fragments_of_a_leaf_tile_it_exactly() {
    let mut rng = Lcg(0x5EED_0001);
    for case in 0..300 {
        let polygon = random_polygon(&mut rng);
        let leaf = flat_leaf(&polygon, 8);
        let fragments = rasterise(&leaf, TILE);
        let sum: f64 = fragments.iter().map(|f| f.polygon.area()).sum();
        let area = polygon.area();
        assert!(
            (sum - area).abs() <= AREA_RELATIVE_TOLERANCE * area,
            "case {case}: fragments {sum} against polygon {area}"
        );
        // Row-major, every pixel once, every piece inside its cell.
        let pixels: Vec<(u32, u32)> = fragments.iter().map(|f| f.pixel).collect();
        let mut sorted = pixels.clone();
        sorted.sort_by_key(|&(x, y)| (y, x));
        sorted.dedup();
        assert_eq!(pixels, sorted, "case {case}: pixels not unique or ordered");
        for fragment in &fragments {
            let (x, y) = (f64::from(fragment.pixel.0), f64::from(fragment.pixel.1));
            for v in fragment.polygon.vertices() {
                assert!(
                    v.x >= x - 0.5 - 1e-9
                        && v.x <= x + 0.5 + 1e-9
                        && v.y >= y - 0.5 - 1e-9
                        && v.y <= y + 0.5 + 1e-9,
                    "case {case}: vertex {v:?} outside pixel {:?}",
                    fragment.pixel
                );
            }
        }
    }
}

#[test]
fn tiles_split_a_leaf_without_loss_or_overlap() {
    let mut rng = Lcg(0x5EED_0002);
    for case in 0..100 {
        let leaf = flat_leaf(&random_polygon(&mut rng), 8);
        let whole = total_area(&leaf, TILE);
        let mut parts = 0.0;
        for (x0, y0) in [(0, 0), (16, 0), (0, 16), (16, 16)] {
            parts += total_area(
                &leaf,
                TileRect {
                    x0,
                    y0,
                    x1: x0 + 16,
                    y1: y0 + 16,
                },
            );
        }
        assert!(
            (parts - whole).abs() <= AREA_RELATIVE_TOLERANCE * whole,
            "case {case}: the four tiles hold {parts}, the whole {whole}"
        );
    }
}

#[test]
fn a_polygon_that_is_exactly_one_pixel_cell_gives_one_fragment() {
    // Pixel (3, 5) is [2.5, 3.5) x [4.5, 5.5).
    let leaf = flat_leaf(&rectangle(2.5, 4.5, 3.5, 5.5), 8);
    let fragments = rasterise(&leaf, TILE);
    assert_eq!(fragments.len(), 1);
    assert_eq!(fragments[0].pixel, (3, 5));
    assert_eq!(fragments[0].polygon.area(), 1.0);
}

#[test]
fn an_edge_on_a_pixel_boundary_is_counted_once() {
    // x from -0.5 to 2.5 covers pixels 0, 1, 2; y from 0.5 to 1.5 is exactly row 1. Both y edges
    // lie on pixel boundaries: rows 0 and 2 get nothing, not even a zero-area fragment.
    let leaf = flat_leaf(&rectangle(-0.5, 0.5, 2.5, 1.5), 8);
    let fragments = rasterise(&leaf, TILE);
    let pixels: Vec<(u32, u32)> = fragments.iter().map(|f| f.pixel).collect();
    assert_eq!(pixels, vec![(0, 1), (1, 1), (2, 1)]);
    for fragment in &fragments {
        assert_eq!(fragment.polygon.area(), 1.0);
    }
    // Two polygons that share an edge on a boundary tile their union with no double count.
    let left = flat_leaf(&rectangle(0.5, 0.5, 1.5, 3.5), 8);
    let right = flat_leaf(&rectangle(1.5, 0.5, 2.5, 3.5), 8);
    let pixels_of = |leaf: &Leaf| -> Vec<(u32, u32)> {
        rasterise(leaf, TILE).iter().map(|f| f.pixel).collect()
    };
    let (left_pixels, right_pixels) = (pixels_of(&left), pixels_of(&right));
    assert_eq!(left_pixels, vec![(1, 1), (1, 2), (1, 3)]);
    assert_eq!(right_pixels, vec![(2, 1), (2, 2), (2, 3)]);
}

#[test]
fn a_triangle_on_the_grid_gets_the_exact_pixel_mix() {
    // The right triangle (-0.5,-0.5), (1.5,-0.5), (-0.5,1.5) has area 2. Pixel (0,0) lies fully
    // inside (area 1). Pixels (1,0) and (0,1) are cut by the hypotenuse x + y = 1 into pieces of
    // area 1/2 each. Pixel (1,1) lies beyond the hypotenuse.
    let triangle = ConvexPolygon::from_image_points(&[
        DVec2::new(-0.5, -0.5),
        DVec2::new(1.5, -0.5),
        DVec2::new(-0.5, 1.5),
    ])
    .expect("a triangle");
    let leaf = flat_leaf(&triangle, 8);
    let fragments = rasterise(&leaf, TILE);
    let area_of = |pixel: (u32, u32)| -> f64 {
        fragments
            .iter()
            .filter(|f| f.pixel == pixel)
            .map(|f| f.polygon.area())
            .sum()
    };
    let tolerance = 1.0e-12;
    assert!((area_of((0, 0)) - 1.0).abs() < tolerance);
    assert!((area_of((1, 0)) - 0.5).abs() < tolerance);
    assert!((area_of((0, 1)) - 0.5).abs() < tolerance);
    assert_eq!(area_of((1, 1)), 0.0);
    assert!((total_area(&leaf, TILE) - 2.0).abs() < tolerance);
}

#[test]
fn clip_interval_follows_the_half_open_rule() {
    let square = [
        DVec2::new(0.0, 0.0),
        DVec2::new(1.0, 0.0),
        DVec2::new(1.0, 1.0),
        DVec2::new(0.0, 1.0),
    ];
    // [0, 1) keeps the whole square (its upper edge x = 1 is the cell's excluded edge, but the
    // area below it is inside).
    let whole = clip_interval(&square, Axis::X, 0.0, 1.0);
    assert_eq!(whole.len(), 4);
    // [1, 2) holds only the edge x = 1: no area.
    assert!(clip_interval(&square, Axis::X, 1.0, 2.0).len() < 3);
    // [-1, 0) holds only the edge x = 0: no area.
    assert!(clip_interval(&square, Axis::X, -1.0, 0.0).len() < 3);
    // [0.25, 0.75) cuts a vertical band of width 1/2, with its crossings exactly on the lines.
    let band = clip_interval(&square, Axis::X, 0.25, 0.75);
    assert_eq!(band.len(), 4);
    assert!(band.iter().all(|p| p.x == 0.25 || p.x == 0.75));
    // The same along y.
    let upper = clip_interval(&square, Axis::Y, 0.5, 5.0);
    assert_eq!(upper.len(), 4);
    assert!(upper.iter().all(|p| p.y >= 0.5));
}

#[test]
fn rasterise_polygon_matches_rasterise() {
    let mut rng = Lcg(0x5EED_0003);
    let polygon = random_polygon(&mut rng);
    let leaf = flat_leaf(&polygon, 8);
    let direct = rasterise(&leaf, TILE);
    let pieces = rasterise_polygon(&polygon, TILE);
    assert_eq!(direct.len(), pieces.len());
    for (fragment, (pixel, piece)) in direct.iter().zip(&pieces) {
        assert_eq!(fragment.pixel, *pixel);
        assert_eq!(fragment.polygon, *piece);
    }
}

/// The low and high halves of a packed word as `f32`.
fn unpack_pair(word: u32) -> (f32, f32) {
    (
        f16_bits_to_f32((word & 0xffff) as u16),
        f16_bits_to_f32((word >> 16) as u16),
    )
}

/// A leaf with `nodes` nodes whose directions fan out with the node (a dispersive exit).
fn dispersive_leaf(image: &ConvexPolygon, nodes: u16) -> Leaf {
    let mut leaf = flat_leaf(image, nodes);
    let count = usize::from(nodes);
    for v in 0..leaf.image.len() {
        for n in 0..count {
            let i = v * count + n;
            let spread = 0.01 * 0.5_f32.mul_add(-(count as f32), n as f32) / count as f32;
            leaf.dirs[i] = Vec3::new(
                0.05_f32.mul_add(v as f32, 0.3) + spread,
                0.8,
                2.0_f32.mul_add(spread, -0.2),
            );
            leaf.weights[i] = 0.01_f32.mul_add(n as f32, 0.05_f32.mul_add(v as f32, 0.1));
        }
    }
    leaf.spectrum = SpectralInterval {
        first: 8,
        last: 8 + nodes - 1,
    };
    leaf
}

#[test]
fn pack_leaves_round_trips_within_half_precision() {
    let mut rng = Lcg(0x5EED_0004);
    let polygon = random_polygon(&mut rng);
    let mut leaf = dispersive_leaf(&polygon, 16);
    leaf.glare = true;
    leaf.reflection = false;
    let other = flat_leaf(&rectangle(1.0, 1.0, 4.0, 3.0), 8);
    let packed = pack_leaves(&[(7, vec![leaf.clone(), other]), (9, Vec::new())]);
    assert_eq!(packed.tiles, vec![7, 9]);
    assert_eq!(
        packed.tile_offsets,
        vec![0, 2 + presplit_extra(&leaf), 2 + presplit_extra(&leaf)]
    );
    assert_eq!(packed.headers.len(), 2 + presplit_extra(&leaf) as usize);

    let header = packed.headers[0];
    assert_eq!(header.sig, leaf.sig);
    assert_eq!(header.first_node, 8);
    assert_eq!(header.node_count, 16);
    assert_eq!(header.tau_omega, leaf.tau_omega);
    assert_eq!(
        header.flags,
        LEAF_FLAG_GLARE | LEAF_FLAG_DELTAS,
        "glare and deltas, not reflection"
    );
    let first_vertex = header.first_vertex as usize;
    assert!((3..=GPU_LEAF_MAX_VERTICES).contains(&(header.vertex_count as usize)));
    if leaf.image.len() <= GPU_LEAF_MAX_VERTICES {
        assert_eq!(header.vertex_count as usize, leaf.image.len());
    }
    let nodes = 16;
    let hero = usize::from(leaf.spectrum.centre() - leaf.spectrum.first);
    // The first piece keeps the polygon's leading vertices only when it was not split; compare by
    // matching image points to the leaf's vertices.
    let (points, count) = leaf.image.points_2d();
    for k in 0..header.vertex_count as usize {
        let vertex = packed.vertices[first_vertex + k];
        let v = (0..count)
            .find(|&v| {
                (f64::from(vertex.image[0]) - points[v].x).abs() < 1e-5
                    && (f64::from(vertex.image[1]) - points[v].y).abs() < 1e-5
            })
            .expect("a packed vertex is a leaf vertex");
        assert_eq!(vertex.hero_dir, leaf.dirs[v * nodes + hero].to_array());
        for n in 0..nodes {
            let packed_node = packed.nodes[vertex.first_node as usize + n];
            let (weight, dx) = unpack_pair(packed_node.weight_dx);
            let (dy, dz) = unpack_pair(packed_node.dy_dz);
            let expected_weight = leaf.weights[v * nodes + n];
            assert!(
                (weight - expected_weight).abs() <= F16_RELATIVE_TOLERANCE * expected_weight,
                "weight {weight} against {expected_weight}"
            );
            let delta = leaf.dirs[v * nodes + n] - leaf.dirs[v * nodes + hero];
            assert!(
                (dx - delta.x).abs() <= F16_DELTA_TOLERANCE,
                "dx {dx} {}",
                delta.x
            );
            assert!(
                (dy - delta.y).abs() <= F16_DELTA_TOLERANCE,
                "dy {dy} {}",
                delta.y
            );
            assert!(
                (dz - delta.z).abs() <= F16_DELTA_TOLERANCE,
                "dz {dz} {}",
                delta.z
            );
        }
    }

    // The flat leaf has no deltas and keeps its reflection and glare bits.
    let flat_header = packed.headers[packed.headers.len() - 1];
    assert_eq!(flat_header.flags, LEAF_FLAG_GLARE | LEAF_FLAG_REFLECTION);
    assert_eq!(flat_header.node_count, 8);
    let flat_first = packed.vertices[flat_header.first_vertex as usize].first_node as usize;
    for n in 0..8 {
        let node = packed.nodes[flat_first + n];
        assert_eq!(unpack_pair(node.weight_dx), (1.0, 0.0));
        assert_eq!(unpack_pair(node.dy_dz), (0.0, 0.0));
    }
}

/// The extra pieces `pack_leaves` makes of `leaf` beyond one.
fn presplit_extra(leaf: &Leaf) -> u32 {
    let mut stats = BeamStats::default();
    let _ = pack_leaves_counted(&[(0, vec![leaf.clone()])], &mut stats);
    u32::try_from(stats.gpu_presplits).expect("a handful of presplits")
}

#[test]
fn a_leaf_with_more_than_eight_vertices_is_split_without_losing_area() {
    let points: Vec<DVec2> = (0..12)
        .map(|i| {
            let angle = std::f64::consts::TAU * f64::from(i) / 12.0;
            DVec2::new(10.0, 10.0) + 5.0 * DVec2::new(angle.cos(), angle.sin())
        })
        .collect();
    let polygon = ConvexPolygon::from_image_points(&points).expect("a dodecagon");
    let leaf = dispersive_leaf(&polygon, 8);
    let mut stats = BeamStats::default();
    let packed = pack_leaves_counted(&[(3, vec![leaf.clone()])], &mut stats);
    assert!(packed.headers.len() >= 2, "{} pieces", packed.headers.len());
    assert_eq!(stats.gpu_presplits, packed.headers.len() as u64 - 1);
    assert_eq!(
        packed.tile_offsets,
        vec![
            0,
            u32::try_from(packed.headers.len()).expect("a handful of headers")
        ]
    );
    let mut area = 0.0;
    for header in &packed.headers {
        let count = header.vertex_count as usize;
        assert!(
            (3..=GPU_LEAF_MAX_VERTICES).contains(&count),
            "{count} vertices"
        );
        let first = header.first_vertex as usize;
        let ring: Vec<DVec2> = packed.vertices[first..first + count]
            .iter()
            .map(|v| DVec2::new(f64::from(v.image[0]), f64::from(v.image[1])))
            .collect();
        let piece = ConvexPolygon::from_image_points(&ring).expect("a convex piece");
        // Counter-clockwise like the parent: the vertex order was not reversed.
        assert_eq!(piece.vertices().len(), count);
        area += piece.area();
    }
    let expected = leaf.image.area();
    assert!(
        (area - expected).abs() <= 1.0e-5 * expected,
        "pieces {area} against polygon {expected}"
    );
}

#[test]
fn exit_weight_clamps_at_zero_and_exit_direction_normalises() {
    let polygon = rectangle(0.0, 0.0, 4.0, 4.0);
    let mut leaf = flat_leaf(&polygon, 8);
    // Vertex 0 negative, the rest positive: the interpolation near vertex 0 is negative.
    leaf.weights[0..8].fill(-1.0);
    let near_vertex_zero = DVec2::new(0.05, 0.05);
    assert_eq!(exit_weight(&leaf, near_vertex_zero, 0), 0.0);
    let far = DVec2::new(3.5, 3.5);
    assert!(exit_weight(&leaf, far, 0) > 0.0);
    // The direction field is constant (0, 1, 0) scaled by 3: the interpolation normalises.
    leaf.dirs.fill(Vec3::new(0.0, 3.0, 0.0));
    let d = exit_direction(&leaf, DVec2::new(1.7, 2.2), 3);
    assert!((d - Vec3::Y).length() < 1.0e-6, "{d:?}");
}

#[test]
fn the_cie_table_weights_bit_for_bit_like_integrate_nodes_to_xyz() {
    // The table replaces the per-fragment observer interpolation and must match it bit for bit.
    use super::raster::CieTable;
    use crate::optics::raytracer::color::integrate_nodes_to_xyz;

    for n_lambda in [24_u32, 48] {
        let table = CieTable::new(n_lambda);
        for first in (0..n_lambda).step_by(8) {
            let values: Vec<f32> = (0..8_u8)
                .map(|i| 0.37_f32.mul_add(f32::from(i), 0.011))
                .collect();
            let expected = integrate_nodes_to_xyz(&values, first, n_lambda);
            let got = table.integrate(&values, first);
            assert_eq!(got.x.to_bits(), expected.x.to_bits());
            assert_eq!(got.y.to_bits(), expected.y.to_bits());
            assert_eq!(got.z.to_bits(), expected.z.to_bits());
        }
        // A node past the grid falls back to the direct evaluation.
        let past = table.integrate(&[1.0], n_lambda + 2);
        let direct = integrate_nodes_to_xyz(&[1.0], n_lambda + 2, n_lambda);
        assert_eq!(past.x.to_bits(), direct.x.to_bits());
    }
}
