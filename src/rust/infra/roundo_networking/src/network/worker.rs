//! Asynchronous transport workers and their World-facing result messages.

use super::Peer;
use crate::{NetworkError, SyncConnection, wire::SyncBatch};
use crossbeam::channel::Sender;
use tokio::{runtime::Handle, sync::mpsc, task::JoinHandle};

pub(super) enum WorkerEvent {
    Received { peer: Peer, batch: SyncBatch },
    Submitted { peer: Peer, updates: usize },
    Failed { peer: Peer, error: NetworkError },
}

pub(super) fn spawn_sender(
    runtime: &Handle,
    connection: SyncConnection,
    peer: Peer,
    events: Sender<WorkerEvent>,
    mut batches: mpsc::UnboundedReceiver<Vec<SyncBatch>>,
) -> JoinHandle<()> {
    runtime.spawn(async move {
        while let Some(batches) = batches.recv().await {
            for batch in batches {
                let updates = batch.0.len();
                match connection.send(&batch).await {
                    Ok(()) => {
                        let _ = events.send(WorkerEvent::Submitted {
                            peer: peer.clone(),
                            updates,
                        });
                    }
                    Err(error) => {
                        let _ = events.send(WorkerEvent::Failed {
                            peer,
                            error: error.into(),
                        });
                        return;
                    }
                }
            }
        }
    })
}

pub(super) fn spawn_receiver(
    runtime: &Handle,
    connection: SyncConnection,
    peer: Peer,
    events: Sender<WorkerEvent>,
) -> JoinHandle<()> {
    runtime.spawn(async move {
        loop {
            match connection.receive().await {
                Ok(batch) => {
                    if events
                        .send(WorkerEvent::Received {
                            peer: peer.clone(),
                            batch,
                        })
                        .is_err()
                    {
                        return;
                    }
                }
                Err(error) => {
                    let _ = events.send(WorkerEvent::Failed {
                        peer,
                        error: error.into(),
                    });
                    return;
                }
            }
        }
    })
}
