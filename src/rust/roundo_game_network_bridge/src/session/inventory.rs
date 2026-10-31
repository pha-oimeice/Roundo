//! A single bounded, session-scoped inventory observation. Only the client host
//! refreshes it; pages subscribe to snapshots, never poll network commands.
use super::*;
use roundo_contracts::{ControllerBindingId, inventory::*};
use serde::Serialize;
use std::time::Duration;

const REFRESH: Duration = Duration::from_millis(500);
const TIMEOUT: Duration = Duration::from_secs(3);

#[derive(Clone, Serialize)]
pub struct InventoryView {
    pub status: &'static str,
    pub controller_id: Option<String>,
    pub bodies: Vec<BodyInventories>,
}
impl InventoryView {
    fn empty(status: &'static str) -> Self {
        Self {
            status,
            controller_id: None,
            bodies: Vec::new(),
        }
    }
}

#[derive(Default)]
pub(super) struct InventoryObservation {
    next: u64,
    subject: Option<(u64, ControllerId, ControllerBindingId)>,
    pending: Option<(InventoryRequest, Instant)>,
    last_send: Option<Instant>,
    received: Option<Instant>,
    view: Option<InventoryView>,
}
impl InventoryObservation {
    pub(super) fn clear(&mut self) {
        // Correlation is never reused, including across reconnects.
        *self = Self {
            next: self.next,
            ..Self::default()
        };
    }
    pub(super) fn receive(&mut self, reply: InventoryReply) {
        let Some((request, sent)) = self.pending else {
            return;
        };
        if request != reply.request || sent.elapsed() >= TIMEOUT {
            return;
        }
        self.pending = None;
        let (status, bodies) = match reply.result {
            Ok(bodies) => ("ready", bodies),
            Err(InventoryReadError::NotBound) => ("not_bound", Vec::new()),
            Err(InventoryReadError::Unavailable) => ("unavailable", Vec::new()),
            Err(InventoryReadError::TooLarge) => ("too_large", Vec::new()),
        };
        self.received = Some(Instant::now());
        self.view = Some(InventoryView {
            status,
            controller_id: Some(request.controller_id.0.to_string()),
            bodies,
        });
    }
}

impl ClientGameSessionLifecycle {
    /// Local binding observation only selects the request subject. The server
    /// independently revalidates the exact active binding before reading custody.
    /// `None` drops all cached/private data, including outstanding correlation.
    pub fn inventory_view(
        &self,
        network: Option<&ClientNetwork>,
        subject: Option<(u64, ControllerId, ControllerBindingId)>,
    ) -> InventoryView {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        let subject = subject.filter(|(epoch, controller, _)| {
            state.active_epoch == Some(*epoch)
                && state.activated_epoch == Some(*epoch)
                && !state.blocked_controllers.contains(controller)
        });
        let (Some(network), Some(subject)) = (network, subject) else {
            state.inventory.clear();
            return InventoryView::empty("not_bound");
        };
        let observation = &mut state.inventory;
        if observation.subject != Some(subject) {
            observation.clear();
            observation.subject = Some(subject);
        }
        if observation
            .pending
            .is_some_and(|(_, sent)| sent.elapsed() >= TIMEOUT)
        {
            observation.pending = None;
            observation.view = Some(InventoryView::empty("timeout"));
        }
        if observation
            .received
            .is_some_and(|received| received.elapsed() >= TIMEOUT)
        {
            observation.received = None;
            observation.view = Some(InventoryView::empty("timeout"));
        }
        if observation.pending.is_none()
            && observation
                .last_send
                .is_none_or(|sent| sent.elapsed() >= REFRESH)
        {
            let Some(next) = observation.next.checked_add(1) else {
                return InventoryView::empty("unavailable");
            };
            observation.next = next;
            let request = InventoryRequest {
                request_id: next,
                controller_id: subject.1,
                binding: subject.2,
            };
            let now = Instant::now();
            observation.last_send = Some(now);
            if network.send(ClientGameMessage::Inventory(request)).is_ok() {
                observation.pending = Some((request, now));
            } else {
                observation.view = Some(InventoryView::empty("unavailable"));
            }
        }
        observation
            .view
            .clone()
            .unwrap_or_else(|| InventoryView::empty("loading"))
    }
}
