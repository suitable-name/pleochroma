//! Tests of the import report: the verdict, the findings with their places, the repaired
//! triangles, the outline and the read progress, on small fixtures.

use super::{
    FindingKind, ImportNotes, ImportReport, LEVEL_INFO, LEVEL_PROBLEM, LEVEL_REPAIRED,
    ReadProgress, ScanImport, ScanVerdict, crossing_place, fixtures::CUBE_OBJ, import_scan,
    parse_mesh_progress, parse_obj_mesh,
};
use glam::DVec3;
use std::collections::BTreeMap;

/// A cube of side `edge` with its lowest corner at `lo`, as points and outward triangles.
fn cube_at(lo: DVec3, edge: f64) -> (Vec<DVec3>, Vec<[u32; 3]>) {
    let (points, triangles) = parse_obj_mesh(CUBE_OBJ).expect("the cube parses");
    let moved = points.iter().map(|&p| p * (edge / 20.0) + lo).collect();
    (moved, triangles)
}

/// The surface of a box `size` lattice steps across (a vertex at every lattice point), every
/// face split into triangles wound outward.
fn lattice_box(size: [u32; 3]) -> (Vec<DVec3>, Vec<[u32; 3]>) {
    let mut ids: BTreeMap<[u32; 3], u32> = BTreeMap::new();
    let mut points = Vec::new();
    let mut triangles = Vec::new();
    let mut vertex = |at: [u32; 3]| -> u32 {
        *ids.entry(at).or_insert_with(|| {
            points.push(DVec3::new(
                f64::from(at[0]),
                f64::from(at[1]),
                f64::from(at[2]),
            ));
            u32::try_from(points.len() - 1).expect("a small mesh")
        })
    };
    for axis in 0..3 {
        let (b, c) = ((axis + 1) % 3, (axis + 2) % 3);
        for side in [0, size[axis]] {
            for i in 0..size[b] {
                for j in 0..size[c] {
                    let corner = |di: u32, dj: u32| {
                        let mut at = [0; 3];
                        at[axis] = side;
                        at[b] = i + di;
                        at[c] = j + dj;
                        at
                    };
                    // The ring b, c, axis is right-handed, so b x c points along +axis.
                    let mut ring = [corner(0, 0), corner(1, 0), corner(1, 1), corner(0, 1)];
                    if side == 0 {
                        ring.reverse();
                    }
                    let [p0, p1, p2, p3] = ring.map(&mut vertex);
                    triangles.push([p0, p1, p2]);
                    triangles.push([p0, p2, p3]);
                }
            }
        }
    }
    (points, triangles)
}

/// A bar 100 x 4 x 4 mm with a 2 mm cavity inside it, and one triangle of its outer surface
/// left out: a small hole in a mesh that is not convex, so the repair fills it and the mesh is
/// kept.
fn bar_with_cavity_and_hole() -> (Vec<DVec3>, Vec<[u32; 3]>) {
    let (mut points, mut triangles) = lattice_box([100, 4, 4]);
    triangles.remove(5);
    let (cavity, cavity_tris) = cube_at(DVec3::new(40.0, 1.0, 1.0), 2.0);
    let offset = u32::try_from(points.len()).expect("a small mesh");
    points.extend(cavity);
    triangles.extend(cavity_tris.iter().map(|t| t.map(|v| v + offset)));
    (points, triangles)
}

/// Two 10 mm cubes, the second shifted by 5 mm along every axis: their surfaces cross.
fn crossing_cubes() -> (Vec<DVec3>, Vec<[u32; 3]>) {
    let (mut points, mut triangles) = cube_at(DVec3::ZERO, 10.0);
    let (more, more_tris) = cube_at(DVec3::splat(5.0), 10.0);
    let offset = u32::try_from(points.len()).expect("a small mesh");
    points.extend(more);
    triangles.extend(more_tris.iter().map(|t| t.map(|v| v + offset)));
    (points, triangles)
}

/// The import of `points` and `triangles` with no parser note and no size check.
fn imported(points: &[DVec3], triangles: &[[u32; 3]]) -> ScanImport {
    import_scan(points, triangles, None, |_| None).expect("the fixture imports")
}

#[test]
fn a_closed_cube_is_the_scan_itself_and_says_nothing() {
    let (points, triangles) = parse_obj_mesh(CUBE_OBJ).expect("parses");
    let scan = imported(&points, &triangles);
    assert_eq!(scan.report.verdict, ScanVerdict::ScanItself);
    assert_eq!(scan.report.verdict.index(), 0);
    assert!(
        scan.report.findings.is_empty(),
        "{:?}",
        scan.report.findings
    );
    assert_eq!(scan.note, None);
    assert_eq!(scan.report.triangles, 12);
    assert_eq!(scan.report.cavities, 0);
    assert_eq!(scan.report.outline_planes, 6);
    assert!(scan.report.repaired_triangles.is_empty() && scan.report.crossings.is_empty());
    assert_eq!(scan.layers.outline.len(), 12, "the twelve edges of a cube");
}

#[test]
fn a_mesh_with_a_big_hole_falls_back_to_its_outline_and_says_why() {
    let (points, mut triangles) = parse_obj_mesh(CUBE_OBJ).expect("parses");
    triangles.pop();
    let scan = imported(&points, &triangles);
    assert_eq!(scan.report.verdict, ScanVerdict::ConvexOutline);
    let note = scan.note.expect("the old note is kept");
    assert!(
        note.contains("its convex outline is used instead"),
        "{note}"
    );
    let [finding] = scan.report.findings.as_slice() else {
        panic!("one finding: {:?}", scan.report.findings);
    };
    assert_eq!(finding.level, LEVEL_PROBLEM);
    assert_eq!(finding.kind, FindingKind::Fallback);
    assert!(
        finding.text.starts_with("The mesh surface has a hole"),
        "{}",
        finding.text
    );
    assert_eq!(finding.place, None);
}

#[test]
fn a_small_hole_is_repaired_and_its_triangles_are_marked_with_a_place() {
    let (points, triangles) = bar_with_cavity_and_hole();
    let scan = imported(&points, &triangles);
    assert_eq!(scan.report.verdict, ScanVerdict::ScanItself);
    let note = scan.note.as_deref().expect("the old note is kept");
    assert!(note.starts_with("The mesh was repaired:"), "{note}");
    let filled = scan
        .report
        .finding_of(FindingKind::Filled)
        .expect("a finding for the hole");
    assert_eq!(filled.level, LEVEL_REPAIRED);
    assert!(filled.text.starts_with("Filled 1 hole"), "{}", filled.text);
    let place = filled.place.expect("the fill has a place");
    assert!(
        place[0] <= 100.0 && place[1] <= 4.0 && place[2] <= 4.0,
        "{place:?}"
    );
    assert_eq!(scan.report.repaired_triangles.len(), 1);
    assert_eq!(scan.layers.repaired.len(), 1);
    assert_eq!(scan.report.cavities, 1);
    assert_eq!(scan.report.triangles, triangles.len());
}

#[test]
fn a_mesh_that_crosses_itself_uses_its_outline_and_names_the_place() {
    let (points, triangles) = crossing_cubes();
    let scan = imported(&points, &triangles);
    assert_eq!(scan.report.verdict, ScanVerdict::ConvexOutline);
    let crossing = scan
        .report
        .finding_of(FindingKind::Crossing)
        .expect("a finding for the crossing");
    assert_eq!(crossing.level, LEVEL_PROBLEM);
    assert!(
        crossing.text.starts_with("The mesh crosses itself in"),
        "{}",
        crossing.text
    );
    assert!(crossing.detail.contains("near x"), "{}", crossing.detail);
    let place = crossing.place.expect("the crossing has a place");
    for coordinate in place {
        assert!((0.0..=15.0).contains(&coordinate), "{place:?}");
    }
    assert_eq!(scan.report.crossings, vec![place]);
    assert_eq!(scan.layers.crossings.len(), 1);
    assert!(!scan.layers.outline.is_empty(), "the outline used is drawn");
}

#[test]
fn a_point_cloud_is_planned_on_its_outline_with_an_info_finding() {
    let (points, _) = parse_obj_mesh(CUBE_OBJ).expect("parses");
    let scan = imported(&points, &[]);
    assert_eq!(scan.report.verdict, ScanVerdict::ConvexOutline);
    let [finding] = scan.report.findings.as_slice() else {
        panic!("one finding: {:?}", scan.report.findings);
    };
    assert_eq!(
        (finding.level, finding.kind),
        (LEVEL_INFO, FindingKind::NoFaces)
    );
    assert!(finding.text.contains("no faces"), "{}", finding.text);
    assert!(scan.note.expect("a note").contains("no faces"));
    assert_eq!(scan.report.triangles, 0);
}

#[test]
fn the_refusal_of_the_size_check_is_the_error_of_the_import() {
    let (points, triangles) = parse_obj_mesh(CUBE_OBJ).expect("parses");
    let error = import_scan(&points, &triangles, None, |_| Some("too small".to_string()))
        .expect_err("refused");
    assert_eq!(error, "too small");
    let report = ImportReport::refused(&error);
    assert_eq!(report.verdict, ScanVerdict::Refused);
    assert_eq!(report.verdict.index(), 2);
    assert_eq!(report.verdict_text, "too small");
}

#[test]
fn the_notes_of_a_repair_become_one_finding_per_clause() {
    let notes = ImportNotes {
        repair_note: Some(
            "The mesh was repaired: closed gaps by joining 12 vertices closer than 0.010 mm; \
             turned 3 faces to wind the surface consistently; filled 2 holes, the largest 0.4 mm across.",
        ),
        ..ImportNotes::default()
    };
    let report = ImportReport::from_notes(&notes);
    let kinds: Vec<FindingKind> = report.findings.iter().map(|f| f.kind).collect();
    assert_eq!(
        kinds,
        [
            FindingKind::Joined,
            FindingKind::Turned,
            FindingKind::Filled
        ]
    );
    assert!(report.findings.iter().all(|f| f.level == LEVEL_REPAIRED));
    assert_eq!(
        report.findings[0].text,
        "Closed gaps by joining 12 vertices closer than 0.010 mm"
    );
    assert_eq!(report.verdict, ScanVerdict::ScanItself);
}

#[test]
fn a_smooth_scan_keeps_the_scan_and_tells_the_grid_outline() {
    let note = "The scan's outline had 900 faces; a simplified outline of 48 planes is used for the \
                grid, and the scan itself decides where the material is.";
    let report = ImportReport::from_notes(&ImportNotes {
        outline_note: Some(note),
        ..ImportNotes::default()
    });
    assert_eq!(report.verdict, ScanVerdict::ScanItself);
    let [finding] = report.findings.as_slice() else {
        panic!("one finding");
    };
    assert_eq!(
        (finding.level, finding.kind),
        (LEVEL_INFO, FindingKind::Outline)
    );
    assert_eq!(finding.text, note, "the note's own words");
}

#[test]
fn the_crossing_place_is_read_from_the_note_in_the_files_coordinates() {
    let note =
        "the mesh crosses itself in at least 16 places, the first near x 12.30, y -4.05, z 7.80 mm";
    assert_eq!(crossing_place(note), Some([12.3, -4.05, 7.8]));
    assert_eq!(crossing_place("the mesh has a hole"), None);
    assert_eq!(crossing_place("the first near x 1.00, y 2.00 mm"), None);
}

#[test]
fn a_saved_plan_reports_that_it_was_repaired_on_import() {
    let report = ImportReport::saved(true, 6, 1200, 1);
    assert_eq!(report.verdict, ScanVerdict::ScanItself);
    assert_eq!(report.findings[0].kind, FindingKind::Saved);
    assert_eq!(
        report.findings[0].text,
        "Saved with the plan, repaired on import"
    );
    assert_eq!(
        ImportReport::saved(false, 6, 0, 0).verdict,
        ScanVerdict::ConvexOutline
    );
}

#[test]
fn an_obj_read_counts_its_triangles_and_reaches_one() {
    let progress = ReadProgress::default();
    assert_eq!(progress.fraction(), None, "nothing known before the read");
    let (points, triangles, note) =
        parse_mesh_progress(CUBE_OBJ.as_bytes(), Some("obj"), &progress).expect("parses");
    assert_eq!((points.len(), triangles.len(), note), (8, 12, None));
    assert_eq!(progress.fraction(), Some(1.0));
    // A binary format does not count: the progress stays unknown.
    let other = ReadProgress::default();
    let stl = super::fixtures::binary_stl_of("solid", &points, &triangles);
    parse_mesh_progress(&stl, Some("stl"), &other).expect("parses");
    assert_eq!(other.fraction(), None);
}

#[test]
fn layers_scale_only_for_the_same_shape() {
    let (points, triangles) = parse_obj_mesh(CUBE_OBJ).expect("parses");
    let layers = imported(&points, &triangles).layers;
    assert_eq!(layers.scale_for([20.0, 20.0, 20.0]), Some(1.0));
    let doubled = layers
        .scale_for([40.0, 40.0, 40.0])
        .expect("the same shape, bigger");
    assert!((doubled - 2.0).abs() < 1e-12);
    assert_eq!(layers.scale_for([40.0, 20.0, 20.0]), None);
}
