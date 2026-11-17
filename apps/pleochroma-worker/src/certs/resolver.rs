//! The coordinator's server certificate, swappable while listeners keep running.
//!
//! [`SwappableCert`] is the `rustls` certificate resolver of the coordinator's server
//! configs. Every new handshake asks it for the current certificate; [`SwappableCert::swap`]
//! replaces that certificate without restarting a listener and without touching a
//! connection that is already established (a running render keeps its session).
//!
//! The workspace has no `arc-swap`, so the certificate sits behind an `RwLock<Arc<_>>`: a
//! handshake takes the read lock only long enough to clone the `Arc`.

use rustls::{
    server::{ClientHello, ResolvesServerCert},
    sign::CertifiedKey,
};
use std::{
    fmt,
    sync::{Arc, PoisonError, RwLock},
};

/// A server certificate that can be replaced at run time.
pub struct SwappableCert {
    current: RwLock<Arc<CertifiedKey>>,
}

impl SwappableCert {
    /// Starts with `key` as the certificate every handshake gets.
    #[must_use]
    pub fn new(key: CertifiedKey) -> Self {
        Self {
            current: RwLock::new(Arc::new(key)),
        }
    }

    /// The certificate a handshake started now would present.
    #[must_use]
    pub fn current(&self) -> Arc<CertifiedKey> {
        Arc::clone(&self.current.read().unwrap_or_else(PoisonError::into_inner))
    }

    /// Replaces the certificate. Handshakes that start after this call present `key`;
    /// established connections are not affected.
    pub fn swap(&self, key: CertifiedKey) {
        *self.current.write().unwrap_or_else(PoisonError::into_inner) = Arc::new(key);
    }
}

// Only the certificate count is printed: a signing key must never reach a log.
impl fmt::Debug for SwappableCert {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SwappableCert")
            .field("chain_len", &self.current().cert.len())
            .finish_non_exhaustive()
    }
}

impl ResolvesServerCert for SwappableCert {
    fn resolve(&self, _client_hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        Some(self.current())
    }
}
