//! The live connections of one listener, each with its own kill switch.
//!
//! [`ConnectionTable::register_certificate`] is called by the viewer accept loop once TLS has
//! shown who is on the other end; the returned [`TrackedConnection`] keeps the entry alive
//! for the connection's whole life and removes it again when dropped. Revoking a device
//! calls [`ConnectionTable::kill`], which shuts that device's sockets down so its threads
//! unwind. The table also tells the [`EventHub`] when a device connects and disconnects.
//!
//! This is separate from [`super::shutdown::ConnectionSet`], which closes every socket at
//! shutdown and knows nothing about who is on the other end.

use super::events::EventHub;
use crate::pki::role::{ADMIN_CN_PREFIX, MONITOR_CN_PREFIX, WORKER_CN_PREFIX, subject_common_name};
use pleochroma_net::{
    admin::status::{CoordinatorEvent, fingerprint_short},
    messages::PeerRole,
    tls::{Fingerprint, fingerprint, fingerprint_to_hex},
};
use std::{
    collections::HashMap,
    net::{Shutdown, SocketAddr, TcpStream},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::SystemTime,
};

/// What the table knows about one live connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectionInfo {
    /// The table's id for this connection (unique within the table).
    pub id: u64,
    /// The device name: its certificate's Common Name without the role prefix.
    pub name: String,
    /// SHA-256 fingerprint of the device's certificate.
    pub fingerprint: Fingerprint,
    /// The certificate's role.
    pub role: PeerRole,
    /// The peer's address, when the socket could report one.
    pub peer: Option<SocketAddr>,
    /// When the connection was registered.
    pub connected_since: SystemTime,
}

/// One table entry: the public facts and the extra socket handle that kills it.
#[derive(Debug)]
struct Entry {
    info: ConnectionInfo,
    kill: Option<TcpStream>,
    /// Whether the connection shows in [`ConnectionTable::list`] and has been announced. A
    /// connection registered with [`ConnectionTable::register_certificate_pending`] starts
    /// unlisted (it can still be killed) and is listed by [`TrackedConnection::list_now`].
    listed: bool,
}

#[derive(Debug, Default)]
struct TableState {
    next: u64,
    entries: HashMap<u64, Entry>,
}

#[derive(Debug)]
struct TableShared {
    state: Mutex<TableState>,
    hub: Option<EventHub>,
}

impl TableShared {
    fn lock(&self) -> MutexGuard<'_, TableState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn publish(&self, event: &CoordinatorEvent) {
        if let Some(hub) = &self.hub {
            hub.publish(event);
        }
    }
}

/// The live, identified connections of one listener. Clones share the table.
#[derive(Debug, Clone)]
pub struct ConnectionTable {
    shared: Arc<TableShared>,
}

/// Keeps one connection in its [`ConnectionTable`]; dropping it removes the entry and
/// reports the device as disconnected.
#[derive(Debug)]
#[must_use = "dropping the guard removes the connection from the table at once"]
pub struct TrackedConnection {
    shared: Arc<TableShared>,
    id: u64,
}

/// The device name a Common Name stands for: the name without its role prefix.
#[must_use]
pub fn device_name_of_common_name(common_name: &str) -> &str {
    [WORKER_CN_PREFIX, MONITOR_CN_PREFIX, ADMIN_CN_PREFIX]
        .iter()
        .find_map(|prefix| common_name.strip_prefix(prefix))
        .unwrap_or(common_name)
}

impl ConnectionTable {
    /// An empty table that publishes connect and disconnect events to `hub`.
    #[must_use]
    pub fn new(hub: EventHub) -> Self {
        Self::build(Some(hub))
    }

    /// An empty table that publishes nothing.
    #[must_use]
    pub fn silent() -> Self {
        Self::build(None)
    }

    fn build(hub: Option<EventHub>) -> Self {
        Self {
            shared: Arc::new(TableShared {
                state: Mutex::new(TableState::default()),
                hub,
            }),
        }
    }

    /// Adds a connection whose client certificate is `certificate`.
    ///
    /// The name is the certificate's Common Name without its role prefix (the short
    /// fingerprint when there is none). `kill` is a second handle to the connection's socket;
    /// without one the entry is listed but [`Self::kill`] cannot close it.
    pub fn register_certificate(
        &self,
        certificate: &rustls::pki_types::CertificateDer<'_>,
        role: PeerRole,
        peer: Option<SocketAddr>,
        kill: Option<TcpStream>,
    ) -> TrackedConnection {
        let (name, fp) = certificate_identity(certificate);
        self.insert(name, fp, role, peer, kill, true)
    }

    /// [`Self::register_certificate`] for a connection that has not said who it is yet.
    ///
    /// It can be killed (revoking its device closes it) but is neither listed nor announced until
    /// [`TrackedConnection::list_now`] runs, which the connection handler does once the peer's
    /// `HELLO` was accepted. A connection that is refused or never speaks never shows as connected.
    pub fn register_certificate_pending(
        &self,
        certificate: &rustls::pki_types::CertificateDer<'_>,
        role: PeerRole,
        peer: Option<SocketAddr>,
        kill: Option<TcpStream>,
    ) -> TrackedConnection {
        let (name, fp) = certificate_identity(certificate);
        self.insert(name, fp, role, peer, kill, false)
    }

    /// Adds a connection with a known `name` and `fp`. See [`Self::register_certificate`].
    pub fn register(
        &self,
        name: String,
        fp: Fingerprint,
        role: PeerRole,
        peer: Option<SocketAddr>,
        kill: Option<TcpStream>,
    ) -> TrackedConnection {
        self.insert(name, fp, role, peer, kill, true)
    }

    fn insert(
        &self,
        name: String,
        fp: Fingerprint,
        role: PeerRole,
        peer: Option<SocketAddr>,
        kill: Option<TcpStream>,
        listed: bool,
    ) -> TrackedConnection {
        let (id, event) = {
            let mut state = self.shared.lock();
            let id = state.next;
            state.next += 1;
            let event = CoordinatorEvent::DeviceConnected {
                name: name.clone(),
                fingerprint: fingerprint_to_hex(&fp),
                role,
                peer,
            };
            state.entries.insert(
                id,
                Entry {
                    info: ConnectionInfo {
                        id,
                        name,
                        fingerprint: fp,
                        role,
                        peer,
                        connected_since: SystemTime::now(),
                    },
                    kill,
                    listed,
                },
            );
            drop(state);
            (id, event)
        };
        if listed {
            self.shared.publish(&event);
        }
        TrackedConnection {
            shared: Arc::clone(&self.shared),
            id,
        }
    }

    /// Every listed live connection, oldest first.
    #[must_use]
    pub fn list(&self) -> Vec<ConnectionInfo> {
        let mut list: Vec<ConnectionInfo> = self
            .shared
            .lock()
            .entries
            .values()
            .filter(|entry| entry.listed)
            .map(|entry| entry.info.clone())
            .collect();
        list.sort_by_key(|info| info.id);
        list
    }

    /// The live connections of the device with fingerprint `fp`.
    #[must_use]
    pub fn for_fingerprint(&self, fp: &Fingerprint) -> Vec<ConnectionInfo> {
        self.list()
            .into_iter()
            .filter(|info| info.fingerprint == *fp)
            .collect()
    }

    /// How many connections are listed.
    #[must_use]
    pub fn len(&self) -> usize {
        self.shared
            .lock()
            .entries
            .values()
            .filter(|entry| entry.listed)
            .count()
    }

    /// Whether no connection is live.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Shuts down every socket of the device with fingerprint `fp`, in both directions, so the
    /// threads serving it return an I/O error and end.
    ///
    /// Returns how many entries had a kill switch. The entries leave the table when their threads
    /// drop their guards.
    #[must_use]
    pub fn kill(&self, fp: &Fingerprint) -> usize {
        let state = self.shared.lock();
        state
            .entries
            .values()
            .filter(|entry| entry.info.fingerprint == *fp)
            .filter_map(|entry| entry.kill.as_ref())
            .inspect(|stream| {
                let _ = stream.shutdown(Shutdown::Both);
            })
            .count()
    }

    /// Shuts down the one connection with table id `id`, in both directions. Returns
    /// whether it was listed with a kill switch.
    #[must_use]
    pub fn kill_connection(&self, id: u64) -> bool {
        let state = self.shared.lock();
        state
            .entries
            .get(&id)
            .and_then(|entry| entry.kill.as_ref())
            .is_some_and(|stream| {
                let _ = stream.shutdown(Shutdown::Both);
                true
            })
    }

    /// Gives every live connection of `fp` the new display `name`.
    pub fn rename(&self, fp: &Fingerprint, name: &str) {
        let mut state = self.shared.lock();
        for entry in state.entries.values_mut() {
            if entry.info.fingerprint == *fp {
                entry.info.name = name.to_string();
            }
        }
    }
}

/// The name and fingerprint a client certificate stands for: its Common Name without the role
/// prefix (the short fingerprint when there is none).
fn certificate_identity(
    certificate: &rustls::pki_types::CertificateDer<'_>,
) -> (String, Fingerprint) {
    let fp = fingerprint(certificate);
    let name = subject_common_name(certificate.as_ref())
        .ok()
        .flatten()
        .map(|cn| device_name_of_common_name(&cn).to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| fingerprint_short(&fingerprint_to_hex(&fp)));
    (name, fp)
}

impl TrackedConnection {
    /// Lists a connection registered with [`ConnectionTable::register_certificate_pending`]
    /// and announces it as connected. Does nothing for one that is listed already.
    pub fn list_now(&self) {
        let event = {
            let mut state = self.shared.lock();
            state
                .entries
                .get_mut(&self.id)
                .filter(|entry| !entry.listed)
                .map(|entry| {
                    entry.listed = true;
                    CoordinatorEvent::DeviceConnected {
                        name: entry.info.name.clone(),
                        fingerprint: fingerprint_to_hex(&entry.info.fingerprint),
                        role: entry.info.role,
                        peer: entry.info.peer,
                    }
                })
        };
        if let Some(event) = event {
            self.shared.publish(&event);
        }
    }
}

impl Drop for TrackedConnection {
    fn drop(&mut self) {
        let removed = self.shared.lock().entries.remove(&self.id);
        if let Some(entry) = removed
            && entry.listed
        {
            self.shared.publish(&CoordinatorEvent::DeviceDisconnected {
                name: entry.info.name,
                fingerprint: fingerprint_to_hex(&entry.info.fingerprint),
                role: entry.info.role,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ConnectionTable, device_name_of_common_name};
    use crate::control::events::EventHub;
    use pleochroma_net::{admin::status::CoordinatorEvent, messages::PeerRole};
    use std::{
        io::Read,
        net::{TcpListener, TcpStream},
        thread,
        time::Duration,
    };

    fn pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        (client, server)
    }

    #[test]
    fn names_lose_their_role_prefix() {
        assert_eq!(device_name_of_common_name("worker:attic"), "attic");
        assert_eq!(device_name_of_common_name("monitor:wall"), "wall");
        assert_eq!(device_name_of_common_name("admin:ops"), "ops");
        assert_eq!(device_name_of_common_name("laptop"), "laptop");
    }

    #[test]
    fn register_lists_the_connection_and_drop_removes_it_with_events() {
        let hub = EventHub::new();
        let events = hub.subscribe();
        let table = ConnectionTable::new(hub);
        let (_client, server) = pair();
        let peer = server.peer_addr().ok();

        let tracked = table.register(
            "S23".to_string(),
            [1; 32],
            PeerRole::Viewer,
            peer,
            server.try_clone().ok(),
        );
        assert_eq!(table.len(), 1);
        let listed = table.list();
        assert_eq!(listed[0].name, "S23");
        assert_eq!(listed[0].role, PeerRole::Viewer);
        assert_eq!(listed[0].peer, peer);
        assert_eq!(table.for_fingerprint(&[1; 32]).len(), 1);
        assert!(table.for_fingerprint(&[2; 32]).is_empty());
        assert!(matches!(
            events.try_recv().unwrap(),
            CoordinatorEvent::DeviceConnected { name, role: PeerRole::Viewer, .. } if name == "S23"
        ));

        drop(tracked);
        assert!(table.is_empty());
        assert!(matches!(
            events.try_recv().unwrap(),
            CoordinatorEvent::DeviceDisconnected { name, .. } if name == "S23"
        ));
    }

    #[test]
    fn a_pending_connection_is_not_listed_or_announced_until_it_says_hello() {
        let hub = EventHub::new();
        let events = hub.subscribe();
        let table = ConnectionTable::new(hub);
        let der = rustls::pki_types::CertificateDer::from(vec![0_u8; 8]);

        let tracked = table.register_certificate_pending(&der, PeerRole::Viewer, None, None);
        assert!(table.is_empty(), "not listed before the HELLO");
        assert!(events.try_recv().is_err(), "nothing announced");

        tracked.list_now();
        assert_eq!(table.len(), 1);
        assert!(matches!(
            events.try_recv().unwrap(),
            CoordinatorEvent::DeviceConnected { .. }
        ));
        tracked.list_now();
        assert!(events.try_recv().is_err(), "announced once");

        drop(tracked);
        assert!(matches!(
            events.try_recv().unwrap(),
            CoordinatorEvent::DeviceDisconnected { .. }
        ));

        // A refused connection leaves without a trace.
        let refused = table.register_certificate_pending(&der, PeerRole::Viewer, None, None);
        drop(refused);
        assert!(events.try_recv().is_err());
    }

    #[test]
    fn kill_closes_only_the_matching_device_and_rename_changes_the_name() {
        let table = ConnectionTable::silent();
        let (_client_a, mut server_a) = pair();
        let (mut client_b, server_b) = pair();
        let _a = table.register(
            "a".to_string(),
            [1; 32],
            PeerRole::Viewer,
            None,
            server_a.try_clone().ok(),
        );
        let _b = table.register(
            "b".to_string(),
            [2; 32],
            PeerRole::Viewer,
            None,
            server_b.try_clone().ok(),
        );

        table.rename(&[1; 32], "Alpha");
        assert_eq!(table.for_fingerprint(&[1; 32])[0].name, "Alpha");
        assert_eq!(table.for_fingerprint(&[2; 32])[0].name, "b");

        let reader = thread::spawn(move || {
            let mut byte = [0_u8; 1];
            // Blocks until the kill switch closes the socket: the client never writes.
            server_a.read(&mut byte)
        });
        thread::sleep(Duration::from_millis(100));
        assert_eq!(table.kill(&[1; 32]), 1);
        let outcome = reader.join().unwrap();
        assert!(matches!(outcome, Ok(0) | Err(_)), "{outcome:?}");

        // The other device's socket still works.
        client_b
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        let mut byte = [0_u8; 1];
        let still_open = client_b.read(&mut byte);
        assert!(
            matches!(&still_open, Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut)),
            "the other connection must stay open: {still_open:?}"
        );
        assert_eq!(table.kill(&[9; 32]), 0);
    }

    #[test]
    fn kill_connection_closes_one_connection_of_a_device() {
        let table = ConnectionTable::silent();
        let (_client_a, mut server_a) = pair();
        let (mut client_b, server_b) = pair();
        let a = table.register(
            "w".to_string(),
            [1; 32],
            PeerRole::Worker,
            None,
            server_a.try_clone().ok(),
        );
        let b = table.register(
            "w".to_string(),
            [1; 32],
            PeerRole::Worker,
            None,
            server_b.try_clone().ok(),
        );
        let ids: Vec<u64> = table.list().iter().map(|info| info.id).collect();
        assert_eq!(ids.len(), 2);

        let reader = thread::spawn(move || {
            let mut byte = [0_u8; 1];
            server_a.read(&mut byte)
        });
        thread::sleep(Duration::from_millis(100));
        assert!(table.kill_connection(ids[0]));
        let outcome = reader.join().unwrap();
        assert!(matches!(outcome, Ok(0) | Err(_)), "{outcome:?}");

        client_b
            .set_read_timeout(Some(Duration::from_millis(100)))
            .unwrap();
        let mut byte = [0_u8; 1];
        let still_open = client_b.read(&mut byte);
        assert!(
            matches!(&still_open, Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut)),
            "the second connection of the device stays open: {still_open:?}"
        );
        assert!(!table.kill_connection(9_999));
        drop((a, b));
    }
}
