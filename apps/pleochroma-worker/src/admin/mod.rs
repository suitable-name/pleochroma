//! Remote administration of this coordinator, served on the viewer port.
//!
//! A monitor or admin device (paired with a code from `cert issue-token --role monitor|admin`
//! or the coordinator window, only while `--allow-remote-admin` is on) connects to the viewer
//! port with its own certificate and a `HELLO` of its role. The accept loop hands such a
//! connection to [`serve_connection`], which checks it ([`crate::serve::handshake`]) and runs a
//! [`session`]: requests in (`pleochroma_net::admin::protocol::AdminRequest`), answers and
//! pushes out (`AdminPush`).
//!
//! - [`session`]: the gate and the request/push loop, including the permission table, the
//!   limits, the audit trail and the heartbeat;
//! - [`io`]: the stream trait and the timeout-tolerant message poll the loop is built on;
//! - [`results`]: a finished job's picture, cut into chunks;
//! - [`restart`]: the hook that lets the owner of the coordinator restart it on request.
//!
//! Everything a session does goes through the control API the coordinator window uses
//! ([`crate::serve::ServeCore`]), so a remote administrator can do exactly what the window
//! can, minus what only exists on the coordinator's own computer (see
//! `pleochroma_net::admin::permissions::NeverRemote`).
//!
//! The module is compiled in every build: without the `worker` feature there are no jobs and
//! no render computers, and the matching requests answer in plain words.

pub mod io;
pub mod restart;
pub mod results;
pub mod session;
#[cfg(test)]
mod tests;

pub use io::SessionIo;
pub use session::{run, serve_connection};
