//! Bidirectional in-process transport for paired worker endpoints.

use crossbeam::channel::{Receiver, Sender, unbounded};

#[derive(Debug)]
/// Owns clonable endpoints for asymmetric request and response types.
///
/// Cloning the pipe or an endpoint shares the same two MPMC queues; it does not
/// create a private conversation. Both queues are unbounded, so higher layers
/// own admission limits and overload policy.
pub struct CrossbeamThreadPipe<AToB, BToA = AToB> {
    endpoint_a: CrossbeamThreadPipeEndpointA<AToB, BToA>,
    endpoint_b: CrossbeamThreadPipeEndpointB<AToB, BToA>,
}

#[derive(Debug)]
/// Sends `AToB` values and receives `BToA` values.
pub struct CrossbeamThreadPipeEndpointA<AToB, BToA> {
    sender: Sender<AToB>,
    receiver: Receiver<BToA>,
}

#[derive(Debug)]
/// Sends `BToA` values and receives `AToB` values.
pub struct CrossbeamThreadPipeEndpointB<AToB, BToA> {
    sender: Sender<BToA>,
    receiver: Receiver<AToB>,
}

impl<AToB, BToA> CrossbeamThreadPipe<AToB, BToA> {
    /// Creates two endpoints backed by independent unbounded channels.
    pub fn new() -> Self {
        let (sender_a, receiver_b) = unbounded::<AToB>();
        let (sender_b, receiver_a) = unbounded::<BToA>();
        Self {
            endpoint_a: CrossbeamThreadPipeEndpointA {
                sender: sender_a,
                receiver: receiver_a,
            },
            endpoint_b: CrossbeamThreadPipeEndpointB {
                sender: sender_b,
                receiver: receiver_b,
            },
        }
    }

    /// Clones endpoint A without transferring pipe ownership.
    pub fn endpoint_a(&self) -> CrossbeamThreadPipeEndpointA<AToB, BToA> {
        self.endpoint_a.clone()
    }

    /// Clones endpoint B without transferring pipe ownership.
    pub fn endpoint_b(&self) -> CrossbeamThreadPipeEndpointB<AToB, BToA> {
        self.endpoint_b.clone()
    }

    /// Sends from A without blocking, returning the payload if B is disconnected.
    pub fn try_send_from_a(&self, data: AToB) -> Result<(), AToB> {
        self.endpoint_a.try_send(data)
    }

    /// Receives at A without blocking; `None` conflates empty and disconnected.
    pub fn try_receive_from_a(&self) -> Option<BToA> {
        self.endpoint_a.try_receive()
    }

    /// Sends from B without blocking, returning the payload if A is disconnected.
    pub fn try_send_from_b(&self, data: BToA) -> Result<(), BToA> {
        self.endpoint_b.try_send(data)
    }

    /// Receives at B without blocking; `None` conflates empty and disconnected.
    pub fn try_receive_from_b(&self) -> Option<AToB> {
        self.endpoint_b.try_receive()
    }

    /// Blocks the current OS thread until A receives a value or disconnects.
    pub fn receive_from_a(&self) -> Option<BToA> {
        self.endpoint_a.receive()
    }

    /// Blocks the current OS thread until B receives a value or disconnects.
    pub fn receive_from_b(&self) -> Option<AToB> {
        self.endpoint_b.receive()
    }
}

impl<AToB, BToA> Clone for CrossbeamThreadPipe<AToB, BToA> {
    fn clone(&self) -> Self {
        Self {
            endpoint_a: self.endpoint_a(),
            endpoint_b: self.endpoint_b(),
        }
    }
}

impl<AToB, BToA> Clone for CrossbeamThreadPipeEndpointA<AToB, BToA> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            receiver: self.receiver.clone(),
        }
    }
}

impl<AToB, BToA> Clone for CrossbeamThreadPipeEndpointB<AToB, BToA> {
    fn clone(&self) -> Self {
        Self {
            sender: self.sender.clone(),
            receiver: self.receiver.clone(),
        }
    }
}

// Endpoint methods convert channel disconnection into payload recovery or `None`.
impl<AToB, BToA> CrossbeamThreadPipeEndpointA<AToB, BToA> {
    /// Sends without blocking, returning the payload if all B receivers dropped.
    pub fn try_send(&self, data: AToB) -> Result<(), AToB> {
        self.sender
            .try_send(data)
            .map_err(|error| error.into_inner())
    }

    /// Receives without blocking; `None` means empty or disconnected.
    pub fn try_receive(&self) -> Option<BToA> {
        self.receiver.try_recv().ok()
    }

    /// Returns a momentary observation, not a synchronization guarantee.
    pub fn is_empty(&self) -> bool {
        self.receiver.is_empty()
    }

    /// Blocks the current OS thread until a value arrives or all senders drop.
    pub fn receive(&self) -> Option<BToA> {
        self.receiver.recv().ok()
    }
}

// Endpoint B mirrors A with the transport types reversed.
impl<AToB, BToA> CrossbeamThreadPipeEndpointB<AToB, BToA> {
    /// Sends without blocking, returning the payload if all A receivers dropped.
    pub fn try_send(&self, data: BToA) -> Result<(), BToA> {
        self.sender
            .try_send(data)
            .map_err(|error| error.into_inner())
    }

    /// Receives without blocking; `None` means empty or disconnected.
    pub fn try_receive(&self) -> Option<AToB> {
        self.receiver.try_recv().ok()
    }

    /// Returns a momentary observation, not a synchronization guarantee.
    pub fn is_empty(&self) -> bool {
        self.receiver.is_empty()
    }

    /// Blocks the current OS thread until a value arrives or all senders drop.
    pub fn receive(&self) -> Option<AToB> {
        self.receiver.recv().ok()
    }
}

impl<AToB, BToA> Default for CrossbeamThreadPipe<AToB, BToA> {
    fn default() -> Self {
        Self::new()
    }
}
