//! A scripted, in-memory [`LibraryTransport`] and the design/summary/database fixtures the
//! mirror-sync tests build against -- no socket, no TLS, no real `pleochroma-worker`.
//!
//! Compiled for this crate's own tests and, behind the `test-support` feature, for the tests of
//! the crates that use it.

use crate::transport::LibraryTransport;
use pleochroma_net::{
    client::ClientError,
    library::{
        AngleSettingWire, AttributeRangesWire, DesignRecord, DesignSummary, FrostedTierWire,
        LibraryRequest, LibraryResponse,
    },
    messages::{ErrorMsg, error_codes},
};
use pleochroma_vault::db::sqlite::Database;
use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};

/// A native design file the fake serves for one entry.
struct NativeFile {
    file_name: String,
    content: Vec<u8>,
}

/// An `.asc` the fake serves for one entry.
struct SourceFile {
    file_name: String,
    asc_text: String,
    frosting: Vec<FrostedTierWire>,
}

/// A scripted, in-memory [`LibraryTransport`] with no socket, TLS or real worker.
///
/// It counts each
/// request kind, so tests can assert exact call counts. It pages `search_result` at
/// `page_size` rows per `SearchPage` reply, so a multi-page walk needs only a few rows.
///
/// By default it answers only what a mirror sync sends (`SearchPage`, `FetchDesign`,
/// `FetchAttachment`) and panics on anything else, so the mirror tests still assert that a sync
/// never sends `Search`. [`Self::for_browse`] makes it answer the browse requests too.
pub struct FakeTransport {
    search_result: Vec<DesignSummary>,
    page_size: usize,
    designs: HashMap<i64, DesignRecord>,
    attachments: HashMap<i64, (String, Vec<u8>)>,
    natives: HashMap<i64, NativeFile>,
    sources: HashMap<i64, SourceFile>,
    browse: bool,
    /// When `Some`, every request is answered with a [`LibraryResponse::Error`] carrying this
    /// message.
    error_reply: Option<String>,
    on_fetch_design: Option<Box<dyn Fn(i64)>>,
    /// When `Some(id)`, [`LibraryRequest::FetchDesign`] for that entry id returns a
    /// transport-level [`ClientError::Net`] error instead of a reply. This stands in for a held
    /// connection that dropped and could not be re-established, so the error reaches the sync as
    /// a transport failure, not as [`LibraryResponse::NotFound`].
    fail_fetch_design_for: Option<i64>,
    /// How many `SearchPage` requests were answered.
    pub search_calls: AtomicUsize,
    /// How many `FetchDesign` requests were answered.
    pub fetch_design_calls: AtomicUsize,
    /// How many `FetchAttachment` requests were answered.
    pub fetch_attachment_calls: AtomicUsize,
    /// The entry ids [`LibraryRequest::FetchDesign`] was actually called for -- lets a test
    /// assert WHICH designs were (not) re-fetched, not just a count.
    pub fetched_entry_ids: RefCell<Vec<i64>>,
    /// The last [`LibraryRequest::Search`] received (browse mode).
    pub last_search: RefCell<Option<LibraryRequest>>,
}

impl FakeTransport {
    /// A fake whose catalogue is `summaries`, with `designs` as the records `FetchDesign`
    /// returns.
    #[must_use]
    pub fn new(summaries: Vec<DesignSummary>, designs: Vec<DesignRecord>) -> Self {
        Self {
            search_result: summaries,
            page_size: usize::MAX,
            designs: designs.into_iter().map(|d| (d.entry_id, d)).collect(),
            attachments: HashMap::new(),
            natives: HashMap::new(),
            sources: HashMap::new(),
            browse: false,
            error_reply: None,
            on_fetch_design: None,
            fail_fetch_design_for: None,
            search_calls: AtomicUsize::new(0),
            fetch_design_calls: AtomicUsize::new(0),
            fetch_attachment_calls: AtomicUsize::new(0),
            fetched_entry_ids: RefCell::new(Vec::new()),
            last_search: RefCell::new(None),
        }
    }

    /// Serves `content` as attachment `id`.
    #[must_use]
    pub fn with_attachment(mut self, id: i64, name: &str, content: Vec<u8>) -> Self {
        self.attachments.insert(id, (name.to_string(), content));
        self
    }

    /// Shrinks how many rows one `SearchPage` reply hands back, so that the catalogue walk makes
    /// more than one round trip.
    #[must_use]
    pub const fn with_page_size(mut self, page_size: usize) -> Self {
        self.page_size = page_size;
        self
    }

    /// Makes [`LibraryRequest::FetchDesign`] for `entry_id` fail with a transport-level error --
    /// see the field's own doc comment for what this simulates.
    #[must_use]
    pub const fn with_fetch_design_failure(mut self, entry_id: i64) -> Self {
        self.fail_fetch_design_for = Some(entry_id);
        self
    }

    /// Serves `bytes` as `entry_id`'s native design file (`FetchDesignNative`). Needs
    /// [`Self::for_browse`].
    #[must_use]
    pub fn with_native(mut self, entry_id: i64, file_name: &str, bytes: Vec<u8>) -> Self {
        self.natives.insert(
            entry_id,
            NativeFile {
                file_name: file_name.to_string(),
                content: bytes,
            },
        );
        self
    }

    /// Serves `entry_id`'s `.asc` and its frosting (`FetchDesignSource`). Needs
    /// [`Self::for_browse`].
    #[must_use]
    pub fn with_source(
        mut self,
        entry_id: i64,
        file_name: &str,
        asc_text: &str,
        frosting: Vec<FrostedTierWire>,
    ) -> Self {
        self.sources.insert(
            entry_id,
            SourceFile {
                file_name: file_name.to_string(),
                asc_text: asc_text.to_string(),
                frosting,
            },
        );
        self
    }

    /// Calls `hook` with the entry id each time `FetchDesign` is answered, after the request is
    /// counted and before the reply is returned. A test uses it to do something to the database
    /// while a design is being fetched.
    #[must_use]
    pub fn with_on_fetch_design(mut self, hook: Box<dyn Fn(i64)>) -> Self {
        self.on_fetch_design = Some(hook);
        self
    }

    /// Lets the fake answer the browse requests: `Search`, `FilterOptions`,
    /// `FetchDesignNative` and `FetchDesignSource`.
    #[must_use]
    pub const fn for_browse(mut self) -> Self {
        self.browse = true;
        self
    }

    /// Answers every request with a worker error carrying `message`.
    #[must_use]
    pub fn with_error_reply(mut self, message: &str) -> Self {
        self.error_reply = Some(message.to_string());
        self
    }
}

impl LibraryTransport for FakeTransport {
    type Error = ClientError;

    fn request(&self, req: &LibraryRequest) -> Result<LibraryResponse, ClientError> {
        if let Some(message) = &self.error_reply {
            return Ok(LibraryResponse::Error(ErrorMsg {
                code: error_codes::LIBRARY_FAILED,
                message: message.clone(),
                request_id: None,
            }));
        }
        match req {
            LibraryRequest::Search { .. } => {
                assert!(
                    self.browse,
                    "mirror sync always pages via SearchPage now, never sends Search directly"
                );
                *self.last_search.borrow_mut() = Some(req.clone());
                Ok(LibraryResponse::SearchResults {
                    items: self.search_result.clone(),
                    excluded_for_missing_curves: 0,
                })
            }
            LibraryRequest::SearchPage { cursor, .. } => Ok(self.answer_search_page(*cursor)),
            LibraryRequest::FetchDesign { entry_id } => self.answer_fetch_design(*entry_id),
            LibraryRequest::FetchAttachment { attachment_id } => {
                self.fetch_attachment_calls.fetch_add(1, Ordering::Relaxed);
                match self.attachments.get(attachment_id) {
                    Some((name, content)) => Ok(LibraryResponse::Attachment {
                        name: name.clone(),
                        content: content.clone(),
                    }),
                    None => Ok(LibraryResponse::NotFound),
                }
            }
            LibraryRequest::FilterOptions => {
                assert!(self.browse, "mirror sync never sends this");
                Ok(Self::filter_options())
            }
            LibraryRequest::FetchDesignSource { entry_id } => {
                assert!(
                    self.browse,
                    "mirror sync never sends this -- only the editor's remote 'Load Selected' does"
                );
                Ok(self.design_source(*entry_id))
            }
            LibraryRequest::FetchDesignNative { entry_id } => {
                assert!(
                    self.browse,
                    "mirror sync never sends this -- only the editor's remote 'Load Selected' does"
                );
                Ok(self.design_native(*entry_id))
            }
        }
    }
}

impl FakeTransport {
    /// The `SearchPage` reply for the page after `cursor`.
    fn answer_search_page(&self, cursor: Option<i64>) -> LibraryResponse {
        self.search_calls.fetch_add(1, Ordering::Relaxed);
        let start = cursor.map_or(0, |after| {
            self.search_result
                .iter()
                .position(|s| s.entry_id == after)
                .map_or(self.search_result.len(), |i| i + 1)
        });
        let end = start
            .saturating_add(self.page_size)
            .min(self.search_result.len());
        let page = self.search_result[start..end].to_vec();
        // Mirrors the worker's rule (`serve::library::search_page`): a full page
        // may be followed by more, a short page is the last one.
        let next_cursor = if page.len() == self.page_size {
            page.last().map(|s| s.entry_id)
        } else {
            None
        };
        LibraryResponse::SearchResultsPage {
            results: page,
            next_cursor,
            // No performance filter is modelled, so nothing is excluded.
            excluded_for_missing_curves: 0,
        }
    }

    /// The `FetchDesign` reply, or the scripted connection failure.
    fn answer_fetch_design(&self, entry_id: i64) -> Result<LibraryResponse, ClientError> {
        self.fetch_design_calls.fetch_add(1, Ordering::Relaxed);
        self.fetched_entry_ids.borrow_mut().push(entry_id);
        if let Some(hook) = &self.on_fetch_design {
            hook(entry_id);
        }
        if self.fail_fetch_design_for == Some(entry_id) {
            return Err(ClientError::Net(
                pleochroma_net::messages::NetError::Framing(
                    pleochroma_net::framing::FramingError::Io(std::io::Error::new(
                        std::io::ErrorKind::ConnectionReset,
                        "connection dropped and could not be re-established",
                    )),
                ),
            ));
        }
        Ok(self
            .designs
            .get(&entry_id)
            .map_or(LibraryResponse::NotFound, |d| {
                LibraryResponse::Design(Box::new(d.clone()))
            }))
    }

    /// The fixed `FilterOptions` reply.
    fn filter_options() -> LibraryResponse {
        LibraryResponse::FilterOptions {
            shapes: vec!["Round".to_string()],
            gears: vec!["96".to_string()],
            ranges: AttributeRangesWire {
                ri: (1.4, 2.4),
                lw_ratio: (0.8, 3.0),
                volume: (0.4, 0.9),
                facets: (8, 120),
            },
        }
    }

    /// The `FetchDesignSource` reply.
    fn design_source(&self, entry_id: i64) -> LibraryResponse {
        match self.sources.get(&entry_id) {
            Some(source) => LibraryResponse::DesignSource {
                entry_id,
                file_name: source.file_name.clone(),
                asc_text: source.asc_text.clone(),
                frosting: source.frosting.clone(),
            },
            None if self.designs.contains_key(&entry_id) => {
                LibraryResponse::DesignSourceNotAvailable
            }
            None => LibraryResponse::NotFound,
        }
    }

    /// The `FetchDesignNative` reply.
    fn design_native(&self, entry_id: i64) -> LibraryResponse {
        match self.natives.get(&entry_id) {
            Some(native) => LibraryResponse::DesignNative {
                entry_id,
                file_name: native.file_name.clone(),
                content: native.content.clone(),
            },
            None if self.designs.contains_key(&entry_id) => {
                LibraryResponse::DesignNativeNotAvailable
            }
            None => LibraryResponse::NotFound,
        }
    }
}

/// A fresh, empty temp database for one test.
///
/// The file name holds the process id and a counter. A file left behind under the same name
/// is removed first, so a stale database never leaks into a test. The caller removes the file
/// when the test is done.
///
/// # Panics
///
/// When the database cannot be created in the temp directory.
#[must_use]
pub fn temp_db() -> (Arc<Mutex<Database>>, std::path::PathBuf) {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let path = std::env::temp_dir().join(format!(
        "pleochroma-library-mirror-test-{}-{n}.sqlite",
        std::process::id()
    ));
    let _ = std::fs::remove_file(&path);
    let db = Database::new(Some(path.to_str().unwrap())).unwrap();
    (Arc::new(Mutex::new(db)), path)
}

/// [`design_record_at`] at revision `0`.
#[must_use]
pub fn design_record(entry_id: i64, title: &str, url: &str, extra_field: &str) -> DesignRecord {
    design_record_at(entry_id, title, url, extra_field, 0)
}

/// A record at `revision`.
///
/// Its `version` is the revision token of `url` at that
/// revision, the same value a summary from [`design_summary_at`] at the same revision
/// carries as `design_version` -- as the real worker stamps both from one stamp.
#[must_use]
pub fn design_record_at(
    entry_id: i64,
    title: &str,
    url: &str,
    extra_field: &str,
    revision: u32,
) -> DesignRecord {
    DesignRecord {
        entry_id,
        title: title.to_string(),
        url: url.to_string(),
        design_id: Some(format!("D-{entry_id}")),
        page_url: url.to_string(),
        diagram_image_name: None,
        diagram_image_data: None,
        competition_diagram: None,
        lw_ratio: Some(extra_field.to_string()),
        refractive_index: Some("1.72".to_string()),
        index_gear: Some("96".to_string()),
        volume: Some("0.65".to_string()),
        facets_count: Some("57".to_string()),
        shape: Some("Round".to_string()),
        designer_info: Some("Capps, Jerry".to_string()),
        // No previews generated yet.
        preview_material: None,
        // Not exercised by these tests.
        hw_ratio: None,
        tw_ratio: None,
        uw_ratio: None,
        pw_ratio: None,
        cw_ratio: None,
        symmetry_order: None,
        mirror_symmetry: None,
        designer: None,
        // Not exercised by these tests either.
        source_citation: None,
        pdf_file: None,
        gem_file: None,
        shape_category: None,
        angle_settings: vec![AngleSettingWire {
            order_index: 0,
            facet: "P1".to_string(),
            angle: "41.0".to_string(),
            index: "96".to_string(),
            notes: String::new(),
            tool_line: None,
        }],
        attachments: Vec::new(),
        version: revision_token(url, revision),
        // Unfrosted; the frosting tests set their own.
        frosting: Vec::new(),
    }
}

/// [`design_summary_at`] at revision `0`.
#[must_use]
pub fn design_summary(entry_id: i64, title: &str, url: &str, extra_field: &str) -> DesignSummary {
    design_summary_at(entry_id, title, url, extra_field, 0)
}

/// A summary whose `version` hashes only the fields a search row carries.
///
/// Those fields are the title, url and `extra_field`. Its `design_version` is the revision token
/// at `revision`, so two summaries differing only in `revision` have equal `version`s, like a remote
/// edit that leaves the search fields alone.
#[must_use]
pub fn design_summary_at(
    entry_id: i64,
    title: &str,
    url: &str,
    extra_field: &str,
    revision: u32,
) -> DesignSummary {
    DesignSummary {
        entry_id,
        title: title.to_string(),
        url: url.to_string(),
        design_id: Some(format!("D-{entry_id}")),
        shape: Some("Round".to_string()),
        index_gear: Some("96".to_string()),
        facets_count: Some("57".to_string()),
        designer_info: Some("Capps, Jerry".to_string()),
        lw_ratio: Some(extra_field.to_string()),
        refractive_index: Some("1.72".to_string()),
        volume: Some("0.65".to_string()),
        competition_diagram: None,
        ignored: false,
        version: content_hash(&[title.as_bytes(), url.as_bytes(), extra_field.as_bytes()]),
        design_version: revision_token(url, revision),
        concave_tiers: 0,
    }
}

/// A trivial, deterministic stand-in for the worker's revision token of `url` at
/// `revision`; nonzero for every input, like the real one.
fn revision_token(url: &str, revision: u32) -> [u8; 32] {
    content_hash(&[b"revision", url.as_bytes(), &revision.to_le_bytes()])
}

/// A deterministic stand-in for the worker's SHA-256 content hash (see
/// `apps/pleochroma-worker/src/serve/library/version_hash.rs`). The sync only compares hashes
/// for equality, so any deterministic function of the test's content works.
fn content_hash(parts: &[&[u8]]) -> [u8; 32] {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for p in parts {
        hasher.update(p);
    }
    hasher.finalize().into()
}
