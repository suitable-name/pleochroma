//! Finding a paired coordinator again after it moved to another address.
//!
//! A remote paired through a link is verified by its fixed certificate name, so it is still
//! the same coordinator at a new IP. When no saved address answers, [`notify_unreachable`]
//! (called by the connection code) starts one background search of the local network
//! ([`pleochroma_net::discovery::browse`], matched by the saved certificate authority's
//! fingerprint) and hands the result to the handler `gui::remote` installed with
//! [`set_handler`]. The search never blocks the connection that triggered it, runs at most
//! one at a time and at most once a minute.

use pleochroma_net::{
    discovery::{self, Found},
    tls,
};
use std::{
    path::Path,
    sync::{
        Mutex, OnceLock, PoisonError,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

/// How long one search listens before it gives up.
pub const SEARCH_TIME: Duration = Duration::from_secs(20);

/// The shortest time between two searches, so an offline coordinator is not searched for on
/// every failed connection.
const RETRY_AFTER: Duration = Duration::from_secs(60);

/// What runs on the search thread: it gets the certificate folder of the unreachable remote.
type Handler = Box<dyn Fn(&str) + Send + Sync>;

static HANDLER: OnceLock<Handler> = OnceLock::new();
static SEARCHING: AtomicBool = AtomicBool::new(false);
static LAST_SEARCH: Mutex<Option<Instant>> = Mutex::new(None);

/// Installs the function that searches and saves the result. Returns `false` when one is
/// installed already (the first one stays).
pub fn set_handler(handler: impl Fn(&str) + Send + Sync + 'static) -> bool {
    HANDLER.set(Box::new(handler)).is_ok()
}

/// Starts a background search for the remote whose certificates live in `cert_dir`, unless
/// no handler is installed, a search is running or one ran in the last minute.
pub fn notify_unreachable(cert_dir: &str) {
    let Some(handler) = HANDLER.get() else {
        return;
    };
    if SEARCHING.swap(true, Ordering::AcqRel) {
        return;
    }
    {
        let mut last = LAST_SEARCH.lock().unwrap_or_else(PoisonError::into_inner);
        if last.is_some_and(|at| at.elapsed() < RETRY_AFTER) {
            SEARCHING.store(false, Ordering::Release);
            return;
        }
        *last = Some(Instant::now());
    }
    let cert_dir = cert_dir.to_owned();
    let started = std::thread::Builder::new()
        .name("remote-rediscover".into())
        .spawn(move || {
            handler(&cert_dir);
            SEARCHING.store(false, Ordering::Release);
        });
    if started.is_err() {
        SEARCHING.store(false, Ordering::Release);
    }
}

/// Listens on the local network for the coordinator of the certificate authority saved in
/// `cert_dir` and returns the first announcement that `verify` accepts, or an error after
/// `timeout`. An announcement `verify` rejects is skipped and the search goes on.
///
/// An announcement only carries the fingerprint of a public certificate authority, which anyone
/// on the network can copy. `verify` must therefore prove the announcer holds a certificate of
/// that authority (a real handshake with the saved certificates, as
/// [`crate::bridge::remote::remote_render::verify_announced`] does) before the result is used.
///
/// Blocking: call it from a background thread. Dropping the browser at the end stops the
/// search.
///
/// # Errors
///
/// A plain sentence when the saved certificate authority cannot be read, the discovery service
/// cannot start, or no announcement is verified in time.
pub fn find(
    cert_dir: &str,
    timeout: Duration,
    verify: impl Fn(&Found) -> bool,
) -> Result<Found, String> {
    let ca_path = Path::new(cert_dir).join("ca.pem");
    let certs = tls::load_certs(&ca_path)
        .map_err(|_| "The saved certificate authority could not be read.".to_owned())?;
    let ca = certs
        .first()
        .ok_or_else(|| "The saved certificate authority is empty.".to_owned())?;
    let fingerprint = tls::fingerprint(ca);
    let (sender, receiver) = mpsc::channel();
    let browser = discovery::browse(&fingerprint, move |found| {
        // The receiver is gone once a result was taken; later announcements are not needed.
        let _ = sender.send(found);
    })?;
    let result = first_verified(&receiver, timeout, verify);
    drop(browser);
    result.ok_or_else(|| "The coordinator was not found on this network.".to_owned())
}

/// The first announcement `verify` accepts that arrives on `receiver` before `timeout` is over.
fn first_verified(
    receiver: &mpsc::Receiver<Found>,
    timeout: Duration,
    verify: impl Fn(&Found) -> bool,
) -> Option<Found> {
    let deadline = Instant::now() + timeout;
    loop {
        let left = deadline.checked_duration_since(Instant::now())?;
        let found = receiver.recv_timeout(left).ok()?;
        if verify(&found) {
            return Some(found);
        }
    }
}

/// The `host:port` list for a coordinator found at `found`: every announced address with the
/// announced port, at most three (what a pairing link carries).
#[must_use]
pub fn addresses_for(found: &Found) -> Vec<String> {
    found
        .addrs
        .iter()
        .take(crate::bridge::remote::route::MAX_DIALED_ADDRESSES)
        .map(|ip| format!("{ip}:{}", found.port))
        .collect()
}

/// The sentence shown after a coordinator was found at a new address, for example
/// "Studio PC moved to 192.168.1.40." `name` may be empty.
#[must_use]
pub fn moved_status(name: &str, addresses: &[String]) -> String {
    let who = if name.trim().is_empty() {
        "The coordinator"
    } else {
        name.trim()
    };
    let host = addresses
        .first()
        .and_then(|address| tls::host_for_server_name(address))
        .unwrap_or("a new address");
    format!("{who} moved to {host}.")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::Ipv4Addr;

    fn found(addrs: &[Ipv4Addr]) -> Found {
        Found {
            name: "Studio PC".to_owned(),
            addrs: addrs.to_vec(),
            port: 7878,
        }
    }

    #[test]
    fn a_found_coordinator_becomes_a_host_port_list_of_at_most_three() {
        let list = addresses_for(&found(&[
            Ipv4Addr::new(192, 168, 1, 40),
            Ipv4Addr::new(10, 0, 0, 5),
            Ipv4Addr::new(10, 0, 1, 5),
            Ipv4Addr::new(10, 0, 2, 5),
        ]));
        assert_eq!(
            list,
            vec!["192.168.1.40:7878", "10.0.0.5:7878", "10.0.1.5:7878"]
        );
    }

    #[test]
    fn the_status_names_the_coordinator_and_its_new_host_without_a_port() {
        let list = addresses_for(&found(&[Ipv4Addr::new(192, 168, 1, 40)]));
        assert_eq!(
            moved_status("Studio PC", &list),
            "Studio PC moved to 192.168.1.40."
        );
        assert_eq!(
            moved_status("  ", &list),
            "The coordinator moved to 192.168.1.40."
        );
    }

    #[test]
    fn a_folder_without_a_certificate_authority_is_reported_in_plain_words() {
        let error = find(
            "Z:/this/folder/does/not/exist",
            Duration::from_millis(10),
            |_| true,
        )
        .expect_err("no ca.pem there");
        assert!(error.contains("certificate authority"), "{error}");
    }

    /// A fake announcement (the public authority tag, but no handshake with the saved
    /// certificates) is skipped, and the search keeps listening for the real one.
    #[test]
    fn a_rejected_announcement_is_skipped_and_the_real_one_is_taken() {
        let (sender, receiver) = mpsc::channel();
        let mut fake = found(&[Ipv4Addr::new(10, 9, 9, 9)]);
        fake.port = 1;
        sender.send(fake).expect("open channel");
        sender
            .send(found(&[Ipv4Addr::new(192, 168, 1, 40)]))
            .expect("open channel");
        let accepted = first_verified(&receiver, Duration::from_secs(2), |f| f.port == 7878)
            .expect("the real announcement is accepted");
        assert_eq!(addresses_for(&accepted), vec!["192.168.1.40:7878"]);
    }

    /// When nothing passes verification nothing is returned, so nothing is ever saved.
    #[test]
    fn nothing_is_returned_when_every_announcement_fails_verification() {
        let (sender, receiver) = mpsc::channel();
        sender
            .send(found(&[Ipv4Addr::new(10, 9, 9, 9)]))
            .expect("open channel");
        assert!(first_verified(&receiver, Duration::from_millis(100), |_| false).is_none());
    }
}
