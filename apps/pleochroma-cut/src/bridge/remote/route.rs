//! Where the saved remote can be reached: its address list and how its certificate is verified.
//!
//! The connection code takes a [`crate::settings::WorkerSettings`] (one `address`, one
//! certificate folder). A paired remote has up to three addresses and is verified by its fixed
//! certificate name, so the settings layer publishes that extra knowledge here, keyed by the
//! certificate folder, whenever the saved endpoint loads, changes or is found at a new address
//! ([`publish`]). [`super::remote_render`] asks [`lookup`] before it dials and before it builds the
//! TLS configuration.
//!
//! A folder nothing was published for (a hand-entered remote, or a remote saved by 0.11.1)
//! behaves exactly as before: one address, verified by the dialed name.

use std::{
    collections::BTreeMap,
    sync::{Mutex, PoisonError},
};

/// How many saved addresses one connection attempt dials (a pairing link carries three).
pub const MAX_DIALED_ADDRESSES: usize = 3;

/// The reachability facts of one saved remote.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Route {
    /// `host:port` addresses in the order they are tried.
    pub addresses: Vec<String>,
    /// Whether the coordinator is verified by its fixed certificate name, whatever IP is dialed.
    /// On for a remote paired through a pairing link, off for one entered by hand.
    pub fixed_name: bool,
}

/// How many left-behind addresses are kept per remote.
const MAX_RETIRED_ADDRESSES: usize = 8;

static ROUTES: Mutex<BTreeMap<String, Route>> = Mutex::new(BTreeMap::new());

/// Addresses a fixed-name remote has moved away from, by certificate folder. A long-lived holder
/// (the library source, a mirror, the export capability probe) keeps the `WorkerSettings` it was
/// built with; [`lookup_for`] still finds the moved remote's route for such an address, so the
/// holder dials the new addresses too.
static RETIRED: Mutex<BTreeMap<String, Vec<String>>> = Mutex::new(BTreeMap::new());

/// Remembers the route of the remote whose certificates live in `cert_dir`, replacing any
/// earlier one. An empty `cert_dir` is ignored: there is nothing to key it by.
///
/// When a fixed-name remote is published with a different address list, the addresses it left
/// are kept as retired ones (see [`lookup_for`]); any other change forgets them.
pub fn publish(cert_dir: &str, addresses: &[String], fixed_name: bool) {
    if cert_dir.trim().is_empty() {
        return;
    }
    let left_behind: Vec<String> = {
        let mut routes = ROUTES.lock().unwrap_or_else(PoisonError::into_inner);
        let earlier = routes.insert(
            cert_dir.to_owned(),
            Route {
                addresses: addresses.to_vec(),
                fixed_name,
            },
        );
        drop(routes);
        match earlier {
            Some(old) if old.fixed_name && fixed_name => old
                .addresses
                .into_iter()
                .filter(|a| !addresses.iter().any(|now| now.trim() == a.trim()))
                .collect(),
            _ => Vec::new(),
        }
    };
    let mut retired = RETIRED.lock().unwrap_or_else(PoisonError::into_inner);
    if !fixed_name {
        retired.remove(cert_dir);
        return;
    }
    let list = retired.entry(cert_dir.to_owned()).or_default();
    list.retain(|a| !addresses.iter().any(|now| now.trim() == a.trim()));
    for address in left_behind {
        if !list.contains(&address) {
            list.push(address);
        }
    }
    if list.len() > MAX_RETIRED_ADDRESSES {
        let excess = list.len() - MAX_RETIRED_ADDRESSES;
        list.drain(..excess);
    }
    drop(retired);
}

/// The route published for `cert_dir`, if any.
#[must_use]
pub fn lookup(cert_dir: &str) -> Option<Route> {
    ROUTES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(cert_dir)
        .cloned()
}

/// Forgets the route of `cert_dir`: the remote saved there was removed, so a later form that
/// reuses the folder starts as a hand-entered remote again.
pub fn remove(cert_dir: &str) {
    ROUTES
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(cert_dir);
    RETIRED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(cert_dir);
}

/// The route published for `cert_dir`, but only when `address` is one of its addresses or, for
/// a fixed-name remote, one it moved away from. A test of an unsaved form names its own
/// address; it must not inherit the saved remote's other addresses or its fixed-name
/// verification.
#[must_use]
pub fn lookup_for(cert_dir: &str, address: &str) -> Option<Route> {
    let address = address.trim();
    let route = lookup(cert_dir)?;
    if route.addresses.iter().any(|a| a.trim() == address) {
        return Some(route);
    }
    let was_retired = route.fixed_name
        && RETIRED
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(cert_dir)
            .is_some_and(|list| list.iter().any(|a| a.trim() == address));
    was_retired.then_some(route)
}

/// The addresses to dial, in order: the worker's own `primary` address first, then the rest of
/// the published route that differs from it, at most [`MAX_DIALED_ADDRESSES`] in all.
///
/// Pure, so the order is testable without a socket.
#[must_use]
pub fn dial_order(primary: &str, route: Option<&Route>) -> Vec<String> {
    let mut order: Vec<String> = Vec::new();
    let candidates = std::iter::once(primary).chain(
        route
            .into_iter()
            .flat_map(|r| r.addresses.iter().map(String::as_str)),
    );
    for candidate in candidates {
        let candidate = candidate.trim();
        if !candidate.is_empty() && !order.iter().any(|seen| seen == candidate) {
            order.push(candidate.to_owned());
        }
    }
    order.truncate(MAX_DIALED_ADDRESSES);
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    fn route(addresses: &[&str], fixed_name: bool) -> Route {
        Route {
            addresses: addresses.iter().map(|a| (*a).to_owned()).collect(),
            fixed_name,
        }
    }

    #[test]
    fn without_a_route_only_the_saved_address_is_dialed() {
        assert_eq!(dial_order("coord.lan:7878", None), vec!["coord.lan:7878"]);
        assert!(dial_order("  ", None).is_empty());
    }

    #[test]
    fn the_saved_address_goes_first_and_the_list_keeps_its_order() {
        let r = route(
            &["192.168.1.20:7878", "10.0.0.5:7878", "172.16.0.9:7878"],
            true,
        );
        assert_eq!(
            dial_order("192.168.1.20:7878", Some(&r)),
            vec!["192.168.1.20:7878", "10.0.0.5:7878", "172.16.0.9:7878"]
        );
        // A primary that is not first in the list still leads, without being repeated.
        assert_eq!(
            dial_order("10.0.0.5:7878", Some(&r)),
            vec!["10.0.0.5:7878", "192.168.1.20:7878", "172.16.0.9:7878"]
        );
    }

    #[test]
    fn duplicates_blanks_and_extra_addresses_are_dropped() {
        let r = route(&["a:1", " ", "a:1", "b:1", "c:1", "d:1"], false);
        assert_eq!(dial_order("a:1", Some(&r)), vec!["a:1", "b:1", "c:1"]);
    }

    #[test]
    fn a_published_route_is_found_by_its_certificate_folder() {
        publish("C:/certs/route-test", &["x:1".to_owned()], true);
        let found = lookup("C:/certs/route-test").expect("published");
        assert!(found.fixed_name);
        assert_eq!(found.addresses, vec!["x:1".to_owned()]);
        assert_eq!(lookup("C:/certs/never-published"), None);
        // A blank folder is never stored.
        publish("", &["y:1".to_owned()], true);
        assert_eq!(lookup(""), None);
    }

    #[test]
    fn a_moved_fixed_name_remote_still_answers_for_its_old_address() {
        let dir = "C:/certs/route-moved-test";
        publish(dir, &["a:1".to_owned()], true);
        publish(dir, &["b:1".to_owned()], true);
        let found = lookup_for(dir, "a:1").expect("old address still maps to the route");
        assert_eq!(found.addresses, vec!["b:1".to_owned()]);
        // A hand-entered remote forgets what it left.
        publish(dir, &["c:1".to_owned()], false);
        assert_eq!(lookup_for(dir, "a:1"), None);
        remove(dir);
    }

    #[test]
    fn a_route_is_used_only_for_an_address_it_lists_and_ends_on_remove() {
        publish(
            "C:/certs/route-for-test",
            &["a:1".to_owned(), "b:1".to_owned()],
            true,
        );
        assert!(lookup_for("C:/certs/route-for-test", " b:1 ").is_some());
        // An unsaved form with its own address does not inherit the route.
        assert_eq!(lookup_for("C:/certs/route-for-test", "other:1"), None);
        remove("C:/certs/route-for-test");
        assert_eq!(lookup("C:/certs/route-for-test"), None);
    }
}
