//! The coordinator's control layer: one way to stop its listeners, its status, and the
//! backend trait the coordinator window drives (locally, or remotely over the admin protocol).

pub mod api;
pub mod backend;
pub mod events;
#[cfg(feature = "worker")]
pub mod jobs;
#[cfg(feature = "worker")]
pub mod renders;
pub mod shutdown;
pub mod status;
pub mod table;
