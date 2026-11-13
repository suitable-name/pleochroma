//! Tests for crown/pavilion classification and angle parsing in
//! `StandardGemCuts::from_database_angles`.
//!
//! Classification trusts explicit evidence (facet-name prefixes and `index_val` markers)
//! over the positional guess. The parser accepts a trailing UTF-8 degree sign, the
//! "N girdle facets" form, and skips individually unparseable rows.

use glam::Vec3;
use pleochroma::{FacetSpec, geometry::cuts::StandardGemCuts};

// ---------------------------------------------------------------------------
// Crown/pavilion classification in from_database_angles trusts explicit evidence
// (facet-name prefixes and `index_val` markers) over the positional guess.
// ---------------------------------------------------------------------------
#[test]
fn from_database_angles_classifies_by_facet_name_prefix_not_position() {
    // With 4 rows, a purely positional split would call indices 0-1 pavilion and 2-3 crown.
    // A C-prefixed (crown) facet sits at index 0 and a P-prefixed (pavilion) facet at index 3,
    // so the test shows the facet name decides. Indices 1-2 are neutral filler with no C, P
    // or G marker and use the positional guess (not tested here).
    let filler = || FacetSpec {
        facet: "5".into(),
        angle: "45.0".into(),
        index: "10".into(),
        notes: String::new(),
    };

    let angles = vec![
        FacetSpec {
            facet: "C1".into(),
            angle: "40.0".into(),
            index: "0".into(),
            notes: String::new(),
        },
        filler(),
        filler(),
        FacetSpec {
            facet: "P1".into(),
            angle: "40.0".into(),
            index: "48".into(),
            notes: String::new(),
        },
    ];

    let planes = StandardGemCuts::from_database_angles(&angles, 96);
    assert_eq!(planes.len(), 4);

    // C1 (index 0, which the positional guess calls pavilion) must be crown: positive Y.
    let n0 = Vec3::from(planes[0].normal);
    assert!(
        n0.y > 0.0,
        "C1 facet should be classified as crown (normal.y > 0) despite its early position, got {n0:?}"
    );

    // P1 (index 3, which the positional guess calls crown) must be pavilion: negative Y.
    let n3 = Vec3::from(planes[3].normal);
    assert!(
        n3.y < 0.0,
        "P1 facet should be classified as pavilion (normal.y < 0) despite its late position, got {n3:?}"
    );
}

#[test]
fn from_database_angles_classifies_table_and_culet_via_index_val() {
    // from_database_angles() falls back to the built-in standard_round_brilliant()
    // table whenever it reconstructs fewer than 4 planes total, so pad this out
    // with a couple of multi-index filler tiers to stay above that floor while
    // keeping the Table/Culet rows as the ones actually under test.
    //
    // The angles use the stored format, with a trailing degree sign, since that is what the
    // parser must accept. A plain "0.0" would not exercise it.
    let angles = vec![
        FacetSpec {
            facet: "U".into(),
            angle: "0.00\u{b0}".into(),
            index: "Table".into(),
            notes: String::new(),
        },
        FacetSpec {
            facet: "5".into(),
            angle: "35.00\u{b0}".into(),
            index: "10-20".into(),
            notes: String::new(),
        },
        FacetSpec {
            facet: "6".into(),
            angle: "35.00\u{b0}".into(),
            index: "30-40".into(),
            notes: String::new(),
        },
        FacetSpec {
            facet: "41".into(),
            angle: "0.00\u{b0}".into(),
            index: "Culet".into(),
            notes: String::new(),
        },
    ];

    let planes = StandardGemCuts::from_database_angles(&angles, 96);
    assert_eq!(planes.len(), 6);

    let table_n = Vec3::from(planes[0].normal);
    assert!(
        (table_n - Vec3::new(0.0, 1.0, 0.0)).length() < 1e-5,
        "index_val=Table must map to crown table (0,1,0), got {table_n:?}"
    );

    let culet_n = Vec3::from(planes[5].normal);
    assert!(
        (culet_n - Vec3::new(0.0, -1.0, 0.0)).length() < 1e-5,
        "index_val=Culet must map to pavilion culet (0,-1,0), got {culet_n:?}"
    );
}

// ---------------------------------------------------------------------------
// Every `angle_settings.angle` value carries a trailing UTF-8 degree sign (e.g. "44.86°"),
// which `f32::from_str` rejects. The lenient parser must accept it, or each angle would be
// read wrongly. These tests cover the parser, the "N girdle facets" expansion and the
// bail-out to the standard cut.
// ---------------------------------------------------------------------------
#[test]
fn from_database_angles_parses_real_degree_sign_format() {
    let angles = vec![
        FacetSpec {
            facet: "C1".into(),
            angle: "10.00\u{b0}".into(), // literal "10.00°", exactly as stored in the DB
            index: String::new(),
            notes: String::new(),
        },
        FacetSpec {
            facet: "5".into(),
            angle: "45.00\u{b0}".into(),
            index: "10-20".into(),
            notes: String::new(),
        },
        FacetSpec {
            facet: "6".into(),
            angle: "45.00\u{b0}".into(),
            index: "30-40".into(),
            notes: String::new(),
        },
    ];

    let planes = StandardGemCuts::from_database_angles(&angles, 96);
    // item 0 (empty index_val) -> 1 plane via the single-default-orientation branch;
    // items 1 and 2 each carry 2 indices -> 2 planes each. Total 5.
    assert_eq!(planes.len(), 5);

    let n0 = Vec3::from(planes[0].normal);
    let expected_y = 10.0f32.to_radians().cos();
    assert!(
        (n0.y - expected_y).abs() < 1e-4,
        "\"10.00°\" should parse as 10 degrees (cos ~= {expected_y}), got normal {n0:?}"
    );

    // Must not have fallen back to a 45 degree default.
    let old_buggy_y = 45.0f32.to_radians().cos();
    assert!(
        (n0.y - old_buggy_y).abs() > 0.05,
        "angle parse appears to have silently fallen back to the old 45 degree default: {n0:?}"
    );
}

#[test]
fn from_database_angles_expands_n_girdle_facets_form() {
    let angles = vec![FacetSpec {
        facet: "G1".into(),
        angle: "90.00\u{b0}".into(),
        index: "48 girdle facets".into(),
        notes: String::new(),
    }];

    let planes = StandardGemCuts::from_database_angles(&angles, 96);
    assert_eq!(
        planes.len(),
        48,
        "\"48 girdle facets\" must expand into 48 separate, evenly spaced facets"
    );

    // At 90 degrees the crown/pavilion formulas coincide (cos(90) == 0), so every
    // facet normal must be perfectly horizontal regardless of classification.
    for p in &planes {
        let n = Vec3::from(p.normal);
        assert!(
            n.y.abs() < 1e-4,
            "girdle facet normal must be horizontal (y == 0), got {n:?}"
        );
    }

    // Adjacent facets should be evenly spaced at 360/48 = 7.5 degrees apart
    // (indices generated as i * gear_teeth / N for i in 0..N).
    let phi = |p: &pleochroma::geometry::GpuFacetPlane| p.normal[2].atan2(p.normal[0]).to_degrees();
    let mut delta = phi(&planes[1]) - phi(&planes[0]);
    if delta < 0.0 {
        delta += 360.0;
    }
    assert!(
        (delta - 7.5).abs() < 0.5,
        "expected ~7.5 deg spacing between adjacent expanded girdle facets, got {delta}"
    );
}

#[test]
fn from_database_angles_bails_out_to_srb_when_mostly_unparseable() {
    // 3 of 4 angle strings are garbage (75% unparseable, well over the bail-out
    // threshold) -- the function must refuse to build a solid out of fabricated
    // angles and fall back to the built-in standard_round_brilliant() table instead.
    let angles = vec![
        FacetSpec {
            facet: "1".into(),
            angle: "not-a-number".into(),
            index: "10".into(),
            notes: String::new(),
        },
        FacetSpec {
            facet: "2".into(),
            angle: "???".into(),
            index: "20".into(),
            notes: String::new(),
        },
        FacetSpec {
            facet: "3".into(),
            angle: String::new(),
            index: "30".into(),
            notes: String::new(),
        },
        FacetSpec {
            facet: "4".into(),
            angle: "40.00\u{b0}".into(),
            index: "40".into(),
            notes: String::new(),
        },
    ];

    let planes = StandardGemCuts::from_database_angles(&angles, 96);
    let srb = StandardGemCuts::standard_round_brilliant();
    assert_eq!(
        planes.len(),
        srb.len(),
        "should bail out to standard_round_brilliant() when most angle values are unparseable"
    );
}

#[test]
fn from_database_angles_skips_single_bad_row_without_bailing_out() {
    // A single unparseable row among many (below the bail-out threshold) should be
    // skipped individually rather than either fabricating an angle for it or
    // discarding the whole, otherwise-good reconstruction.
    let mut angles: Vec<FacetSpec> = (0..10)
        .map(|i| FacetSpec {
            facet: format!("{}", i + 1),
            angle: "30.00\u{b0}".into(),
            index: String::new(),
            notes: String::new(),
        })
        .collect();
    angles.push(FacetSpec {
        facet: "bad".into(),
        angle: "garbage".into(),
        index: String::new(),
        notes: String::new(),
    });

    let planes = StandardGemCuts::from_database_angles(&angles, 96);
    // 10 good rows -> 10 planes (single-default-orientation branch, one each); the
    // one bad row contributes nothing and must not trigger the SRB bail-out (11
    // planes != standard_round_brilliant()'s facet count).
    assert_eq!(
        planes.len(),
        10,
        "the single unparseable row should be skipped, not fabricated or bailed out on"
    );
}
