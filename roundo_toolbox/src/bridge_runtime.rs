//! Shared lifetime and idle policy for blocking thread bridge adapters.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::JoinHandle;
use std::time::Duration;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BridgeStep {
    Idle,
    Forwarded,
    Stop,
}

/// Runs one non-blocking adapter until it stops itself or its owner shuts down.
pub fn run_polling_bridge(stop: &AtomicBool, mut forward_one: impl FnMut() -> BridgeStep) {
    while !stop.load(Ordering::Acquire) {
        match forward_one() {
            BridgeStep::Idle => std::thread::sleep(Duration::from_millis(1)),
            BridgeStep::Forwarded => {}
            BridgeStep::Stop => break,
        }
    }
}

/// Owns bridge worker threads and guarantees deterministic stop-before-join.
#[derive(Default)]
pub struct BridgeThreadGroup {
    stop: Arc<AtomicBool>,
    workers: Vec<JoinHandle<()>>,
}

impl BridgeThreadGroup {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn spawn(
        &mut self,
        name: impl Into<String>,
        worker: impl FnOnce(Arc<AtomicBool>) + Send + 'static,
    ) -> std::io::Result<()> {
        let stop = Arc::clone(&self.stop);
        let handle = std::thread::Builder::new()
            .name(name.into())
            .spawn(move || worker(stop))?;
        self.workers.push(handle);
        Ok(())
    }

    pub fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Release);
        for worker in self.workers.drain(..) {
            if let Err(error) = worker.join() {
                log::error!("bridge worker panicked during shutdown: {error:?}");
            }
        }
    }

    pub fn is_empty(&self) -> bool {
        self.workers.is_empty()
    }
}

impl Drop for BridgeThreadGroup {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;

    #[test]
    fn thread_group_stops_and_joins_workers() {
        let (entered_sender, entered_receiver) = mpsc::sync_channel(1);
        let mut group = BridgeThreadGroup::new();
        group
            .spawn("bridge-test", move |stop| {
                let mut entered_sender = Some(entered_sender);
                run_polling_bridge(&stop, || {
                    if let Some(sender) = entered_sender.take() {
                        sender.send(()).unwrap();
                    }
                    BridgeStep::Idle
                });
            })
            .unwrap();
        entered_receiver
            .recv_timeout(Duration::from_secs(1))
            .unwrap();

        group.shutdown();

        assert!(group.is_empty());
    }

    #[test]
    fn adapter_can_stop_itself() {
        let stop = AtomicBool::new(false);
        let mut calls = 0;
        run_polling_bridge(&stop, || {
            calls += 1;
            BridgeStep::Stop
        });
        assert_eq!(calls, 1);
    }
}
