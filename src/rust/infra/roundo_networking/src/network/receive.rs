//! Encoded receive queues and decoding at the typed consumption interface.

use super::{Network, Peer, worker::WorkerEvent};
use crate::{
    DataTransferObject, NetworkError, NetworkTypeId, SharedEntityId, UpdateKind, wire::SharedBytes,
};
use bevy::prelude::{Component, Resource};
use serde::de::DeserializeOwned;
use std::collections::VecDeque;

/// A component update delivered without creating or accessing a local Entity.
#[derive(Debug)]
pub struct ComponentUpdate<T> {
    pub source: Peer,
    pub shared_entity_id: SharedEntityId,
    pub value: T,
}

#[derive(Debug)]
pub struct ResourceUpdate<T> {
    pub source: Peer,
    pub value: T,
}

/// Application-visible outcomes. Submission is not remote application commit.
#[derive(Debug)]
pub enum NetworkEvent {
    Submitted { peer: Peer, updates: usize },
    Error { peer: Peer, error: NetworkError },
}

pub(super) struct Received {
    peer: Peer,
    entity: Option<SharedEntityId>,
    data: SharedBytes,
}

impl Network {
    /// Pulls background results into World-owned queues of encoded updates.
    /// No type catalogue or decoder registration is needed. Payload decoding
    /// happens when a typed receive interface is called, not during this poll.
    /// Transport errors remain available through pop_event.
    pub fn poll(&mut self) {
        while let Ok(event) = self.incoming.try_recv() {
            match event {
                WorkerEvent::Submitted { peer, updates } => {
                    self.events
                        .push_back(NetworkEvent::Submitted { peer, updates });
                }
                WorkerEvent::Failed { peer, error } => {
                    self.events.push_back(NetworkEvent::Error { peer, error });
                }
                WorkerEvent::Received { peer, batch } => {
                    for update in batch.0 {
                        let key = (update.type_id, update.kind());
                        self.received.entry(key).or_default().push_back(Received {
                            peer: peer.clone(),
                            entity: update.entity,
                            data: update.data,
                        });
                    }
                }
            }
        }
    }

    /// Decodes one queued component as T. No prior registration is required.
    /// The returned value owns its data independently of the encoded buffer;
    /// malformed payloads return an error instead of being reported as absent.
    pub fn receive_component<T>(&mut self) -> Result<Option<ComponentUpdate<T>>, NetworkError>
    where
        T: Component + DataTransferObject + DeserializeOwned,
    {
        let Some(received) = self.take::<T>(UpdateKind::Component) else {
            return Ok(None);
        };
        let entity = received.entity.ok_or(NetworkError::TypeMismatch)?;
        let value = decode::<T>(received.data.0.as_slice())?;
        Ok(Some(ComponentUpdate {
            source: received.peer,
            shared_entity_id: entity,
            value,
        }))
    }

    /// Decodes one queued resource as T. Payload errors are returned here,
    /// while poll handles only envelope routing and background outcomes.
    pub fn receive_resource<T>(&mut self) -> Result<Option<ResourceUpdate<T>>, NetworkError>
    where
        T: Resource + DataTransferObject + DeserializeOwned,
    {
        let Some(received) = self.take::<T>(UpdateKind::Resource) else {
            return Ok(None);
        };
        let value = decode::<T>(received.data.0.as_slice())?;
        Ok(Some(ResourceUpdate {
            source: received.peer,
            value,
        }))
    }

    fn take<T: 'static>(&mut self, kind: UpdateKind) -> Option<Received> {
        let id = NetworkTypeId::of::<T>();
        self.received
            .get_mut(&(id, kind))
            .and_then(VecDeque::pop_front)
    }

    pub fn pop_event(&mut self) -> Option<NetworkEvent> {
        self.events.pop_front()
    }
}

fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, NetworkError> {
    let (value, rest) = postcard::take_from_bytes::<T>(bytes)?;
    if !rest.is_empty() {
        return Err(NetworkError::TrailingBytes);
    }
    Ok(value)
}
