//! Authenticated setup boundary. A connection test never uploads a scene.
use crate::{
    platform::Platform,
    settings::{MobileSettings, RenderBackend},
};
use pleochroma_net::{
    client, discovery,
    enroll::ClaimAnyError,
    messages::{Backend, PeerRole, RenderCapability},
    pairing::{PairingLink, PairingLinkError},
    tls,
};
use serde::{Deserialize, Serialize};
use std::{
    io::{self, Read, Write},
    net::{SocketAddr, TcpStream, ToSocketAddrs},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, RecvTimeoutError},
    },
    time::{Duration, Instant},
};
use zeroize::{Zeroize, Zeroizing};

pub mod final_image;
pub mod jobs;
pub mod library;
pub mod rendering;

const MAX_BUNDLE_BYTES: usize = 128 * 1024;

/// The message of a connection the caller's cancel check ended before it was made.
const CONNECTION_CANCELLED: &str = "Connection cancelled.";

/// How many of the saved addresses a connection tries (a pairing link carries three).
const MAX_DIALED_ADDRESSES: usize = 3;

/// Stored only through the platform credential vault, never regular settings/export.
#[derive(Serialize, Deserialize)]
struct Credentials {
    ca: String,
    certificate: String,
    key: String,
    /// Paired through a pairing link: the coordinator is verified by its fixed TLS name
    /// whatever address was dialed. Bundles stored before 0.12.0 (and hand enrollment)
    /// leave it off and verify the dialed name as before. It sits in the bundle, not in
    /// the settings, so it can never disagree with the certificate it describes.
    #[serde(default)]
    fixed_name: bool,
}
impl Drop for Credentials {
    fn drop(&mut self) {
        self.key.zeroize();
    }
}

/// What a render endpoint is, in plain words.
const fn backend_words(backend: &Backend) -> &'static str {
    match backend {
        Backend::Cpu { .. } => "CPU render computer",
        Backend::Gpu { .. } => "GPU render computer",
        Backend::Coordinator { .. } => "coordinator",
    }
}

/// The start of the message of a connection that reached no saved address.
///
/// A render, an export or a jobs request that fails with it asks the app to look for the
/// computer on the network ([`is_unreachable`]).
pub const UNREACHABLE: &str = "Could not reach the computer";

/// Whether `message` says that no address answered (as opposed to a refusal or a protocol
/// problem, which a new address does not fix).
#[must_use]
pub fn is_unreachable(message: &str) -> bool {
    message.contains(UNREACHABLE)
}

#[derive(Debug, Clone)]
pub struct ConnectionReport {
    pub certificate_identity: String,
    pub render: Option<RenderCapability>,
    pub library: bool,
    pub zoning: bool,
}
impl ConnectionReport {
    #[must_use]
    pub fn summary(&self) -> String {
        match &self.render {
            Some(capability) => format!(
                "Connected to a {}. It can render pictures up to {} pixels{}. {}",
                backend_words(&capability.backend),
                capability.max_pixels,
                if capability.hdr {
                    ", with HDR lighting"
                } else {
                    ""
                },
                if matches!(capability.backend, Backend::Coordinator { .. }) {
                    "It sends finished pictures."
                } else {
                    "It needs the full data transfer choice in Remote settings."
                }
            ),
            None if self.library => {
                "Connected to a computer that only holds designs. It cannot render pictures.".into()
            }
            None => "Connected, but this computer cannot render pictures right now.".into(),
        }
    }
}

/// Validated, still-uninstalled credentials. Dropping an obsolete claim erases its key.
pub struct Enrollment {
    encoded: Zeroizing<String>,
}

impl Enrollment {
    /// Commit only after the UI accepts the still-current request. Once accepted,
    /// this short atomic vault write finishes on its worker even if the page closes.
    ///
    /// # Errors
    ///
    /// The platform's message when the vault write fails.
    pub fn install(self, platform: &dyn Platform) -> Result<(), String> {
        platform
            .store_remote_credentials(&self.encoded)
            .map_err(|error| error.to_string())
    }
}

/// Redeems and validates the CA-pinned enrollment protocol without changing the vault.
///
/// The caller must accept the current request before installing the returned bundle.
///
/// # Errors
///
/// A message when the enrollment is cancelled or interrupted, the token cannot be
/// redeemed, or the credential bundle is invalid or too large.
pub fn enroll(
    token: &str,
    enrollment_address: &str,
    cancelled: &AtomicBool,
) -> Result<Enrollment, String> {
    if cancelled.load(Ordering::Acquire) {
        return Err("Enrollment cancelled.".into());
    }
    let bundle = pleochroma_net::enroll::claim(token, enrollment_address)
        .map_err(|error| error.to_string())?;
    if cancelled.load(Ordering::Acquire) {
        return Err(
            "Enrollment interrupted; credentials were not installed. Enter a new code if needed."
                .into(),
        );
    }
    let credentials = Credentials {
        ca: bundle.ca_pem,
        certificate: bundle.client_cert_pem,
        key: bundle.client_key_pem.to_string(),
        fixed_name: false,
    };
    encode_enrollment(&credentials)
}

/// Checks that `credentials` build a TLS configuration and packs them for the vault.
fn encode_enrollment(credentials: &Credentials) -> Result<Enrollment, String> {
    configuration(credentials)?;
    let encoded =
        Zeroizing::new(serde_json::to_string(credentials).map_err(|error| error.to_string())?);
    if encoded.len() > MAX_BUNDLE_BYTES {
        return Err("Credential bundle exceeds the phone limit.".into());
    }
    Ok(Enrollment { encoded })
}

/// A pairing link this phone is willing to act on: a viewer code that parsed cleanly.
///
/// The phone shows it to the user first ("Pair with <name>?"); nothing connects until the
/// user confirms. `Debug` is redacted like the link's, and the code is wiped on drop.
pub struct PairingOffer {
    link: PairingLink,
}

impl std::fmt::Debug for PairingOffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PairingOffer")
            .field("name", &self.link.name)
            .field("addrs", &self.link.addrs)
            .field("enroll_port", &self.link.enroll_port)
            .finish_non_exhaustive()
    }
}

impl PairingOffer {
    /// The computer's name as the link spells it; "your computer" when it has none.
    #[must_use]
    pub fn name(&self) -> String {
        let name = self.link.name.trim();
        if name.is_empty() {
            "your computer".to_owned()
        } else {
            name.to_owned()
        }
    }

    /// The rendering addresses the link carries, as `host:port`, main network first.
    #[must_use]
    pub fn addresses(&self) -> Vec<String> {
        self.link.addrs.iter().map(ToString::to_string).collect()
    }

    /// One line for the confirm sheet: the first address and how many more there are.
    #[must_use]
    pub fn address_summary(&self) -> String {
        let all = self.addresses();
        match all.split_first() {
            None => String::new(),
            Some((first, [])) => first.clone(),
            Some((first, rest)) => format!("{first} (+{} more)", rest.len()),
        }
    }

    /// The addresses of the enrollment listener: each link address with the enrollment port.
    fn enrollment_addresses(&self) -> Vec<SocketAddr> {
        self.link
            .addrs
            .iter()
            .map(|addr| SocketAddr::new(addr.ip(), self.link.enroll_port))
            .collect()
    }
}

/// Reads a pairing link the phone received (a deep link or pasted text).
///
/// Only a viewer code is accepted: a render computer's code and the Monitor and Admin
/// codes are refused in plain words, so a phone never takes a role it cannot use.
///
/// # Errors
///
/// A plain sentence for the user when the text is not a pairing link, is damaged or too
/// new, or carries a code for something other than a phone. It never repeats the link.
pub fn read_pairing_link(text: &str) -> Result<PairingOffer, String> {
    let link = PairingLink::parse(text).map_err(|error| link_error_words(error).to_owned())?;
    match link.role {
        PeerRole::Viewer => Ok(PairingOffer { link }),
        PeerRole::Worker => Err(
            "This code is for a render computer. On the computer, choose Connect a phone.".into(),
        ),
        PeerRole::Monitor | PeerRole::Admin => Err(
            "This code is for managing a computer, which this app cannot do. On the computer, \
             choose Connect a phone."
                .into(),
        ),
    }
}

/// The plain words for a link that could not be read. None of them contains any of the link.
#[must_use]
pub const fn link_error_words(error: PairingLinkError) -> &'static str {
    match error {
        PairingLinkError::NotAPairingLink => {
            "This is not a Pleochroma pairing link. On the computer, choose Connect a phone."
        }
        PairingLinkError::TooLong => "This pairing link is too long to be real.",
        PairingLinkError::NeedsNewerVersion => {
            "This link needs a newer Pleochroma. Update the app, then try again."
        }
        PairingLinkError::MissingPart | PairingLinkError::RepeatedPart => {
            "This pairing link is incomplete or damaged. Show the code on the computer again."
        }
        PairingLinkError::UnknownRole => {
            "This pairing link names an access level this app does not know."
        }
        PairingLinkError::BadName => "This pairing link has a computer name this app cannot use.",
        PairingLinkError::BadAddress | PairingLinkError::NotIpv4 => {
            "This pairing link has an address this app cannot use."
        }
        PairingLinkError::NoAddress => "This pairing link has no address to connect to.",
        PairingLinkError::TooManyAddresses => "This pairing link has too many addresses.",
        PairingLinkError::BadPort => "This pairing link has a port this app cannot use.",
        PairingLinkError::MissingToken => "This pairing link has no code in it.",
        PairingLinkError::BadToken | PairingLinkError::BadEncoding => {
            "This pairing link is damaged. Show the code on the computer again."
        }
    }
}

/// The plain words for a failed claim. None of them contains the code.
#[must_use]
pub fn claim_error_words(error: &ClaimAnyError) -> String {
    match error {
        ClaimAnyError::NoAddresses => {
            "The pairing link has no address to connect to. Show the code on the computer again."
                .to_owned()
        }
        ClaimAnyError::InvalidCode => {
            "That is not a valid pairing code. On the computer, choose Connect a phone.".to_owned()
        }
        ClaimAnyError::AllTimedOut => {
            "The computer did not answer. Is this phone on the same Wi-Fi as the computer?"
                .to_owned()
        }
        ClaimAnyError::Refused => {
            "The computer refused the connection. Is Pleochroma running there, with Connect a \
             phone open?"
                .to_owned()
        }
        ClaimAnyError::Unreachable => {
            "This phone cannot reach the computer. Same Wi-Fi?".to_owned()
        }
        ClaimAnyError::WrongCoordinator => {
            "A different computer answered than the one this code belongs to, so pairing was \
             stopped."
                .to_owned()
        }
        ClaimAnyError::CodeExpiredOrUsed => {
            "This code expired or was already used. If you didn't use it, someone else may have \
             paired \u{2014} check Devices on the computer."
                .to_owned()
        }
        ClaimAnyError::Failed(reason) => format!("Pairing failed: {reason}"),
    }
}

/// What the settings remember about a computer this phone paired with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairedServer {
    /// The computer's name.
    pub name: String,
    /// Its rendering addresses (`host:port`), the one that answered the claim first.
    pub addresses: Vec<String>,
    /// The port of its enrollment listener.
    pub enroll_port: u16,
}

/// Redeems a confirmed pairing offer over every address of its link at once and keeps the
/// first computer that passes the CA pin the code carries.
///
/// Like [`enroll`] it changes nothing on the phone: the caller installs the returned
/// [`Enrollment`] once the screen still wants it, then calls [`remember_paired`]. The
/// credentials are marked for fixed-name verification, so the computer is recognized by
/// its certificate whatever address it has later.
///
/// # Errors
///
/// A plain sentence ([`claim_error_words`]) for a refused or unreachable computer, a code
/// that expired or was already used, or an interrupted pairing; or a message when the
/// bundle is invalid or too large.
pub fn pair(
    offer: &PairingOffer,
    cancelled: &AtomicBool,
) -> Result<(Enrollment, PairedServer), String> {
    if cancelled.load(Ordering::Acquire) {
        return Err("Pairing cancelled.".into());
    }
    let outcome =
        pleochroma_net::enroll::claim_any(offer.link.token(), &offer.enrollment_addresses())
            .map_err(|error| claim_error_words(&error))?;
    if cancelled.load(Ordering::Acquire) {
        return Err(
            "Pairing interrupted; nothing was installed. Show a new code on the computer if needed."
                .into(),
        );
    }
    let bundle = outcome.bundle;
    let credentials = Credentials {
        ca: bundle.ca_pem,
        certificate: bundle.client_cert_pem,
        key: bundle.client_key_pem.to_string(),
        fixed_name: true,
    };
    let enrollment = encode_enrollment(&credentials)?;
    // The address that answered goes first, so the next connection tries it first.
    let answered = outcome.addr.ip();
    let mut addresses = offer.addresses();
    addresses.sort_by_key(|text| {
        !text
            .parse::<SocketAddr>()
            .is_ok_and(|addr| addr.ip() == answered)
    });
    Ok((
        enrollment,
        PairedServer {
            name: offer.name(),
            addresses,
            enroll_port: offer.link.enroll_port,
        },
    ))
}

/// Writes a paired computer into the settings.
///
/// That is its name, addresses, enrollment port and the enrolled flag. The first address is also the one the Remote page shows. Whether the computer
/// serves a design library is not known for a new pairing (`serves_library` goes back to
/// unknown); the connection test after pairing finds out.
///
/// Returns whether the render backend is Remote (so the caller stops a render that was
/// talking to the computer this one replaces).
pub fn remember_paired(settings: &mut MobileSettings, paired: &PairedServer) -> bool {
    settings.remote.name.clone_from(&paired.name);
    settings.remote.address = paired.addresses.first().cloned().unwrap_or_default();
    settings.remote.addresses.clone_from(&paired.addresses);
    settings.remote.enroll_port = paired.enroll_port;
    settings.remote.enrolled = true;
    settings.remote.serves_library = None;
    settings.render.backend == RenderBackend::Remote
}

/// Erases the pairing credentials: the encrypted bundle and the platform key that wraps it.
///
/// Having nothing stored is a success, and the call can be repeated. Afterwards
/// [`test_connection`] and every other connection say "Enroll this phone first." until a
/// new token is redeemed.
///
/// This does not touch the settings; call [`clear_server_settings`] once it has
/// succeeded. It also cannot revoke the phone's certificate on the server: the server
/// keeps its own list of paired phones and its server jobs until its operator removes
/// them.
///
/// # Errors
///
/// The platform's message when the bundle or its key could not be erased. The settings
/// then still say the phone is enrolled, so the user can try again.
pub fn forget(platform: &dyn Platform) -> Result<(), String> {
    platform
        .forget_remote_credentials()
        .map_err(|error| error.to_string())
}

/// The discovery tags of the CA this phone is paired with.
///
/// Each tag is 16 lowercase hex characters; there is one per CA certificate in the stored
/// bundle. The Android rediscovery compares them with the `ca` value of the announcements it
/// finds ([`crate::remote_discovery::belongs_to`]).
///
/// # Errors
///
/// A message when no credentials are stored, the bundle cannot be unlocked or read, or it
/// holds no CA certificate.
pub fn saved_ca_tags(platform: &dyn Platform) -> Result<Vec<String>, String> {
    let encoded = Zeroizing::new(
        platform
            .load_remote_credentials()
            .map_err(|error| error.to_string())?
            .ok_or("Enroll this phone first.")?,
    );
    if encoded.len() > MAX_BUNDLE_BYTES {
        return Err("Credential bundle exceeds the phone limit.".into());
    }
    let credentials: Credentials = serde_json::from_str(&encoded)
        .map_err(|_| "The stored credential bundle is invalid.".to_owned())?;
    ca_tags_of_pem(&credentials.ca)
}

/// The discovery tag of every certificate in a PEM text.
fn ca_tags_of_pem(pem: &str) -> Result<Vec<String>, String> {
    let certificates = rustls_pemfile::certs(&mut pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    if certificates.is_empty() {
        return Err("The credential bundle contains no CA certificate.".into());
    }
    Ok(certificates
        .iter()
        .map(|certificate| discovery::txt::ca_tag(&tls::fingerprint(certificate)))
        .collect())
}

/// Clears what the settings remember about the paired server: its name, address and enrolled flag.
///
/// A selected Remote render backend goes back to Automatic, so no
/// render or export tries to reach a server that was forgotten. The transfer mode, the
/// update interval and the metered-network choice are preferences, not part of the
/// server's identity, and stay.
///
/// Returns whether the render backend was switched away from Remote.
pub fn clear_server_settings(settings: &mut MobileSettings) -> bool {
    settings.remote.clear_server();
    let was_remote = settings.render.backend == RenderBackend::Remote;
    if was_remote {
        settings.render.backend = RenderBackend::Automatic;
    }
    was_remote
}

fn configuration(credentials: &Credentials) -> Result<Arc<rustls::ClientConfig>, String> {
    let ca = rustls_pemfile::certs(&mut credentials.ca.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    if ca.is_empty() {
        return Err("The credential bundle contains no CA certificate.".into());
    }
    let mut roots = rustls::RootCertStore::empty();
    for certificate in ca {
        roots.add(certificate).map_err(|error| error.to_string())?;
    }
    let certificates = rustls_pemfile::certs(&mut credentials.certificate.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let key = rustls_pemfile::private_key(&mut credentials.key.as_bytes())
        .map_err(|error| error.to_string())?
        .ok_or("No TLS private key in the credential bundle.")?;
    let config = if credentials.fixed_name {
        tls::client_config_fixed_name(roots, certificates, key)
    } else {
        tls::client_config(roots, certificates, key)
    };
    config.map_err(|error| error.to_string())
}

/// Connects with the stored credentials and reports the endpoint's identity.
///
/// One worker owns both TLS directions throughout connect/handshake. Finite socket
/// polls preserve partial protocol reads while cancellation and deadlines are checked.
///
/// # Errors
///
/// A message when no credentials are enrolled, the connection or handshake fails or is
/// cancelled, or the endpoint supplies no identity.
pub fn test_connection(
    platform: &dyn Platform,
    address: &str,
    cancelled: &AtomicBool,
) -> Result<ConnectionReport, String> {
    test_connection_any(platform, &[address.to_owned()], cancelled)
}

/// [`test_connection`] over a list of addresses, dialed at the same time; the first one
/// that answers is used.
///
/// # Errors
///
/// A message when no credentials are enrolled, no address answers, the handshake fails
/// or is cancelled, or the endpoint supplies no identity.
pub fn test_connection_any(
    platform: &dyn Platform,
    addresses: &[String],
    cancelled: &AtomicBool,
) -> Result<ConnectionReport, String> {
    let cancelled = || cancelled.load(Ordering::Acquire);
    let (stream, welcome) = connect(platform, addresses, &cancelled)?;
    let certificate = stream
        .tls
        .conn
        .peer_certificates()
        .and_then(|chain| chain.first())
        .ok_or("The TLS endpoint supplied no identity.")?;
    let certificate_identity = tls::fingerprint_to_hex(&tls::fingerprint(certificate));
    Ok(ConnectionReport {
        certificate_identity,
        render: welcome.render,
        library: welcome.library,
        zoning: welcome.zoning,
    })
}

/// What dialing one saved address produced: the open socket and the host it reached, or why
/// nothing answered.
type Dialed = Result<(TcpStream, String), String>;

/// Dials one address: tries up to four resolved endpoints, two seconds apiece.
fn dial_one(address: &str) -> Dialed {
    let Some(host) = tls::host_for_server_name(address) else {
        return Err("Enter a host:port rendering address.".to_owned());
    };
    let resolved = address
        .to_socket_addrs()
        .map_err(|error| error.to_string())?;
    let mut last = "No network address was found.".to_owned();
    for endpoint in resolved.take(4) {
        match TcpStream::connect_timeout(&endpoint, Duration::from_secs(2)) {
            Ok(socket) => return Ok((socket, host.to_owned())),
            Err(error) => last = error.to_string(),
        }
    }
    Err(last)
}

/// Opens one authenticated connection to the first of `addresses` that answers.
///
/// The addresses are dialed at the same time, like the desktop does, so a dead one costs no
/// waiting; the first socket that opens is used and the others are dropped. The TLS server
/// name is the host of the address that connected, so a certificate that lacks the
/// coordinator's fixed name is still checked against what was dialed.
fn connect<'a>(
    platform: &dyn Platform,
    addresses: &[String],
    cancelled: &'a dyn Fn() -> bool,
) -> Result<(DeadlineStream<'a>, pleochroma_net::messages::Welcome), String> {
    let encoded = Zeroizing::new(
        platform
            .load_remote_credentials()
            .map_err(|error| error.to_string())?
            .ok_or("Enroll this phone first.")?,
    );
    if encoded.len() > MAX_BUNDLE_BYTES {
        return Err("Credential bundle exceeds the phone limit.".into());
    }
    let credentials: Credentials = serde_json::from_str(&encoded)
        .map_err(|_| "The stored credential bundle is invalid.".to_owned())?;
    let config = configuration(&credentials)?;
    if addresses.iter().all(|address| address.trim().is_empty()) {
        return Err("Enter a host:port rendering address.".into());
    }
    let (sender, receiver) = mpsc::channel::<Dialed>();
    let mut pending = 0_usize;
    let mut last = "No network address was found.".to_owned();
    for address in addresses.iter().take(MAX_DIALED_ADDRESSES) {
        let address = address.clone();
        let sender = sender.clone();
        let spawned = std::thread::Builder::new()
            .name("remote-dial".to_owned())
            .spawn(move || {
                // The receiver is gone once another address won: the socket drops here.
                let _sent = sender.send(dial_one(&address));
            });
        match spawned {
            Ok(_) => pending += 1,
            Err(error) => last = error.to_string(),
        }
    }
    drop(sender);
    let mut connected = None;
    while pending > 0 {
        if cancelled() {
            return Err(CONNECTION_CANCELLED.into());
        }
        match receiver.recv_timeout(Duration::from_millis(50)) {
            Ok(Ok(found)) => {
                connected = Some(found);
                break;
            }
            Ok(Err(error)) => {
                last = error;
                pending -= 1;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    let (socket, host) = connected.ok_or_else(|| format!("{UNREACHABLE}: {last}"))?;
    let server_name =
        rustls::pki_types::ServerName::try_from(host).map_err(|error| error.to_string())?;
    socket
        .set_nodelay(true)
        .map_err(|error| error.to_string())?;
    socket
        .set_read_timeout(Some(Duration::from_millis(200)))
        .map_err(|error| error.to_string())?;
    socket
        .set_write_timeout(Some(Duration::from_millis(200)))
        .map_err(|error| error.to_string())?;
    let connection =
        rustls::ClientConnection::new(config, server_name).map_err(|error| error.to_string())?;
    let mut stream = DeadlineStream {
        tls: rustls::StreamOwned::new(connection, socket),
        cancelled,
        deadline: Instant::now() + Duration::from_secs(12),
    };
    // The shared helper verifies protocol, engine build and source identity. Merely
    // establishing a TLS socket never grants render compatibility.
    let welcome = client::handshake::handshake(&mut stream).map_err(|error| error.to_string())?;
    Ok((stream, welcome))
}

struct DeadlineStream<'a> {
    tls: rustls::StreamOwned<rustls::ClientConnection, TcpStream>,
    cancelled: &'a dyn Fn() -> bool,
    deadline: Instant,
}
impl DeadlineStream<'_> {
    fn check(&self) -> io::Result<()> {
        if (self.cancelled)() {
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "Connection cancelled",
            ));
        }
        if Instant::now() >= self.deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "Connection deadline exceeded",
            ));
        }
        Ok(())
    }
}
fn retryable(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
    )
}
impl Read for DeadlineStream<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        loop {
            self.check()?;
            match self.tls.read(bytes) {
                Err(error) if retryable(&error) => {}
                result => return result,
            }
        }
    }
}
impl Write for DeadlineStream<'_> {
    /// A write retries through the 200 ms socket timeouts until the TLS layer takes the
    /// bytes, so a large upload (an HDR asset is tens of MiB) survives a full send buffer.
    ///
    /// Retrying is safe because of how `rustls::StreamOwned::write` is built: it completes
    /// any earlier pending output first (a timeout there is returned BEFORE any of `bytes`
    /// is consumed), then buffers `bytes` and attempts the socket write while ignoring its
    /// errors (the already-consumed bytes are reported as written). A timeout error
    /// therefore always means "none of these bytes were taken", never "some were". Any
    /// other error, or the deadline or cancellation, still ends the connection.
    ///
    /// The deadline is a stall limit for writes: every accepted write pushes it out to
    /// [`WRITE_STALL`] from now, so a slow but moving upload is not cut off.
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        loop {
            self.check()?;
            match self.tls.write(bytes) {
                Err(error) if retryable(&error) => {}
                Ok(written) => {
                    self.deadline = self.deadline.max(Instant::now() + WRITE_STALL);
                    return Ok(written);
                }
                result => return result,
            }
        }
    }
    /// Flushing retries like [`Self::write`]: rustls keeps the unsent records buffered, so
    /// a timeout leaves nothing duplicated.
    fn flush(&mut self) -> io::Result<()> {
        loop {
            self.check()?;
            match self.tls.flush() {
                Err(error) if retryable(&error) => {}
                result => return result,
            }
        }
    }
}

/// How long a write may make no progress before the connection is given up.
const WRITE_STALL: Duration = Duration::from_secs(12);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        platform::bare::BarePlatform, platform_mock::MockPlatform, settings::RemoteTransfer,
    };

    #[test]
    fn missing_render_capacity_is_never_render_ready() {
        let report = ConnectionReport {
            certificate_identity: String::new(),
            render: None,
            library: true,
            zoning: false,
        };
        assert!(report.summary().contains("only holds designs"));
        assert!(report.summary().contains("cannot render"));
    }
    #[test]
    fn empty_credentials_do_not_disable_tls_verification() {
        let credentials = Credentials {
            ca: String::new(),
            certificate: String::new(),
            key: String::new(),
            fixed_name: false,
        };
        assert!(configuration(&credentials).is_err());
        let fixed = Credentials {
            ca: String::new(),
            certificate: String::new(),
            key: String::new(),
            fixed_name: true,
        };
        assert!(configuration(&fixed).is_err());
    }

    #[test]
    fn a_bundle_stored_before_0_12_0_loads_without_the_fixed_name() {
        let old = r#"{"ca":"a","certificate":"b","key":"c"}"#;
        let credentials: Credentials = serde_json::from_str(old).expect("old bundle loads");
        assert!(!credentials.fixed_name);
    }

    #[test]
    fn only_a_connection_that_reached_nothing_is_unreachable() {
        assert!(is_unreachable(&format!("{UNREACHABLE}: refused")));
        // A render that gave up after reconnecting wraps the same sentence.
        assert!(is_unreachable(&format!(
            "{UNREACHABLE}: timed out (the connection was lost)"
        )));
        assert!(!is_unreachable("Enroll this phone first."));
        assert!(!is_unreachable("The endpoint refused the request."));
    }

    #[test]
    fn the_summary_names_the_endpoint_in_plain_words() {
        let coordinator = Backend::Coordinator {
            workers: 2,
            threads: 16,
            gpus: 1,
        };
        assert_eq!(backend_words(&coordinator), "coordinator");
        assert_eq!(
            backend_words(&Backend::Cpu { threads: 8 }),
            "CPU render computer"
        );
    }

    #[test]
    fn a_bundle_without_a_ca_has_no_discovery_tags() {
        assert!(ca_tags_of_pem("").is_err());
        assert!(ca_tags_of_pem("not a certificate").is_err());
    }

    const TOKEN: &str = "GW1-AbCdEf-0123456789";

    fn link_text(role: PeerRole, name: &str, addrs: &[&str]) -> String {
        PairingLink::new(
            role,
            name,
            addrs.iter().map(|a| a.parse().expect("address")).collect(),
            7879,
            TOKEN,
        )
        .build()
        .expect("a valid link")
    }

    #[test]
    fn a_viewer_link_becomes_an_offer_the_sheet_can_describe() {
        let text = link_text(
            PeerRole::Viewer,
            "Studio PC",
            &["192.168.178.20:7878", "10.0.0.5:7878"],
        );
        let offer = read_pairing_link(&text).expect("a viewer link");
        assert_eq!(offer.name(), "Studio PC");
        assert_eq!(offer.addresses(), ["192.168.178.20:7878", "10.0.0.5:7878"]);
        assert_eq!(offer.address_summary(), "192.168.178.20:7878 (+1 more)");
        let one = read_pairing_link(&link_text(PeerRole::Viewer, "", &["10.0.0.5:7878"]))
            .expect("a link without a name");
        assert_eq!(one.name(), "your computer");
        assert_eq!(one.address_summary(), "10.0.0.5:7878");
        // Enrollment listens on the link's host at the enrollment port.
        assert_eq!(one.enrollment_addresses()[0].to_string(), "10.0.0.5:7879");
    }

    #[test]
    fn the_offer_never_shows_the_code() {
        let offer = read_pairing_link(&link_text(PeerRole::Viewer, "Studio", &["10.0.0.5:7878"]))
            .expect("a viewer link");
        assert!(!format!("{offer:?}").contains(TOKEN));
    }

    #[test]
    fn a_worker_code_scanned_in_the_phone_says_where_it_belongs() {
        let text = link_text(PeerRole::Worker, "Render box", &["10.0.0.5:7878"]);
        let error = read_pairing_link(&text).expect_err("a worker code is not for a phone");
        assert_eq!(
            error,
            "This code is for a render computer. On the computer, choose Connect a phone."
        );
        for role in [PeerRole::Monitor, PeerRole::Admin] {
            let text = link_text(role, "Studio", &["10.0.0.5:7878"]);
            let error = read_pairing_link(&text).expect_err("a phone only pairs as a viewer");
            assert!(error.contains("cannot do"), "{error}");
            assert!(!error.contains(TOKEN));
        }
    }

    #[test]
    fn damaged_links_get_plain_words_that_hold_none_of_the_link() {
        for text in [
            "",
            "hello",
            "https://example.com/pair?t=GW1-x",
            "pleochroma://pair?v=2&r=viewer",
            "pleochroma://pair?v=1&r=viewer&a=10.0.0.5:7878&e=7879",
        ] {
            let error = read_pairing_link(text).expect_err("not a usable link");
            assert!(error.ends_with('.'), "{error}");
            assert!(!error.contains("GW1-"), "{error}");
        }
        let newer = read_pairing_link("pleochroma://pair?v=2&r=viewer").expect_err("newer");
        assert!(newer.contains("Update the app"), "{newer}");
    }

    #[test]
    fn every_claim_error_has_a_plain_sentence() {
        let used = claim_error_words(&ClaimAnyError::CodeExpiredOrUsed);
        assert_eq!(
            used,
            "This code expired or was already used. If you didn't use it, someone else may have \
             paired \u{2014} check Devices on the computer."
        );
        assert!(claim_error_words(&ClaimAnyError::Unreachable).contains("Same Wi-Fi?"));
        assert!(claim_error_words(&ClaimAnyError::AllTimedOut).contains("same Wi-Fi"));
        for error in [
            ClaimAnyError::NoAddresses,
            ClaimAnyError::InvalidCode,
            ClaimAnyError::AllTimedOut,
            ClaimAnyError::Refused,
            ClaimAnyError::Unreachable,
            ClaimAnyError::WrongCoordinator,
            ClaimAnyError::Failed("The exchange broke.".into()),
        ] {
            let words = claim_error_words(&error);
            assert!(words.ends_with('.') || words.ends_with('?'), "{words}");
        }
        assert_eq!(
            claim_error_words(&ClaimAnyError::Failed("The exchange broke.".into())),
            "Pairing failed: The exchange broke."
        );
    }

    #[test]
    fn a_paired_computer_is_remembered_with_every_address() {
        let paired = PairedServer {
            name: "Studio PC".to_owned(),
            addresses: vec!["10.0.0.5:7878".to_owned(), "192.168.178.20:7878".to_owned()],
            enroll_port: 7879,
        };
        let mut settings = MobileSettings::default();
        settings.render.backend = RenderBackend::Remote;
        assert!(remember_paired(&mut settings, &paired));
        assert_eq!(settings.remote.name, "Studio PC");
        assert_eq!(settings.remote.address, "10.0.0.5:7878");
        assert_eq!(settings.remote.addresses.len(), 2);
        assert_eq!(settings.remote.enroll_port, 7879);
        assert!(settings.remote.enrolled);
        // Pairing does not switch the backend; the caller stops what was running.
        assert_eq!(settings.render.backend, RenderBackend::Remote);

        assert!(clear_server_settings(&mut settings));
        assert!(settings.remote.endpoint_addresses().is_empty());
        assert_eq!(settings.remote.enroll_port, 0);
    }

    fn paired_platform() -> MockPlatform {
        let platform = MockPlatform::default();
        Enrollment {
            encoded: Zeroizing::new("bundle".to_owned()),
        }
        .install(&platform)
        .expect("the mock vault accepts the bundle");
        assert_eq!(platform.stored_credentials().as_deref(), Some("bundle"));
        platform
    }

    #[test]
    fn forgetting_erases_the_stored_bundle() {
        let platform = paired_platform();
        forget(&platform).expect("the mock vault erases");
        assert_eq!(platform.stored_credentials(), None);
        assert_eq!(platform.load_remote_credentials(), Ok(None));
    }

    #[test]
    fn a_forgotten_server_cannot_be_reached_until_the_phone_pairs_again() {
        let platform = paired_platform();
        forget(&platform).expect("the mock vault erases");
        let error = test_connection(&platform, "127.0.0.1:1", &AtomicBool::new(false))
            .expect_err("without credentials nothing connects");
        assert_eq!(error, "Enroll this phone first.");
    }

    #[test]
    fn forgetting_with_nothing_stored_succeeds_and_can_be_repeated() {
        let platform = MockPlatform::default();
        assert_eq!(forget(&platform), Ok(()));
        assert_eq!(forget(&platform), Ok(()));
        assert_eq!(platform.stored_credentials(), None);
    }

    #[test]
    fn a_failed_erase_reports_the_error_and_a_retry_finishes_it() {
        let platform = paired_platform();
        platform.set_forget_failure(true);
        let error = forget(&platform).expect_err("the vault refuses");
        assert!(error.contains("could not be erased"), "{error}");
        assert_eq!(platform.stored_credentials().as_deref(), Some("bundle"));

        platform.set_forget_failure(false);
        assert_eq!(forget(&platform), Ok(()));
        assert_eq!(platform.stored_credentials(), None);
    }

    #[test]
    fn a_platform_without_a_vault_cannot_claim_to_have_forgotten() {
        // The trait's own default (nothing overridden), not a mock's copy of its message.
        let platform = BarePlatform::new();
        let error = forget(&platform).expect_err("no vault, no claim");
        assert!(error.contains("no encrypted credential vault"), "{error}");

        // A mock that is told it has no vault behaves the same.
        let mock = MockPlatform::default();
        mock.set_vault_available(false);
        let error = forget(&mock).expect_err("no vault, no claim");
        assert!(error.contains("no encrypted credential vault"), "{error}");
    }

    #[test]
    fn clearing_the_server_settings_keeps_the_preferences() {
        let mut settings = MobileSettings::default();
        settings.remote.name = "Studio".to_owned();
        settings.remote.address = "192.168.1.5:7000".to_owned();
        settings.remote.enrolled = true;
        settings.remote.transfer = RemoteTransfer::FullData;
        settings.remote.cadence_ms = 900;
        settings.remote.allow_metered = true;
        settings.render.backend = RenderBackend::Remote;

        assert!(clear_server_settings(&mut settings));

        assert_eq!(settings.remote.name, "");
        assert_eq!(settings.remote.address, "");
        assert!(!settings.remote.enrolled);
        assert_eq!(settings.render.backend, RenderBackend::Automatic);
        assert_eq!(settings.remote.transfer, RemoteTransfer::FullData);
        assert_eq!(settings.remote.cadence_ms, 900);
        assert!(settings.remote.allow_metered);
    }

    #[test]
    fn clearing_the_server_settings_leaves_another_backend_alone() {
        for backend in [
            RenderBackend::Automatic,
            RenderBackend::DeviceCpu,
            RenderBackend::DeviceGpu,
        ] {
            let mut settings = MobileSettings::default();
            settings.remote.address = "host:1".to_owned();
            settings.remote.enrolled = true;
            settings.render.backend = backend;

            assert!(!clear_server_settings(&mut settings));

            assert_eq!(settings.render.backend, backend);
            assert_eq!(settings.remote.address, "");
            assert!(!settings.remote.enrolled);
        }
    }
}
