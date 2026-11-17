//! The coordinator's administration types.
//!
//! Its status, events, log lines, settings patch and permissions. The coordinator window
//! reads them locally; remote administration sends them over the wire.

pub mod permissions;
pub mod protocol;
pub mod status;
pub mod validate;
