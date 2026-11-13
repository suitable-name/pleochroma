//! `.asc`'s single, lossy RI slot versus the design file's own authored/override
//! refractive-index fields.

use super::fixtures::{fully_anchored_design, reopen, simple_design};
use crate::{
    design::Design,
    native::{DesignExtras, catalogue_asc, design_from_str, design_to_string},
};

/// `.asc` has one RI slot, and export writes the effective value there. Re-importing that
/// `.asc` alone would overwrite the authored `meta.refractive_index`. The design file's
/// `[schedule]` `refractive_index` must restore the authored value on a real save and load.
#[test]
fn authored_refractive_index_round_trips_through_save_and_load_even_when_the_asc_i_line_differs() {
    let mut design = simple_design();
    // `simple_design` pins an override equal to the authored RI. It is removed here so the
    // effective and authored values diverge.
    design.material.refractive_index_override = None;
    let authored = design.meta.refractive_index;
    assert_ne!(
        design.effective_refractive_index(),
        authored,
        "fixture must actually exercise the authored/effective divergence"
    );

    // Importing the catalogue `.asc` alone derives the effective RI as `meta.refractive_index`.
    // That shows the fixture exercises the lossy round trip, not a case where the two agree.
    let saved = catalogue_asc(&design, None, None).expect("must write");
    let schedule = pleochroma_formats::asc::parse_asc(&saved.asc_text).expect("must parse");
    let asc_only = Design::from_asc_schedule(design.preform.clone(), &schedule);
    assert_ne!(asc_only.meta.refractive_index, authored);

    let loaded = reopen(&design);
    assert_eq!(
        loaded.design.meta.refractive_index, authored,
        "the AUTHORED RI must survive, not the effective one a plain .asc re-import would derive"
    );
}

/// `MaterialTable::refractive_index_override` must round-trip through a real
/// save/load exactly like every other `material` field already does.
#[test]
fn refractive_index_override_round_trips_through_save_and_load() {
    let mut design = fully_anchored_design();
    design.material.refractive_index_override = Some(1.90);
    let text = design_to_string(&design, None, &DesignExtras::default()).expect("must save");
    assert!(text.contains("refractive_index_override"), "{text}");

    let loaded = design_from_str(&text).expect("must load");
    assert_eq!(loaded.design.material.refractive_index_override, Some(1.90));
}
