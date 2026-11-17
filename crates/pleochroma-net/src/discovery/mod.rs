//! LAN discovery of coordinators over mDNS (`_pleochroma._tcp`).
//!
//! A coordinator calls [`announce`] to be found; a viewer or phone calls [`browse`] to list
//! the coordinators of its own CA. Both use `mdns-sd` (pure Rust, no system service), so
//! they work the same on Windows, Linux, macOS and Android.
//!
//! **Switchable.** Discovery is a setting (off by default on the coordinator). `announce`
//! must only be called while the setting is on; dropping the returned [`Announcer`] stops
//! the announcement (and sends the goodbye packet). Nothing in this module starts on its
//! own.
//!
//! **What is announced.** The instance name, the port, the LAN IPv4 addresses and a TXT
//! record with the protocol version and a 16-hex tag of the CA fingerprint (see [`txt`]).
//! No secret, token or full fingerprint is ever put on the wire. The tag only lets
//! [`browse`] skip other people's coordinators; the TLS handshake against the pinned CA
//! remains the real check.
//!
//! **Browsing.** [`browse`] calls `on_found` on a background thread every time a matching
//! service is resolved. The same coordinator can be reported several times (mDNS refreshes,
//! new addresses), so the caller keys by [`Found::name`]. Announcers of this very machine
//! are reported too; the caller filters itself out if it wants to.

pub mod txt;

use crate::{netinfo, tls::Fingerprint};
use mdns_sd::{ServiceDaemon, ServiceEvent, ServiceInfo};
use std::{
    collections::HashMap,
    net::{IpAddr, Ipv4Addr},
    thread::JoinHandle,
    time::Duration,
};

/// One coordinator found on the LAN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// The instance name the coordinator announced (its display name).
    pub name: String,
    /// Its IPv4 addresses, sorted and without duplicates; never empty.
    pub addrs: Vec<Ipv4Addr>,
    /// The TCP port of its client listener.
    pub port: u16,
}

/// A running announcement. Dropping it unregisters the service and stops the daemon.
pub struct Announcer {
    daemon: ServiceDaemon,
    instance: String,
    host: String,
    port: u16,
    txt: HashMap<String, String>,
}

impl std::fmt::Debug for Announcer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Announcer")
            .field("instance", &self.instance)
            .field("port", &self.port)
            .finish_non_exhaustive()
    }
}

/// Announces a coordinator named `name` on `port` for the CA `ca_fingerprint`, with this
/// machine's LAN IPv4 addresses ([`netinfo::lan_ipv4`]; every interface if there is none).
///
/// Call it only while discovery is switched on. Keep the result alive for as long as the
/// announcement should last, and call [`Announcer::update_addresses`] when the LAN
/// addresses change.
///
/// # Errors
///
/// A human-readable message if the mDNS daemon cannot start or the service is refused.
pub fn announce(name: &str, port: u16, ca_fingerprint: &Fingerprint) -> Result<Announcer, String> {
    let daemon =
        ServiceDaemon::new().map_err(|e| format!("could not start the discovery service: {e}"))?;
    let announcer = Announcer {
        daemon,
        instance: txt::instance_label(name),
        host: txt::host_name(name),
        port,
        txt: txt::encode_txt(ca_fingerprint),
    };
    let addrs: Vec<Ipv4Addr> = netinfo::lan_ipv4().iter().map(|a| a.addr).collect();
    announcer.register(&addrs)?;
    Ok(announcer)
}

impl Announcer {
    /// Announces again with `addrs` (an empty list means every interface). Used when the
    /// network changes; the old record is replaced, not duplicated.
    ///
    /// # Errors
    ///
    /// A human-readable message if the service is refused.
    pub fn update_addresses(&self, addrs: &[Ipv4Addr]) -> Result<(), String> {
        self.register(addrs)
    }

    fn register(&self, addrs: &[Ipv4Addr]) -> Result<(), String> {
        let ips: Vec<IpAddr> = addrs.iter().copied().map(IpAddr::V4).collect();
        let built = if ips.is_empty() {
            ServiceInfo::new(
                txt::SERVICE_TYPE,
                &self.instance,
                &self.host,
                (),
                self.port,
                self.txt.clone(),
            )
            .map(ServiceInfo::enable_addr_auto)
        } else {
            ServiceInfo::new(
                txt::SERVICE_TYPE,
                &self.instance,
                &self.host,
                &ips[..],
                self.port,
                self.txt.clone(),
            )
        };
        let info = built.map_err(|e| format!("could not describe the discovery service: {e}"))?;
        self.daemon
            .register(info)
            .map_err(|e| format!("could not announce the coordinator: {e}"))
    }
}

impl Drop for Announcer {
    fn drop(&mut self) {
        let fullname = format!("{}{}", self.instance, txt::SERVICE_SUFFIX);
        // Commands run in order: the goodbye goes out before the daemon exits. Wait a
        // moment for it, but never hang a shutdown on the network.
        if let Ok(done) = self.daemon.unregister(&fullname) {
            let _ = done.recv_timeout(Duration::from_millis(300));
        }
        let _ = self.daemon.shutdown();
    }
}

/// A running browse. Dropping it stops the search and its thread.
pub struct Browser {
    daemon: ServiceDaemon,
    // Held only so the struct documents the thread's lifetime; the thread ends when the
    // daemon shuts down and its channel closes. It is detached, never joined, so a callback
    // may drop the `Browser` without deadlocking.
    _thread: Option<JoinHandle<()>>,
}

impl std::fmt::Debug for Browser {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Browser").finish_non_exhaustive()
    }
}

/// Browses for coordinators of the CA `ca_fingerprint` and calls `on_found` for each one.
///
/// `on_found` runs on a background thread for each coordinator that resolves. Coordinators
/// of other CAs, and announcements with a missing or malformed TXT record, are skipped.
///
/// # Errors
///
/// A human-readable message if the mDNS daemon cannot start or the browse is refused.
pub fn browse(
    ca_fingerprint: &Fingerprint,
    on_found: impl Fn(Found) + Send + 'static,
) -> Result<Browser, String> {
    let wanted = *ca_fingerprint;
    let daemon =
        ServiceDaemon::new().map_err(|e| format!("could not start the discovery service: {e}"))?;
    let events = daemon
        .browse(txt::SERVICE_TYPE)
        .map_err(|e| format!("could not search for coordinators: {e}"))?;
    let thread = std::thread::Builder::new()
        .name("discovery-browse".into())
        .spawn(move || {
            while let Ok(event) = events.recv() {
                if let ServiceEvent::ServiceResolved(info) = event {
                    let record = txt::decode_txt(|key| info.get_property_val_str(key));
                    if !record.is_some_and(|r| txt::matches_ca(&r, &wanted)) {
                        continue;
                    }
                    let addrs = info.get_addresses_v4().into_iter().copied();
                    if let Some(found) = make_found(info.get_fullname(), info.get_port(), addrs) {
                        on_found(found);
                    }
                }
            }
        })
        .map_err(|e| format!("could not start the discovery thread: {e}"))?;
    Ok(Browser {
        daemon,
        _thread: Some(thread),
    })
}

impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.daemon.stop_browse(txt::SERVICE_TYPE);
        let _ = self.daemon.shutdown();
    }
}

/// Builds a [`Found`] from a resolved service: the instance name, the port, and the
/// addresses sorted and deduplicated. `None` when there is no usable address (an
/// unspecified `0.0.0.0` does not count).
fn make_found(
    fullname: &str,
    port: u16,
    addrs: impl IntoIterator<Item = Ipv4Addr>,
) -> Option<Found> {
    let mut addrs: Vec<Ipv4Addr> = addrs.into_iter().filter(|a| !a.is_unspecified()).collect();
    addrs.sort_unstable();
    addrs.dedup();
    if addrs.is_empty() {
        return None;
    }
    Some(Found {
        name: txt::instance_name(fullname),
        addrs,
        port,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn make_found_sorts_dedups_and_names() {
        let found = make_found(
            "Studio PC._pleochroma._tcp.local.",
            7878,
            [
                Ipv4Addr::new(192, 168, 1, 20),
                Ipv4Addr::new(10, 0, 0, 5),
                Ipv4Addr::new(192, 168, 1, 20),
            ],
        )
        .expect("has addresses");
        assert_eq!(found.name, "Studio PC");
        assert_eq!(found.port, 7878);
        assert_eq!(
            found.addrs,
            vec![Ipv4Addr::new(10, 0, 0, 5), Ipv4Addr::new(192, 168, 1, 20)]
        );
    }

    #[test]
    fn make_found_needs_a_usable_address() {
        assert!(make_found("x._pleochroma._tcp.local.", 1, []).is_none());
        assert!(make_found("x._pleochroma._tcp.local.", 1, [Ipv4Addr::UNSPECIFIED]).is_none());
    }

    /// Announces and browses on this machine's real network. Needs working multicast on at
    /// least one LAN interface, which CI containers and many VPN-only hosts lack, so it is
    /// ignored by default. Run it by hand with `cargo test -p pleochroma-net --features
    /// pairing -- --ignored loopback_announce_and_browse`.
    #[test]
    #[ignore = "needs working multicast on a LAN interface"]
    fn loopback_announce_and_browse() {
        let mut ca = [0_u8; 32];
        ca[0] = 0xA5;
        ca[31] = 0x5A;
        let other_ca = [0x11_u8; 32];

        let (tx, rx) = mpsc::channel();
        let (other_tx, other_rx) = mpsc::channel();
        let _browser = browse(&ca, move |found| {
            let _ = tx.send(found);
        })
        .expect("browse starts");
        let _other = browse(&other_ca, move |found| {
            let _ = other_tx.send(found);
        })
        .expect("second browse starts");

        let _announcer = announce("Discovery Test", 47_811, &ca).expect("announce starts");

        let found = rx
            .recv_timeout(Duration::from_secs(15))
            .expect("the matching browser finds the announcement");
        assert_eq!(found.name, "Discovery Test");
        assert_eq!(found.port, 47_811);
        assert!(!found.addrs.is_empty());

        // The browser of another CA must stay silent.
        assert!(other_rx.recv_timeout(Duration::from_secs(3)).is_err());
    }
}
