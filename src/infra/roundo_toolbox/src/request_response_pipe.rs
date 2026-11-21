use crossbeam::channel::{self, Receiver, Sender, TryRecvError, TrySendError};
use serde_json::Value;
use std::sync::Arc;

/// Maximum serialized JSON command size admitted to a request queue.
pub const MAX_JSON_REQUEST_BYTES: usize = 64 * 1024;

/// A bounded multi-producer request queue with a private one-shot response
/// channel per request. Responses therefore cannot be consumed by another
/// caller, unlike a shared duplex queue.
pub struct RequestResponsePipe<Request, Response> {
    sender: Sender<(Request, ResponseSender<Response>)>,
    receiver: Receiver<(Request, ResponseSender<Response>)>,
}

/// Clonable producer for one shared bounded request queue.
///
/// Clones do not create independent queues. [`submit`](Self::submit) is
/// non-blocking and each accepted call receives its own one-shot response.
#[derive(Clone)]
pub struct RequestResponseIo<Request, Response> {
    sender: Sender<(Request, ResponseSender<Response>)>,
}

/// Handle for the response to exactly one accepted request.
///
/// Dropping it disconnects that response path; it does not cancel work already
/// dequeued by the consumer.
pub struct RequestCall<Response> {
    receiver: Receiver<Response>,
    identity: Arc<()>,
}

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
        self.receiver.try_recv().ok()
    }
}

/// Consuming sender for one request's private response channel.
pub struct ResponseSender<Response> {
    sender: Sender<Response>,
    identity: Arc<()>,
}
impl<Response> ResponseSender<Response> {
    /// Tests exact one-shot correspondence without receiving, completing, or
    /// keeping the response channel alive. This grants no execution authority.
    pub fn matches_call(&self, call: &RequestCall<Response>) -> bool {
        Arc::ptr_eq(&self.identity, &call.identity)
    }

    /// Delivers the response, returning ownership when the caller disappeared.
    pub fn respond(self, response: Response) -> Result<(), Response> {
        return self.sender.send(response).map_err(|error| error.0);
    }
}

impl<Request, Response> RequestResponseIo<Request, Response> {
    /// Attempts non-blocking admission to the shared request queue.
    ///
    /// Success transfers ownership of `request` to the consumer. Failure drops
    /// the request because [`SubmitError`] does not carry it back.
    pub fn submit(&self, request: Request) -> Result<RequestCall<Response>, SubmitError> {
        let (sender, receiver) = channel::bounded(1);
        let identity = Arc::new(());
        let response = ResponseSender {
            sender,
            identity: Arc::clone(&identity),
        };
        match self.sender.try_send((request, response)) {
            Ok(()) => Ok(RequestCall { receiver, identity }),
            Err(TrySendError::Full(_)) => Err(SubmitError::Full),
            Err(TrySendError::Disconnected(_)) => Err(SubmitError::Disconnected),
        }
    }
}

// Admit serialized bytes before submitting the concrete payload, with one error mapping.
fn submit_json_request<Response>(
    command: Value,
    submit: impl FnOnce(Value) -> Result<RequestCall<Response>, SubmitError>,
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
    submit(command).map_err(|error| match error {
        SubmitError::Full => JsonSubmitError::Full,
        SubmitError::Disconnected => JsonSubmitError::Disconnected,
    })
}

impl<Response> JsonRequestResponseIo<Response> {
    pub fn new(inner: RequestResponseIo<Value, Response>) -> Self {
        Self { inner }
    }

    pub fn submit(&self, request: Value) -> Result<RequestCall<Response>, JsonSubmitError> {
        submit_json_request(request, |request| self.inner.submit(request))
    }
}

impl<Response> RequestCall<Response> {
    /// Observes whether the response is pending, ready, or permanently disconnected.
    pub fn poll(&self) -> ResponsePoll<Response> {
        match self.receiver.try_recv() {
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
        self.receiver.recv().ok()
    }
}
