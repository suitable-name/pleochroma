use super::*;
use crate::solve::design_to_toml;
use pleochroma_cut_core::{PreformSpec, ScheduleMeta};

const SAMPLE_ASC: &str = "GemCad 5.0\n\
g 96 0.0\n\
y 6 y\n\
I 1.72\n\
H Round Trichecker-12\n\
a -41.000000 0.64991234 92 n 1 84 76 68 60\n\
a 41.000000 0.5 92 n T\n";

fn file(name: &str, content: &[u8]) -> AttachedFile {
    AttachedFile {
        name: name.to_owned(),
        url: String::new(),
        content: content.to_vec(),
    }
}

fn empty_design() -> Design {
    Design::new(
        PreformSpec::block(2.0, 1.0, 2.0),
        ScheduleMeta {
            gear_teeth: 96,
            ..ScheduleMeta::default()
        },
        Vec::new(),
    )
}

/// The six planes of a box 2 x 2 x 2 (inside is `n . x + d <= 0`).
fn box_planes() -> Vec<GpuFacetPlane> {
    [
        [1.0, 0.0, 0.0],
        [-1.0, 0.0, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, -1.0, 0.0],
        [0.0, 0.0, 1.0],
        [0.0, 0.0, -1.0],
    ]
    .into_iter()
    .map(|normal| GpuFacetPlane { normal, d: -1.0 })
    .collect()
}

#[test]
fn a_design_file_attachment_reads_back_as_its_design() {
    let text = design_to_toml(&empty_design()).unwrap();
    let design = design_of_attachments(&[
        file("notes.pdf", b"%PDF"),
        file("block.psdd", text.as_bytes()),
    ])
    .unwrap();
    assert_eq!(design.meta.gear_teeth, 96);
    assert!(design.tiers.is_empty());
}

#[test]
fn an_asc_attachment_reads_as_a_design() {
    let design = design_of_attachments(&[file("trichecker.asc", SAMPLE_ASC.as_bytes())]).unwrap();
    assert!(!design.tiers.is_empty());
}

#[test]
fn a_design_file_outranks_an_asc_beside_it() {
    let text = design_to_toml(&empty_design()).unwrap();
    let design = design_of_attachments(&[
        file("trichecker.asc", SAMPLE_ASC.as_bytes()),
        file("block.psdd", text.as_bytes()),
    ])
    .unwrap();
    assert!(design.tiers.is_empty(), "the design file was read");
}

#[test]
fn an_entry_without_a_design_file_is_refused_and_a_broken_file_names_itself() {
    let none = design_of_attachments(&[file("notes.pdf", b"%PDF")]).unwrap_err();
    assert!(none.contains("no design file"), "{none}");
    assert!(design_of_attachments(&[]).is_err());
    let broken = design_of_attachments(&[file("bad.psdd", b"not a design")]).unwrap_err();
    assert!(broken.contains("bad.psdd"), "{broken}");
    let not_text = design_of_file(&file("bad.psdd", &[0xff, 0xfe, 0x00])).unwrap_err();
    assert!(not_text.contains("UTF-8"), "{not_text}");
    let other = design_of_file(&file("notes.pdf", b"%PDF")).unwrap_err();
    assert!(other.contains("not a design file"), "{other}");
}

#[test]
fn an_unsolved_design_measures_nothing_and_has_no_girdle() {
    let mut solid = SolvedSolid::default();
    let detail = FacetingDiagramDetail::default();
    assert!(solid.measure(&detail).is_none());
    assert_eq!(solid.girdle_sides(), 0);
    assert!(solid.measure_ref().is_none());
    assert!(solid.planes().is_empty());
}

#[test]
fn an_open_arrangement_measures_nothing() {
    let planes = vec![GpuFacetPlane {
        normal: [0.0, 1.0, 0.0],
        d: -1.0,
    }];
    let mut solid = SolvedSolid::from_planes(planes);
    assert!(solid.measure(&FacetingDiagramDetail::default()).is_none());
}

#[test]
fn a_closed_box_measures_square_in_plan() {
    let mut solid = SolvedSolid::from_planes(box_planes());
    let measured = solid
        .measure(&FacetingDiagramDetail::default())
        .expect("a box closes");
    assert!(measured.volume > 0.0);
    assert!(measured.width_axis > 0.0);
    assert!((measured.length_axis / measured.width_axis - 1.0).abs() < 1e-6);
    assert_eq!(solid.planes().len(), 6);
}
