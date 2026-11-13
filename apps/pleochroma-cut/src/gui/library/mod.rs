//! The design-library UI.
//!
//! It covers browsing and searching ([`diagram_list`], [`search`]), a design's metadata
//! ([`detail`]), copying detail fields ([`clipboard`]), local import, organize and export
//! ([`local`]), and switching between the local database and a remote worker's library with
//! a pull-mirror sync ([`remote`]).

pub mod clipboard;
pub mod detail;
pub mod diagram_list;
pub mod local;
mod read_access;
pub mod remote;
pub mod search;
pub mod tutorial_state;
