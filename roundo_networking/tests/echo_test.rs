// Exercises framing and postcard serialization over an in-memory duplex stream.
use roundo_networking::echo_test::{EchoTestClientConnection, EchoTestServerConnection, dispatch};
use roundo_networking::schema::{ClientMessage, EchoTestRequest, EchoTestResponse, ServerMessage};
use tokio::time::{Duration, timeout};

const TEST_TIMEOUT: Duration = Duration::from_secs(3);
const ECHO_SEQUENCE: u32 = 358;
const ECHO_MESSAGE: &str = "roundo postcard echo";

#[tokio::test]
async fn echo_message_round_trips_over_a_framed_connection() {
    // A single outer deadline covers both peers and task scheduling.
    timeout(TEST_TIMEOUT, async {
        let (client_stream, server_stream) = tokio::io::duplex(1024);
        // The server consumes exactly one request and mirrors it through dispatch.
        let server_task = tokio::spawn(async move {
            let mut connection = EchoTestServerConnection::new(server_stream);
            let request = connection
                .receive()
                .await
                .expect("server should receive the postcard request");
            connection
                .transmit(&dispatch(request))
                .await
                .expect("server should send the postcard response");
        });

        // The client validates the complete typed response, not raw bytes.
        let mut connection = EchoTestClientConnection::new(client_stream);
        connection
            .transmit(&ClientMessage::EchoTest(EchoTestRequest {
                sequence: ECHO_SEQUENCE,
                message: ECHO_MESSAGE.to_string(),
            }))
            .await
            .expect("client should send the postcard request");

        let response = connection
            .receive()
            .await
            .expect("client should receive the postcard response");
        assert_eq!(
            response,
            ServerMessage::EchoTest(EchoTestResponse {
                sequence: ECHO_SEQUENCE,
                message: ECHO_MESSAGE.to_string(),
            })
        );
        // Joining surfaces server-side panics before the test succeeds.
        server_task
            .await
            .expect("echo server task should not panic");
    })
    .await
    .expect("echo exchange should finish before the test timeout");
}
