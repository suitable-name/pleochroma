//! Re-export of [`SampleCursor`] from `bridge::sample_cursor`, which the live viewport shares.

pub(in crate::bridge::export_thread) use crate::bridge::sample_cursor::SampleCursor;
