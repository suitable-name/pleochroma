//! A sync into the phone's library, against a scripted computer: what lands, how it reads, and
//! the rule that a synced design is written only by the sync.

use std::{
    cell::RefCell,
    collections::HashSet,
    sync::{Arc, atomic::Ordering},
};

use pleochroma_library_mirror::{
    MirrorOutcome,
    testing::{FakeTransport, design_record_at, design_summary_at},
};
use pleochroma_net::library::AttachedFileMeta;
use pleochroma_web_core::input::MAX_DESIGN_FILE_BYTES;

use super::{
    LibraryCard, LibraryQuery, MirrorCounts, MirrorHandle, MirrorProgress, SyncOutcome,
    mirror::settle,
    phone_mirror_options, progress_text,
    store::numbered_name,
    tests::{Fixture, picked, template_files},
};
use crate::{
    platform::Platform,
    platform_mock::MockPlatform,
    remote::library::{METERED_SYNC_REFUSAL, RemoteLibrary, RemoteLibraryError},
    settings::RemoteSettings,
};

/// The `source_id` every test syncs under.
const SOURCE: &str = "remote-library:ca-0123456789abcdef";

/// One design on the pretend computer.
struct Remote {
    id: i64,
    title: String,
    url: String,
    file_name: String,
    bytes: Vec<u8>,
    revision: u32,
    /// The size the computer advertises for the file; its real length when `None`.
    declared_size: Option<u64>,
}

fn remote(id: i64, title: &str, url: &str, file_name: &str, bytes: &[u8]) -> Remote {
    Remote {
        id,
        title: title.to_owned(),
        url: url.to_owned(),
        file_name: file_name.to_owned(),
        bytes: bytes.to_vec(),
        revision: 0,
        declared_size: None,
    }
}

/// The id of the one attachment of design `id` on the computer.
const fn attachment_id(id: i64) -> i64 {
    id * 10
}

fn meta(id: i64, name: &str, url: &str, size: u64) -> AttachedFileMeta {
    AttachedFileMeta {
        id,
        name: name.to_owned(),
        url: url.to_owned(),
        size,
    }
}

/// A computer holding `designs`, each with its design file as the one attachment.
fn computer(designs: &[Remote]) -> FakeTransport {
    let summaries = designs
        .iter()
        .map(|d| design_summary_at(d.id, &d.title, &d.url, "1.0", d.revision))
        .collect();
    let records = designs
        .iter()
        .map(|d| {
            let mut record = design_record_at(d.id, &d.title, &d.url, "1.0", d.revision);
            record.attachments = vec![meta(
                attachment_id(d.id),
                &d.file_name,
                &format!("{}/{}", d.url, d.file_name),
                d.declared_size.unwrap_or(d.bytes.len() as u64),
            )];
            record
        })
        .collect();
    designs
        .iter()
        .fold(FakeTransport::new(summaries, records), |transport, d| {
            transport.with_attachment(attachment_id(d.id), &d.file_name, d.bytes.clone())
        })
}

/// Runs a sync of `transport` into the library and returns what it counted.
fn sync(fx: &Fixture, transport: &FakeTransport) -> MirrorCounts {
    let (outcome, stopped) = fx.h().sync_with(
        transport,
        SOURCE,
        phone_mirror_options(),
        &MirrorHandle::new(),
        |_| {},
    );
    assert!(!stopped, "the failure streak must not fire here");
    match outcome {
        MirrorOutcome::Completed(counts) => counts,
        other => panic!("the sync did not complete: {other:?}"),
    }
}

/// The card titled `title`.
fn card(fx: &Fixture, title: &str) -> LibraryCard {
    fx.all()
        .into_iter()
        .find(|card| card.title == title)
        .unwrap_or_else(|| panic!("no card titled {title}"))
}

#[test]
fn a_synced_design_is_searchable_and_opens_offline() {
    let fx = Fixture::new();
    let (native, _) = template_files(1);
    let url = "https://example.test/round";
    let transport = computer(&[remote(
        7,
        "Computer Round",
        url,
        &native.display_name,
        &native.bytes,
    )]);

    let counts = sync(&fx, &transport);
    assert_eq!(
        (counts.new_count, counts.updated_count),
        (1, 0),
        "{counts:?}"
    );

    let synced = card(&fx, "Computer Round");
    assert!(synced.synced && !synced.mine, "{synced:?}");
    assert_eq!((synced.shape.as_str(), synced.gear), ("Round", 96));
    let (found, total) = fx
        .h()
        .search(
            &LibraryQuery {
                text: "computer".to_owned(),
                ..LibraryQuery::default()
            },
            0,
            10,
        )
        .expect("searches");
    assert_eq!(
        (found.len(), total),
        (1, 1),
        "the text search finds the copy"
    );

    // Opening needs no connection: the transport is not asked again.
    let before = transport.fetch_design_calls.load(Ordering::Relaxed);
    let design = fx.h().load(synced.id).expect("loads");
    assert_eq!(design.file_name, native.display_name);
    assert_eq!(design.bytes, native.bytes);
    assert_eq!(transport.fetch_design_calls.load(Ordering::Relaxed), before);
}

#[test]
fn a_catalogue_design_is_synced_not_mine_and_a_computer_import_is_both() {
    let fx = Fixture::new();
    let (native, _) = template_files(1);
    let (other, _) = template_files(2);
    let transport = computer(&[
        remote(
            1,
            "Catalogue",
            "https://example.test/catalogue",
            &native.display_name,
            &native.bytes,
        ),
        remote(
            2,
            "Computer Import",
            "local://Imported.psdd",
            "Imported.psdd",
            &other.bytes,
        ),
    ]);
    assert_eq!(sync(&fx, &transport).new_count, 2);
    let own = fx
        .h()
        .save(None, "Mine.psdd", &native.bytes)
        .expect("saves a design of the phone's own");

    let catalogue = card(&fx, "Catalogue");
    assert!(catalogue.synced && !catalogue.mine, "{catalogue:?}");
    let imported = card(&fx, "Computer Import");
    assert!(imported.synced && imported.mine, "{imported:?}");
    let phone = fx
        .all()
        .into_iter()
        .find(|card| card.id == own)
        .expect("the phone's own design");
    assert!(phone.mine && !phone.synced, "{phone:?}");
}

#[test]
fn a_second_sync_fetches_nothing() {
    let fx = Fixture::new();
    let (native, _) = template_files(1);
    let transport = computer(&[
        remote(
            1,
            "One",
            "https://example.test/1",
            "one.psdd",
            &native.bytes,
        ),
        remote(
            2,
            "Two",
            "https://example.test/2",
            "two.psdd",
            &native.bytes,
        ),
    ]);
    assert_eq!(sync(&fx, &transport).new_count, 2);
    let designs = transport.fetch_design_calls.load(Ordering::Relaxed);
    let attachments = transport.fetch_attachment_calls.load(Ordering::Relaxed);
    assert_eq!((designs, attachments), (2, 2));

    let again = sync(&fx, &transport);
    assert_eq!(
        (
            again.new_count,
            again.updated_count,
            again.skipped_unchanged
        ),
        (0, 0, 2),
        "{again:?}"
    );
    assert_eq!(
        transport.fetch_design_calls.load(Ordering::Relaxed),
        designs
    );
    assert_eq!(
        transport.fetch_attachment_calls.load(Ordering::Relaxed),
        attachments
    );
}

#[test]
fn the_phone_keeps_only_design_files() {
    let fx = Fixture::new();
    let (native, _) = template_files(1);
    let url = "https://example.test/files";
    let mut record = design_record_at(3, "Files", url, "1.0", 0);
    record.diagram_image_name = Some("diagram.png".to_owned());
    record.diagram_image_data = Some(vec![1, 2, 3]);
    record.attachments = vec![
        meta(
            30,
            "files.psdd",
            "https://example.test/f.psdd",
            native.bytes.len() as u64,
        ),
        meta(31, "manual.pdf", "https://example.test/m.pdf", 4),
        meta(32, "photo.png", "https://example.test/p.png", 4),
    ];
    let transport = FakeTransport::new(
        vec![design_summary_at(3, "Files", url, "1.0", 0)],
        vec![record],
    )
    .with_attachment(30, "files.psdd", native.bytes)
    .with_attachment(31, "manual.pdf", b"%PDF".to_vec())
    .with_attachment(32, "photo.png", vec![0; 4]);

    let counts = sync(&fx, &transport);
    assert_eq!(counts.new_count, 1, "{counts:?}");
    assert_eq!(counts.attachments_skipped_by_content, 2);
    assert_eq!(transport.fetch_attachment_calls.load(Ordering::Relaxed), 1);

    let id = card(&fx, "Files").id;
    let stored = fx
        .h()
        .lock()
        .get_diagram_full(id)
        .expect("reads")
        .expect("the row exists");
    let names: Vec<&str> = stored
        .attached_files
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(names, ["files.psdd"]);
    assert!(stored.diagram_image_data.is_none());
    assert!(stored.diagram_image_name.is_none());
}

#[test]
fn a_design_whose_file_is_too_big_is_skipped_and_counted() {
    let fx = Fixture::new();
    let (native, _) = template_files(1);
    let mut big = remote(
        1,
        "Big",
        "https://example.test/big",
        "big.psdd",
        &native.bytes,
    );
    big.declared_size = Some(MAX_DESIGN_FILE_BYTES + 1);
    let small = remote(
        2,
        "Small",
        "https://example.test/small",
        "small.psdd",
        &native.bytes,
    );
    let transport = computer(&[big, small]);

    let counts = sync(&fx, &transport);
    assert_eq!(counts.new_count, 1, "{counts:?}");
    assert_eq!(counts.designs_skipped_too_large, 1);
    assert_eq!(fx.all().len(), 1);
    assert!(
        fx.h()
            .lock()
            .get_mirror_state("https://example.test/big")
            .expect("reads")
            .is_none(),
        "no state, so the next sync looks at it again"
    );
    assert_eq!(
        transport.fetch_attachment_calls.load(Ordering::Relaxed),
        1,
        "the big file was never asked for"
    );
    let again = sync(&fx, &transport);
    assert_eq!(
        again.designs_skipped_too_large, 1,
        "checked again: {again:?}"
    );
}

#[test]
fn a_phone_design_with_the_same_name_wins() {
    let fx = Fixture::new();
    let (native, _) = template_files(1);
    let (other, _) = template_files(2);
    let own = fx
        .h()
        .save(None, &native.display_name, &native.bytes)
        .expect("saves");
    let transport = computer(&[remote(
        1,
        "Computer Copy",
        &format!("local://{}", native.display_name),
        &native.display_name,
        &other.bytes,
    )]);

    for _ in 0..2 {
        let counts = sync(&fx, &transport);
        assert_eq!(counts.new_count, 0, "{counts:?}");
        assert_eq!(counts.local_conflicts_skipped, 1);
    }
    let all = fx.all();
    assert_eq!(all.len(), 1);
    assert!(all[0].mine && !all[0].synced, "{:?}", all[0]);
    assert_eq!(fx.h().load(own).expect("loads").bytes, native.bytes);
}

#[test]
fn saving_over_a_synced_design_forks_a_copy() {
    let fx = Fixture::new();
    let (native, asc) = template_files(1);
    let (edited, _) = template_files(2);
    let local_url = format!("local://{}", native.display_name);
    let mut designs = vec![
        remote(
            1,
            "On Computer",
            &local_url,
            &native.display_name,
            &native.bytes,
        ),
        remote(
            2,
            "In Catalogue",
            "https://example.test/catalogue",
            "catalogue.psdd",
            &native.bytes,
        ),
    ];
    assert_eq!(sync(&fx, &computer(&designs)).new_count, 2);
    let synced = card(&fx, "On Computer").id;
    let catalogue = card(&fx, "In Catalogue").id;

    // Saving over a synced design makes a copy of the user's own and leaves the original.
    let fork = fx
        .h()
        .save(Some(synced), &native.display_name, &edited.bytes)
        .expect("saves");
    assert_ne!(fork, synced);
    assert_eq!(fx.h().load(synced).expect("loads").bytes, native.bytes);
    let stored = fx.h().load(fork).expect("loads");
    assert_eq!(stored.bytes, edited.bytes);
    assert!(stored.file_name.contains(" (2)."), "{}", stored.file_name);
    let forked = fx
        .all()
        .into_iter()
        .find(|card| card.id == fork)
        .expect("the copy");
    assert!(forked.mine && !forked.synced, "{forked:?}");
    // A catalogue design forks too.
    let second = fx
        .h()
        .save(Some(catalogue), "catalogue.psdd", &edited.bytes)
        .expect("saves");
    assert_ne!(second, catalogue);
    assert_eq!(fx.h().load(catalogue).expect("loads").bytes, native.bytes);
    assert_eq!(fx.all().len(), 4);

    // The synced original goes on updating from the computer; the copy is not touched.
    designs[0].revision = 1;
    designs[0].file_name = asc.display_name.clone();
    designs[0].bytes = asc.bytes;
    let counts = sync(&fx, &computer(&designs));
    assert_eq!(counts.updated_count, 1, "{counts:?}");
    assert_eq!(
        fx.h().load(synced).expect("loads").file_name,
        asc.display_name
    );
    assert_eq!(fx.h().load(fork).expect("loads").bytes, edited.bytes);
}

#[test]
fn importing_a_file_named_like_a_synced_design_forks_and_reimport_is_a_duplicate() {
    let fx = Fixture::new();
    let (native, _) = template_files(1);
    let (other, _) = template_files(2);
    let transport = computer(&[remote(
        1,
        "Computer Import",
        &format!("local://{}", native.display_name),
        &native.display_name,
        &native.bytes,
    )]);
    assert_eq!(sync(&fx, &transport).new_count, 1);
    let synced = card(&fx, "Computer Import").id;

    let file = picked(&native.display_name, &other.bytes);
    let report = fx.h().import(vec![file.clone()]);
    assert_eq!((report.added, report.duplicates), (1, 0), "{report:?}");
    assert!(report.failed.is_empty(), "{report:?}");
    assert_eq!(
        fx.h().load(synced).expect("loads").bytes,
        native.bytes,
        "the synced design is untouched"
    );
    let free = numbered_name(&native.display_name, 2);
    let forked = fx
        .h()
        .lock()
        .diagram_entry_id_for_url(&format!("local://{free}"))
        .expect("reads")
        .expect("the import went under the free name");
    assert_ne!(forked, synced);

    let again = fx.h().import(vec![file]);
    assert_eq!((again.added, again.duplicates), (0, 1), "{again:?}");
    assert_eq!(fx.all().len(), 2);
}

#[test]
fn deleting_a_synced_design_keeps_it_deleted() {
    let fx = Fixture::new();
    let (native, _) = template_files(1);
    let mut designs = vec![remote(
        1,
        "Doomed",
        "https://example.test/doomed",
        "doomed.psdd",
        &native.bytes,
    )];
    assert_eq!(sync(&fx, &computer(&designs)).new_count, 1);
    let id = card(&fx, "Doomed").id;
    fx.h().delete(&[id]).expect("deletes");

    // The computer changes the design; the deletion still holds.
    designs[0].revision = 1;
    let moved = computer(&designs);
    let counts = sync(&fx, &moved);
    assert_eq!(
        (counts.new_count, counts.skipped_deleted),
        (0, 1),
        "{counts:?}"
    );
    assert!(fx.all().is_empty());
    assert!(
        moved.fetched_entry_ids.borrow().is_empty(),
        "a deleted design is not even fetched"
    );
}

#[test]
fn ten_failures_in_a_row_stop_as_interrupted() {
    let fx = Fixture::new();
    // The computer lists fifteen designs and cannot serve any of them.
    let summaries = (1..=15)
        .map(|n| {
            design_summary_at(
                n,
                &format!("Design {n}"),
                &format!("https://example.test/{n}"),
                "1.0",
                0,
            )
        })
        .collect();
    let transport = FakeTransport::new(summaries, Vec::new());
    let handle = MirrorHandle::new();

    let (outcome, stopped) =
        fx.h()
            .sync_with(&transport, SOURCE, phone_mirror_options(), &handle, |_| {});
    assert!(stopped && handle.is_cancelled());
    assert_eq!(transport.fetch_design_calls.load(Ordering::Relaxed), 10);
    let MirrorOutcome::Cancelled(counts) = &outcome else {
        panic!("the streak ends the run as cancelled: {outcome:?}");
    };
    assert_eq!(counts.failed, 10);

    let settled = settle(
        outcome,
        stopped,
        Some(RemoteLibraryError::Interrupted(
            "The link broke.".to_owned(),
        )),
    );
    let SyncOutcome::Interrupted { error, .. } = &settled else {
        panic!("expected Interrupted, got {settled:?}");
    };
    assert_eq!(
        error,
        &RemoteLibraryError::Interrupted("The link broke.".to_owned())
    );
    assert!(settled.summary("Studio PC").contains("stopped answering"));
}

#[test]
fn a_cancel_is_cancelled_and_a_failed_listing_names_its_reason() {
    let cancelled = settle(
        MirrorOutcome::Cancelled(MirrorCounts::default()),
        false,
        None,
    );
    assert!(
        matches!(cancelled, SyncOutcome::Cancelled(_)),
        "{cancelled:?}"
    );

    let unreachable = RemoteLibraryError::Unreachable("Could not reach the computer: x".to_owned());
    let failed = settle(
        MirrorOutcome::Failed("could not list the remote library: x".to_owned()),
        false,
        Some(unreachable.clone()),
    );
    assert!(
        matches!(&failed, SyncOutcome::Failed(error) if *error == unreachable),
        "{failed:?}"
    );

    let bare = settle(
        MirrorOutcome::Failed("could not list".to_owned()),
        false,
        None,
    );
    assert!(
        matches!(&bare, SyncOutcome::Failed(RemoteLibraryError::Failed(m)) if m == "could not list"),
        "{bare:?}"
    );

    let stopped_while_listing = settle(
        MirrorOutcome::Failed("could not list".to_owned()),
        false,
        Some(RemoteLibraryError::Cancelled),
    );
    assert!(
        matches!(stopped_while_listing, SyncOutcome::Cancelled(_)),
        "a stop while the catalogue is listed is a stop, not a failure"
    );
}

#[test]
fn the_revision_moves_during_and_after_a_sync() {
    let fx = Fixture::new();
    let (native, _) = template_files(1);
    let designs: Vec<Remote> = (1..=30)
        .map(|n| {
            remote(
                n,
                &format!("Design {n}"),
                &format!("https://example.test/{n}"),
                "design.psdd",
                &native.bytes,
            )
        })
        .collect();
    let transport = computer(&designs);
    let seen: RefCell<Vec<(usize, u64)>> = RefCell::new(Vec::new());

    let (outcome, _) = fx.h().sync_with(
        &transport,
        SOURCE,
        phone_mirror_options(),
        &MirrorHandle::new(),
        |progress: &MirrorProgress| {
            seen.borrow_mut()
                .push((progress.processed, fx.h().revision.load(Ordering::Acquire)));
        },
    );
    assert!(
        matches!(outcome, MirrorOutcome::Completed(_)),
        "{outcome:?}"
    );

    let seen = seen.into_inner();
    let at = |processed: usize| {
        seen.iter()
            .find(|(n, _)| *n == processed)
            .map(|(_, revision)| *revision)
            .expect("progress was reported")
    };
    assert_eq!(at(24), 0, "nothing moves before 25 designs are written");
    assert!(at(25) >= 1, "the list refreshes mid-sync");
    let last = fx.h().revision.load(Ordering::Acquire);
    assert!(last > at(25), "and once more at the end");
}

#[test]
fn mirrored_urls_lists_only_live_copies() {
    let fx = Fixture::new();
    let (native, _) = template_files(1);
    let (first, second) = ("https://example.test/1", "https://example.test/2");
    let transport = computer(&[
        remote(1, "One", first, "one.psdd", &native.bytes),
        remote(2, "Two", second, "two.psdd", &native.bytes),
    ]);
    assert_eq!(sync(&fx, &transport).new_count, 2);
    fx.h()
        .save(None, "Mine.psdd", &native.bytes)
        .expect("saves a design of the phone's own");
    let ask = [
        first,
        second,
        "local://Mine.psdd",
        "https://example.test/none",
    ];
    assert_eq!(
        fx.h().mirrored_urls(&ask),
        HashSet::from([first.to_owned(), second.to_owned()])
    );

    let deleted = card(&fx, "One").id;
    fx.h().delete(&[deleted]).expect("deletes");
    assert_eq!(
        fx.h().mirrored_urls(&ask),
        HashSet::from([second.to_owned()]),
        "a deleted design is no longer a copy"
    );
}

#[test]
fn every_sync_outcome_has_a_plain_sentence() {
    let pc = "Studio PC";
    let counts = MirrorCounts {
        new_count: 37,
        updated_count: 5,
        skipped_unchanged: 458,
        ..MirrorCounts::default()
    };
    assert_eq!(
        SyncOutcome::Completed(counts).summary(pc),
        "Synced with Studio PC: 37 new, 5 updated, 458 unchanged."
    );
    let with_extras = MirrorCounts {
        failed: 3,
        designs_skipped_too_large: 2,
        local_conflicts_skipped: 2,
        skipped_deleted: 4,
        ..counts
    };
    assert_eq!(
        SyncOutcome::Completed(with_extras).summary(pc),
        "Synced with Studio PC: 37 new, 5 updated, 458 unchanged. 3 could not be copied; sync \
         again to retry. 2 were too large to copy. 2 kept as they are on this phone (same file \
         name). 4 you deleted were not copied again."
    );
    let singles = MirrorCounts {
        designs_skipped_too_large: 1,
        local_conflicts_skipped: 1,
        skipped_deleted: 1,
        ..counts
    };
    let text = SyncOutcome::Completed(singles).summary(pc);
    assert!(text.contains("1 was too large to copy."), "{text}");
    assert!(text.contains("1 kept as it is on this phone"), "{text}");
    assert!(
        text.contains("1 you deleted was not copied again."),
        "{text}"
    );
    let nothing_new = MirrorCounts {
        skipped_unchanged: 500,
        ..MirrorCounts::default()
    };
    assert_eq!(
        SyncOutcome::Completed(nothing_new).summary(pc),
        "Everything from Studio PC is already on this phone."
    );
    assert_eq!(
        SyncOutcome::Cancelled(counts).summary(pc),
        "Sync stopped. 42 designs were copied; sync again to continue."
    );
    assert_eq!(
        SyncOutcome::Cancelled(MirrorCounts::default()).summary(pc),
        "Sync stopped. No designs were copied; sync again to continue."
    );
    let interrupted = SyncOutcome::Interrupted {
        counts,
        error: RemoteLibraryError::Interrupted("lost".to_owned()),
    };
    assert_eq!(
        interrupted.summary(pc),
        "Studio PC stopped answering. 42 designs were copied; sync again to continue."
    );
    let revoked = SyncOutcome::Interrupted {
        counts,
        error: RemoteLibraryError::Refused("revoked".to_owned()),
    };
    assert!(revoked.summary(pc).contains("Pair with it again"));
    assert_eq!(
        SyncOutcome::Failed(RemoteLibraryError::Unreachable(
            "Could not reach the computer: refused".to_owned()
        ))
        .summary(pc),
        "Couldn't reach Studio PC. Nothing was changed. Check that it is on and on the same Wi-Fi."
    );
    assert_eq!(
        SyncOutcome::Failed(RemoteLibraryError::NoLibrary).summary(pc),
        "Studio PC no longer shares a design library."
    );
    assert_eq!(
        SyncOutcome::Failed(RemoteLibraryError::Refused(METERED_SYNC_REFUSAL.to_owned()))
            .summary(pc),
        METERED_SYNC_REFUSAL
    );
    assert!(SyncOutcome::Completed(counts).changed());
    assert!(SyncOutcome::Cancelled(counts).changed());
    assert!(!SyncOutcome::Completed(nothing_new).changed());
    assert!(!SyncOutcome::Failed(RemoteLibraryError::NoLibrary).changed());
}

#[test]
fn the_progress_line_names_the_design() {
    let progress = MirrorProgress {
        processed: 120,
        counts: MirrorCounts {
            total_found: 500,
            ..MirrorCounts::default()
        },
        current_title: "Round Brilliant".to_owned(),
    };
    assert_eq!(
        progress_text(&progress, "Studio PC"),
        "Copying 120 of 500: Round Brilliant"
    );
    let untitled = MirrorProgress {
        current_title: " ".to_owned(),
        ..progress
    };
    assert_eq!(
        progress_text(&untitled, "Studio PC"),
        "Copying 120 of 500 from Studio PC"
    );
}

#[test]
fn a_sync_that_cannot_start_writes_nothing() {
    let fx = Fixture::new();
    let mock = Arc::new(MockPlatform::default());
    let platform: Arc<dyn Platform> = mock.clone();
    let settings = RemoteSettings {
        name: "Studio PC".to_owned(),
        address: "127.0.0.1:1".to_owned(),
        enrolled: true,
        ..RemoteSettings::default()
    };
    let remote = RemoteLibrary::new(platform, settings);

    // Metered and not allowed: refused before any request.
    mock.set_network_metered(Some(true));
    let refused = fx.h().sync_from(&remote, &MirrorHandle::new(), |_| {});
    assert!(
        matches!(&refused, SyncOutcome::Failed(RemoteLibraryError::Refused(m)) if m == METERED_SYNC_REFUSAL),
        "{refused:?}"
    );

    // Not metered, but nothing is enrolled in the mock's vault.
    mock.set_network_metered(Some(false));
    let unenrolled = fx.h().sync_from(&remote, &MirrorHandle::new(), |_| {});
    assert!(
        matches!(&unenrolled, SyncOutcome::Failed(RemoteLibraryError::Refused(m)) if m == "Enroll this phone first."),
        "{unenrolled:?}"
    );
    assert!(fx.all().is_empty());
}
