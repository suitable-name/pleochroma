//! The phone's side of the computer's library: how a query is sent, how a connection is held,
//! reconnected and limited, and how a design file is opened. No sockets: the held stream is an
//! in-memory fake.

use std::{
    cell::Cell,
    io::{self, Cursor},
};

use pleochroma_cut_core::{TierFrosting, native::design_from_str};
use pleochroma_library_mirror::{
    browse::DesignFile,
    testing::{FakeTransport, design_summary},
};
use pleochroma_net::messages::ErrorMsg;
use pleochroma_vault::model::frosting::StoredTierFrosting;

use super::*;
use crate::{
    app_state::AppState,
    document::{asc_bytes, new_from_template},
    platform_mock::MockPlatform,
};

/// A held connection that replays queued replies and records what the session did to it.
struct FakeStream {
    input: Cursor<Vec<u8>>,
    output: Vec<u8>,
    restarts: usize,
}

impl FakeStream {
    fn from_bytes(input: Vec<u8>) -> Self {
        Self {
            input: Cursor::new(input),
            output: Vec::new(),
            restarts: 0,
        }
    }

    /// A stream that answers each of `replies` in turn.
    fn replying(replies: &[LibraryResponse]) -> Self {
        let mut bytes = Vec::new();
        for reply in replies {
            messages::write_message(&mut bytes, reply).expect("the reply encodes");
        }
        Self::from_bytes(bytes)
    }

    /// A stream with nothing to read: the request goes out, then the read hits the end, as on a
    /// connection that dropped.
    fn dead() -> Self {
        Self::from_bytes(Vec::new())
    }
}

impl Read for FakeStream {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.input.read(buf)
    }
}

impl Write for FakeStream {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.output.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl AnswerDeadline for FakeStream {
    fn restart_deadline(&mut self, _wait: Duration) {
        self.restarts += 1;
    }
}

fn never_cancelled() -> bool {
    false
}

fn platform() -> (Arc<MockPlatform>, Arc<dyn Platform>) {
    let mock = Arc::new(MockPlatform::default());
    let platform: Arc<dyn Platform> = mock.clone();
    (mock, platform)
}

fn paired_settings() -> RemoteSettings {
    RemoteSettings {
        name: "Studio PC".to_owned(),
        address: "127.0.0.1:1".to_owned(),
        enrolled: true,
        ..RemoteSettings::default()
    }
}

#[test]
fn the_query_maps_to_one_search_request() {
    let transport = FakeTransport::new(Vec::new(), Vec::new()).for_browse();
    let query = LibraryQuery {
        text: "brill".to_owned(),
        shape: Some("Round".to_owned()),
        gear: Some(96),
        ri_min: Some(1.5),
        ri_max: Some(1.8),
        mine_only: true,
        tag: Some("favorite".to_owned()),
        show_ignored: true,
        sort: LibrarySort::Newest,
    };
    let found = search_via(&transport, &query).expect("searches");
    assert!(found.cards.is_empty() && !found.capped);
    assert_eq!(
        transport.last_search.borrow().clone(),
        Some(LibraryRequest::Search {
            query: "brill".to_owned(),
            shape_filter: "Round".to_owned(),
            gear_filter: "96".to_owned(),
            range: RangeFilterWire {
                ri_min: Some(1.5),
                ri_max: Some(1.8),
                include_ignored: true,
                ..RangeFilterWire::default()
            },
            order: SortOrderWire::Newest,
            // The computer does not list its tags, and "mine only" has no meaning there.
            tag_filter: None,
        })
    );

    search_via(&transport, &LibraryQuery::default()).expect("searches");
    assert_eq!(
        transport.last_search.borrow().clone(),
        Some(LibraryRequest::Search {
            query: String::new(),
            shape_filter: String::new(),
            gear_filter: String::new(),
            range: RangeFilterWire::default(),
            order: SortOrderWire::CatalogueOrder,
            tag_filter: None,
        })
    );

    let options = filter_options_via(&transport).expect("options");
    assert_eq!(options.shapes, ["Round".to_owned()]);
    assert_eq!(options.gears, [96]);
    assert!(options.tags.is_empty());
}

#[test]
fn a_summary_becomes_a_remote_card() {
    let mut summary = design_summary(7, "Round Brilliant", "https://example.test/7", "v1");
    summary.designer_info = Some("Capps, Jerry; Lapidary Journal".to_owned());
    summary.facets_count = Some("55+6".to_owned());
    summary.ignored = true;
    let remote = card_of_summary(&summary);
    assert_eq!(remote.url, "https://example.test/7");
    assert_eq!(remote.revision, summary.design_version);
    assert_ne!(remote.revision, [0; 32]);
    let card = remote.card;
    assert_eq!((card.id, card.title.as_str()), (7, "Round Brilliant"));
    assert_eq!(card.designer, "Capps, Jerry");
    assert_eq!(
        (card.shape.as_str(), card.gear, card.facets),
        ("Round", 96, 61)
    );
    assert_eq!(card.ri, Some(1.72));
    assert!(card.tags.is_empty());
    assert!(!card.mine && !card.synced && card.ignored);
}

#[test]
fn a_reply_over_the_limit_drops_the_connection() {
    // A length prefix one byte past the limit: refused before any payload is read.
    let prefix = (MAX_LIBRARY_REPLY + 1).to_le_bytes().to_vec();
    let mut held = Some(FakeStream::from_bytes(prefix));
    let dials = Cell::new(0);
    let result = with_reconnect(
        &mut held,
        &LibraryRequest::FilterOptions,
        &never_cancelled,
        || {
            dials.set(dials.get() + 1);
            Ok(FakeStream::dead())
        },
    );
    assert!(
        matches!(&result, Err(RemoteLibraryError::Failed(message)) if message.contains("exceeds max")),
        "{result:?}"
    );
    assert!(held.is_none(), "the connection is dropped");
    assert_eq!(dials.get(), 0, "an oversized frame is not retried");
}

#[test]
fn a_lost_connection_is_redialed_once_then_reported() {
    let request = LibraryRequest::FilterOptions;

    // The held connection is dead; the new one answers.
    let mut held = Some(FakeStream::dead());
    let dials = Cell::new(0);
    let reply = with_reconnect(&mut held, &request, &never_cancelled, || {
        dials.set(dials.get() + 1);
        Ok(FakeStream::replying(&[LibraryResponse::NotFound]))
    });
    assert_eq!(reply, Ok(LibraryResponse::NotFound));
    assert_eq!(dials.get(), 1);
    assert!(held.is_some(), "the new connection is kept");

    // Both are dead: one retry, then the error.
    let mut held = Some(FakeStream::dead());
    let dials = Cell::new(0);
    let result = with_reconnect(&mut held, &request, &never_cancelled, || {
        dials.set(dials.get() + 1);
        Ok(FakeStream::dead())
    });
    assert!(
        matches!(result, Err(RemoteLibraryError::Interrupted(_))),
        "{result:?}"
    );
    assert_eq!(dials.get(), 1, "exactly one retry");
    assert!(held.is_none());

    // The computer cannot be reached again: that is the error.
    let mut held = Some(FakeStream::dead());
    let result = with_reconnect(&mut held, &request, &never_cancelled, || {
        Err::<FakeStream, _>(RemoteLibraryError::Unreachable(
            "Could not reach the computer: refused".to_owned(),
        ))
    });
    assert!(
        matches!(&result, Err(error) if error.is_unreachable()),
        "{result:?}"
    );

    // Nothing is held yet: the first request dials, once.
    let mut held: Option<FakeStream> = None;
    let dials = Cell::new(0);
    let reply = with_reconnect(&mut held, &request, &never_cancelled, || {
        dials.set(dials.get() + 1);
        Ok(FakeStream::replying(&[LibraryResponse::NotFound]))
    });
    assert_eq!(reply, Ok(LibraryResponse::NotFound));
    assert_eq!(dials.get(), 1);
}

#[test]
fn an_error_reply_or_a_cancel_is_not_retried() {
    let request = LibraryRequest::FilterOptions;
    let refusal = LibraryResponse::Error(ErrorMsg {
        code: 1,
        message: "bad filter".to_owned(),
        request_id: None,
    });
    let mut held = Some(FakeStream::replying(&[refusal]));
    let dials = Cell::new(0);
    let result = with_reconnect(&mut held, &request, &never_cancelled, || {
        dials.set(dials.get() + 1);
        Ok(FakeStream::dead())
    });
    assert_eq!(
        result,
        Err(RemoteLibraryError::Failed("bad filter".to_owned()))
    );
    assert_eq!(
        dials.get(),
        0,
        "the computer answered; the connection is fine"
    );
    assert!(held.is_some());

    // A connection that dies because the user cancelled is not dialed again.
    let mut held = Some(FakeStream::dead());
    let result = with_reconnect(&mut held, &request, &|| true, || {
        dials.set(dials.get() + 1);
        Ok(FakeStream::dead())
    });
    assert_eq!(result, Err(RemoteLibraryError::Cancelled));
    assert_eq!(dials.get(), 0);
}

#[test]
fn the_answer_deadline_is_reset_per_request() {
    let both = [LibraryResponse::NotFound, LibraryResponse::NotFound];
    let mut held = Some(FakeStream::replying(&both));
    let request = LibraryRequest::FilterOptions;
    for answered in 1..=2 {
        let reply = with_reconnect(&mut held, &request, &never_cancelled, || {
            Ok(FakeStream::dead())
        });
        assert_eq!(reply, Ok(LibraryResponse::NotFound));
        // Once before the write and once before the read, on every request.
        assert_eq!(
            held.as_ref().map(|stream| stream.restarts),
            Some(2 * answered)
        );
    }
}

#[test]
fn without_credentials_the_browse_says_enroll_first() {
    let (_mock, platform) = platform();
    let library = RemoteLibrary::new(platform, paired_settings());
    let enroll = RemoteLibraryError::Refused("Enroll this phone first.".to_owned());
    assert_eq!(
        library
            .search(&LibraryQuery::default(), &never_cancelled)
            .map(|_| ()),
        Err(enroll.clone())
    );
    assert_eq!(
        library
            .open(&LibraryQuery::default(), &never_cancelled)
            .map(|_| ()),
        Err(enroll.clone())
    );
    assert_eq!(
        library
            .fetch_design(1, "Round", &never_cancelled)
            .map(|_| ()),
        Err(enroll.clone())
    );
    assert_eq!(library.probe(&never_cancelled), Err(enroll.clone()));
    assert_eq!(library.source_id(), Err(enroll));
}

#[test]
fn a_metered_network_refuses_a_sync_unless_allowed() {
    let (mock, platform) = platform();
    let library = RemoteLibrary::new(platform.clone(), paired_settings());
    assert_eq!(
        library.sync_refusal(),
        None,
        "an unknown network is unmetered"
    );
    mock.set_network_metered(Some(false));
    assert_eq!(library.sync_refusal(), None);

    mock.set_network_metered(Some(true));
    let refusal = library.sync_refusal().expect("metered, not allowed");
    assert!(
        refusal.contains("metered") && refusal.contains("Wi-Fi"),
        "{refusal}"
    );
    assert_eq!(refusal, METERED_SYNC_REFUSAL);

    let allowed = RemoteLibrary::new(
        platform,
        RemoteSettings {
            allow_metered: true,
            ..paired_settings()
        },
    );
    assert_eq!(allowed.sync_refusal(), None);
}

#[test]
fn the_computer_has_a_name_and_every_failure_a_sentence() {
    let (_mock, platform) = platform();
    assert_eq!(
        RemoteLibrary::new(platform.clone(), paired_settings()).computer_name(),
        "Studio PC"
    );
    let unnamed = RemoteLibrary::new(platform, RemoteSettings::default());
    assert_eq!(unnamed.computer_name(), "Computer");

    let unreachable = classify("Could not reach the computer: timed out");
    assert!(unreachable.is_unreachable());
    assert!(is_unreachable(&unreachable.message()));
    assert_eq!(
        classify("Connection cancelled."),
        RemoteLibraryError::Cancelled
    );
    assert_eq!(
        classify("Enroll this phone first."),
        RemoteLibraryError::Refused("Enroll this phone first.".to_owned())
    );
    for error in [
        RemoteLibraryError::NoLibrary,
        RemoteLibraryError::Cancelled,
        RemoteLibraryError::Failed("x".to_owned()),
    ] {
        assert!(!error.message().is_empty());
        assert_eq!(error.to_string(), error.message());
    }
    let gone = from_browse(BrowseError::NotFound, "Studio PC");
    assert_eq!(gone.message(), "That design is no longer on Studio PC.");
}

/// The first template as `.asc` text, with the name the app gives it.
fn template_asc() -> (String, String) {
    let (_mock, platform) = platform();
    let mut state = AppState::new(platform);
    new_from_template(&mut state, 1).expect("template 1 exists");
    let file = asc_bytes(&state).expect("exports").file;
    (
        file.suggested_name,
        String::from_utf8(file.bytes).expect("the .asc is text"),
    )
}

#[test]
fn a_frosted_asc_opens_as_a_design_file() {
    let (name, text) = template_asc();

    // Without frosting the .asc opens as it is.
    let plain = design_file_bytes(
        DesignFile::Asc {
            file_name: name.clone(),
            text: text.clone(),
            frosting: Vec::new(),
        },
        "Round",
    )
    .expect("opens");
    assert_eq!(plain.file_name, name);
    assert_eq!(plain.bytes, text.as_bytes());

    // With frosting it becomes a native design file that states it, named after the title.
    let frosted = design_file_bytes(
        DesignFile::Asc {
            file_name: name,
            text,
            frosting: vec![StoredTierFrosting::whole(1)],
        },
        "Frosted Round",
    )
    .expect("opens");
    assert!(
        std::path::Path::new(&frosted.file_name)
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("psdd")),
        "{}",
        frosted.file_name
    );
    assert!(
        frosted.file_name.starts_with("Frosted Round"),
        "{}",
        frosted.file_name
    );
    let design = design_from_str(std::str::from_utf8(&frosted.bytes).expect("text"))
        .expect("the design file reads")
        .design;
    assert_eq!(design.tier_frosting(1), Some(&TierFrosting::Tier));
}

#[test]
fn a_native_design_file_opens_as_it_came_unless_it_is_too_large() {
    let native = DesignFile::Native {
        file_name: "round.psdd".to_owned(),
        bytes: b"design bytes".to_vec(),
    };
    let opened = design_file_bytes(native, "Round").expect("opens");
    assert_eq!(
        (opened.file_name.as_str(), opened.bytes.as_slice()),
        ("round.psdd", b"design bytes".as_slice())
    );

    // The phone opens no design file over 16 MiB, so it says so instead of downloading for
    // nothing and failing in the editor.
    let big = DesignFile::Native {
        file_name: "big.psdd".to_owned(),
        bytes: vec![0_u8; 16 * 1024 * 1024 + 1],
    };
    assert_eq!(
        design_file_bytes(big, "Big Round"),
        Err("Big Round is too large to open on this phone.".to_owned())
    );
}
