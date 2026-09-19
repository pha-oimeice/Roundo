use crossbeam::channel::{self, Receiver, Sender, TryRecvError, TrySendError};
use serde_json::Value;

/// Maximum serialized JSON command size admitted to a request queue.
pub const MAX_JSON_REQUEST_BYTES: usize = 64 * 1024;

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

/// Clonable producer for one shared bounded request queue.
///
/// Clones do not create independent queues. [`submit`](Self::submit) is
/// non-blocking and each accepted call receives its own one-shot response.
#[derive(Clone)]
pub struct RequestResponseIo<Request, Response> {
    sender: Sender<(Request, Sender<Response>)>,
}

/// Handle for the response to exactly one accepted request.
///
/// Dropping it disconnects that response path; it does not cancel work already
/// dequeued by the consumer.
pub struct RequestCall<Response>(Receiver<Response>);

/// Non-blocking state of one accepted request's private response channel.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum ResponsePoll<Response> {
    /// The response sender still exists but has not sent a value.
    Pending,
    /// The one response value was received.
    Ready(Response),
    /// Every response sender was dropped without sending a value.
    Disconnected,
}

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

/// Failure to admit a JSON request; no request is queued on any variant.
#[derive(Debug, Clone, Eq, PartialEq)]
pub enum JsonSubmitError {
    Full,
    Disconnected,
    InputTooLarge { command: String },
}

/// Failure to admit a typed request; no request is queued on either variant.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum SubmitError {
    Full,
    Disconnected,
}

impl<Request, Response> RequestResponsePipe<Request, Response> {
    /// Creates a pipe whose shared request queue holds at most `capacity` items.
    ///
    /// A zero capacity creates a rendezvous channel: non-blocking submission
    /// succeeds only while a receiver is ready.
    pub fn bounded(capacity: usize) -> Self {
        let (sender, receiver) = channel::bounded(capacity);
        Self { sender, receiver }
    }

    /// Clones a producer connected to this pipe's request queue.
    pub fn io(&self) -> RequestResponseIo<Request, Response> {
        RequestResponseIo {
            sender: self.sender.clone(),
        }
    }

    /// Attempts to dequeue one request without blocking.
    ///
    /// `None` means either that the queue is currently empty or that all
    /// producers are disconnected; this API intentionally does not distinguish
    /// those states. Accepted requests are returned in channel FIFO order.
    pub fn try_receive(&self) -> Option<(Request, ResponseSender<Response>)> {
        self.receiver
            .try_recv()
            .ok()
            .map(|(request, sender)| (request, ResponseSender(sender)))
    }
}

/// Consuming sender for one request's private response channel.
pub struct ResponseSender<Response>(Sender<Response>);
impl<Response> ResponseSender<Response> {
    /// Delivers the response, returning ownership when the caller disappeared.
    pub fn respond(self, response: Response) -> Result<(), Response> {
        return self.0.send(response).map_err(|error| error.0);
    }
}

impl<Request, Response> RequestResponseIo<Request, Response> {
    /// Attempts non-blocking admission to the shared request queue.
    ///
    /// Success transfers ownership of `request` to the consumer. Failure drops
    /// the request because [`SubmitError`] does not carry it back.
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
        let command_value = request.get("command");
        let command = command_value
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
        let command_value = command.get("command");
        let command_name = command_value
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
    /// Observes whether the response is pending, ready, or permanently disconnected.
    pub fn poll(&self) -> ResponsePoll<Response> {
        match self.0.try_recv() {
            Ok(value) => ResponsePoll::Ready(value),
            Err(TryRecvError::Empty) => ResponsePoll::Pending,
            Err(TryRecvError::Disconnected) => ResponsePoll::Disconnected,
        }
    }

    /// Attempts to receive the response without blocking.
    ///
    /// This compatibility projection maps both pending and disconnected to
    /// `None`. Long-lived polling adapters should use [`Self::poll`] so a lost
    /// response sender cannot leave a request retained forever.
    pub fn try_result(&self) -> Option<Response> {
        match self.poll() {
            ResponsePoll::Ready(value) => Some(value),
            ResponsePoll::Pending | ResponsePoll::Disconnected => None,
        }
    }
    /// Blocks the current OS thread until a response arrives or its sender drops.
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
    fn accepted_call_exposes_response_sender_disconnect() {
        let pipe = RequestResponsePipe::<(), ()>::bounded(1);
        let io = pipe.io();
        let call = io.submit(()).unwrap();
        let (_, response) = pipe.try_receive().unwrap();
        drop(response);

        assert_eq!(call.poll(), ResponsePoll::Disconnected);
    }

    #[test]
    fn contextual_json_request_keeps_context_outside_command_payload() {
        let pipe = RequestResponsePipe::<CommandTransport<u64>, ()>::bounded(1);
        let io = ContextualJsonRequestResponseIo::new(pipe.io());
        let command = serde_json::json!({
            "version": 1,
            "command": "ui.back",
            "arguments": {},
        });
        let expected = command.clone();
        io.submit(command, 9).unwrap();
        let (transport, _) = pipe.try_receive().unwrap();
        assert_eq!(transport.command, expected);
        assert_eq!(transport.context, 9);
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
