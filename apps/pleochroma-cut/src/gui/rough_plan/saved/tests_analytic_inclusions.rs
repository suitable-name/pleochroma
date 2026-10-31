//! Tests of the inclusions of a block, a cylinder or a pebble in the plan file (schema version
//! 6): that they round trip, that a plan without them is written as it always was, that an
//! older build and an older version number cannot silently lose them, and what the loader
//! refuses. The inclusions of an imported mesh (version 3) are in `tests_mesh`.

use super::{
    axis::orientation_to_dto,
    convert::CandidateSource,
    dto::{MeshDto, SavedPlanDto, SourceFrameDto},
    fixtures::{block, expect_error, sample_layout_in, settings, write},
    format::{SerializeInput, parse_and_validate_plan, payload_version_of, serialize_checked_plan},
    store,
    tests_db::library,
    transfer::{export_text, import_text},
};
use pleochroma_cut_core::{
    crystal_axis::CrystalOrientation,
    rough_plan::{
        Axis, MAX_MESH_TRIANGLES, PrimitivePose, PrimitiveShape, RoughBase, RoughCut, RoughMesh,
        RoughModel, primitive_mesh, shape::hull::MAX_INCLUSIONS,
    },
};

/// The box `size_mm` on every edge, centred at `centre_mm`, as a closed shell.
fn shell_box(centre_mm: [f64; 3], size_mm: f64) -> RoughMesh {
    primitive_mesh(
        PrimitiveShape::Box {
            size_mm: [size_mm; 3],
        },
        PrimitivePose::at(centre_mm),
    )
    .expect("a box")
}

/// The sphere of `radius_mm` centred at `centre_mm` as a closed shell.
fn shell_sphere(centre_mm: [f64; 3], radius_mm: f64) -> RoughMesh {
    primitive_mesh(
        PrimitiveShape::Sphere { radius_mm },
        PrimitivePose::at(centre_mm),
    )
    .expect("a sphere")
}

/// A face cut of `depth_mm` at the `normal` side that stays clear of [`shells_of`].
const fn clear_cut(normal: [f64; 3]) -> RoughCut {
    RoughCut::Face {
        normal,
        depth_mm: 1.0,
    }
}

/// The three kinds of rough without inclusions, each with a face cut: the 20 x 12 x 10 mm
/// block of the fixtures, a cylinder 12 mm across and 30 mm long, and an 18.4 x 11 x 9.6 mm
/// pebble.
fn plain_kinds() -> Vec<(&'static str, RoughModel)> {
    vec![
        ("block", block(vec![clear_cut([0.0, 1.0, 0.0])])),
        (
            "cylinder",
            RoughModel::new(
                RoughBase::Cylinder {
                    diameter_mm: 12.0,
                    length_mm: 30.0,
                    axis: Axis::Z,
                },
                vec![clear_cut([0.0, 1.0, 0.0])],
            ),
        ),
        (
            "pebble",
            RoughModel::new(
                RoughBase::Pebble {
                    x_mm: 18.4,
                    y_mm: 11.0,
                    z_mm: 9.6,
                },
                vec![clear_cut([1.0, 0.0, 0.0])],
            ),
        ),
    ]
}

/// Two shells that lie well inside the rough of `kind` (see [`plain_kinds`]), in the frame of
/// its uncut base: a box and a sphere.
fn shells_of(kind: &str) -> [RoughMesh; 2] {
    match kind {
        "block" => [
            shell_box([14.0, 6.0, 5.0], 4.0),
            shell_sphere([5.0, 6.0, 5.0], 2.0),
        ],
        "cylinder" => [
            shell_box([6.0, 6.0, 8.0], 3.0),
            shell_sphere([6.0, 6.0, 22.0], 1.5),
        ],
        _ => [
            shell_box([6.0, 5.5, 4.8], 3.0),
            shell_sphere([13.0, 5.5, 4.8], 1.2),
        ],
    }
}

/// `model` of `kind` with both of its shells, each with a 0.3 mm margin.
fn with_shells(kind: &str, model: &RoughModel) -> RoughModel {
    model
        .with_inclusions_added(&shells_of(kind), 0.3)
        .expect("the inclusions fit the rough")
}

/// The three kinds of rough with their two inclusions.
fn kinds() -> Vec<(&'static str, RoughModel)> {
    plain_kinds()
        .into_iter()
        .map(|(kind, model)| (kind, with_shells(kind, &model)))
        .collect()
}

fn document(text: &str) -> SavedPlanDto {
    toml::from_str(text).expect("the document parses")
}

/// The loader's refusal of `doc` written out again.
fn refused(doc: &SavedPlanDto) -> String {
    expect_error(&toml::to_string_pretty(doc).expect("the document is written"))
}

fn relative(a: f64, b: f64) -> f64 {
    (a - b).abs() / b.abs()
}

#[test]
fn inclusions_in_a_block_a_cylinder_and_a_pebble_round_trip_at_version_6() {
    for (kind, model) in kinds() {
        assert_eq!(model.inclusion_count(), 2, "{kind}");
        let layouts = vec![sample_layout_in(&model)];
        let text = write(&model, &layouts);
        assert_eq!(payload_version_of(&text), Some(6), "{kind}: {text}");
        assert!(text.contains("version = 6"), "{kind}: {text}");
        assert!(text.contains("[[rough.inclusions]]"), "{kind}: {text}");
        for absent in ["[rough.mesh]", "source_frame", "hull = "] {
            assert!(!text.contains(absent), "{kind}: {absent} in {text}");
        }

        let loaded = parse_and_validate_plan(&text).expect("the plan loads");
        assert_eq!(loaded.version, 6, "{kind}");
        assert_eq!(loaded.model, model, "{kind}");
        assert_eq!(
            loaded.model.inclusions.id(),
            model.inclusions.id(),
            "{kind}"
        );
        assert_eq!(loaded.model.inclusion_count(), 2, "{kind}");
        assert_eq!(loaded.layouts, layouts, "{kind}");
        assert!(loaded.model.mesh().is_some(), "{kind}: the mesh builds");

        // The weight of the plan is the gross volume, and the inclusions weigh the same.
        let (a, b) = (
            model.measure().expect("measures"),
            loaded.model.measure().expect("measures"),
        );
        assert!(relative(a.volume_mm3, b.volume_mm3) <= 1e-12, "{kind}");
        assert!(a.inclusion_mm3 > 0.0, "{kind}");
        assert!(
            relative(a.inclusion_mm3, b.inclusion_mm3) <= 1e-12,
            "{kind}"
        );

        // And it is a fixed point: writing it again gives the same text.
        assert_eq!(write(&loaded.model, &layouts), text, "{kind}");
    }
}

#[test]
fn a_plan_without_inclusions_is_written_as_it_always_was() {
    for (kind, plain) in plain_kinds() {
        let layouts = vec![sample_layout_in(&plain)];
        let text = write(&plain, &layouts);
        assert_eq!(payload_version_of(&text), Some(1), "{kind}: {text}");
        assert!(!text.contains("inclusions"), "{kind}: {text}");
        // Inclusions added and removed again leave the very same rough, so the same bytes.
        let back = with_shells(kind, &plain)
            .with_inclusion_removed(1)
            .expect("removes")
            .with_inclusion_removed(0)
            .expect("removes");
        assert_eq!(back, plain, "{kind}");
        assert_eq!(write(&back, &layouts), text, "{kind}");
    }
}

#[test]
fn the_shells_are_written_grown_in_the_frame_of_the_uncut_base() {
    for (kind, model) in kinds() {
        let doc = document(&write(&model, &[]));
        let rough = &doc.rough;
        assert_eq!(rough.base, kind);
        assert!(rough.mesh.is_none() && rough.hull.is_empty(), "{kind}");
        assert!(rough.source_frame.is_none(), "{kind}");
        assert_eq!(rough.inclusions.len(), 2, "{kind}");
        let parts = model.inclusions.to_parts();
        for (written, (points, triangles)) in rough.inclusions.iter().zip(&parts) {
            // `+ 0.0` turns a negative zero into a zero, as the writer does.
            let written_points: Vec<[f64; 3]> = written
                .vertices
                .iter()
                .map(|&v| v.map(|c| c + 0.0))
                .collect();
            let expected: Vec<[f64; 3]> = points
                .iter()
                .map(|p| p.to_array().map(|c| c + 0.0))
                .collect();
            assert_eq!(written_points, expected, "{kind}");
            assert_eq!(&written.triangles, triangles, "{kind}");
        }
        // Grown: the first shell is wider along x than the shape that was typed.
        let x_extent = |xs: Vec<f64>| {
            xs.iter().fold(f64::MIN, |hi, &x| hi.max(x))
                - xs.iter().fold(f64::MAX, |lo, &x| lo.min(x))
        };
        let typed = shells_of(kind);
        let typed_width = x_extent(typed[0].vertices().iter().map(|p| p.x).collect());
        let stored_width = x_extent(rough.inclusions[0].vertices.iter().map(|v| v[0]).collect());
        assert!(
            stored_width > typed_width,
            "{kind}: the margin is baked into the stored shell ({stored_width} against {typed_width})"
        );
    }
}

#[test]
fn inclusions_in_a_block_cylinder_or_pebble_need_version_6() {
    for (kind, model) in kinds() {
        let good = write(&model, &[]);
        for version in 1..=5_u32 {
            let text = good.replacen("version = 6", &format!("version = {version}"), 1);
            let error = expect_error(&text);
            assert!(
                error.contains("rough.inclusions"),
                "{kind} v{version}: {error}"
            );
            if version >= 3 {
                assert!(
                    error.contains("plan version 6"),
                    "{kind} v{version}: {error}"
                );
            }
        }
    }
}

#[test]
fn the_version_ladder_puts_inclusions_in_a_block_on_top() {
    let (_, model) = kinds().remove(0);
    let mut doc = document(&write(&model, &[]));
    assert!(doc.rough.analytic_inclusions());
    assert_eq!(doc.content_version(), 6);

    // A rough orientation alone is version 5; with the inclusions the plan is still 6.
    doc.rough.orientation = Some(orientation_to_dto(&CrystalOrientation::from_angles(
        40.0,
        25.0,
        Some(10.0),
    )));
    assert_eq!(doc.content_version(), 6);
    let shells = std::mem::take(&mut doc.rough.inclusions);
    assert!(!doc.rough.analytic_inclusions());
    assert_eq!(doc.content_version(), 5);

    // Inclusions that lie in a mesh keep the version they always had.
    doc.rough.orientation = None;
    doc.rough.mesh = Some(MeshDto {
        vertices: Vec::new(),
        triangles: Vec::new(),
    });
    doc.rough.inclusions = shells;
    assert!(!doc.rough.analytic_inclusions());
    assert_eq!(doc.content_version(), 3);
}

#[test]
fn a_rough_orientation_and_inclusions_are_both_kept_in_a_version_6_plan() {
    let (_, model) = kinds().remove(0);
    let oriented = model.with_orientation(Some(CrystalOrientation::from_angles(
        40.0,
        25.0,
        Some(10.0),
    )));
    let text = write(&oriented, &[sample_layout_in(&oriented)]);
    assert_eq!(payload_version_of(&text), Some(6), "{text}");
    assert!(text.contains("[rough.orientation]"), "{text}");
    let loaded = parse_and_validate_plan(&text).expect("the plan loads");
    assert_eq!(loaded.version, 6);
    assert!(loaded.model.orientation.is_some());
    assert_eq!(loaded.model.inclusions, oriented.inclusions);
}

#[test]
fn a_shell_that_does_not_fit_the_rough_is_refused() {
    for (kind, model) in kinds() {
        let good = document(&write(&model, &[]));
        let moved_by = |dx: f64| {
            let mut doc = good.clone();
            for v in &mut doc.rough.inclusions[0].vertices {
                v[0] += dx;
            }
            doc
        };
        // Outside the material.
        let error = refused(&moved_by(100.0));
        assert!(error.contains("rough.inclusions"), "{kind}: {error}");

        // Through the surface (the block's far face is at 20 mm).
        if kind == "block" {
            let error = refused(&moved_by(5.0));
            assert!(error.contains("rough.inclusions"), "{kind}: {error}");
        }

        // Two shells that overlap: a copy of the first moved 1.5 mm, so the two surfaces
        // really cross (an exact copy would depend on how coincident triangles count).
        let mut doc = good.clone();
        let mut first = doc.rough.inclusions[0].clone();
        for v in &mut first.vertices {
            v[0] += 1.5;
        }
        doc.rough.inclusions.push(first);
        let error = refused(&doc);
        assert!(error.contains("rough.inclusions"), "{kind}: {error}");

        // An open surface: half the triangles missing.
        let mut doc = good.clone();
        let shell = &mut doc.rough.inclusions[0];
        shell.triangles.truncate(shell.triangles.len() / 2);
        let error = refused(&doc);
        assert!(error.contains("rough.inclusions"), "{kind}: {error}");

        // A coordinate that is not a number.
        let mut doc = good.clone();
        doc.rough.inclusions[0].vertices[0][0] = f64::NAN;
        let error = refused(&doc);
        assert!(error.contains("rough.inclusions"), "{kind}: {error}");

        // More shells than a rough may have.
        let mut doc = good.clone();
        doc.rough.inclusions = vec![good.rough.inclusions[0].clone(); MAX_INCLUSIONS + 1];
        let error = refused(&doc);
        assert!(error.contains("rough.inclusions lists"), "{kind}: {error}");

        // A shell with more triangles than a rough may have.
        let mut doc = good.clone();
        doc.rough.inclusions[0].triangles = vec![[0, 1, 2]; MAX_MESH_TRIANGLES + 1];
        let error = refused(&doc);
        assert!(error.contains("rough.inclusions[0]"), "{kind}: {error}");
    }
}

#[test]
fn the_tables_of_an_imported_mesh_do_not_belong_beside_the_inclusions_of_a_block() {
    let (_, model) = kinds().remove(0);
    let good = document(&write(&model, &[]));

    let mut doc = good.clone();
    doc.rough.mesh = Some(good.rough.inclusions[0].clone());
    let error = refused(&doc);
    assert!(error.contains("imported mesh"), "{error}");

    let mut doc = good.clone();
    doc.rough.source_frame = Some(SourceFrameDto {
        scale: 1.0,
        offset_mm: [0.0; 3],
    });
    let error = refused(&doc);
    assert!(error.contains("imported mesh"), "{error}");

    let mut doc = good;
    doc.rough.hull = vec![[0.0; 3], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
    let error = refused(&doc);
    assert!(error.contains("imported mesh"), "{error}");
}

#[test]
fn a_plan_whose_inclusion_no_longer_fits_the_rough_is_not_saved() {
    let (_, model) = kinds().remove(0);
    // The size edit that took the material away from the inclusions.
    let mut edited = model;
    edited.base = RoughBase::Block {
        x_mm: 8.0,
        y_mm: 12.0,
        z_mm: 10.0,
    };
    let plan_settings = settings();
    let error = serialize_checked_plan(&SerializeInput {
        name: "Edited",
        created_at: 1_790_000_000,
        library_id: None,
        model: &edited,
        material_name: "Aquamarine",
        weighed_ct: None,
        settings: &plan_settings,
        candidate_source: CandidateSource::Library,
        designs: &[],
        layouts: &[],
    })
    .expect_err("the plan would not open again");
    assert!(error.contains("rough.inclusions"), "{error}");
}

#[test]
fn an_export_and_an_import_keep_the_inclusions_of_a_block() {
    for (kind, model) in kinds() {
        let layouts = vec![sample_layout_in(&model)];
        let text = write(&model, &layouts);
        let first = library();
        let id = store::save_plan(&first, "Test plan", &text).expect("stored");
        assert_eq!(
            store::load_saved(&first, id)
                .expect("loaded")
                .payload_version,
            6,
            "{kind}"
        );
        let exported = export_text(&first, id).expect("exported");
        assert_eq!(exported, text, "{kind}");

        let second = library();
        let opened = import_text(&second, "plan.toml", &exported).expect("imported");
        assert_eq!(opened.plan.version, 6, "{kind}");
        assert_eq!(opened.plan.model, model, "{kind}");
        let stored = store::load_saved(&second, opened.plan_id).expect("loaded");
        assert_eq!(stored.payload_version, 6, "{kind}");
        assert_eq!(
            export_text(&second, opened.plan_id).expect("exported"),
            text,
            "{kind}"
        );
    }
}
