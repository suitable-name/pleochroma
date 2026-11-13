//! Pins of the branch signature step (`signature::branch_step`), shared by the path cache, the
//! reference tracer and the WGSL completion.

use super::{
    bounce::EventKind,
    signature::{BRANCH_SIG_SEED, branch_step},
};

#[test]
fn branch_step_pins() {
    let first = branch_step(BRANCH_SIG_SEED, 7, EventKind::Reflect, false);
    assert_eq!(first, [0x0707_A745, 0x3F6E_6B9A]);
    let second = branch_step(first, 12, EventKind::Transmit, true);
    assert_eq!(second, [0x5312_98E6, 0x62FA_FEB4]);
}

#[test]
fn partial_and_total_reflection_share_one_signature() {
    for facet in [0, 7, 12, 4_000] {
        for is_ext in [false, true] {
            assert_eq!(
                branch_step(BRANCH_SIG_SEED, facet, EventKind::Reflect, is_ext),
                branch_step(BRANCH_SIG_SEED, facet, EventKind::TotalReflect, is_ext),
                "a TIR and a partial reflection on facet {facet} must hash alike"
            );
        }
    }
    assert_ne!(
        branch_step(BRANCH_SIG_SEED, 7, EventKind::Reflect, false),
        branch_step(BRANCH_SIG_SEED, 7, EventKind::Transmit, false),
        "a reflection and a transmission on one facet must differ"
    );
}
