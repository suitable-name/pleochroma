//! Opening the TCP socket, the mutual-TLS handshake and `HELLO`/`WELCOME` exchange,
//! and the settings UI's standalone "Test connection" operation built on top of it.

use super::super::types::{RemoteError, RemoteStream, host_from_address};
use crate::{
    bridge::remote::{rediscover, route},
    settings::WorkerSettings,
};
use socket2::{SockRef, TcpKeepalive};
use std::{
    collections::HashMap,
    net::{TcpStream, ToSocketAddrs},
    sync::{Arc, Mutex, OnceLock, PoisonError},
};

/// Connects to `address` (`host:port`) within [`super::CONNECT_TIMEOUT`] and tunes the
/// socket for a long-lived, latency-sensitive connection.
///
/// A new connection costs about two round trips (TCP connect, then TLS handshake), which is
/// why [`super::persistent::spawn_remote_connection`] keeps one open across a live session.
///
/// - `TCP_NODELAY`: protocol messages are sent one at a time and must not wait in Nagle's
///   buffer for more data.
/// - OS-level TCP keepalive ([`super::KEEPALIVE_TIME`]/[`super::KEEPALIVE_INTERVAL`], set
///   through `socket2`, since `std` has no keepalive tuning) lets the kernel notice a dead
///   peer while nothing here is reading or writing.
///
/// # Errors
///
/// Any I/O error resolving `address` (including "no address found" for a string that
/// doesn't resolve to anything), connecting within [`super::CONNECT_TIMEOUT`], or
/// configuring the socket.
fn open_tcp(address: &str) -> std::io::Result<TcpStream> {
    let addr = address.to_socket_addrs()?.next().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            format!("no address found for {address:?}"),
        )
    })?;
    let tcp = TcpStream::connect_timeout(&addr, super::CONNECT_TIMEOUT)?;
    tcp.set_nodelay(true)?;
    let keepalive = TcpKeepalive::new()
        .with_time(super::KEEPALIVE_TIME)
        .with_interval(super::KEEPALIVE_INTERVAL);
    SockRef::from(&tcp).set_tcp_keepalive(&keepalive)?;
    Ok(tcp)
}

/// Connects to `worker.address` over mutual TLS and performs the `HELLO`/`WELCOME` handshake.
///
/// Certificates come from `worker.cert_dir`, and the client-side check of
/// [`pleochroma_net::handshake::verify_compatible`] runs. The one-shot and persistent
/// lifecycles, [`test_connection`] and the design-library client all connect through here.
///
/// The stream keeps [`super::WRITE_TIMEOUT`] for its whole life. After the handshake it has
/// no read timeout; each reader sets its own poll timeout (see `super::stream_io`).
///
/// # Write timeouts show as `WriteZero`
///
/// A write that hits [`super::WRITE_TIMEOUT`] reaches the caller as `ErrorKind::WriteZero`,
/// because rustls maps it so. Every write site treats any error as fatal, so the kind does
/// not matter here.
///
/// # Errors
///
/// See [`RemoteError`]'s variants.
pub fn connect_and_handshake(
    worker: &WorkerSettings,
) -> Result<(RemoteStream, pleochroma_net::messages::Welcome), RemoteError> {
    let config = cached_client_config(worker)?;
    connect_and_handshake_with(worker, &config)
}

/// The modification times of the CA, client certificate and client key files, in that
/// order; `None` for a file whose metadata cannot be read.
type CertStamps = [Option<std::time::SystemTime>; 3];

/// One cached `ClientConfig`, with the file stamps and verification mode it was built from.
struct CachedConfig {
    stamps: CertStamps,
    fixed_name: bool,
    config: Arc<rustls::ClientConfig>,
}

/// Dials every address of `addresses` at the same time and returns the first connection that
/// opens, together with its address. An unreachable computer costs one connect timeout, not
/// one per address.
///
/// # Errors
///
/// The last connect error, or "no address is saved" for an empty list.
fn open_any(addresses: Vec<String>) -> std::io::Result<(TcpStream, String)> {
    if let [only] = addresses.as_slice() {
        return open_tcp(only).map(|tcp| (tcp, only.clone()));
    }
    let (sender, receiver) = std::sync::mpsc::channel();
    for address in addresses {
        let sender = sender.clone();
        std::thread::spawn(move || {
            let opened = open_tcp(&address);
            // The receiver is gone once one address answered; later results are dropped.
            let _ = sender.send((address, opened));
        });
    }
    drop(sender);
    let mut last: Option<std::io::Error> = None;
    while let Ok((address, opened)) = receiver.recv() {
        match opened {
            Ok(tcp) => return Ok((tcp, address)),
            Err(error) => last = Some(error),
        }
    }
    Err(last.unwrap_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "no address is saved")
    }))
}

/// Dials `worker`'s addresses in [`route::dial_order`] (its own address first, then the rest of
/// the saved list, when the worker's address is one of them) and returns the first connection
/// together with the address that answered.
///
/// When none answers and the remote was paired through a link (verified by its fixed
/// certificate name), a background search for the moved coordinator starts
/// ([`rediscover::notify_unreachable`]); this call still fails at once.
fn open_first(worker: &WorkerSettings) -> Result<(TcpStream, String), RemoteError> {
    let saved = route::lookup_for(&worker.cert_dir, &worker.address);
    let order = route::dial_order(&worker.address, saved.as_ref());
    let opened = open_any(order);
    if opened.is_err() && saved.is_some_and(|r| r.fixed_name) {
        rediscover::notify_unreachable(&worker.cert_dir);
    }
    opened.map_err(RemoteError::Io)
}

/// Connects to the announced `addresses` of a coordinator found on the network and performs the
/// full mutual-TLS and `HELLO`/`WELCOME` handshake with the certificates saved for `worker`,
/// verified by the fixed certificate name. Only the announced addresses are dialed, never the
/// saved ones, so success proves the announcer holds a certificate of the saved authority.
///
/// # Errors
///
/// See [`RemoteError`]'s variants.
pub fn verify_announced(
    worker: &WorkerSettings,
    addresses: &[String],
) -> Result<pleochroma_net::client::ConnectionInfo, RemoteError> {
    let config = cached_client_config(worker)?;
    let (tcp, address) = open_any(addresses.to_vec()).map_err(RemoteError::Io)?;
    let (_stream, welcome) = handshake_over(tcp, &address, &config)?;
    Ok(welcome.into())
}

/// The modification times of `worker`'s three TLS files, from three `stat` calls. Cheaper
/// than reloading and parsing the files.
fn cert_stamps(worker: &WorkerSettings) -> CertStamps {
    let stamp = |path: std::path::PathBuf| {
        std::fs::metadata(path)
            .and_then(|meta| meta.modified())
            .ok()
    };
    [
        stamp(worker.ca_path()),
        stamp(worker.client_cert_path()),
        stamp(worker.client_key_path()),
    ]
}

/// The mutual-TLS `ClientConfig` for `worker`'s certificate directory, shared by every connection
/// to that worker.
///
/// The PEM files are read again only when the directory is new or one of their modification times
/// changed (enrollment rewrites them).
///
/// Sharing one config also keeps rustls' in-memory session cache, so a later connection to
/// the same worker resumes its TLS session instead of running a full handshake.
///
/// # Errors
///
/// A missing or malformed CA, certificate or key file.
pub fn cached_client_config(
    worker: &WorkerSettings,
) -> Result<Arc<rustls::ClientConfig>, RemoteError> {
    static CACHE: OnceLock<Mutex<HashMap<String, CachedConfig>>> = OnceLock::new();
    let stamps = cert_stamps(worker);
    // A remote paired through a link is verified by its fixed certificate name, whatever IP
    // was dialed; a hand-entered one keeps checking the dialed name.
    let fixed_name =
        route::lookup_for(&worker.cert_dir, &worker.address).is_some_and(|r| r.fixed_name);
    let mut cache = CACHE
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(PoisonError::into_inner);
    if let Some(hit) = cache.get(&worker.cert_dir)
        && hit.stamps == stamps
        && hit.fixed_name == fixed_name
    {
        return Ok(Arc::clone(&hit.config));
    }
    let ca = pleochroma_net::tls::load_ca(&worker.ca_path())?;
    let cert_chain = pleochroma_net::tls::load_certs(&worker.client_cert_path())?;
    let key = pleochroma_net::tls::load_private_key(&worker.client_key_path())?;
    let config = if fixed_name {
        pleochroma_net::tls::client_config_fixed_name(ca, cert_chain, key)?
    } else {
        pleochroma_net::tls::client_config(ca, cert_chain, key)?
    };
    cache.insert(
        worker.cert_dir.clone(),
        CachedConfig {
            stamps,
            fixed_name,
            config: Arc::clone(&config),
        },
    );
    drop(cache);
    Ok(config)
}

/// [`connect_and_handshake`] with the `ClientConfig` already prepared by
/// [`cached_client_config`]; everything else (socket tuning, timeouts, `HELLO`/`WELCOME`)
/// is identical.
///
/// # Errors
///
/// See [`RemoteError`]'s variants.
pub fn connect_and_handshake_with(
    worker: &WorkerSettings,
    config: &Arc<rustls::ClientConfig>,
) -> Result<(RemoteStream, pleochroma_net::messages::Welcome), RemoteError> {
    let (tcp, address) = open_first(worker)?;
    handshake_over(tcp, &address, config)
}

/// The TLS handshake and `HELLO`/`WELCOME` exchange over `tcp`, which reached `address`.
fn handshake_over(
    tcp: TcpStream,
    address: &str,
    config: &Arc<rustls::ClientConfig>,
) -> Result<(RemoteStream, pleochroma_net::messages::Welcome), RemoteError> {
    let host = host_from_address(address);
    let server_name = rustls::pki_types::ServerName::try_from(host.to_string())
        .map_err(|_| RemoteError::InvalidServerName(host.to_string()))?;
    let conn = rustls::ClientConnection::new(Arc::clone(config), server_name)
        .map_err(pleochroma_net::tls::TlsError::Rustls)?;
    let mut stream = rustls::StreamOwned::new(conn, tcp);
    // Bounds the TLS handshake and the HELLO/WELCOME read below.
    stream
        .sock
        .set_read_timeout(Some(super::HANDSHAKE_TIMEOUT))
        .map_err(RemoteError::Io)?;
    // Stays set for the connection's whole life; see the `WriteZero` note above.
    stream
        .sock
        .set_write_timeout(Some(super::WRITE_TIMEOUT))
        .map_err(RemoteError::Io)?;
    // Completes the TLS handshake now, as the worker does in
    // `apps/pleochroma-worker/src/serve/tls.rs`, so a TLS failure (wrong CA, expired
    // certificate, clock skew, SAN mismatch) is reported as such and not as an opaque I/O
    // error from the HELLO exchange.
    stream
        .conn
        .complete_io(&mut stream.sock)
        .map_err(RemoteError::Io)?;

    let welcome = pleochroma_net::client::handshake::handshake(&mut stream)?;
    // Clears the handshake read timeout; every later read sets its own.
    stream
        .sock
        .set_read_timeout(None)
        .map_err(RemoteError::Io)?;
    Ok((RemoteStream::new(stream), welcome))
}

/// The settings UI's "Test connection" operation: connect, handshake, report worker
/// identity/backend/build compatibility, then disconnect (simply by dropping the
/// stream when this returns) -- no render. Blocking; callers run this on their own
/// worker thread (see `gui::remote::setup_worker_callbacks`) and report the result back
/// to the UI thread themselves.
///
/// # Errors
///
/// See [`RemoteError`]'s variants.
pub fn test_connection(
    worker: &WorkerSettings,
) -> Result<pleochroma_net::client::ConnectionInfo, RemoteError> {
    let (_stream, welcome) = connect_and_handshake(worker)?;
    Ok(welcome.into())
}
