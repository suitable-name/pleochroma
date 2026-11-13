//! The line drawings: their lines, their labels inside the facets, and the leader labels of the
//! facets too small for one, on an octahedron and on Pentalume.

use super::{
    leaders::{CAPTION_EM_PER_CHAR, LEADER_END_GAP_EMS, TextBox, segments_cross},
    *,
};
use pleochroma::geometry::stone_metrics::{SolidStatus, build_solid_mesh};
use std::collections::BTreeSet;

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
        // Every label fits inside its face, so no face needs a leader.
        assert_eq!(count(view, LineWeight::Leader), 0, "{}", view.caption);
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

/// A corner repeated a hair off its place (an edge seen end-on) leaves the polygon's inside
/// inside. Without a tolerance, the middle of this polygon would count as outside.
#[test]
fn a_repeated_corner_does_not_empty_a_polygon() {
    let polygon = [
        (0.0, 0.0),
        (10.0, 0.0),
        (10.0, 10.0),
        (10.000_01, 9.999_98),
        (0.0, 10.0),
    ];
    assert!(inside_convex(&polygon, (5.0, 5.0), 0.06));
    assert!(!inside_convex(&polygon, (11.0, 5.0), 0.06));
    assert!(!inside_convex(&polygon, (5.0, -1.0), 0.06));
}

/// A tiny triangle far from the origin keeps its interior point inside it.
#[test]
fn the_interior_point_of_a_tiny_far_triangle_is_inside_it() {
    let triangle = [(500.0, 500.0), (500.5, 500.0), (500.0, 500.3)];
    let point = interior_point(&triangle);
    assert!(inside_convex(&triangle, point, 0.0), "{point:?}");
    assert!((point.0 - (500.0 + 0.5 / 3.0)).abs() < 1e-3, "{point:?}");
    assert!((point.1 - 500.1).abs() < 1e-3, "{point:?}");
}

/// When no spot of a view can hold a label (every label is wider than the view), each tier still
/// gets exactly one: the first facet of a tier takes the last-resort spot, the other facet of
/// that tier gets none.
#[test]
fn every_tier_keeps_one_label_when_the_view_has_no_room() {
    let wide_a = "A".repeat(100);
    let wide_b = "B".repeat(100);
    let style = DiagramStyle {
        facet_labels: (0..8)
            .map(|i| {
                if i % 2 == 0 {
                    wide_a.clone()
                } else {
                    wide_b.clone()
                }
            })
            .collect(),
        ..DiagramStyle::default()
    };
    let views = render_vector_views(&octahedron(), &config(), &style).expect("views");
    for view in &views {
        let labels_of = |text: &str| {
            view.labels
                .iter()
                .filter(|label| label.text == text)
                .count()
        };
        // The side view looks at the four faces toward +Z, the odd ones; every other view shows
        // two faces of each tier.
        let expected_a = usize::from(view.caption != "PROFILE");
        assert_eq!(labels_of(wide_a.as_str()), expected_a, "{}", view.caption);
        assert_eq!(labels_of(wide_b.as_str()), 1, "{}", view.caption);
    }
}

/// Pentalume (`designs/Pentalume_Sapphire`): its P3 (0.28 mm² at a 10 mm width)
/// and C3 (0.37 mm²) facets are too small to hold a label.
const PENTALUME_ASC: &str = "GemCad 5.0\n\
    g 80 0\n\
    y 5 y\n\
    I 1.7680773735046387\n\
    H Pentalume Sapphire\n\
    H Designed by Niklas Gussek\n\
    a -43 0.7130808923813132 0 n P1_Five_broad_mains 16 32 48 64\n\
    a -40.5 0.7661935481511087 8 n P2_Five_corner_guards 24 40 56 72\n\
    a -44 0.7955073176906 76 n P3_Ten_shoulder_splits 4 12 20 28 36 44 52 60 68\n\
    a 90 1 0 n G1_Five_long_sides 16 32 48 64 G Set stone size.\n\
    a 90 1.13 8 n G2_Five_clipping_flats 24 40 56 72\n\
    a 34 0.5944270003043362 0 n C1_Five_broad_bezels 16 32 48 64\n\
    a 30 0.6018060796608384 8 n C2_Five_corner_bezels 24 40 56 72\n\
    a 33 0.6346315320439263 76 n C3_Ten_shoulder_facets 4 12 20 28 36 44 52 60 68\n\
    a 0 0.35 n Table\n";

/// Pentalume's closed mesh, and the sheet's labels: each facet's tier code alone, as
/// `render_vector_cut_views` passes them.
fn pentalume() -> (SolidMesh, DiagramStyle) {
    let schedule = pleochroma_formats::asc::parse_asc(PENTALUME_ASC).expect("Pentalume parses");
    let design = pleochroma_cut_core::Design::from_asc_schedule(
        pleochroma_cut_core::PreformSpec::block(3.0, 1.0, 3.0),
        &schedule,
    );
    let solved = design.solve().expect("every tier is pinned");
    let SolidStatus::Closed(mesh) = build_solid_mesh(&design.planes_from_solved(&solved)) else {
        panic!("Pentalume closes");
    };
    let map = crate::facet_map::FacetMap::from_design(&design, &solved);
    let facet_labels = (0..map.facet_count())
        .map(|id| {
            map.facet_label(id)
                .split_whitespace()
                .next()
                .unwrap_or_default()
                .to_string()
        })
        .collect();
    let style = DiagramStyle {
        facet_labels,
        ..DiagramStyle::default()
    };
    (mesh, style)
}

/// The sheet's frame for Pentalume's 80-tooth gear.
fn pentalume_config() -> DiagramConfig {
    DiagramConfig {
        gear_teeth: 80,
        symmetry_order: 5,
        ..config()
    }
}

/// One view as [`render_vector_views`] draws it, with what went into it.
struct Traced {
    /// The drawing.
    view: VectorView,
    /// Its leader labels, each with its facet.
    leaders: Vec<LeaderLabel>,
    /// The pieces it draws.
    pieces: Vec<Piece>,
    /// Its layout.
    layout: PanelLayout,
}

/// The four views of `mesh`, in the order of [`render_vector_views`].
fn traced_views(mesh: &SolidMesh, config: &DiagramConfig, style: &DiagramStyle) -> Vec<Traced> {
    let wheel = Wheel {
        teeth: config.gear_teeth.max(1),
        reference: config.gear_reference_angle,
    };
    let turned = turned_quarter(mesh);
    [
        (mesh, PanelKind::Crown, "CROWN"),
        (mesh, PanelKind::Profile, "PROFILE"),
        (&turned, PanelKind::Profile, "END"),
        (mesh, PanelKind::Pavilion, "PAVILION"),
    ]
    .into_iter()
    .map(|(drawn, kind, caption)| {
        let (view, leaders) = vector_view_with_leaders(drawn, config, style, kind, caption, wheel);
        let layout = compute_single_panel_layout(drawn, config.width, config.height, kind);
        let pieces = facing_pieces(drawn, &layout, style);
        Traced {
            view,
            leaders,
            pieces,
            layout,
        }
    })
    .collect()
}

/// Every tier with a drawn facet keeps a label in every view of Pentalume, its tiny P3 and C3
/// included, and the drawings come out the same twice over.
#[test]
fn every_tier_of_pentalume_is_labelled_in_every_view() {
    let (mesh, style) = pentalume();
    let config = pentalume_config();
    for traced in traced_views(&mesh, &config, &style) {
        let drawn: BTreeSet<&str> = traced
            .pieces
            .iter()
            .filter_map(|piece| style.facet_labels.get(piece.facet_id))
            .map(String::as_str)
            .filter(|text| !text.is_empty())
            .collect();
        assert!(drawn.len() >= 3, "{}: {drawn:?}", traced.view.caption);
        for tier in drawn {
            assert!(
                traced.view.labels.iter().any(|label| label.text == tier),
                "{} has no {tier} label",
                traced.view.caption
            );
        }
    }
    // The tiny facets keep their labels where they sit: C3 on the crown, P3 on the pavilion.
    let views = render_vector_views(&mesh, &config, &style).expect("views");
    let [crown, _, _, pavilion] = &views;
    assert!(crown.labels.iter().any(|label| label.text == "C3"));
    assert!(pavilion.labels.iter().any(|label| label.text == "P3"));
    assert_eq!(
        Some(views.clone()),
        render_vector_views(&mesh, &config, &style)
    );
}

/// Pentalume's leader labels stay inside their view and overlap no other text: no label inside
/// a facet, no tooth number, no other leader label, not the caption. No two leaders cross.
#[test]
fn pentalume_leader_labels_keep_clear_of_all_other_text() {
    let (mesh, style) = pentalume();
    for traced in traced_views(&mesh, &pentalume_config(), &style) {
        let view = &traced.view;
        assert!(!traced.leaders.is_empty(), "{} has no leader", view.caption);
        let caption = TextBox::of_label(&view.caption_label(), CAPTION_EM_PER_CHAR);
        for leader in &traced.leaders {
            let own = TextBox::of_label(&leader.label, LABEL_EM_PER_CHAR);
            assert!(own.inside(view.width, view.height), "{}", view.caption);
            assert!(
                !own.overlaps(caption),
                "{}: {:?}",
                view.caption,
                leader.label
            );
            // Every text of the view but itself: the labels inside facets, the tooth numbers and
            // the other leader labels.
            let overlapping = view
                .labels
                .iter()
                .filter(|label| own.overlaps(TextBox::of_label(label, LABEL_EM_PER_CHAR)))
                .count();
            assert_eq!(overlapping, 1, "{}: {:?}", view.caption, leader.label);
        }
        let lines: Vec<VectorLine> = traced
            .leaders
            .iter()
            .filter_map(|leader| leader.line)
            .collect();
        for (index, first) in lines.iter().enumerate() {
            for second in &lines[index + 1..] {
                assert!(
                    !segments_cross(first.from, first.to, second.from, second.to),
                    "{}: {first:?} crosses {second:?}",
                    view.caption
                );
            }
        }
    }
}

/// Every leader of Pentalume starts inside the facet its label names and stops just short of the
/// label; on the crown and pavilion some leader labels sit in the band inside the wheel's ticks.
#[test]
fn pentalume_leaders_run_from_their_facet_to_their_label() {
    let (mesh, style) = pentalume();
    for traced in traced_views(&mesh, &pentalume_config(), &style) {
        let view = &traced.view;
        let tolerance = corner_tolerance(view);
        assert_eq!(
            count(view, LineWeight::Leader),
            traced
                .leaders
                .iter()
                .filter(|leader| leader.line.is_some())
                .count(),
            "{}",
            view.caption
        );
        for leader in &traced.leaders {
            assert_eq!(leader.label.text, style.facet_labels[leader.facet_id]);
            let Some(line) = leader.line else {
                continue;
            };
            assert_eq!(line.weight, LineWeight::Leader);
            assert!(view.lines.contains(&line), "{}", view.caption);
            assert!(
                traced
                    .pieces
                    .iter()
                    .any(|piece| piece.facet_id == leader.facet_id
                        && inside_convex(&piece.points, line.from, tolerance)),
                "{}: the {} leader starts outside its facet",
                view.caption,
                leader.label.text
            );
            let end_gap = LEADER_END_GAP_EMS * leader.label.size;
            let short =
                TextBox::of_label(&leader.label, LABEL_EM_PER_CHAR).nearest_distance(line.to);
            assert!(
                short >= end_gap * 0.9 && short <= end_gap * 1.5,
                "{}: the {} leader stops {short} from its label",
                view.caption,
                leader.label.text
            );
        }
        if traced.layout.kind.has_index_wheel() {
            let centre = (traced.layout.center_x, traced.layout.center_y);
            let ticks = traced.layout.wheel_radius_px * TICK_INNER;
            assert!(
                traced.leaders.iter().any(|leader| {
                    TextBox::of_label(&leader.label, LABEL_EM_PER_CHAR).farthest_distance(centre)
                        < ticks
                }),
                "{}: no leader label in the band",
                view.caption
            );
        }
    }
}

/// The SVG draws the leaders as a path of their own, first and thinnest.
#[test]
fn the_svg_draws_the_leaders_first_and_thinnest() {
    let (mesh, style) = pentalume();
    let views = render_vector_views(&mesh, &pentalume_config(), &style).expect("views");
    let pavilion = &views[3];
    let svg = view_to_svg(pavilion);
    assert_eq!(
        svg.matches("<path ").count(),
        4,
        "leader, tick, edge and outline"
    );
    let leader_width = format!(
        "stroke-width=\"{}\"",
        number(pavilion.stroke_width(LineWeight::Leader))
    );
    assert_eq!(leader_width, "stroke-width=\"1.01\"");
    let first_path = &svg[svg.find("<path ").expect("a path")..];
    let first_path = &first_path[..first_path.find("/>").expect("a closed path")];
    assert!(first_path.contains(&leader_width), "{first_path}");
    for weight in [LineWeight::Tick, LineWeight::Edge, LineWeight::Outline] {
        assert!(
            pavilion.stroke_width(LineWeight::Leader) < pavilion.stroke_width(weight),
            "{weight:?}"
        );
    }
}
