//! Minimal request/response protocol used to verify framed connections.

use crate::connection::Connection;
use crate::schema::{ClientMessage, EchoTestResponse, ServerMessage};

pub type EchoTestClientConnection<Stream> = Connection<Stream, ServerMessage, ClientMessage>;
pub type EchoTestServerConnection<Stream> = Connection<Stream, ClientMessage, ServerMessage>;

/// Mirrors an echo request while preserving its correlation sequence.
pub fn dispatch(message: ClientMessage) -> ServerMessage {
    match message {
        ClientMessage::EchoTest(request) => ServerMessage::EchoTest(EchoTestResponse {
            sequence: request.sequence,
            message: request.message,
        }),
    }
}
