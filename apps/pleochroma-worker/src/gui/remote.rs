//! The backend for a coordinator on another computer, managed over the administration
//! protocol ([`pleochroma_net::admin::protocol`]).
//!
//! [`open`] returns at once. A background thread connects to the saved addresses all at once,
//! sets up mutual TLS with the credentials in `remotes/<id>/` ([`crate::settings::remotes`]),
//! says HELLO as a monitor or administrator and subscribes. From then on every pushed status is
//! cached (so [`CoordinatorBackend::status`] never waits on the network), events fan out to the
//! receivers [`CoordinatorBackend::events`] handed out, and log lines go into a bounded cache.
//! A command sends an [`AdminRequest`] and waits up to ten seconds for the response with the
//! same `req_id`; a refusal comes back as the coordinator's own plain words.
//!
//! The link is watched: 45 seconds without any push (the coordinator sends a heartbeat every
//! 15) counts as lost. A lost or failed connection is retried after 1, 2, 5, 10 and then every 30
//! seconds. When no saved address answers, the coordinator is looked for on the network by its
//! certificate authority ([`pleochroma_net::discovery::browse`]) and the saved addresses are
//! updated if it moved.
//!
//! Until the session is up, the status says [`CoordinatorState::Starting`] (the window shows
//! "Connecting..."); after a round of failures it says `Error("Can't reach <name>")`, or a plain
//! sentence for a refusal or a different build. The last status that arrived is kept, so a page
//! can show it greyed while the link is down.
//!
//! Nothing here logs or returns a key, a pairing code or a token.

use crate::{
    control::backend::{
        AccessRole, Capabilities, CoordinatorBackend, CoordinatorEvent, CoordinatorState,
        CoordinatorStatus, Fingerprint, IssuedToken, JobId, LogLevel, LogLine, PeerRole,
        SettingsPatch, TokenId, WorkerId,
    },
    settings::remotes::{self, RemoteEntry, RemoteId},
};
use pleochroma_net::{
    admin::protocol::{AdminPush, AdminReply, AdminRequest, HEARTBEAT_TIMEOUT},
    client::{ClientError, handshake_with_hello},
    discovery, framing, handshake,
    messages::{self, Hello, error_codes},
    tls,
};
use rustls::pki_types::ServerName;
use std::{
    collections::{HashMap, VecDeque},
    io::{Read, Write},
    net::{IpAddr, SocketAddr, TcpStream},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, MutexGuard, PoisonError,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc,
    },
    time::{Duration, Instant},
};

/// How long a command waits for its response.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);

/// The waits between connection attempts; the last one repeats.
const BACKOFF: [Duration; 5] = [
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(5),
    Duration::from_secs(10),
    Duration::from_secs(30),
];

/// How long a session must last before the waits between attempts start over.
const STABLE_SESSION: Duration = Duration::from_secs(10);

/// How long one TCP connect to a saved address may take.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

/// How long the network is searched for a coordinator that moved.
const REDISCOVER_WAIT: Duration = Duration::from_secs(4);

/// The least time between two searches of the network.
const REDISCOVER_EVERY: Duration = Duration::from_secs(30);

/// How long a read or write on a connected socket may block before the loop looks around.
const POLL: Duration = Duration::from_millis(100);

/// How long the rest of a message may take once its first byte has arrived.
const MESSAGE_TIMEOUT: Duration = Duration::from_secs(10);

/// How many log lines are kept.
const LOG_CACHE: usize = 2000;

/// The log level the session subscribes at; lines more detailed than this are not pushed. It is
/// Debug so the Log page's "Details" switch shows more for a remote coordinator too.
const SUBSCRIBED_LEVEL: LogLevel = LogLevel::Debug;

/// The waits and limits of one backend. Production values come from [`Timing::production`];
/// tests shorten them.
#[derive(Debug, Clone)]
struct Timing {
    /// Socket wait inside the session loop.
    poll: Duration,
    /// Silence after which the link counts as lost.
    heartbeat_timeout: Duration,
    /// How long a command waits for its response.
    request_timeout: Duration,
    /// Waits between connection attempts; the last one repeats.
    backoff: Vec<Duration>,
    /// How long one TCP connect may take.
    connect_timeout: Duration,
    /// Whether to search the network when no saved address answers.
    rediscover: bool,
}

impl Timing {
    /// The values the window runs with.
    fn production() -> Self {
        Self {
            poll: POLL,
            heartbeat_timeout: HEARTBEAT_TIMEOUT,
            request_timeout: REQUEST_TIMEOUT,
            backoff: BACKOFF.to_vec(),
            connect_timeout: CONNECT_TIMEOUT,
            rediscover: true,
        }
    }
}

/// Where the link stands.
#[derive(Debug, Clone)]
enum Link {
    /// Trying (the first time, or again after a loss).
    Connecting,
    /// A session is up.
    Connected,
    /// The last round of attempts failed; the text is the plain reason.
    Down(String),
}

/// Why one attempt to reach the coordinator failed.
#[derive(Debug, Clone, PartialEq, Eq)]
enum ConnectFail {
    /// No address answered, or nothing was listening.
    Unreachable,
    /// The saved credentials cannot be read.
    Credentials,
    /// The coordinator answered but did not accept this computer (a removed access, a wrong
    /// certificate authority).
    Rejected,
    /// The coordinator said no, in its own words.
    Refused(String),
    /// A different Pleochroma build; the version when it is known.
    Mismatch(Option<String>),
}

impl ConnectFail {
    /// Higher wins when several addresses fail differently.
    const fn rank(&self) -> u8 {
        match self {
            Self::Unreachable => 0,
            Self::Credentials => 1,
            Self::Rejected => 2,
            Self::Refused(_) => 3,
            Self::Mismatch(_) => 4,
        }
    }

    /// The failure in plain words for the status line.
    fn text(&self, name: &str, known_version: Option<&str>) -> String {
        match self {
            Self::Unreachable => format!("Can't reach {name}"),
            Self::Credentials => {
                format!("The saved access for {name} cannot be read. Remove it and pair again.")
            }
            Self::Rejected => format!(
                "{name} did not accept this computer. Its access may have been removed there. \
                 Remove it here and pair again."
            ),
            Self::Refused(why) => format!("{name} refused the connection: {why}"),
            Self::Mismatch(version) => version.as_deref().or(known_version).map_or_else(
                || {
                    format!(
                        "{name} runs a different Pleochroma build. Use the same version on both."
                    )
                },
                |version| {
                    format!(
                        "{name} runs a different Pleochroma build ({version}). Use the same \
                         version on both."
                    )
                },
            ),
        }
    }
}

/// A connected, handshaken TLS stream.
type Tls = rustls::StreamOwned<rustls::ClientConnection, TcpStream>;

/// What the connection thread and the callers share.
struct Shared {
    /// The saved entry; the name and addresses can change while running.
    entry: Mutex<RemoteEntry>,
    /// Where `ca.pem`, `client.pem` and `client.key` are.
    cert_dir: PathBuf,
    /// The settings folder to write changes of the entry to, if any.
    base: Option<PathBuf>,
    timing: Timing,
    link: Mutex<Link>,
    /// The last status the coordinator pushed.
    status: Mutex<Option<CoordinatorStatus>>,
    /// The coordinator's version from its last status.
    version: Mutex<Option<String>>,
    subscribers: Mutex<Vec<mpsc::Sender<CoordinatorEvent>>>,
    logs: Mutex<VecDeque<LogLine>>,
    /// Commands waiting for their response, by `req_id`.
    pending: Mutex<
        HashMap<u64, mpsc::Sender<Result<AdminReply, pleochroma_net::admin::protocol::AdminError>>>,
    >,
    /// Where commands go to reach the connection thread; `None` while not connected.
    sender: Mutex<Option<mpsc::Sender<AdminRequest>>>,
    next_id: AtomicU64,
    stop: AtomicBool,
}

/// Locks `m`, ignoring a lock poisoned by a panic elsewhere.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

impl Shared {
    fn new(entry: RemoteEntry, cert_dir: PathBuf, base: Option<PathBuf>, timing: Timing) -> Self {
        Self {
            entry: Mutex::new(entry),
            cert_dir,
            base,
            timing,
            link: Mutex::new(Link::Connecting),
            status: Mutex::new(None),
            version: Mutex::new(None),
            subscribers: Mutex::new(Vec::new()),
            logs: Mutex::new(VecDeque::new()),
            pending: Mutex::new(HashMap::new()),
            sender: Mutex::new(None),
            next_id: AtomicU64::new(1),
            stop: AtomicBool::new(false),
        }
    }

    fn stopped(&self) -> bool {
        self.stop.load(Ordering::SeqCst)
    }

    fn name(&self) -> String {
        lock(&self.entry).name.clone()
    }

    fn set_link(&self, link: Link) {
        *lock(&self.link) = link;
    }

    /// Sleeps `total`, waking early when the backend is dropped.
    fn sleep(&self, total: Duration) {
        let end = Instant::now() + total;
        while !self.stopped() {
            let left = end.saturating_duration_since(Instant::now());
            if left.is_zero() {
                break;
            }
            std::thread::sleep(left.min(Duration::from_millis(50)));
        }
    }

    /// Sends `event` to every receiver that still listens.
    fn fan_out(&self, event: &CoordinatorEvent) {
        lock(&self.subscribers).retain(|tx| tx.send(event.clone()).is_ok());
    }

    /// Ends the session's hold on the callers: no more commands, and waiting ones are released.
    fn drop_session(&self) {
        *lock(&self.sender) = None;
        lock(&self.pending).clear();
    }

    /// Takes in one message from the coordinator. An `Err` ends the session.
    fn handle_push(&self, push: AdminPush, subscribe_id: u64) -> Result<(), String> {
        match push {
            AdminPush::Status(status) => self.take_status(status),
            AdminPush::Event(event) => self.fan_out(&event),
            AdminPush::Log(line) => {
                let mut logs = lock(&self.logs);
                if logs.len() >= LOG_CACHE {
                    logs.pop_front();
                }
                logs.push_back(line);
            }
            AdminPush::Heartbeat => {}
            AdminPush::Response { req_id, result } => {
                if req_id == subscribe_id {
                    if let Err(refused) = result {
                        return Err(refused.message);
                    }
                } else {
                    let waiting = lock(&self.pending).remove(&req_id);
                    if let Some(waiting) = waiting {
                        // The caller may have given up already.
                        let _ = waiting.send(result);
                    }
                }
            }
        }
        Ok(())
    }

    /// Caches a pushed status and keeps the saved name in step with the coordinator's own.
    fn take_status(&self, status: CoordinatorStatus) {
        *lock(&self.version) = Some(status.version.clone());
        let renamed = {
            let mut entry = lock(&self.entry);
            if !status.name.trim().is_empty() && entry.name != status.name {
                entry.name.clone_from(&status.name);
                Some((entry.id.clone(), status.name.clone()))
            } else {
                None
            }
        };
        if let (Some((id, name)), Some(base)) = (renamed, &self.base) {
            remotes::update_in(base, &id, |e| e.name = name);
        }
        *lock(&self.status) = Some(status);
    }

    /// Writes "seen now" into the entry.
    fn mark_seen(&self) {
        let now = remotes::unix_now();
        let id = {
            let mut entry = lock(&self.entry);
            entry.last_seen = Some(now);
            entry.id.clone()
        };
        if let Some(base) = &self.base {
            remotes::update_in(base, &id, |e| e.last_seen = Some(now));
        }
    }
}

/// A coordinator on another computer, driven over the administration protocol.
pub struct RemoteBackend {
    shared: Arc<Shared>,
}

impl Drop for RemoteBackend {
    fn drop(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
    }
}

/// Starts the connection to the saved coordinator `id` and returns at once.
///
/// `status()` says "Connecting..." until the session is up. An id that is not saved (or no
/// settings folder yet) gives a backend that only says so.
#[must_use]
pub fn open(id: &RemoteId) -> Arc<dyn CoordinatorBackend> {
    let found =
        remotes::base_dir().and_then(|base| remotes::find_in(&base, id).map(|entry| (base, entry)));
    found.map_or_else(
        || Arc::new(RemoteBackend::unavailable(id)),
        |(base, entry)| {
            let cert_dir = remotes::cert_dir_in(&base, id);
            RemoteBackend::start(entry, cert_dir, Some(base), Timing::production())
        },
    )
}

impl RemoteBackend {
    /// A backend whose connection thread is running.
    fn start(
        entry: RemoteEntry,
        cert_dir: PathBuf,
        base: Option<PathBuf>,
        timing: Timing,
    ) -> Arc<Self> {
        let shared = Arc::new(Shared::new(entry, cert_dir, base, timing));
        let thread_shared = Arc::clone(&shared);
        let spawned = std::thread::Builder::new()
            .name("remote-admin".into())
            .spawn(move || run(&thread_shared));
        if spawned.is_err() {
            shared.set_link(Link::Down(
                "This computer could not start the connection. Close other programs and try again."
                    .to_owned(),
            ));
        }
        Arc::new(Self { shared })
    }

    /// A backend for an id that is not saved: it only reports that.
    fn unavailable(id: &RemoteId) -> Self {
        let entry = RemoteEntry {
            id: id.clone(),
            name: id.0.clone(),
            addresses: Vec::new(),
            role: AccessRole::Monitor,
            ca_fingerprint: String::new(),
            last_seen: None,
        };
        let shared = Shared::new(entry, PathBuf::new(), None, Timing::production());
        shared.set_link(Link::Down(
            "This coordinator is no longer saved on this computer.".to_owned(),
        ));
        Self {
            shared: Arc::new(shared),
        }
    }

    /// Sends one request and waits for the response with its `req_id`.
    fn request(&self, build: impl FnOnce(u64) -> AdminRequest) -> Result<AdminReply, String> {
        let shared = &self.shared;
        let name = shared.name();
        let not_connected = || format!("Not connected to {name} right now. Try again in a moment.");
        let id = shared.next_id.fetch_add(1, Ordering::SeqCst);
        let sender = lock(&shared.sender).clone().ok_or_else(not_connected)?;
        let (reply_tx, reply_rx) = mpsc::channel();
        lock(&shared.pending).insert(id, reply_tx);
        if sender.send(build(id)).is_err() {
            lock(&shared.pending).remove(&id);
            return Err(not_connected());
        }
        match reply_rx.recv_timeout(shared.timing.request_timeout) {
            Ok(Ok(reply)) => Ok(reply),
            Ok(Err(refused)) => Err(refused.message),
            Err(mpsc::RecvTimeoutError::Timeout) => {
                lock(&shared.pending).remove(&id);
                Err(format!("{name} did not answer in time. Try again."))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => Err(format!(
                "The connection to {name} was lost. Try again once it is back."
            )),
        }
    }

    /// [`Self::request`] for a command that answers `Done`.
    fn command(&self, build: impl FnOnce(u64) -> AdminRequest) -> Result<(), String> {
        if matches!(self.request(build)?, AdminReply::Done) {
            Ok(())
        } else {
            Err("The coordinator sent an answer this computer did not expect.".to_owned())
        }
    }
}

impl CoordinatorBackend for RemoteBackend {
    fn status(&self) -> CoordinatorStatus {
        let shared = &self.shared;
        let link = lock(&shared.link).clone();
        let cached = lock(&shared.status).clone();
        let placeholder = |state| {
            CoordinatorStatus::minimal(
                shared.name(),
                env!("CARGO_PKG_VERSION"),
                messages::PROTOCOL_VERSION,
                state,
            )
        };
        match link {
            Link::Connected => cached.unwrap_or_else(|| placeholder(CoordinatorState::Starting)),
            Link::Connecting => {
                let mut status = cached.unwrap_or_else(|| placeholder(CoordinatorState::Starting));
                status.state = CoordinatorState::Starting;
                status
            }
            Link::Down(text) => {
                let state = CoordinatorState::Error(text);
                let mut status = cached.unwrap_or_else(|| placeholder(state.clone()));
                status.state = state;
                status
            }
        }
    }

    fn events(&self) -> mpsc::Receiver<CoordinatorEvent> {
        let (tx, rx) = mpsc::channel();
        lock(&self.shared.subscribers).push(tx);
        rx
    }

    fn logs(&self, min: tracing::Level, limit: usize) -> Vec<LogLine> {
        let min = LogLevel::from(min);
        let mut newest: Vec<LogLine> = lock(&self.shared.logs)
            .iter()
            .rev()
            .filter(|line| line.level.passes(min))
            .take(limit)
            .cloned()
            .collect();
        newest.reverse();
        newest
    }

    fn issue_token(&self, role: PeerRole, default_name: &str) -> Result<IssuedToken, String> {
        let reply = self.request(|req_id| AdminRequest::IssueToken {
            req_id,
            role,
            name: default_name.to_owned(),
        })?;
        if let AdminReply::Token(token) = reply {
            Ok(token)
        } else {
            Err("The coordinator sent an answer this computer did not expect.".to_owned())
        }
    }

    fn cancel_token(&self, id: TokenId) {
        // No answer is awaited: the trait gives no way to report one, and a stale code
        // expires by itself. The response to an unknown `req_id` is ignored.
        let Some(sender) = lock(&self.shared.sender).clone() else {
            return;
        };
        let req_id = self.shared.next_id.fetch_add(1, Ordering::SeqCst);
        let _ = sender.send(AdminRequest::CancelToken { req_id, id });
    }

    fn revoke(&self, fp: &Fingerprint) -> Result<(), String> {
        let fingerprint = tls::fingerprint_to_hex(fp);
        self.command(|req_id| AdminRequest::Revoke {
            req_id,
            fingerprint,
        })
    }

    fn rename(&self, fp: &Fingerprint, name: &str) -> Result<(), String> {
        let fingerprint = tls::fingerprint_to_hex(fp);
        self.command(|req_id| AdminRequest::Rename {
            req_id,
            fingerprint,
            name: name.to_owned(),
        })
    }

    fn cancel_job(&self, id: JobId) -> Result<(), String> {
        self.command(|req_id| AdminRequest::CancelJob { req_id, id })
    }

    fn delete_job(&self, id: JobId) -> Result<(), String> {
        self.command(|req_id| AdminRequest::DeleteJob { req_id, id })
    }

    fn disconnect_worker(&self, id: WorkerId) -> Result<(), String> {
        self.command(|req_id| AdminRequest::DisconnectWorker { req_id, id })
    }

    fn apply_settings(&self, patch: &SettingsPatch) -> Result<(), String> {
        let patch = patch.clone();
        self.command(|req_id| AdminRequest::ApplySettings { req_id, patch })
    }

    fn restart(&self) -> Result<(), String> {
        self.command(|req_id| AdminRequest::Restart { req_id })
    }

    fn capabilities(&self) -> Capabilities {
        let role = lock(&self.shared.entry).role;
        match role {
            AccessRole::Admin => Capabilities::admin(),
            AccessRole::Monitor | AccessRole::Local => Capabilities::monitor(),
        }
    }
}

/// The connection thread: connect, run the session, back off, repeat until dropped.
fn run(shared: &Arc<Shared>) {
    let mut attempt = 0_usize;
    let mut last_search: Option<Instant> = None;
    while !shared.stopped() {
        match connect(shared) {
            Ok(tls) => {
                shared.mark_seen();
                tracing::info!("remote coordinator {}: connected", shared.name());
                let started = Instant::now();
                let end = session(shared, tls);
                shared.drop_session();
                tracing::info!(
                    "remote coordinator {}: connection ended ({})",
                    shared.name(),
                    end.reason
                );
                if shared.stopped() {
                    return;
                }
                // A session that held for a while starts the waits over; one that was refused
                // or dropped at once keeps growing them, so a coordinator that says no is not
                // asked again every second.
                if started.elapsed() >= STABLE_SESSION && !end.refused {
                    attempt = 0;
                }
                if end.refused {
                    let known = lock(&shared.version).clone();
                    let fail = ConnectFail::Refused(end.reason);
                    shared.set_link(Link::Down(fail.text(&shared.name(), known.as_deref())));
                } else {
                    shared.set_link(Link::Connecting);
                }
            }
            Err(fail) => {
                let search_due = last_search.is_none_or(|t| t.elapsed() >= REDISCOVER_EVERY);
                if fail == ConnectFail::Unreachable && shared.timing.rediscover && search_due {
                    last_search = Some(Instant::now());
                    if rediscover(shared) {
                        // The coordinator moved: try the new addresses at once.
                        continue;
                    }
                }
                let known = lock(&shared.version).clone();
                shared.set_link(Link::Down(fail.text(&shared.name(), known.as_deref())));
            }
        }
        let wait = shared
            .timing
            .backoff
            .get(attempt)
            .or_else(|| shared.timing.backoff.last())
            .copied()
            .unwrap_or(Duration::from_secs(30));
        attempt += 1;
        shared.sleep(wait);
    }
}

/// Dials every saved address at once and handshakes on the first that answers.
fn connect(shared: &Arc<Shared>) -> Result<Tls, ConnectFail> {
    let config = client_config(&shared.cert_dir).map_err(|_| ConnectFail::Credentials)?;
    let (addresses, role) = {
        let entry = lock(&shared.entry);
        (entry.addresses.clone(), entry.role)
    };
    let peer_role = match role {
        AccessRole::Admin => PeerRole::Admin,
        AccessRole::Monitor | AccessRole::Local => PeerRole::Monitor,
    };
    let addrs: Vec<SocketAddr> = addresses.iter().filter_map(|a| a.parse().ok()).collect();
    if addrs.is_empty() {
        return Err(ConnectFail::Unreachable);
    }

    let (tx, rx) = mpsc::channel::<(SocketAddr, std::io::Result<TcpStream>)>();
    for addr in addrs {
        let tx = tx.clone();
        let timeout = shared.timing.connect_timeout;
        std::thread::spawn(move || {
            // The receiver is gone once another address won.
            let _ = tx.send((addr, TcpStream::connect_timeout(&addr, timeout)));
        });
    }
    drop(tx);

    let mut worst = ConnectFail::Unreachable;
    for (addr, connected) in rx {
        if shared.stopped() {
            return Err(ConnectFail::Unreachable);
        }
        let Ok(tcp) = connected else {
            continue;
        };
        let known = lock(&shared.version).clone();
        match handshake_over(tcp, addr, &config, peer_role, known.as_deref()) {
            Ok(tls) => return Ok(tls),
            Err(fail) => {
                if fail.rank() > worst.rank() {
                    worst = fail;
                }
            }
        }
    }
    Err(worst)
}

/// Mutual TLS and HELLO/WELCOME over a connected socket.
fn handshake_over(
    tcp: TcpStream,
    addr: SocketAddr,
    config: &Arc<rustls::ClientConfig>,
    role: PeerRole,
    known_version: Option<&str>,
) -> Result<Tls, ConnectFail> {
    let _ = tcp.set_nodelay(true);
    tcp.set_read_timeout(Some(MESSAGE_TIMEOUT))
        .and_then(|()| tcp.set_write_timeout(Some(MESSAGE_TIMEOUT)))
        .map_err(|_| ConnectFail::Unreachable)?;
    let server_name = ServerName::IpAddress(addr.ip().into());
    let conn = rustls::ClientConnection::new(Arc::clone(config), server_name)
        .map_err(|_| ConnectFail::Rejected)?;
    let mut tls = rustls::StreamOwned::new(conn, tcp);
    tls.conn
        .complete_io(&mut tls.sock)
        .map_err(|_| ConnectFail::Rejected)?;

    let hello = Hello {
        role,
        ..handshake::local_hello()
    };
    let welcome = handshake_with_hello(&mut tls, &hello).map_err(|e| match e {
        ClientError::Refused(refusal) if refusal.code == error_codes::BUILD_MISMATCH => {
            ConnectFail::Mismatch(known_version.map(str::to_owned))
        }
        ClientError::Refused(refusal) => ConnectFail::Refused(refusal.message),
        ClientError::Incompatible(handshake::Incompatible::ProtocolVersionMismatch {
            remote,
            ..
        }) => ConnectFail::Mismatch(Some(format!("protocol {remote}"))),
        ClientError::Incompatible(_) => ConnectFail::Mismatch(known_version.map(str::to_owned)),
        _ => ConnectFail::Rejected,
    })?;
    // An administrator needs the same protocol, not the same render build: the coordinator's
    // `WELCOME` to a Monitor or Admin carries no build identity (`UNKNOWN_BUILD_HASH`, see
    // `crate::admin::session::serve_connection`), so only the protocol version is compared
    // here. A build-hash check would refuse every coordinator as "a different build".
    if welcome.protocol_version != messages::PROTOCOL_VERSION {
        return Err(ConnectFail::Mismatch(Some(format!(
            "protocol {}",
            welcome.protocol_version
        ))));
    }
    Ok(tls)
}

/// The TLS client config for the saved credentials in `dir`.
fn client_config(dir: &Path) -> Result<Arc<rustls::ClientConfig>, String> {
    let ca = tls::load_ca(&dir.join(crate::pki::CA_CERT_FILE)).map_err(|e| e.to_string())?;
    let certs =
        tls::load_certs(&dir.join(crate::pki::CLIENT_CERT_FILE)).map_err(|e| e.to_string())?;
    let key =
        tls::load_private_key(&dir.join(crate::pki::CLIENT_KEY_FILE)).map_err(|e| e.to_string())?;
    tls::client_config_fixed_name(ca, certs, key).map_err(|e| e.to_string())
}

/// Looks for the coordinator on the network by its certificate authority and, if it answers
/// from other addresses than the saved ones, saves those. Returns true when the saved
/// addresses changed.
fn rediscover(shared: &Arc<Shared>) -> bool {
    let (ca, old) = {
        let entry = lock(&shared.entry);
        (
            tls::fingerprint_from_hex(&entry.ca_fingerprint),
            entry.addresses.clone(),
        )
    };
    let Some(ca) = ca else {
        return false;
    };
    let (tx, rx) = mpsc::channel();
    let Ok(browser) = discovery::browse(&ca, move |found| {
        let _ = tx.send(found);
    }) else {
        return false;
    };
    let end = Instant::now() + REDISCOVER_WAIT;
    let mut found = None;
    while found.is_none() && !shared.stopped() {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        match rx.recv_timeout(left.min(Duration::from_millis(100))) {
            Ok(one) => found = Some(one),
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    drop(browser);
    let Some(found) = found else {
        return false;
    };
    let addresses: Vec<String> = found
        .addrs
        .iter()
        .map(|ip| SocketAddr::new(IpAddr::V4(*ip), found.port).to_string())
        .collect();
    if addresses.is_empty() || addresses == old {
        return false;
    }
    let id = {
        let mut entry = lock(&shared.entry);
        entry.addresses.clone_from(&addresses);
        entry.id.clone()
    };
    if let Some(base) = &shared.base {
        remotes::update_in(base, &id, |e| e.addresses = addresses);
    }
    true
}

/// How a connected session ended.
struct SessionEnd {
    /// Why, in words for the log (for a refusal, the coordinator's own plain words).
    reason: String,
    /// True when the coordinator refused the subscription.
    refused: bool,
}

impl SessionEnd {
    /// The link was lost or closed.
    fn lost(reason: impl Into<String>) -> Self {
        Self {
            reason: reason.into(),
            refused: false,
        }
    }
}

/// One connected session: subscribe, then read pushes and send commands until the link ends.
fn session(shared: &Arc<Shared>, mut tls: Tls) -> SessionEnd {
    let (command_tx, command_rx) = mpsc::channel::<AdminRequest>();
    let subscribe_id = shared.next_id.fetch_add(1, Ordering::SeqCst);
    let subscribe = AdminRequest::Subscribe {
        req_id: subscribe_id,
        status_ms: pleochroma_net::admin::protocol::MIN_STATUS_INTERVAL_MS,
        level: Some(SUBSCRIBED_LEVEL),
    };
    if let Err(e) = send(&mut tls, &subscribe) {
        return SessionEnd::lost(e);
    }
    if tls.sock.set_read_timeout(Some(shared.timing.poll)).is_err() {
        return SessionEnd::lost("the socket could not be set up");
    }
    *lock(&shared.sender) = Some(command_tx);
    // The link counts as connected only once the coordinator has answered the subscription or
    // pushed something, so a refusal never flashes "connected" on every retry.
    let mut announced = false;

    let mut last_push = Instant::now();
    loop {
        if shared.stopped() {
            return SessionEnd::lost("closed by this computer");
        }
        while let Ok(request) = command_rx.try_recv() {
            if let Err(e) = send(&mut tls, &request) {
                return SessionEnd::lost(e);
            }
        }
        match read_push(&mut tls, shared.timing.poll) {
            Ok(Some(push)) => {
                last_push = Instant::now();
                if let Err(reason) = shared.handle_push(push, subscribe_id) {
                    return SessionEnd {
                        reason,
                        refused: true,
                    };
                }
                if !announced {
                    announced = true;
                    shared.set_link(Link::Connected);
                }
            }
            Ok(None) => {}
            Err(e) => return SessionEnd::lost(e),
        }
        if last_push.elapsed() > shared.timing.heartbeat_timeout {
            return SessionEnd::lost("the coordinator went silent");
        }
    }
}

/// Writes one request and flushes it.
fn send(tls: &mut Tls, request: &AdminRequest) -> Result<(), String> {
    messages::write_message(tls, request).map_err(|e| e.to_string())?;
    tls.flush().map_err(|e| e.to_string())
}

/// Waits up to `poll` for the next push. `Ok(None)` means nothing came in that time.
fn read_push(tls: &mut Tls, poll: Duration) -> Result<Option<AdminPush>, String> {
    let mut first = [0_u8; 1];
    match tls.read(&mut first) {
        Ok(0) => return Err("the coordinator closed the connection".to_owned()),
        Ok(_) => {}
        Err(e)
            if matches!(
                e.kind(),
                std::io::ErrorKind::WouldBlock
                    | std::io::ErrorKind::TimedOut
                    | std::io::ErrorKind::Interrupted
            ) =>
        {
            return Ok(None);
        }
        Err(e) => return Err(e.to_string()),
    }
    // The first byte is in; the rest of the message is due without a long wait.
    tls.sock
        .set_read_timeout(Some(MESSAGE_TIMEOUT))
        .map_err(|e| e.to_string())?;
    let frame = framing::read_frame_continuing(tls, &first, framing::MAX_CONTROL_FRAME_LEN)
        .map_err(|e| e.to_string())?;
    tls.sock
        .set_read_timeout(Some(poll))
        .map_err(|e| e.to_string())?;
    messages::decode_control_frame::<AdminPush>(&frame)
        .map(Some)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_net::{
        admin::{
            protocol::AdminError,
            status::{CoordinatorState as State, IssuedToken as Issued},
        },
        messages::{PayloadEncoding, Welcome},
    };
    use std::{
        net::TcpListener,
        sync::atomic::AtomicUsize,
        time::{SystemTime, UNIX_EPOCH},
    };

    /// What the stub coordinator does after the handshake.
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Mode {
        /// Answers every request and pushes a status, an event and a log line after subscribing.
        Normal,
        /// Answers the subscription and then says nothing at all.
        Silent,
    }

    /// A hand-written coordinator speaking the administration messages over real mutual TLS.
    struct Stub {
        addr: SocketAddr,
        accepted: Arc<AtomicUsize>,
        seen: Arc<Mutex<Vec<AdminRequest>>>,
        roles: Arc<Mutex<Vec<PeerRole>>>,
        stop: Arc<AtomicBool>,
    }

    impl Drop for Stub {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::SeqCst);
        }
    }

    fn temp_base(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pleochroma-remote-test-{tag}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or(Duration::ZERO)
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A certificate authority, a server certificate for 127.0.0.1 and an administrator
    /// bundle in `<base>/remotes/stub`. Returns the authority folder.
    fn make_pki(base: &Path, role: PeerRole) -> PathBuf {
        let pki_dir = base.join("pki");
        crate::pki::init(&pki_dir).unwrap();
        crate::pki::issue_server(&pki_dir, &[], &[IpAddr::from([127, 0, 0, 1])]).unwrap();
        let out = remotes::cert_dir_in(base, &RemoteId("stub".to_owned()));
        crate::pki::issue_client_with_role(&pki_dir, "tester", &out, role).unwrap();
        pki_dir
    }

    fn stub_status() -> CoordinatorStatus {
        CoordinatorStatus::minimal("Stub PC", "0.12.0", 45, State::Running)
    }

    fn spawn_stub(pki_dir: &Path, mode: Mode) -> Stub {
        let ca = tls::load_ca(&pki_dir.join(crate::pki::CA_CERT_FILE)).unwrap();
        let certs = tls::load_certs(&pki_dir.join(crate::pki::SERVER_CERT_FILE)).unwrap();
        let key = tls::load_private_key(&pki_dir.join(crate::pki::SERVER_KEY_FILE)).unwrap();
        let config = tls::server_config(ca, certs, key).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        listener.set_nonblocking(true).unwrap();
        let stub = Stub {
            addr: listener.local_addr().unwrap(),
            accepted: Arc::new(AtomicUsize::new(0)),
            seen: Arc::new(Mutex::new(Vec::new())),
            roles: Arc::new(Mutex::new(Vec::new())),
            stop: Arc::new(AtomicBool::new(false)),
        };
        let (accepted, seen, roles, stop) = (
            Arc::clone(&stub.accepted),
            Arc::clone(&stub.seen),
            Arc::clone(&stub.roles),
            Arc::clone(&stub.stop),
        );
        std::thread::spawn(move || {
            while !stop.load(Ordering::SeqCst) {
                if let Ok((tcp, _)) = listener.accept() {
                    accepted.fetch_add(1, Ordering::SeqCst);
                    let (config, seen, roles) =
                        (Arc::clone(&config), Arc::clone(&seen), Arc::clone(&roles));
                    std::thread::spawn(move || serve_one(&config, tcp, mode, &seen, &roles));
                } else {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
        });
        stub
    }

    /// Handshake, then the conversation of `mode`, until the client goes away.
    fn serve_one(
        config: &Arc<rustls::ServerConfig>,
        tcp: TcpStream,
        mode: Mode,
        seen: &Mutex<Vec<AdminRequest>>,
        roles: &Mutex<Vec<PeerRole>>,
    ) {
        tcp.set_nonblocking(false).unwrap();
        tcp.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
        let conn = rustls::ServerConnection::new(Arc::clone(config)).unwrap();
        let mut tls = rustls::StreamOwned::new(conn, tcp);
        let Ok(hello) = messages::read_control_message::<_, Hello>(&mut tls) else {
            return;
        };
        roles.lock().unwrap().push(hello.role);
        // Exactly what the real coordinator sends an administrator: no build identity
        // (`crate::admin::session::serve_connection`).
        let welcome = Welcome::new(
            handshake::UNKNOWN_BUILD_HASH,
            handshake::UNKNOWN_BUILD_HASH,
            None,
            true,
            false,
            None,
            PayloadEncoding::Raw,
        );
        if messages::write_message(&mut tls, &welcome).is_err() || tls.flush().is_err() {
            return;
        }
        let push = |tls: &mut rustls::StreamOwned<rustls::ServerConnection, TcpStream>,
                    message: &AdminPush| {
            messages::write_message(tls, message).is_ok() && tls.flush().is_ok()
        };
        while let Ok(request) = messages::read_control_message::<_, AdminRequest>(&mut tls) {
            seen.lock().unwrap().push(request.clone());
            let ok = match request {
                AdminRequest::Subscribe { req_id, .. } => {
                    let done = push(&mut tls, &AdminPush::ok(req_id, AdminReply::Done));
                    if mode == Mode::Normal {
                        // Late enough for the test to have its event receiver.
                        std::thread::sleep(Duration::from_millis(300));
                        done && push(&mut tls, &AdminPush::Status(stub_status()))
                            && push(
                                &mut tls,
                                &AdminPush::Event(CoordinatorEvent::Warning {
                                    text: "Disk is almost full".to_owned(),
                                }),
                            )
                            && push(
                                &mut tls,
                                &AdminPush::Log(LogLine {
                                    time: 7,
                                    level: LogLevel::Warn,
                                    target: "stub".to_owned(),
                                    message: "hello from the stub".to_owned(),
                                }),
                            )
                    } else {
                        done
                    }
                }
                AdminRequest::Rename { req_id, .. } => {
                    // A response for some other request first: it must not be mistaken for ours.
                    push(
                        &mut tls,
                        &AdminPush::err(req_id + 1000, AdminError::invalid("not yours")),
                    ) && push(&mut tls, &AdminPush::ok(req_id, AdminReply::Done))
                }
                AdminRequest::Revoke { req_id, .. } => push(
                    &mut tls,
                    &AdminPush::err(req_id, AdminError::denied("You have monitor access.")),
                ),
                AdminRequest::IssueToken { req_id, .. } => push(
                    &mut tls,
                    &AdminPush::ok(
                        req_id,
                        AdminReply::Token(Issued {
                            id: TokenId(9),
                            token: "SECRET-TOKEN".to_owned(),
                            link: "pleochroma://pair?SECRET".to_owned(),
                            expires_at: SystemTime::UNIX_EPOCH,
                        }),
                    ),
                ),
                other => push(&mut tls, &AdminPush::ok(other.req_id(), AdminReply::Done)),
            };
            if !ok {
                return;
            }
        }
    }

    fn fast_timing(heartbeat_timeout: Duration) -> Timing {
        Timing {
            poll: Duration::from_millis(20),
            heartbeat_timeout,
            request_timeout: Duration::from_secs(5),
            backoff: vec![Duration::from_millis(100), Duration::from_millis(200)],
            connect_timeout: Duration::from_secs(2),
            rediscover: false,
        }
    }

    fn entry_for(addr: SocketAddr, role: AccessRole) -> RemoteEntry {
        RemoteEntry {
            id: RemoteId("stub".to_owned()),
            name: "Garage server".to_owned(),
            addresses: vec![addr.to_string()],
            role,
            ca_fingerprint: "00".repeat(32),
            last_seen: None,
        }
    }

    fn open_stubbed(
        base: &Path,
        addr: SocketAddr,
        role: AccessRole,
        timing: Timing,
    ) -> Arc<RemoteBackend> {
        let id = RemoteId("stub".to_owned());
        RemoteBackend::start(
            entry_for(addr, role),
            remotes::cert_dir_in(base, &id),
            None,
            timing,
        )
    }

    fn wait_for(what: &str, mut condition: impl FnMut() -> bool) {
        let end = Instant::now() + Duration::from_secs(20);
        while Instant::now() < end {
            if condition() {
                return;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        panic!("timed out waiting for: {what}");
    }

    fn connected(backend: &RemoteBackend) {
        wait_for("the stub's status", || {
            backend.status().name == "Stub PC" && backend.status().state == State::Running
        });
    }

    #[test]
    fn before_the_session_is_up_the_status_says_connecting() {
        let base = temp_base("connecting");
        let pki_dir = make_pki(&base, PeerRole::Admin);
        let stub = spawn_stub(&pki_dir, Mode::Normal);
        let backend = open_stubbed(
            &base,
            stub.addr,
            AccessRole::Admin,
            fast_timing(Duration::from_secs(5)),
        );
        // `start` returns at once, long before the TLS handshake can have finished.
        let first = backend.status();
        assert_eq!(first.name, "Garage server");
        assert_eq!(first.state, State::Starting);
        connected(&backend);
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn a_status_push_reaches_the_cache_with_events_and_logs() {
        let base = temp_base("push");
        let pki_dir = make_pki(&base, PeerRole::Admin);
        let stub = spawn_stub(&pki_dir, Mode::Normal);
        let backend = open_stubbed(
            &base,
            stub.addr,
            AccessRole::Admin,
            fast_timing(Duration::from_secs(5)),
        );
        let events = backend.events();
        connected(&backend);

        let event = events.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!(
            event,
            CoordinatorEvent::Warning {
                text: "Disk is almost full".to_owned()
            }
        );
        wait_for("the log line", || {
            !backend.logs(tracing::Level::WARN, 10).is_empty()
        });
        let logs = backend.logs(tracing::Level::WARN, 10);
        assert_eq!(logs.len(), 1);
        assert_eq!(logs[0].message, "hello from the stub");
        assert!(backend.logs(tracing::Level::ERROR, 10).is_empty());
        assert_eq!(*stub.roles.lock().unwrap(), vec![PeerRole::Admin]);
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn a_command_is_answered_by_its_own_req_id() {
        let base = temp_base("commands");
        let pki_dir = make_pki(&base, PeerRole::Admin);
        let stub = spawn_stub(&pki_dir, Mode::Normal);
        let backend = open_stubbed(
            &base,
            stub.addr,
            AccessRole::Admin,
            fast_timing(Duration::from_secs(5)),
        );
        connected(&backend);

        backend.rename(&[1_u8; 32], "Phone").unwrap();
        let token = backend.issue_token(PeerRole::Viewer, "Tablet").unwrap();
        assert_eq!(token.id, TokenId(9));
        assert!(!format!("{token:?}").contains("SECRET"));
        backend.restart().unwrap();

        let seen = stub.seen.lock().unwrap().clone();
        assert!(seen.iter().any(|r| matches!(
            r,
            AdminRequest::Rename { fingerprint, name, .. }
                if *fingerprint == "01".repeat(32) && name == "Phone"
        )));
        assert!(
            seen.iter()
                .any(|r| matches!(r, AdminRequest::Restart { .. }))
        );
        // Every request got its own id.
        let mut ids: Vec<u64> = seen.iter().map(AdminRequest::req_id).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), seen.len());
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn a_denied_command_comes_back_as_the_reason() {
        let base = temp_base("denied");
        let pki_dir = make_pki(&base, PeerRole::Monitor);
        let stub = spawn_stub(&pki_dir, Mode::Normal);
        let backend = open_stubbed(
            &base,
            stub.addr,
            AccessRole::Monitor,
            fast_timing(Duration::from_secs(5)),
        );
        connected(&backend);

        assert!(backend.capabilities().is_read_only());
        assert_eq!(
            backend.revoke(&[2_u8; 32]).unwrap_err(),
            "You have monitor access."
        );
        assert_eq!(*stub.roles.lock().unwrap(), vec![PeerRole::Monitor]);
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn a_silent_coordinator_is_dropped_and_dialed_again() {
        let base = temp_base("silent");
        let pki_dir = make_pki(&base, PeerRole::Admin);
        let stub = spawn_stub(&pki_dir, Mode::Silent);
        let backend = open_stubbed(
            &base,
            stub.addr,
            AccessRole::Admin,
            fast_timing(Duration::from_millis(500)),
        );
        wait_for("a second connection after the silence", || {
            stub.accepted.load(Ordering::SeqCst) >= 2
        });
        // Each connection subscribes again. The second one is accepted before its request
        // arrives, so wait for the request rather than count it at once.
        wait_for("a second subscription", || {
            stub.seen
                .lock()
                .unwrap()
                .iter()
                .filter(|r| matches!(r, AdminRequest::Subscribe { .. }))
                .count()
                >= 2
        });
        drop(backend);
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn nothing_listening_says_cant_reach_with_the_name() {
        let base = temp_base("unreachable");
        make_pki(&base, PeerRole::Admin);
        let dead = {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            listener.local_addr().unwrap()
        };
        let backend = open_stubbed(
            &base,
            dead,
            AccessRole::Admin,
            fast_timing(Duration::from_secs(5)),
        );
        wait_for("the failure to show", || {
            backend.status().state == State::Error("Can't reach Garage server".to_owned())
        });
        assert_eq!(backend.status().name, "Garage server");
        let err = backend.restart().unwrap_err();
        assert!(err.contains("Garage server"), "{err}");
        std::fs::remove_dir_all(&base).ok();
    }

    #[test]
    fn a_different_build_is_named_in_plain_words() {
        let with_version =
            ConnectFail::Mismatch(Some("0.12.1".to_owned())).text("Garage server", None);
        assert_eq!(
            with_version,
            "Garage server runs a different Pleochroma build (0.12.1). Use the same version on both."
        );
        let cached = ConnectFail::Mismatch(None).text("Garage server", Some("0.11.1"));
        assert!(cached.contains("(0.11.1)"), "{cached}");
        assert!(ConnectFail::Mismatch(None).rank() > ConnectFail::Refused("x".to_owned()).rank());
        assert!(ConnectFail::Refused("x".to_owned()).rank() > ConnectFail::Unreachable.rank());
    }

    #[test]
    fn an_unsaved_id_reports_it_instead_of_connecting() {
        let backend = RemoteBackend::unavailable(&RemoteId("gone".to_owned()));
        let status = backend.status();
        assert!(matches!(status.state, State::Error(ref text) if text.contains("no longer saved")));
        assert!(backend.restart().is_err());
        assert!(backend.capabilities().is_read_only());
    }
}
