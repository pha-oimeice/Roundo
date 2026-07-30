use crate::connection::Connection;
use crate::schema::{ClientMessage, EchoTestResponse, ServerMessage};

pub type EchoTestClientConnection<Stream> = Connection<Stream, ServerMessage, ClientMessage>;
pub type EchoTestServerConnection<Stream> = Connection<Stream, ClientMessage, ServerMessage>;

pub fn dispatch(message: ClientMessage) -> ServerMessage {
    match message {
        ClientMessage::EchoTest(request) => ServerMessage::EchoTest(EchoTestResponse {
            sequence: request.sequence,
            message: request.message,
        }),
    }
}
