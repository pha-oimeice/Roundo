//! Session-scoped diagnostic request correlation. Queue admission is never a
//! mutation receipt. Accepted operations are not retried after loss or timeout.
use super::*;
use roundo_contracts::debug::*;
use serde::Serialize;
use std::time::Duration;
const CAPACITY: usize = 32;

#[derive(Clone, Debug, Serialize)]
pub struct DebugRequestStatus {
    pub request_id: String,
    pub result: Option<Result<DebugData, DebugFailure>>,
}
#[derive(Clone, Debug, Serialize)]
pub struct DebugStatusSnapshot {
    pub epoch: Option<String>,
    pub requests: Vec<DebugRequestStatus>,
}
#[derive(Default)]
pub(super) struct DebugRequests {
    next: u64,
    entries: VecDeque<(Instant, DebugRequestStatus)>,
}
impl DebugRequests {
    pub(super) fn clear(&mut self) {
        self.entries.clear();
    }
    pub(super) fn receive(&mut self, reply: DebugReply) {
        if let Some((_, status)) = self
            .entries
            .iter_mut()
            .find(|(_, entry)| entry.request_id == reply.request_id && entry.result.is_none())
        {
            status.result = Some(reply.result);
        }
    }
    fn expire(&mut self) {
        for (started, status) in &mut self.entries {
            if status.result.is_none() && started.elapsed() >= Duration::from_secs(10) {
                status.result = Some(Err(DebugFailure::new(
                    "outcome_unknown",
                    "诊断回执超时；操作可能已执行，请刷新核实，勿自动重试",
                )));
            }
        }
    }
}
impl ClientGameSessionLifecycle {
    /// Reserves correlation and enqueues once under the same epoch lock as
    /// begin/end/inbound. Full queues and inactive sessions fail immediately.
    pub fn send_debug(
        &self,
        network: &ClientNetwork,
        operation: DebugOperation,
    ) -> Result<String, String> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.active_epoch.is_none() {
            return Err("尚未连接到服务端".into());
        }
        state.debug.expire();
        if state.debug.entries.len() == CAPACITY {
            if let Some(index) = state
                .debug
                .entries
                .iter()
                .position(|(_, e)| e.result.is_some())
            {
                state.debug.entries.remove(index);
            } else {
                return Err("诊断在途请求容量已满".into());
            }
        }
        state.debug.next = state.debug.next.checked_add(1).ok_or("诊断请求编号耗尽")?;
        let request_id = state.debug.next.to_string();
        network
            .send(ClientGameMessage::Debug(DebugRequest {
                request_id: request_id.clone(),
                operation,
            }))
            .map_err(|error| error.to_string())?;
        state.debug.entries.push_back((
            Instant::now(),
            DebugRequestStatus {
                request_id: request_id.clone(),
                result: None,
            },
        ));
        Ok(request_id)
    }
    pub fn debug_status(&self) -> DebugStatusSnapshot {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        state.debug.expire();
        DebugStatusSnapshot {
            epoch: state.active_epoch.map(|e| e.to_string()),
            requests: state
                .debug
                .entries
                .iter()
                .map(|(_, entry)| entry.clone())
                .collect(),
        }
    }
}
