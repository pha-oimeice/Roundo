use crossbeam::channel::{Receiver, Sender, unbounded};

#[derive(Debug)]
pub struct CrossbeamThreadPipe<AToB, BToA = AToB> {
    endpoint_a: CrossbeamThreadPipeEndpointA<AToB, BToA>,
    endpoint_b: CrossbeamThreadPipeEndpointB<AToB, BToA>,
}

#[derive(Debug)]
pub struct CrossbeamThreadPipeEndpointA<AToB, BToA> {
    sender: Sender<AToB>,
    receiver: Receiver<BToA>,
}

#[derive(Debug)]
pub struct CrossbeamThreadPipeEndpointB<AToB, BToA> {
    sender: Sender<BToA>,
    receiver: Receiver<AToB>,
}

impl<AToB, BToA> CrossbeamThreadPipe<AToB, BToA> {
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

    pub fn endpoint_a(&self) -> CrossbeamThreadPipeEndpointA<AToB, BToA> {
        self.endpoint_a.clone()
    }

    pub fn endpoint_b(&self) -> CrossbeamThreadPipeEndpointB<AToB, BToA> {
        self.endpoint_b.clone()
    }

    pub fn try_send_from_a(&self, data: AToB) -> Result<(), AToB> {
        self.endpoint_a.try_send(data)
    }

    pub fn try_receive_from_a(&self) -> Option<BToA> {
        self.endpoint_a.try_receive()
    }

    pub fn try_send_from_b(&self, data: BToA) -> Result<(), BToA> {
        self.endpoint_b.try_send(data)
    }

    pub fn try_receive_from_b(&self) -> Option<AToB> {
        self.endpoint_b.try_receive()
    }

    pub fn send_from_a(&self, data: AToB) {
        self.endpoint_a.send(data);
    }

    pub fn receive_from_a(&self) -> Option<BToA> {
        self.endpoint_a.receive()
    }

    pub fn send_from_b(&self, data: BToA) {
        self.endpoint_b.send(data);
    }

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

impl<AToB, BToA> CrossbeamThreadPipeEndpointA<AToB, BToA> {
    pub fn try_send(&self, data: AToB) -> Result<(), AToB> {
        self.sender
            .try_send(data)
            .map_err(|error| error.into_inner())
    }

    pub fn try_receive(&self) -> Option<BToA> {
        self.receiver.try_recv().ok()
    }

    pub fn is_empty(&self) -> bool {
        self.receiver.is_empty()
    }

    pub fn send(&self, data: AToB) {
        self.sender
            .send(data)
            .expect("crossbeam thread pipe endpoint B disconnected");
    }

    pub fn receive(&self) -> Option<BToA> {
        self.receiver.recv().ok()
    }
}

impl<AToB, BToA> CrossbeamThreadPipeEndpointB<AToB, BToA> {
    pub fn try_send(&self, data: BToA) -> Result<(), BToA> {
        self.sender
            .try_send(data)
            .map_err(|error| error.into_inner())
    }

    pub fn try_receive(&self) -> Option<AToB> {
        self.receiver.try_recv().ok()
    }

    pub fn is_empty(&self) -> bool {
        self.receiver.is_empty()
    }

    pub fn send(&self, data: BToA) {
        self.sender
            .send(data)
            .expect("crossbeam thread pipe endpoint A disconnected");
    }

    pub fn receive(&self) -> Option<AToB> {
        self.receiver.recv().ok()
    }
}

impl<AToB, BToA> Default for CrossbeamThreadPipe<AToB, BToA> {
    fn default() -> Self {
        Self::new()
    }
}
