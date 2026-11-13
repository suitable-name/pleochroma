//! `TILT_CURVES` leaves the connection usable for a later `LibraryRequest`.

use super::fixtures::{test_db, tiny_scene};
use crate::serve::handle_connection;
use pleochroma_net::{handshake, messages::ClientMessage};
use std::net::TcpListener;

/// `serve::tilt::handle_tilt_curves_request` restores the blocking read timeout that
/// `poll_for_cancel` leaves on the socket.
///
/// Otherwise the connection's next `read_message` fails unless its frame is already
/// buffered. Runs over a real loopback `TcpStream`, because the `DuplexHalf` doubles cannot
/// time out a read, and sends the second message only after the first reply.
#[test]
fn tilt_curves_request_then_library_request_both_get_replies_over_a_real_socket() {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();

    let server = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        handle_connection(stream, 2, &test_db()).unwrap();
    });

    let mut client = std::net::TcpStream::connect(addr).unwrap();
    pleochroma_net::messages::write_message(&mut client, &handshake::local_hello()).unwrap();
    let welcome: pleochroma_net::messages::Welcome =
        pleochroma_net::messages::read_message(&mut client).unwrap();
    assert!(welcome.tilt_curves);

    let tilt_request = pleochroma_net::messages::TiltCurvesRequest {
        request_id: 1,
        scene: tiny_scene(),
    };
    pleochroma_net::messages::write_message(
        &mut client,
        &ClientMessage::TiltCurvesRequest(Box::new(tilt_request)),
    )
    .unwrap();
    let tilt_response: pleochroma_net::messages::TiltCurvesResponse =
        pleochroma_net::messages::read_message(&mut client).unwrap();
    assert!(
        matches!(
            tilt_response,
            pleochroma_net::messages::TiltCurvesResponse::Curves(_)
        ),
        "{tilt_response:?}"
    );

    // Sent only after the TILT_CURVES reply, so it cannot already be buffered: this proves
    // the socket was restored to blocking reads.
    pleochroma_net::messages::write_message(
        &mut client,
        &ClientMessage::Library(Box::new(
            pleochroma_net::library::LibraryRequest::FilterOptions,
        )),
    )
    .unwrap();
    let library_response: pleochroma_net::library::LibraryResponse =
        pleochroma_net::messages::read_message(&mut client).unwrap();
    assert!(
        matches!(
            library_response,
            pleochroma_net::library::LibraryResponse::FilterOptions { .. }
        ),
        "{library_response:?}"
    );

    drop(client);
    server.join().unwrap();
}
