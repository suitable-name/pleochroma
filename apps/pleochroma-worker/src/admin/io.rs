//! The byte stream of an administration session and the way it is polled.
//!
//! A session sends pushes on its own schedule while it waits for the next request, on one
//! stream that cannot be split (a TLS stream has one owner). So the session polls: it waits
//! a short time for the first byte of a request and, once a byte has arrived, reads the rest
//! of that frame under a longer, bounded timeout. A poll that finds nothing is not an error.

use pleochroma_net::{
    framing::{self, FramingError, LEN_PREFIX_BYTES, MAX_CONTROL_FRAME_LEN},
    messages::NetError,
};
use serde::de::DeserializeOwned;
use std::{
    io::{self, Read, Write},
    net::TcpStream,
    time::Duration,
};

/// How long the rest of a request may take once its first byte has arrived. A frame that
/// stalls in the middle is a broken peer, not a quiet one.
pub const FRAME_REMAINDER_TIMEOUT: Duration = Duration::from_secs(10);

/// A stream an administration session can run on: readable and writable, with timeouts the
/// session sets itself. Implemented for the accepted TLS stream and for a plain `TcpStream`.
pub trait SessionIo: Read + Write {
    /// Sets how long a read waits (`None` waits forever).
    ///
    /// # Errors
    ///
    /// The operating system's refusal.
    fn set_read_timeout(&mut self, timeout: Option<Duration>) -> io::Result<()>;

    /// Sets how long a write waits (`None` waits forever).
    ///
    /// # Errors
    ///
    /// The operating system's refusal.
    fn set_write_timeout(&mut self, timeout: Option<Duration>) -> io::Result<()>;
}

impl SessionIo for TcpStream {
    fn set_read_timeout(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        Self::set_read_timeout(&*self, timeout)
    }

    fn set_write_timeout(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        Self::set_write_timeout(&*self, timeout)
    }
}

impl SessionIo for rustls::StreamOwned<rustls::ServerConnection, TcpStream> {
    fn set_read_timeout(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        TcpStream::set_read_timeout(&self.sock, timeout)
    }

    fn set_write_timeout(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        TcpStream::set_write_timeout(&self.sock, timeout)
    }
}

impl SessionIo for rustls::StreamOwned<rustls::ClientConnection, TcpStream> {
    fn set_read_timeout(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        TcpStream::set_read_timeout(&self.sock, timeout)
    }

    fn set_write_timeout(&mut self, timeout: Option<Duration>) -> io::Result<()> {
        TcpStream::set_write_timeout(&self.sock, timeout)
    }
}

/// Remembers the read timeout last set, so a poll every 100 ms does not make a system call
/// every time.
#[derive(Debug, Default)]
pub struct ReadTimeout {
    current: Applied,
}

/// The read timeout last set on the stream.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Applied {
    /// Nothing set yet.
    #[default]
    Unset,
    /// Blocking reads (no timeout).
    Blocking,
    /// This timeout.
    After(Duration),
}

impl From<Option<Duration>> for Applied {
    fn from(timeout: Option<Duration>) -> Self {
        timeout.map_or(Self::Blocking, Self::After)
    }
}

impl ReadTimeout {
    /// Sets `timeout` on `stream` unless it is already in force.
    fn apply<S: SessionIo>(&mut self, stream: &mut S, timeout: Option<Duration>) -> io::Result<()> {
        let wanted = Applied::from(timeout);
        if self.current != wanted {
            stream.set_read_timeout(timeout)?;
            self.current = wanted;
        }
        Ok(())
    }
}

/// What one poll of the stream found.
#[derive(Debug, PartialEq, Eq)]
pub enum Poll<T> {
    /// Nothing arrived within the poll window.
    Pending,
    /// The peer closed the connection (cleanly or not).
    Closed,
    /// One whole message.
    Message(T),
}

/// Whether `error` is a read timeout. A TLS stream reports the inner socket's timeout as
/// `WriteZero` on some platforms, so that counts too.
#[must_use]
pub fn is_timeout(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut | io::ErrorKind::WriteZero
    )
}

/// Whether `error` means the peer is gone.
fn is_disconnect(error: &io::Error) -> bool {
    matches!(
        error.kind(),
        io::ErrorKind::UnexpectedEof
            | io::ErrorKind::ConnectionReset
            | io::ErrorKind::ConnectionAborted
            | io::ErrorKind::BrokenPipe
            | io::ErrorKind::NotConnected
    )
}

/// Waits up to `poll` for the first byte of a message and reads the whole message when one
/// comes.
///
/// The length prefix is read with a single `read`, so a timeout can only land before any
/// byte of a new message has arrived; after that the rest is read under
/// [`FRAME_REMAINDER_TIMEOUT`].
///
/// # Errors
///
/// [`NetError`] for a transport failure inside a frame, a frame over the control limit or
/// bytes that do not decode as `T`.
pub fn poll_message<S: SessionIo, T: DeserializeOwned>(
    stream: &mut S,
    poll: Duration,
    timeouts: &mut ReadTimeout,
) -> Result<Poll<T>, NetError> {
    timeouts
        .apply(stream, Some(poll))
        .map_err(|e| NetError::Framing(FramingError::Io(e)))?;

    let mut len_bytes = [0u8; LEN_PREFIX_BYTES];
    let read = match stream.read(&mut len_bytes) {
        Ok(0) => return Ok(Poll::Closed),
        Ok(n) => n,
        Err(e) if is_timeout(&e) => return Ok(Poll::Pending),
        Err(e) if is_disconnect(&e) => return Ok(Poll::Closed),
        Err(e) => return Err(NetError::Framing(FramingError::Io(e))),
    };

    // At least one byte has arrived: the rest of the frame is read under its own bound.
    timeouts
        .apply(stream, Some(FRAME_REMAINDER_TIMEOUT))
        .map_err(|e| NetError::Framing(FramingError::Io(e)))?;
    let payload =
        framing::read_frame_continuing(stream, &len_bytes[..read], MAX_CONTROL_FRAME_LEN)?;
    Ok(Poll::Message(
        pleochroma_net::messages::decode_control_frame(&payload)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pleochroma_net::admin::protocol::AdminRequest;
    use std::net::TcpListener;

    fn pair() -> (TcpStream, TcpStream) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (server, _) = listener.accept().unwrap();
        (server, client)
    }

    #[test]
    fn a_quiet_stream_is_pending_a_message_arrives_whole_and_a_close_is_seen() {
        let (mut server, mut client) = pair();
        let mut timeouts = ReadTimeout::default();
        let poll = Duration::from_millis(30);

        assert_eq!(
            poll_message::<_, AdminRequest>(&mut server, poll, &mut timeouts).unwrap(),
            Poll::Pending
        );

        let request = AdminRequest::Status { req_id: 7 };
        pleochroma_net::messages::write_message(&mut client, &request).unwrap();
        assert_eq!(
            poll_message::<_, AdminRequest>(&mut server, Duration::from_secs(2), &mut timeouts)
                .unwrap(),
            Poll::Message(request)
        );

        drop(client);
        assert_eq!(
            poll_message::<_, AdminRequest>(&mut server, Duration::from_secs(2), &mut timeouts)
                .unwrap(),
            Poll::Closed
        );
    }

    #[test]
    fn a_message_split_across_two_writes_is_still_one_message() {
        let (mut server, mut client) = pair();
        let mut timeouts = ReadTimeout::default();
        let request = AdminRequest::Status { req_id: 9 };
        let mut wire = Vec::new();
        pleochroma_net::messages::write_message(&mut wire, &request).unwrap();

        let writer = std::thread::spawn(move || {
            client.write_all(&wire[..2]).unwrap();
            client.flush().unwrap();
            std::thread::sleep(Duration::from_millis(150));
            client.write_all(&wire[2..]).unwrap();
            client.flush().unwrap();
            client
        });
        // The poll window ends before the second half arrives; the frame is not lost.
        let got =
            poll_message::<_, AdminRequest>(&mut server, Duration::from_secs(2), &mut timeouts)
                .unwrap();
        assert_eq!(got, Poll::Message(request));
        drop(writer.join().unwrap());
    }

    #[test]
    fn an_oversized_frame_is_refused_before_it_is_read() {
        let (mut server, mut client) = pair();
        let mut timeouts = ReadTimeout::default();
        client
            .write_all(&(MAX_CONTROL_FRAME_LEN + 1).to_le_bytes())
            .unwrap();
        let result =
            poll_message::<_, AdminRequest>(&mut server, Duration::from_secs(2), &mut timeouts);
        assert!(result.is_err());
    }
}
