//! Shared lifetime and idle policy for blocking thread bridge adapters.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::thread::JoinHandle;
use std::time::Duration;

/// Result of one cooperative bridge polling step.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BridgeStep {
    /// No work was available; the loop sleeps for one millisecond.
    Idle,
    /// One item was forwarded; the next poll starts immediately.
    Forwarded,
    /// The adapter requests loop termination.
    Stop,
}

/// Runs a cooperative polling adapter on the current thread.
///
/// The loop checks `stop` before each callback. [`BridgeStep::Idle`] sleeps the
/// current OS thread for one millisecond; [`BridgeStep::Forwarded`] can therefore
/// spin if the callback reports progress without doing bounded work. Callback
/// panics are not caught.
pub fn run_polling_bridge(stop: &AtomicBool, mut forward_one: impl FnMut() -> BridgeStep) {
    while !stop.load(Ordering::Acquire) {
        match forward_one() {
            BridgeStep::Idle => std::thread::sleep(Duration::from_millis(1)),
            BridgeStep::Forwarded => {}
            BridgeStep::Stop => break,
        }
    }
}

/// Owns bridge worker threads and signals cooperative stop before joining them.
///
/// Workers must observe the supplied flag or otherwise terminate themselves;
/// shutdown has no forced-cancellation mechanism and may block indefinitely.
#[derive(Default)]
pub struct BridgeThreadGroup {
    stop: Arc<AtomicBool>,
    workers: Vec<JoinHandle<()>>,
}

impl BridgeThreadGroup {
    /// Creates an empty group with its stop flag cleared.
    pub fn new() -> Self {
        Self::default()
    }

    /// Spawns and retains one named worker.
    ///
    /// The worker receives the group's shared stop flag. If shutdown was already
    /// requested, that flag is initially `true`; this method does not reset it.
    ///
    /// # Errors
    ///
    /// Returns the OS thread-spawn error without adding a worker to the group.
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

    /// Signals stop and joins every retained worker on the current OS thread.
    ///
    /// The operation is idempotent after all handles are drained. Worker panics
    /// are logged and do not stop remaining joins.
    pub fn shutdown(&mut self) {
        self.stop.store(true, Ordering::Release);
        for worker in self.workers.drain(..) {
            if let Err(error) = worker.join() {
                log::error!("bridge worker panicked during shutdown: {error:?}");
            }
        }
    }

    /// Returns whether the group retains no join handles.
    ///
    /// A worker that has exited but has not yet been joined still counts as retained.
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
                        let notification = sender.send(());
                        notification.unwrap();
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
