//! Tests for the staleness check of library searches: `bump_search_seq` and
//! `is_current_search`.

use super::dispatch::{bump_search_seq, is_current_search};

#[test]
fn a_freshly_bumped_sequence_number_is_current() {
    let seq = bump_search_seq();
    assert!(is_current_search(seq));
}

#[test]
fn an_older_sequence_number_is_superseded_by_a_newer_bump() {
    let old_seq = bump_search_seq();
    // A later `refresh_diagram_list` call bumps the counter the same way.
    bump_search_seq();
    assert!(!is_current_search(old_seq));
}

#[test]
fn a_sync_refresh_supersedes_an_older_pending_async_dispatch() {
    // A dispatch captures `async_seq`. A later refresh, such as one after the cutter cleared
    // a filter, bumps the counter, and that must supersede the dispatch.
    let async_seq = bump_search_seq();
    assert!(is_current_search(async_seq));
    bump_search_seq();
    assert!(
        !is_current_search(async_seq),
        "a sync refresh must supersede an older async dispatch's sequence number"
    );
}
