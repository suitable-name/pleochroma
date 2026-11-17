//! The paired computer's design library, seen from the phone: live browsing (search, filter
//! options, opening one design) and the held connection a sync copies designs over.
//!
//! This module is a child of `remote`, so it dials with the same private `connect` and
//! `DeadlineStream` the render sessions use: one authenticated connection per browse call, and
//! one held connection for a whole sync. The requests and the single-screen helpers come from
//! `pleochroma_library_mirror` ([`browse`] and the [`LibraryTransport`] the sync runs over);
//! this module supplies the transport and the phone's reading of a failure.
//!
//! # Deadlines and limits
//!
//! `DeadlineStream` has a deadline that only a write moves. A held library session moves it
//! itself, before writing and again before reading, so a request that waits for its answer for up
//! to `LIBRARY_ANSWER_WAIT` never inherits the 12 seconds the handshake left. A reply is read
//! with `read_message_bounded` at `MAX_LIBRARY_REPLY`: a computer cannot make the phone
//! allocate half a gigabyte by sending a long length prefix.
//!
//! # A lost connection
//!
//! A held connection breaks sooner or later in a sync of hundreds of designs. A request that
//! fails because the connection is gone drops it, dials once more and sends the same request
//! again (the policy of Cut's `request_with_reconnect`, here generic over the stream so it can be
//! tested). A reply the computer formed and a frame over the limit are not retried.

use super::{
    CONNECTION_CANCELLED, DeadlineStream, connect, is_unreachable,
    rendering::{RunError, client_error, net_error},
    saved_ca_tags,
};
use crate::{
    library::{FilterOptions, LibraryCard, LibraryDesign, LibraryQuery, design_file_bytes},
    platform::Platform,
    settings::{LibrarySort, RemoteSettings},
};
use pleochroma_library_mirror::{
    LibraryTransport,
    browse::{self, BrowseError, SearchSpec},
};
use pleochroma_net::{
    client,
    library::{DesignSummary, LibraryRequest, LibraryResponse, RangeFilterWire, SortOrderWire},
    messages,
};
use pleochroma_vault::model::facets::parse_facets_count;
use std::{
    fmt,
    io::{Read, Write},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::{Duration, Instant},
};

/// How long one library request waits for its answer. A design list or one design file can take
/// a while on a slow network, but a computer that says nothing for this long is gone.
const LIBRARY_ANSWER_WAIT: Duration = Duration::from_secs(60);

/// The largest reply frame the phone reads from the library: 64 MiB, far above one search page
/// or one design file (the phone itself opens nothing over 16 MiB).
const MAX_LIBRARY_REPLY: u32 = 64 * 1024 * 1024;

/// The sentence for a sync on a metered network the user has not allowed. Also what
/// `SyncOutcome::summary` shows for it, word for word.
pub const METERED_SYNC_REFUSAL: &str = "This network is metered. Connect to Wi-Fi to sync, or turn on Allow Metered Networks in Remote settings.";

/// Why a request to the computer's library did not produce its result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteLibraryError {
    /// No saved address answered. The text starts with [`super::UNREACHABLE`], so
    /// [`super::is_unreachable`] recognizes it and the app looks for the computer again.
    Unreachable(String),
    /// The computer answered but does not serve a design library (any more).
    NoLibrary,
    /// The connection was refused: no credentials, a failed handshake, or a computer that no
    /// longer accepts this phone. Carries the sentence.
    Refused(String),
    /// The connection was lost in the middle of a request, and the one retry failed too.
    Interrupted(String),
    /// The computer answered with an error, an unexpected message or a frame over the limit.
    Failed(String),
    /// The caller's cancel check fired.
    Cancelled,
}

impl RemoteLibraryError {
    /// The sentence for the user.
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::Unreachable(message)
            | Self::Refused(message)
            | Self::Interrupted(message)
            | Self::Failed(message) => message.clone(),
            Self::NoLibrary => "This computer does not share a design library.".to_owned(),
            Self::Cancelled => "Cancelled.".to_owned(),
        }
    }

    /// Whether no saved address answered (as [`super::is_unreachable`] reads a message).
    #[must_use]
    pub const fn is_unreachable(&self) -> bool {
        matches!(self, Self::Unreachable(_))
    }
}

impl fmt::Display for RemoteLibraryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message())
    }
}

/// How a message of [`connect`] reads as a library error.
fn classify(message: &str) -> RemoteLibraryError {
    if is_unreachable(message) {
        RemoteLibraryError::Unreachable(message.to_owned())
    } else if message == CONNECTION_CANCELLED {
        RemoteLibraryError::Cancelled
    } else {
        RemoteLibraryError::Refused(message.to_owned())
    }
}

/// One page of the computer's search result, as cards.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteSearch {
    /// The matching designs, in the requested order.
    pub cards: Vec<RemoteCard>,
    /// Whether the computer's row cap was reached, so more designs may match.
    pub capped: bool,
}

/// One design on the computer, as the library list shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteCard {
    /// The card. `id` is the computer's entry id (not an entry of this phone's library), `tags`
    /// is empty (the computer does not list them) and `mine` and `synced` are `false`: the app
    /// marks the designs it already copied by their `url`.
    pub card: LibraryCard,
    /// The design's identity (`diagram_entries.url`), the key a sync copies it under.
    pub url: String,
    /// The design's revision token; all zero bytes when the computer states none.
    pub revision: [u8; 32],
}

/// The computer's design library: the phone's saved pairing plus the platform that holds its
/// credentials. Cheap to clone and `Send`, so a worker thread can own one; every call dials for
/// itself.
#[derive(Clone)]
pub struct RemoteLibrary {
    platform: Arc<dyn Platform>,
    settings: RemoteSettings,
}

impl RemoteLibrary {
    /// The library of the computer `settings` describes.
    #[must_use]
    pub fn new(platform: Arc<dyn Platform>, settings: RemoteSettings) -> Self {
        Self { platform, settings }
    }

    /// The computer's name as the user gave or got it; "Computer" when it has none.
    #[must_use]
    pub fn computer_name(&self) -> String {
        let name = self.settings.name.trim();
        if name.is_empty() {
            "Computer".to_owned()
        } else {
            name.to_owned()
        }
    }

    /// The `source_id` the designs copied from this computer carry:
    /// `remote-library:ca-<tag>`, with the first discovery tag of the pairing's CA. The CA
    /// stays the same when the computer is found again at other addresses.
    ///
    /// # Errors
    ///
    /// [`RemoteLibraryError::Refused`] when no credentials are stored or they hold no CA.
    pub fn source_id(&self) -> Result<String, RemoteLibraryError> {
        let tags = saved_ca_tags(self.platform.as_ref()).map_err(RemoteLibraryError::Refused)?;
        let tag = tags.first().ok_or_else(|| {
            RemoteLibraryError::Refused("The credential bundle contains no CA certificate.".into())
        })?;
        Ok(pleochroma_library_mirror::source_id(&format!("ca-{tag}")))
    }

    /// Whether the computer serves a design library: one connection and handshake, no request.
    ///
    /// # Errors
    ///
    /// [`RemoteLibraryError::Unreachable`] when no address answers, [`RemoteLibraryError::Refused`]
    /// for a refused connection, [`RemoteLibraryError::Cancelled`] when `cancelled` fires.
    pub fn probe(&self, cancelled: &dyn Fn() -> bool) -> Result<bool, RemoteLibraryError> {
        connect(
            self.platform.as_ref(),
            &self.settings.endpoint_addresses(),
            cancelled,
        )
        .map(|(_stream, welcome)| welcome.library)
        .map_err(|message| classify(&message))
    }

    /// The computer's filter options and the first search on ONE connection: what switching to
    /// the computer's library needs.
    ///
    /// # Errors
    ///
    /// As [`Self::search`].
    pub fn open(
        &self,
        query: &LibraryQuery,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<(FilterOptions, RemoteSearch), RemoteLibraryError> {
        let session = self.session(cancelled);
        let name = self.computer_name();
        let options = filter_options_via(&session).map_err(|error| from_browse(error, &name))?;
        let found = search_via(&session, query).map_err(|error| from_browse(error, &name))?;
        Ok((options, found))
    }

    /// Searches the computer's library. The query maps as follows: the text, shape and gear as
    /// they are, the refractive index band, "show ignored" and the sort order. The tag is not
    /// sent (the computer does not list its tags) and "mine only" is ignored.
    ///
    /// # Errors
    ///
    /// [`RemoteLibraryError::Unreachable`], [`RemoteLibraryError::NoLibrary`],
    /// [`RemoteLibraryError::Refused`], [`RemoteLibraryError::Interrupted`],
    /// [`RemoteLibraryError::Failed`] or [`RemoteLibraryError::Cancelled`].
    pub fn search(
        &self,
        query: &LibraryQuery,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<RemoteSearch, RemoteLibraryError> {
        let session = self.session(cancelled);
        let name = self.computer_name();
        search_via(&session, query).map_err(|error| from_browse(error, &name))
    }

    /// The design file of entry `remote_id` (the computer's id), ready to open: its `.psdd`, else
    /// its `.asc`. A frosted `.asc` comes back as a `<title>.psdd`, so the frosting reaches the
    /// editor, as `LibraryHandle::load` does for a copied design.
    ///
    /// # Errors
    ///
    /// As [`Self::search`], and [`RemoteLibraryError::Failed`] with "<title> is too large to open
    /// on this phone." when the file is over the 16 MiB a design file may have.
    pub fn fetch_design(
        &self,
        remote_id: i64,
        title: &str,
        cancelled: &dyn Fn() -> bool,
    ) -> Result<LibraryDesign, RemoteLibraryError> {
        let session = self.session(cancelled);
        let name = self.computer_name();
        let file = browse::fetch_design_file(&session, remote_id)
            .map_err(|error| from_browse(error, &name))?;
        design_file_bytes(file, title).map_err(RemoteLibraryError::Failed)
    }

    /// Why a sync must not start now: the network is metered and the user has not allowed that.
    /// `None` when it may go on.
    #[must_use]
    pub fn sync_refusal(&self) -> Option<String> {
        (!self.settings.allow_metered && self.platform.network_metered() == Some(true))
            .then(|| METERED_SYNC_REFUSAL.to_owned())
    }

    /// A session that holds one connection for many requests. It dials on the first request.
    pub(crate) fn session<'a>(
        &'a self,
        cancelled: &'a dyn Fn() -> bool,
    ) -> PhoneLibrarySession<'a> {
        PhoneLibrarySession {
            platform: self.platform.as_ref(),
            addresses: self.settings.endpoint_addresses(),
            cancelled,
            held: Mutex::new(None),
            last_error: Mutex::new(None),
        }
    }
}

/// `query` as one search request, and the reply as cards.
fn search_via<T: LibraryTransport>(
    transport: &T,
    query: &LibraryQuery,
) -> Result<RemoteSearch, BrowseError<T::Error>> {
    let gear = query.gear.map(|gear| gear.to_string()).unwrap_or_default();
    let spec = SearchSpec {
        text: &query.text,
        shape: query.shape.as_deref().unwrap_or_default(),
        gear: &gear,
        range: RangeFilterWire {
            ri_min: query.ri_min,
            ri_max: query.ri_max,
            include_ignored: query.show_ignored,
            ..RangeFilterWire::default()
        },
        order: order_of(query.sort),
        tag: None,
    };
    let found = browse::search(transport, &spec)?;
    Ok(RemoteSearch {
        cards: found.items.iter().map(card_of_summary).collect(),
        capped: found.capped,
    })
}

/// The computer's shapes and gears as the filter sheet takes them; it lists no tags.
fn filter_options_via<T: LibraryTransport>(
    transport: &T,
) -> Result<FilterOptions, BrowseError<T::Error>> {
    let remote = browse::filter_options(transport)?;
    let mut gears: Vec<u32> = remote
        .gears
        .iter()
        .filter_map(|gear| gear.trim().parse::<u32>().ok())
        .collect();
    gears.sort_unstable();
    gears.dedup();
    Ok(FilterOptions {
        shapes: remote.shapes,
        gears,
        tags: Vec::new(),
    })
}

/// The wire order for a library sort setting.
const fn order_of(sort: LibrarySort) -> SortOrderWire {
    match sort {
        LibrarySort::CatalogueOrder => SortOrderWire::CatalogueOrder,
        LibrarySort::Title => SortOrderWire::Title,
        LibrarySort::Newest => SortOrderWire::Newest,
        LibrarySort::RecentlyEdited => SortOrderWire::RecentlyEdited,
    }
}

/// The card for one search row of the computer.
fn card_of_summary(summary: &DesignSummary) -> RemoteCard {
    let (main, girdle) = parse_facets_count(summary.facets_count.as_deref());
    let facet_total = main.unwrap_or(0).saturating_add(girdle.unwrap_or(0));
    let designer = summary
        .designer_info
        .as_deref()
        .and_then(|line| line.split(';').next())
        .map(str::trim)
        .unwrap_or_default()
        .to_owned();
    RemoteCard {
        card: LibraryCard {
            id: summary.entry_id,
            title: summary.title.clone(),
            designer,
            shape: summary.shape.clone().unwrap_or_default(),
            gear: summary
                .index_gear
                .as_deref()
                .and_then(|gear| gear.trim().parse::<u32>().ok())
                .unwrap_or(0),
            facets: u32::try_from(facet_total).unwrap_or(0),
            ri: summary
                .refractive_index
                .as_deref()
                .and_then(|ri| ri.trim().parse::<f64>().ok()),
            tags: Vec::new(),
            mine: false,
            synced: false,
            ignored: summary.ignored,
            updated_unix: 0,
        },
        url: summary.url.clone(),
        revision: summary.design_version,
    }
}

/// A browse failure as the sentence for the user; `computer` is the computer's name.
fn from_browse(error: BrowseError<RemoteLibraryError>, computer: &str) -> RemoteLibraryError {
    match error {
        BrowseError::Transport(error) => error,
        BrowseError::NotFound => {
            RemoteLibraryError::Failed(format!("That design is no longer on {computer}."))
        }
        BrowseError::NoDesignFile => {
            RemoteLibraryError::Failed("This design has no design file to open.".to_owned())
        }
        BrowseError::Worker(message) => RemoteLibraryError::Failed(message),
        BrowseError::Unexpected(request) => RemoteLibraryError::Failed(format!(
            "{computer} answered {request} with something this app does not understand."
        )),
    }
}

/// A stream whose answer deadline the session moves itself.
trait AnswerDeadline {
    /// Gives the next read or write `wait` from now, whatever the deadline said before.
    fn restart_deadline(&mut self, wait: Duration);
}

impl AnswerDeadline for DeadlineStream<'_> {
    fn restart_deadline(&mut self, wait: Duration) {
        self.deadline = Instant::now() + wait;
    }
}

/// Why one request-and-reply exchange did not produce a reply.
enum ExchangeError {
    /// The connection is gone (a socket error, a cut frame, a deadline, a cancel).
    Lost(String),
    /// The reply is wrong in itself (over the limit, not decodable).
    Protocol(String),
    /// The computer replied with an error message. The connection is fine.
    Worker(String),
}

impl ExchangeError {
    /// The error a caller sees when no retry follows.
    fn into_error(self) -> RemoteLibraryError {
        match self {
            Self::Lost(message) => RemoteLibraryError::Interrupted(message),
            Self::Protocol(message) | Self::Worker(message) => RemoteLibraryError::Failed(message),
        }
    }
}

impl From<RunError> for ExchangeError {
    fn from(error: RunError) -> Self {
        match error {
            RunError::Interrupted(message) => Self::Lost(message),
            RunError::Failed(message) => Self::Protocol(message),
        }
    }
}

/// Sends `request` and reads exactly one reply, moving the deadline before each half.
fn exchange<S: Read + Write + AnswerDeadline>(
    stream: &mut S,
    request: &LibraryRequest,
) -> Result<LibraryResponse, ExchangeError> {
    stream.restart_deadline(LIBRARY_ANSWER_WAIT);
    client::send_library_request(stream, request)
        .map_err(|error| ExchangeError::from(client_error(&error)))?;
    stream
        .flush()
        .map_err(|error| ExchangeError::Lost(error.to_string()))?;
    stream.restart_deadline(LIBRARY_ANSWER_WAIT);
    let reply: LibraryResponse = messages::read_message_bounded(stream, MAX_LIBRARY_REPLY)
        .map_err(|error| ExchangeError::from(net_error(&error)))?;
    match reply {
        LibraryResponse::Error(error) => Err(ExchangeError::Worker(error.message)),
        other => Ok(other),
    }
}

/// Runs `request` on the held stream, dialing first when nothing is held.
///
/// A good reply, and an error reply of the computer, keep the stream for the next request. A
/// lost connection is dropped, `connect` is called once more and the same request goes out again;
/// that second failure is reported as it is. A frame over the limit (or any other protocol
/// failure) drops the stream and is not retried. When `cancelled` fires, nothing is retried.
fn with_reconnect<S: Read + Write + AnswerDeadline>(
    held: &mut Option<S>,
    request: &LibraryRequest,
    cancelled: &dyn Fn() -> bool,
    mut connect: impl FnMut() -> Result<S, RemoteLibraryError>,
) -> Result<LibraryResponse, RemoteLibraryError> {
    let mut stream = if let Some(stream) = held.take() {
        stream
    } else {
        connect()?
    };
    match exchange(&mut stream, request) {
        Ok(reply) => {
            *held = Some(stream);
            Ok(reply)
        }
        Err(ExchangeError::Worker(message)) => {
            *held = Some(stream);
            Err(RemoteLibraryError::Failed(message))
        }
        Err(_) if cancelled() => Err(RemoteLibraryError::Cancelled),
        Err(error @ ExchangeError::Protocol(_)) => Err(error.into_error()),
        Err(ExchangeError::Lost(_)) => {
            drop(stream);
            let mut fresh = connect()?;
            let reply = exchange(&mut fresh, request).map_err(ExchangeError::into_error)?;
            *held = Some(fresh);
            Ok(reply)
        }
    }
}

/// The lock of `mutex`; a call that panicked while holding it does not poison the session.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// One connection to the computer's library held for many requests: what a sync runs over.
///
/// It borrows the platform (for the credentials) and the caller's cancel check, so it lives on
/// the thread that made it; it is not shared between threads. The first request dials and checks
/// that the computer serves a library. See the module docs for the deadline and reconnect
/// rules.
pub(crate) struct PhoneLibrarySession<'a> {
    platform: &'a dyn Platform,
    addresses: Vec<String>,
    cancelled: &'a dyn Fn() -> bool,
    held: Mutex<Option<DeadlineStream<'a>>>,
    last_error: Mutex<Option<RemoteLibraryError>>,
}

impl PhoneLibrarySession<'_> {
    /// The error of the most recent request that failed, if any. After a failed sync it is the
    /// reason: the sentence the user reads comes from it.
    pub(crate) fn last_error(&self) -> Option<RemoteLibraryError> {
        lock(&self.last_error).clone()
    }
}

impl<'a> PhoneLibrarySession<'a> {
    /// Dials the computer and checks that it serves a design library.
    fn dial(&self) -> Result<DeadlineStream<'a>, RemoteLibraryError> {
        match connect(self.platform, &self.addresses, self.cancelled) {
            Ok((stream, welcome)) if welcome.library => Ok(stream),
            Ok(_) => Err(RemoteLibraryError::NoLibrary),
            Err(message) => Err(classify(&message)),
        }
    }

    /// One request on the held stream.
    fn request_held(
        &self,
        request: &LibraryRequest,
    ) -> Result<LibraryResponse, RemoteLibraryError> {
        let mut held = lock(&self.held);
        with_reconnect(&mut held, request, self.cancelled, || self.dial())
    }
}

impl LibraryTransport for PhoneLibrarySession<'_> {
    type Error = RemoteLibraryError;

    fn request(&self, request: &LibraryRequest) -> Result<LibraryResponse, RemoteLibraryError> {
        let result = self.request_held(request);
        if let Err(error) = &result {
            *lock(&self.last_error) = Some(error.clone());
        }
        result
    }
}

#[cfg(test)]
mod tests;
