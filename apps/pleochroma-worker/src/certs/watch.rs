//! The address watch: a new server certificate when the LAN addresses change.
//!
//! Every [`WATCH_INTERVAL`] (10 seconds) [`AddressWatch`] reads the machine's LAN IPv4
//! addresses ([`pleochroma_net::netinfo::lan_ipv4`]). When the set a certificate lists
//! changes (Wi-Fi to Ethernet, a new network, a VPN that becomes the only adapter), it
//! mints a new server certificate with the same CA and swaps it into the
//! [`SwappableCert`]. Listeners keep running and established connections are not touched.
//!
//! The decision logic lives in [`CertificateRefresh::check`], which takes the address list
//! as an argument, so tests drive it without real interfaces. A machine with no usable LAN
//! address at all keeps its current certificate.

use super::{
    ServerFiles, SwappableCert, certificate_lan_addresses, certificate_lists_addresses,
    issue_server_certificate, load_certified_key,
};
use pleochroma_net::netinfo::{self, LanAddress};
use std::{
    net::Ipv4Addr,
    sync::{
        Arc,
        mpsc::{self, RecvTimeoutError, Sender},
    },
    thread::JoinHandle,
    time::Duration,
};

/// How often the watch reads the LAN addresses.
pub const WATCH_INTERVAL: Duration = Duration::from_secs(10);

/// What a certificate swap reports to the hook.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertificateSwap {
    /// The LAN addresses the new certificate lists (never includes `127.0.0.1`).
    pub addresses: Vec<Ipv4Addr>,
}

/// The hook called after every swap. Runs on the watch thread, so it must not block.
pub type SwapHook = Arc<dyn Fn(&CertificateSwap) + Send + Sync>;

/// A hook that runs `base`, then announces the new addresses again through `announcer`.
///
/// The watch itself works without an announcer: pass the plain hook when discovery is
/// off, and only wrap it when [`pleochroma_net::discovery::announce`] was called. A failed
/// re-announcement is logged and never stops the swap.
#[must_use]
pub fn announcing_hook(
    base: SwapHook,
    announcer: Arc<pleochroma_net::discovery::Announcer>,
) -> SwapHook {
    Arc::new(move |swap| {
        base(swap);
        if let Err(e) = announcer.update_addresses(&swap.addresses) {
            tracing::warn!("pleochroma-worker: could not announce the new addresses: {e}");
        }
    })
}

/// Reissues and swaps the server certificate when the LAN address set changes.
pub struct CertificateRefresh {
    files: ServerFiles,
    extra_names: Vec<String>,
    resolver: Arc<SwappableCert>,
    on_swapped: SwapHook,
    /// The address set the current certificate was issued for; `None` until the first check.
    issued_for: Option<Vec<Ipv4Addr>>,
    /// The address set whose reissue failed and was reported; that set is not retried (and not
    /// warned about again) until the addresses change.
    failed_for: Option<Vec<Ipv4Addr>>,
}

impl CertificateRefresh {
    /// A refresh over the certificate in `files` (the `--cert` and `--key` files of `serve`), whose
    /// live copy is `resolver`.
    ///
    /// `extra_names` are the VPN or DNS names every new certificate lists too; the names of the
    /// certificate being replaced stay as well.
    #[must_use]
    pub const fn new(
        files: ServerFiles,
        extra_names: Vec<String>,
        resolver: Arc<SwappableCert>,
        on_swapped: SwapHook,
    ) -> Self {
        Self {
            files,
            extra_names,
            resolver,
            on_swapped,
            issued_for: None,
            failed_for: None,
        }
    }

    /// One watch step for the current address list `lan`. Returns `Ok(true)` when it
    /// issued and swapped a new certificate.
    ///
    /// The first call compares against the certificate on disk (the machine may have
    /// changed networks while the coordinator was off); later calls compare against the
    /// set of the last issue. No usable address keeps the current certificate.
    ///
    /// # Errors
    ///
    /// A human-readable message if issuing or loading the new certificate fails; the
    /// old certificate stays in use. The message is returned once per address set: later
    /// steps with the same addresses return `Ok(false)` without trying again, and a changed
    /// set tries again.
    pub fn check(&mut self, lan: &[LanAddress]) -> Result<bool, String> {
        let mut wanted = certificate_lan_addresses(lan);
        wanted.sort_unstable();
        if wanted.is_empty() {
            return Ok(false);
        }

        let unchanged = self.issued_for.as_ref().map_or_else(
            || self.disk_certificate_lists(&wanted),
            |previous| *previous == wanted,
        );
        if unchanged {
            self.issued_for = Some(wanted);
            self.failed_for = None;
            return Ok(false);
        }
        // This address set already failed and was reported: stay quiet until it changes.
        if self.failed_for.as_ref() == Some(&wanted) {
            return Ok(false);
        }

        let issued = issue_server_certificate(&self.files, &self.extra_names, lan)
            .map_err(|e| format!("could not reissue the server certificate: {e}"))
            .and_then(|()| load_certified_key(&self.files.cert, &self.files.key));
        let key = match issued {
            Ok(key) => key,
            Err(error) => {
                self.failed_for = Some(wanted);
                return Err(error);
            }
        };
        self.failed_for = None;
        self.resolver.swap(key);
        tracing::info!(
            "pleochroma-worker: network addresses changed, the server certificate now lists {} (same CA, no restart)",
            wanted
                .iter()
                .map(Ipv4Addr::to_string)
                .collect::<Vec<_>>()
                .join(", ")
        );
        let swap = CertificateSwap {
            addresses: wanted.clone(),
        };
        self.issued_for = Some(wanted);
        (self.on_swapped)(&swap);
        Ok(true)
    }

    /// Whether the certificate file already lists every address of `wanted`.
    fn disk_certificate_lists(&self, wanted: &[Ipv4Addr]) -> bool {
        pleochroma_net::tls::load_certs(&self.files.cert).is_ok_and(|chain| {
            chain
                .first()
                .is_some_and(|leaf| certificate_lists_addresses(leaf.as_ref(), wanted))
        })
    }
}

/// The running watch thread. Dropping it, or calling [`AddressWatch::stop`], ends the
/// thread.
pub struct AddressWatch {
    stop: Option<Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl AddressWatch {
    /// Starts the watch: one check now, then one every [`WATCH_INTERVAL`].
    #[must_use]
    pub fn start(refresh: CertificateRefresh) -> Self {
        Self::start_with_interval(refresh, WATCH_INTERVAL)
    }

    fn start_with_interval(mut refresh: CertificateRefresh, interval: Duration) -> Self {
        let (stop_tx, stop_rx) = mpsc::channel::<()>();
        let thread = std::thread::Builder::new()
            .name("certificate-watch".into())
            .spawn(move || {
                loop {
                    if let Err(e) = refresh.check(&netinfo::lan_ipv4()) {
                        tracing::warn!("pleochroma-worker: {e}");
                    }
                    // A message or a dropped sender ends the watch; only a timeout continues.
                    if !matches!(
                        stop_rx.recv_timeout(interval),
                        Err(RecvTimeoutError::Timeout)
                    ) {
                        break;
                    }
                }
            })
            .ok();
        Self {
            stop: Some(stop_tx),
            thread,
        }
    }

    /// Stops the watch and waits for its thread to end.
    pub fn stop(&mut self) {
        // Dropping the sender wakes the thread's `recv_timeout` with `Disconnected`.
        self.stop.take();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for AddressWatch {
    fn drop(&mut self) {
        self.stop();
    }
}
