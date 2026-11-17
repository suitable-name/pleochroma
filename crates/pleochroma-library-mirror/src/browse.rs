//! Live browsing of a remote library: search, filter options, one design's record and its
//! design file, each as a single request over a caller-supplied [`LibraryTransport`].
//!
//! The mirror sync copies a whole library; these helpers fetch only what one screen shows.
//! Nothing here reads or writes a database. Thumbnails are not in the protocol: a
//! [`DesignSummary`] carries no picture, and the diagram image sits on the full
//! [`DesignRecord`], so a caller that wants a picture draws one from [`fetch_design_file`].

use crate::{transport::LibraryTransport, wire::frosting_from_wire};
use pleochroma_net::library::{
    AttachedFileMeta, AttributeRangesWire, DesignRecord, DesignSummary, LibraryRequest,
    LibraryResponse, RangeFilterWire, SortOrderWire,
};
use pleochroma_vault::model::{file::AttachedFile, frosting::StoredTierFrosting};
use std::fmt;

/// The number of rows at which a [`LibraryRequest::Search`] answer is cut off by the worker
/// (the vault's `SEARCH_RESULT_CAP`); an answer this long may be missing rows.
const SEARCH_RESULT_CAP: usize = 1000;

/// Why a browse request did not produce its result. `E` is the transport's own error.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowseError<E> {
    /// The transport failed: no connection, a dropped connection, a framing error.
    Transport(E),
    /// The worker has no such design or attachment.
    NotFound,
    /// The design exists but carries no design file to open.
    NoDesignFile,
    /// The worker answered with a request-level error; carries its message.
    Worker(String),
    /// The worker answered with a reply that does not belong to the request. Names the request.
    Unexpected(&'static str),
}

impl<E: fmt::Display> fmt::Display for BrowseError<E> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Transport(e) => write!(f, "{e}"),
            Self::NotFound => write!(f, "the remote library has no such design"),
            Self::NoDesignFile => write!(f, "the design has no design file to open"),
            Self::Worker(message) => write!(f, "{message}"),
            Self::Unexpected(request) => write!(
                f,
                "the remote library replied to {request} with an unexpected message"
            ),
        }
    }
}

impl<E: fmt::Debug + fmt::Display> std::error::Error for BrowseError<E> {}

/// One search: the text, the shape and gear filters (empty or `"All"` for no filter), the range
/// bounds, the sort order and an optional tag name.
#[derive(Debug, Clone)]
pub struct SearchSpec<'a> {
    /// Free text matched against the design's title and designer.
    pub text: &'a str,
    /// A shape name, or empty for any shape.
    pub shape: &'a str,
    /// A gear as text, or empty for any gear.
    pub gear: &'a str,
    /// Refractive index, ratio, volume and facet bounds, and whether ignored designs show.
    pub range: RangeFilterWire,
    /// The order of the results.
    pub order: SortOrderWire,
    /// Restrict to designs carrying this tag, by name.
    pub tag: Option<&'a str>,
}

/// The rows a search found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchResults {
    /// The matching designs, in the requested order.
    pub items: Vec<DesignSummary>,
    /// Whether the worker's row cap was reached: `items.len() >= 1000`, so more rows may match.
    pub capped: bool,
}

/// What a search screen needs for its filter controls.
#[derive(Debug, Clone, PartialEq)]
pub struct RemoteFilterOptions {
    /// The distinct shapes in the library.
    pub shapes: Vec<String>,
    /// The distinct gears in the library, as the worker states them.
    pub gears: Vec<String>,
    /// The observed bounds of the numeric attributes.
    pub ranges: AttributeRangesWire,
}

/// A design's file, ready for the caller to open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DesignFile {
    /// The self-contained native design file (`.psdd`), byte for byte. The only lossless
    /// carrier of a concave design.
    Native {
        /// The attachment's bare file name.
        file_name: String,
        /// The file's bytes.
        bytes: Vec<u8>,
    },
    /// The design's `.asc` cutting instructions.
    Asc {
        /// The attachment's bare file name.
        file_name: String,
        /// The exact original text.
        text: String,
        /// The frosting the design states, which `.asc` cannot carry itself.
        frosting: Vec<StoredTierFrosting>,
    },
}

/// Sends `req`, turning a transport failure into [`BrowseError::Transport`] and a
/// [`LibraryResponse::Error`] reply into [`BrowseError::Worker`].
fn ask<T: LibraryTransport>(
    transport: &T,
    req: &LibraryRequest,
) -> Result<LibraryResponse, BrowseError<T::Error>> {
    match transport.request(req) {
        Ok(LibraryResponse::Error(e)) => Err(BrowseError::Worker(e.message)),
        Ok(reply) => Ok(reply),
        Err(e) => Err(BrowseError::Transport(e)),
    }
}

/// Runs one [`LibraryRequest::Search`].
///
/// # Errors
///
/// [`BrowseError::Transport`] when the request fails, [`BrowseError::Worker`] when the worker
/// refuses it, [`BrowseError::Unexpected`] for any reply that is not a search result.
pub fn search<T: LibraryTransport>(
    transport: &T,
    spec: &SearchSpec<'_>,
) -> Result<SearchResults, BrowseError<T::Error>> {
    let request = LibraryRequest::Search {
        query: spec.text.to_owned(),
        shape_filter: spec.shape.to_owned(),
        gear_filter: spec.gear.to_owned(),
        range: spec.range.clone(),
        order: spec.order,
        tag_filter: spec.tag.map(str::to_owned),
    };
    match ask(transport, &request)? {
        LibraryResponse::SearchResults { items, .. } => Ok(SearchResults {
            capped: items.len() >= SEARCH_RESULT_CAP,
            items,
        }),
        _ => Err(BrowseError::Unexpected("Search")),
    }
}

/// Runs one [`LibraryRequest::FilterOptions`].
///
/// # Errors
///
/// As [`search`].
pub fn filter_options<T: LibraryTransport>(
    transport: &T,
) -> Result<RemoteFilterOptions, BrowseError<T::Error>> {
    match ask(transport, &LibraryRequest::FilterOptions)? {
        LibraryResponse::FilterOptions {
            shapes,
            gears,
            ranges,
        } => Ok(RemoteFilterOptions {
            shapes,
            gears,
            ranges,
        }),
        _ => Err(BrowseError::Unexpected("FilterOptions")),
    }
}

/// Fetches one design's record (rows and attachment metadata, not attachment bytes) by the
/// worker's entry id.
///
/// # Errors
///
/// [`BrowseError::NotFound`] when the worker has no such entry; otherwise as [`search`].
pub fn fetch_record<T: LibraryTransport>(
    transport: &T,
    entry_id: i64,
) -> Result<DesignRecord, BrowseError<T::Error>> {
    match ask(transport, &LibraryRequest::FetchDesign { entry_id })? {
        LibraryResponse::Design(record) => Ok(*record),
        LibraryResponse::NotFound => Err(BrowseError::NotFound),
        _ => Err(BrowseError::Unexpected("FetchDesign")),
    }
}

/// Fetches one attachment's bytes. `Ok(None)` without any request when `meta.size` is over
/// `max_bytes`.
///
/// # Errors
///
/// [`BrowseError::NotFound`] when the attachment vanished; otherwise as [`search`].
pub fn fetch_attachment<T: LibraryTransport>(
    transport: &T,
    meta: &AttachedFileMeta,
    max_bytes: u64,
) -> Result<Option<AttachedFile>, BrowseError<T::Error>> {
    if meta.size > max_bytes {
        return Ok(None);
    }
    let request = LibraryRequest::FetchAttachment {
        attachment_id: meta.id,
    };
    match ask(transport, &request)? {
        LibraryResponse::Attachment { name, content } => Ok(Some(AttachedFile {
            name,
            url: meta.url.clone(),
            content,
        })),
        LibraryResponse::NotFound => Err(BrowseError::NotFound),
        _ => Err(BrowseError::Unexpected("FetchAttachment")),
    }
}

/// Fetches the file an editor opens for a design: the native `.psdd` first, and when the
/// design has none, its `.asc` with the frosting it states.
///
/// # Errors
///
/// [`BrowseError::NotFound`] when the worker has no such entry, [`BrowseError::NoDesignFile`]
/// when it has neither file; otherwise as [`search`].
pub fn fetch_design_file<T: LibraryTransport>(
    transport: &T,
    entry_id: i64,
) -> Result<DesignFile, BrowseError<T::Error>> {
    match ask(transport, &LibraryRequest::FetchDesignNative { entry_id })? {
        LibraryResponse::DesignNative {
            file_name, content, ..
        } => {
            return Ok(DesignFile::Native {
                file_name,
                bytes: content,
            });
        }
        LibraryResponse::DesignNativeNotAvailable => {}
        LibraryResponse::NotFound => return Err(BrowseError::NotFound),
        _ => return Err(BrowseError::Unexpected("FetchDesignNative")),
    }
    match ask(transport, &LibraryRequest::FetchDesignSource { entry_id })? {
        LibraryResponse::DesignSource {
            file_name,
            asc_text,
            frosting,
            ..
        } => Ok(DesignFile::Asc {
            file_name,
            text: asc_text,
            frosting: frosting_from_wire(&frosting),
        }),
        LibraryResponse::DesignSourceNotAvailable => Err(BrowseError::NoDesignFile),
        LibraryResponse::NotFound => Err(BrowseError::NotFound),
        _ => Err(BrowseError::Unexpected("FetchDesignSource")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FakeTransport, design_record, design_summary};
    use pleochroma_net::library::FrostedTierWire;

    fn spec() -> SearchSpec<'static> {
        SearchSpec {
            text: "",
            shape: "",
            gear: "",
            range: RangeFilterWire::default(),
            order: SortOrderWire::default(),
            tag: None,
        }
    }

    fn one_design() -> FakeTransport {
        FakeTransport::new(
            vec![design_summary(7, "Round", "https://example.test/7", "v1")],
            vec![design_record(7, "Round", "https://example.test/7", "v1")],
        )
        .for_browse()
    }

    #[test]
    fn fetch_design_file_prefers_the_native_file() {
        let transport = one_design()
            .with_native(7, "round.psdd", b"native bytes".to_vec())
            .with_source(7, "round.asc", "asc text", Vec::new());
        let file = fetch_design_file(&transport, 7).unwrap();
        assert_eq!(
            file,
            DesignFile::Native {
                file_name: "round.psdd".to_string(),
                bytes: b"native bytes".to_vec(),
            }
        );
    }

    #[test]
    fn fetch_design_file_falls_back_to_the_asc_with_its_frosting() {
        let transport = one_design().with_source(
            7,
            "round.asc",
            "asc text",
            vec![FrostedTierWire {
                tier: 2,
                facets: vec![1, 3],
            }],
        );
        let file = fetch_design_file(&transport, 7).unwrap();
        assert_eq!(
            file,
            DesignFile::Asc {
                file_name: "round.asc".to_string(),
                text: "asc text".to_string(),
                frosting: vec![StoredTierFrosting {
                    tier: 2,
                    facets: vec![1, 3],
                }],
            }
        );
    }

    #[test]
    fn not_found_and_no_design_file_are_distinct() {
        let transport = one_design();
        // The design exists but has neither file.
        assert!(matches!(
            fetch_design_file(&transport, 7),
            Err(BrowseError::NoDesignFile)
        ));
        // An entry id the worker does not know.
        assert!(matches!(
            fetch_design_file(&transport, 99),
            Err(BrowseError::NotFound)
        ));
        assert!(matches!(
            fetch_record(&transport, 99),
            Err(BrowseError::NotFound)
        ));
        assert_eq!(fetch_record(&transport, 7).unwrap().entry_id, 7);
    }

    #[test]
    fn search_reports_capped() {
        let few = one_design();
        let results = search(&few, &spec()).unwrap();
        assert_eq!(results.items.len(), 1);
        assert!(!results.capped);

        let summaries: Vec<DesignSummary> = (0..1000)
            .map(|id| design_summary(id, "Round", &format!("https://example.test/{id}"), "v1"))
            .collect();
        let many = FakeTransport::new(summaries, Vec::new()).for_browse();
        let results = search(&many, &spec()).unwrap();
        assert_eq!(results.items.len(), 1000);
        assert!(results.capped);
    }

    #[test]
    fn the_spec_reaches_the_worker_as_one_search_request() {
        let transport = one_design();
        let tagged = SearchSpec {
            text: "brilliant",
            shape: "Round",
            gear: "96",
            order: SortOrderWire::Title,
            tag: Some("favorite"),
            ..spec()
        };
        let _ = search(&transport, &tagged).unwrap();
        assert_eq!(
            transport.last_search.borrow().clone(),
            Some(LibraryRequest::Search {
                query: "brilliant".to_string(),
                shape_filter: "Round".to_string(),
                gear_filter: "96".to_string(),
                range: RangeFilterWire::default(),
                order: SortOrderWire::Title,
                tag_filter: Some("favorite".to_string()),
            })
        );
        let options = filter_options(&transport).unwrap();
        assert_eq!(options.shapes, vec!["Round".to_string()]);
        assert_eq!(options.gears, vec!["96".to_string()]);
    }

    #[test]
    fn an_error_reply_becomes_a_worker_error() {
        let transport = one_design().with_error_reply("library database is locked");
        assert!(matches!(
            search(&transport, &spec()),
            Err(BrowseError::Worker(message)) if message == "library database is locked"
        ));
        assert!(matches!(
            fetch_design_file(&transport, 7),
            Err(BrowseError::Worker(message)) if message == "library database is locked"
        ));
    }

    #[test]
    fn an_attachment_over_the_cap_is_not_requested() {
        let transport = one_design().with_attachment(100, "big.pdf", vec![0u8; 600]);
        let meta = AttachedFileMeta {
            id: 100,
            name: "big.pdf".to_string(),
            url: "https://example.test/big.pdf".to_string(),
            size: 600,
        };
        assert!(fetch_attachment(&transport, &meta, 500).unwrap().is_none());
        assert_eq!(
            transport
                .fetch_attachment_calls
                .load(std::sync::atomic::Ordering::Relaxed),
            0,
            "an oversized attachment must not even be asked for"
        );
        let file = fetch_attachment(&transport, &meta, 600).unwrap().unwrap();
        assert_eq!(file.name, "big.pdf");
        assert_eq!(file.url, "https://example.test/big.pdf");
        assert_eq!(file.content.len(), 600);
    }
}
