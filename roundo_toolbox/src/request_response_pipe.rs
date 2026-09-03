use crossbeam::channel::{self, Receiver, Sender, TryRecvError, TrySendError};
use serde_json::Value;

pub const MAX_JSON_REQUEST_BYTES: usize = 64 * 1024;

/// Adapter-owned metadata that accompanies a typed JSON command without
/// becoming part of its command payload. The command dispatcher receives the
/// same JSON envelope from every adapter; a host may use this context for
/// source authority before dispatching it.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CommandTransportContext {
    #[default]
    Host,
    WebView {
        instance_id: u64,
    },
}

/// A typed JSON command plus adapter-owned transport context. `command` is
/// deliberately left untouched so strict command-envelope validation remains
/// the shared execution seam.
#[derive(Clone, Debug, PartialEq)]
pub struct CommandTransport<Context> {
    pub command: Value,
    pub context: Context,
}

/// A bounded multi-producer request queue with a private one-shot response
/// channel per request. Responses therefore cannot be consumed by another
/// caller, unlike a shared duplex queue.
pub struct RequestResponsePipe<Request, Response> {
    sender: Sender<(Request, Sender<Response>)>,
    receiver: Receiver<(Request, Sender<Response>)>,
}

#[derive(Clone)]
pub struct RequestResponseIo<Request, Response> {
    sender: Sender<(Request, Sender<Response>)>,
}

pub struct RequestCall<Response>(Receiver<Response>);

/// Source-neutral JSON request adapter which limits serialized input before it
/// can enter the shared request queue.
#[derive(Clone)]
pub struct JsonRequestResponseIo<Response> {
    inner: RequestResponseIo<Value, Response>,
}

/// JSON submission adapter that carries non-JSON command context separately.
/// This is intentionally a concrete transport shape rather than a trait: the
/// host has two real adapters (terminal and WebView), while their command
/// payload remains source-neutral.
#[derive(Clone)]
pub struct ContextualJsonRequestResponseIo<Context, Response> {
    inner: RequestResponseIo<CommandTransport<Context>, Response>,
}

#[derive(Debug, Clone, Eq, PartialEq)]
pub enum JsonSubmitError {
    Full,
    Disconnected,
    InputTooLarge { command: String },
}

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SubmitError {
    Full,
    Disconnected,
}

impl<Request, Response> RequestResponsePipe<Request, Response> {
    pub fn bounded(capacity: usize) -> Self {
        let (sender, receiver) = channel::bounded(capacity);
        Self { sender, receiver }
    }

    pub fn io(&self) -> RequestResponseIo<Request, Response> {
        RequestResponseIo {
            sender: self.sender.clone(),
        }
    }

    pub fn try_receive(&self) -> Option<(Request, ResponseSender<Response>)> {
        self.receiver
            .try_recv()
            .ok()
            .map(|(request, sender)| (request, ResponseSender(sender)))
    }
}

pub struct ResponseSender<Response>(Sender<Response>);
impl<Response> ResponseSender<Response> {
    pub fn respond(self, response: Response) -> Result<(), Response> {
        self.0.send(response).map_err(|error| error.0)
    }
}

impl<Request, Response> RequestResponseIo<Request, Response> {
    pub fn submit(&self, request: Request) -> Result<RequestCall<Response>, SubmitError> {
        let (sender, receiver) = channel::bounded(1);
        match self.sender.try_send((request, sender)) {
            Ok(()) => Ok(RequestCall(receiver)),
            Err(TrySendError::Full(_)) => Err(SubmitError::Full),
            Err(TrySendError::Disconnected(_)) => Err(SubmitError::Disconnected),
        }
    }
}

impl<Response> JsonRequestResponseIo<Response> {
    pub fn new(inner: RequestResponseIo<Value, Response>) -> Self {
        Self { inner }
    }

    pub fn submit(&self, request: Value) -> Result<RequestCall<Response>, JsonSubmitError> {
        let command = request
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if serde_json::to_vec(&request)
            .expect("serde_json::Value is serializable")
            .len()
            > MAX_JSON_REQUEST_BYTES
        {
            return Err(JsonSubmitError::InputTooLarge { command });
        }
        self.inner.submit(request).map_err(|error| match error {
            SubmitError::Full => JsonSubmitError::Full,
            SubmitError::Disconnected => JsonSubmitError::Disconnected,
        })
    }
}

impl<Context, Response> ContextualJsonRequestResponseIo<Context, Response> {
    pub fn new(inner: RequestResponseIo<CommandTransport<Context>, Response>) -> Self {
        Self { inner }
    }

    pub fn submit(
        &self,
        command: Value,
        context: Context,
    ) -> Result<RequestCall<Response>, JsonSubmitError> {
        let command_name = command
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if serde_json::to_vec(&command)
            .expect("serde_json::Value is serializable")
            .len()
            > MAX_JSON_REQUEST_BYTES
        {
            return Err(JsonSubmitError::InputTooLarge {
                command: command_name,
            });
        }
        self.inner
            .submit(CommandTransport { command, context })
            .map_err(|error| match error {
                SubmitError::Full => JsonSubmitError::Full,
                SubmitError::Disconnected => JsonSubmitError::Disconnected,
            })
    }
}

impl<Response> RequestCall<Response> {
    pub fn try_result(&self) -> Option<Response> {
        match self.0.try_recv() {
            Ok(value) => Some(value),
            Err(TryRecvError::Empty | TryRecvError::Disconnected) => None,
        }
    }
    pub fn wait(self) -> Option<Response> {
        self.0.recv().ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn responses_remain_with_their_original_callers() {
        let pipe = RequestResponsePipe::<u8, u8>::bounded(2);
        let io = pipe.io();
        let first = io.submit(1).unwrap();
        let second = io.submit(2).unwrap();
        let (request, reply) = pipe.try_receive().unwrap();
        assert_eq!(request, 1);
        reply.respond(10).unwrap();
        let (request, reply) = pipe.try_receive().unwrap();
        assert_eq!(request, 2);
        reply.respond(20).unwrap();
        assert_eq!(second.wait(), Some(20));
        assert_eq!(first.wait(), Some(10));
    }
    #[test]
    fn full_queue_does_not_block() {
        let pipe = RequestResponsePipe::<(), ()>::bounded(1);
        let io = pipe.io();
        let _ = io.submit(()).unwrap();
        assert!(matches!(io.submit(()), Err(SubmitError::Full)));
    }

    #[test]
    fn contextual_json_request_keeps_context_outside_command_payload() {
        let pipe = RequestResponsePipe::<CommandTransport<CommandTransportContext>, ()>::bounded(1);
        let io = ContextualJsonRequestResponseIo::new(pipe.io());
        let command = serde_json::json!({
            "version": 1,
            "command": "ui.back",
            "arguments": {},
        });
        let expected = command.clone();
        io.submit(command, CommandTransportContext::WebView { instance_id: 9 })
            .unwrap();
        let (transport, _) = pipe.try_receive().unwrap();
        assert_eq!(transport.command, expected);
        assert_eq!(
            transport.context,
            CommandTransportContext::WebView { instance_id: 9 }
        );
    }

    #[test]
    fn oversized_json_request_never_enters_the_queue() {
        let pipe = RequestResponsePipe::<Value, ()>::bounded(1);
        let io = JsonRequestResponseIo::new(pipe.io());
        let request = serde_json::json!({
            "version": 1,
            "command": "test.command",
            "arguments": {"payload": "x".repeat(MAX_JSON_REQUEST_BYTES)},
        });
        assert!(matches!(
            io.submit(request),
            Err(JsonSubmitError::InputTooLarge { command }) if command == "test.command"
        ));
        assert!(pipe.try_receive().is_none());
    }
}
