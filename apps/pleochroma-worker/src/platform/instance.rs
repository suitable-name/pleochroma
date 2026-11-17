//! One coordinator window per user and folder.
//!
//! The first start binds a loopback port that only this user and folder map to; holding the
//! port is the lock, and the operating system frees it when the process ends, so a crash leaves
//! no stale lock behind. A second start finds the first by asking that port, tells it to bring
//! its window forward (the activate ping), and exits with status 0.
//!
//! # Naming
//!
//! The port comes from a hash of the user name and the data folder: [`CANDIDATES`] ports in
//! the dynamic range, tried in order, so one port taken by an unrelated program does not
//! disable the check. A start asks every candidate first and binds only when none answers with
//! its own key, so the second start never takes a port the first one moved to.
//!
//! # The talk
//!
//! One line each way, over `127.0.0.1` only, with short timeouts:
//! `PLEOCHROMA-ACTIVATE <key>` (bring the window forward) or `PLEOCHROMA-PRESENT <key>` (only
//! ask), answered by `PLEOCHROMA-OK <key>`. The key is the hash, so another user's coordinator
//! that happens to sit on the same port never answers as this user's.

use std::{
    io::{BufRead, BufReader, Read, Write},
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    path::Path,
    time::Duration,
};

/// The first port of the range the instance ports come from (the start of the dynamic range).
const PORT_BASE: u16 = 49_152;

/// How many ports the range holds.
const PORT_SPAN: u64 = 16_384;

/// How many ports a user and folder may use.
pub const CANDIDATES: u64 = 8;

/// How long any one step of the talk may take.
const IO_TIMEOUT: Duration = Duration::from_millis(500);

/// How long a start waits for the holder of a port it could not bind to answer.
const RACE_WAIT: Duration = Duration::from_secs(2);

/// The longest request line read.
const MAX_LINE: u64 = 256;

/// What [`claim`] found.
#[derive(Debug)]
pub enum Claim {
    /// Nothing runs yet: this process is the instance and holds the guard.
    First(InstanceGuard),
    /// Another process of this user already runs, and has been told (or only asked).
    Existing,
    /// No port could be bound and none answered: the check is skipped.
    Unavailable,
}

/// The lock of the running instance. Dropping it frees the port.
#[derive(Debug)]
pub struct InstanceGuard {
    listener: TcpListener,
    key: String,
}

/// What a second start asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Request {
    /// Bring the window forward.
    Activate,
    /// Only report that an instance runs.
    Present,
}

/// The user name the hash uses.
fn user_name() -> String {
    ["USERNAME", "USER", "LOGNAME"]
        .iter()
        .find_map(|key| std::env::var(key).ok().filter(|v| !v.is_empty()))
        .unwrap_or_else(|| "user".to_owned())
}

/// FNV-1a over `bytes`: a stable hash, so every start of the program agrees on the ports.
fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

/// The hash of a user and a data folder. On Windows the folder compares without case.
#[must_use]
pub fn instance_hash(user: &str, scope: &Path) -> u64 {
    let mut text = format!("pleochroma-coordinator|{user}|{}", scope.display());
    if cfg!(windows) {
        text = text.to_lowercase();
    }
    fnv1a(text.as_bytes())
}

/// The key the two sides compare: the hash in 16 hex digits.
#[must_use]
pub fn instance_key(hash: u64) -> String {
    format!("{hash:016x}")
}

/// The ports this hash may use, in the order they are tried.
#[must_use]
pub fn candidate_ports(hash: u64) -> Vec<u16> {
    (0..CANDIDATES)
        .map(|i| {
            let offset = hash.wrapping_add(i) % PORT_SPAN;
            PORT_BASE + u16::try_from(offset).unwrap_or(0)
        })
        .collect()
}

/// The line a second start sends.
fn request_line(request: Request, key: &str) -> String {
    let word = match request {
        Request::Activate => "ACTIVATE",
        Request::Present => "PRESENT",
    };
    format!("PLEOCHROMA-{word} {key}\n")
}

/// What a request line asks for, or `None` when it is not one for this key.
fn parse_request(line: &str, key: &str) -> Option<Request> {
    let mut words = line.split_whitespace();
    let request = match words.next()? {
        "PLEOCHROMA-ACTIVATE" => Request::Activate,
        "PLEOCHROMA-PRESENT" => Request::Present,
        _ => return None,
    };
    (words.next() == Some(key) && words.next().is_none()).then_some(request)
}

/// The answer to a request for this key.
fn reply_line(key: &str) -> String {
    format!("PLEOCHROMA-OK {key}\n")
}

/// Asks the instance on `port`. True when it answered with `key`.
fn ping(port: u16, key: &str, request: Request) -> bool {
    ping_within(port, key, request, IO_TIMEOUT)
}

/// [`ping`] with `wait` as the limit of each step of the talk.
fn ping_within(port: u16, key: &str, request: Request, wait: Duration) -> bool {
    let addr = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let Ok(mut stream) = TcpStream::connect_timeout(&addr, wait) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(wait));
    let _ = stream.set_write_timeout(Some(wait));
    if stream
        .write_all(request_line(request, key).as_bytes())
        .is_err()
    {
        return false;
    }
    let mut answer = String::new();
    BufReader::new(stream.take(MAX_LINE))
        .read_line(&mut answer)
        .is_ok()
        && answer == reply_line(key)
}

/// Finds a running instance of this user and folder, or becomes it.
///
/// `scope` is the coordinator's data folder. With `activate` a running instance is told to
/// bring its window forward; without it (an auto-start) it is only asked, so a login never
/// pops a window up.
#[must_use]
pub fn claim(scope: &Path, activate: bool) -> Claim {
    let hash = instance_hash(&user_name(), scope);
    let key = instance_key(hash);
    let ports = candidate_ports(hash);
    let request = if activate {
        Request::Activate
    } else {
        Request::Present
    };
    if ports.iter().any(|&port| ping(port, &key, request)) {
        return Claim::Existing;
    }
    for port in ports {
        if let Ok(listener) = TcpListener::bind((Ipv4Addr::LOCALHOST, port)) {
            return Claim::First(InstanceGuard { listener, key });
        }
        // The port is taken. Another start of the program that began at the same moment may
        // hold it and not answer yet (its window is still opening): wait for it, so the two
        // do not both become the instance on neighboring ports.
        if ping_within(port, &key, request, RACE_WAIT) {
            return Claim::Existing;
        }
    }
    Claim::Unavailable
}

impl InstanceGuard {
    /// Starts answering second starts on a thread of its own.
    ///
    /// `on_activate` runs (on that thread) for every request to bring the window forward, before
    /// the answer is sent; it must hand the work to the window's thread. The thread, and with it
    /// the lock, lasts as long as the process.
    pub fn listen(self, on_activate: impl Fn() + Send + 'static) {
        let Self { listener, key } = self;
        let spawned = std::thread::Builder::new()
            .name("instance-listener".to_owned())
            .spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { continue };
                    answer(&stream, &key, &on_activate);
                }
            });
        if let Err(e) = spawned {
            tracing::warn!("could not start the single-instance listener: {e}");
        }
    }
}

/// Reads one request from `stream` and answers it.
fn answer(stream: &TcpStream, key: &str, on_activate: &impl Fn()) {
    let _ = stream.set_read_timeout(Some(IO_TIMEOUT));
    let _ = stream.set_write_timeout(Some(IO_TIMEOUT));
    let mut line = String::new();
    if BufReader::new(stream.take(MAX_LINE))
        .read_line(&mut line)
        .is_err()
    {
        return;
    }
    let Some(request) = parse_request(&line, key) else {
        return;
    };
    if request == Request::Activate {
        on_activate();
    }
    let mut writer = stream;
    let _ = writer.write_all(reply_line(key).as_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        path::PathBuf,
        sync::{
            Arc,
            atomic::{AtomicUsize, Ordering},
        },
    };

    #[test]
    fn ports_are_stable_distinct_and_in_the_dynamic_range() {
        let hash = instance_hash("anna", Path::new("/home/anna/.local/share/pleochroma"));
        let ports = candidate_ports(hash);
        assert_eq!(ports, candidate_ports(hash));
        assert_eq!(ports.len(), 8);
        let mut sorted = ports.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 8);
        assert!(ports.iter().all(|&p| p >= 49_152));
    }

    #[test]
    fn ports_stay_in_range_at_the_top_of_the_hash() {
        let ports = candidate_ports(u64::MAX);
        assert!(ports.iter().all(|&p| p >= 49_152));
    }

    #[test]
    fn users_and_folders_get_their_own_keys() {
        let folder = Path::new("/data/a");
        let anna = instance_hash("anna", folder);
        assert_ne!(anna, instance_hash("ben", folder));
        assert_ne!(anna, instance_hash("anna", Path::new("/data/b")));
        assert_eq!(anna, instance_hash("anna", folder));
        assert_eq!(instance_key(0xab), "00000000000000ab");
    }

    #[test]
    fn requests_parse_only_for_their_own_key() {
        let key = "00000000000000ab";
        assert_eq!(
            parse_request(&request_line(Request::Activate, key), key),
            Some(Request::Activate)
        );
        assert_eq!(
            parse_request(&request_line(Request::Present, key), key),
            Some(Request::Present)
        );
        assert_eq!(
            parse_request("PLEOCHROMA-ACTIVATE 0000000000000001\n", key),
            None
        );
        assert_eq!(parse_request("GET / HTTP/1.1\n", key), None);
        assert_eq!(parse_request("PLEOCHROMA-ACTIVATE\n", key), None);
        assert_eq!(
            parse_request("PLEOCHROMA-ACTIVATE 00000000000000ab x\n", key),
            None
        );
        assert_eq!(parse_request("", key), None);
    }

    #[test]
    fn a_second_claim_finds_the_first_and_activates_it() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_nanos());
        let scope = PathBuf::from(format!("/pleochroma-test/{}/{nanos}", std::process::id()));
        let Claim::First(guard) = claim(&scope, true) else {
            // Every candidate port is taken by something else on this machine.
            return;
        };
        let count = Arc::new(AtomicUsize::new(0));
        let seen = Arc::clone(&count);
        guard.listen(move || {
            seen.fetch_add(1, Ordering::SeqCst);
        });
        assert!(matches!(claim(&scope, true), Claim::Existing));
        assert_eq!(count.load(Ordering::SeqCst), 1);
        // An auto-start only asks: the window is not brought forward.
        assert!(matches!(claim(&scope, false), Claim::Existing));
        assert_eq!(count.load(Ordering::SeqCst), 1);
    }
}
