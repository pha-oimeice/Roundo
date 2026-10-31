//! Bounded command transport, Client Data subscriptions and injected bridge scripts.

use super::*;

#[cfg(target_os = "windows")]
pub(super) fn dispatch_webui_message(
    io: &Option<ContextualJsonRequestResponseIo<UiCommandSource, Value>>,
    pending: &Arc<Mutex<Vec<PendingCommand>>>,
    subscriptions: &Arc<Mutex<BTreeSet<String>>>,
    subscription_generation: &Arc<std::sync::atomic::AtomicU64>,
    host_messages: &DocumentMessageQueue,
    document: Option<crate::state::UiDocumentProof>,
    source: UiInstanceId,
    body: &str,
) -> Option<(u64, Value)> {
    if let Ok(value) = serde_json::from_str::<Value>(body)
        && let Some(payload) = value.get("roundo_host_message")
    {
        if body.len() <= 4096
            && let Some(document) = document
        {
            let mut messages = host_messages.lock().unwrap_or_else(|p| p.into_inner());
            if messages.len() < crate::registry::MAX_PREPARED_COMMANDS {
                messages.push((document, payload.clone()));
            } else {
                log::warn!("Web UI host-message queue is full");
            }
        }
        return None;
    }
    let subscription = serde_json::from_str::<Value>(body).ok().and_then(|value| {
        let subscription = value.get("roundo_subscription");
        subscription.cloned()
    });
    if let Some(subscription) = subscription {
        let version = subscription.get("version").and_then(Value::as_u64);
        let resource = subscription.get("resource").and_then(Value::as_str);
        let subscribed = subscription.get("subscribed").and_then(Value::as_bool);
        let Some((resource, subscribed)) = resource.zip(subscribed) else {
            log::warn!("Web UI Client Data subscription was rejected: invalid envelope");
            return None;
        };
        if version != Some(1)
            || resource.is_empty()
            || resource.len() > 128
            || !resource
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
        {
            log::warn!(
                "Web UI Client Data subscription was rejected: invalid version or resource `{resource}`"
            );
            return None;
        }
        let changed = {
            let mut subscriptions = subscriptions
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            if subscribed {
                if subscriptions.contains(resource) {
                    false
                } else if subscriptions.len() >= crate::registry::MAX_PREPARED_COMMANDS {
                    log::warn!(
                        "Web UI Client Data subscription was rejected: subscription limit reached for `{resource}`"
                    );
                    false
                } else {
                    subscriptions.insert(resource.to_owned())
                }
            } else {
                subscriptions.remove(resource)
            }
        };
        if changed {
            subscription_generation.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
            log::debug!(
                "Web UI instance {} {} Client Data resource `{resource}`",
                source.value(),
                if subscribed {
                    "subscribed to"
                } else {
                    "unsubscribed from"
                }
            );
        }
        return None;
    }
    enqueue_webui_command(io, pending, Some(source), body)
}

pub(crate) fn enqueue_webui_command(
    io: &Option<ContextualJsonRequestResponseIo<UiCommandSource, Value>>,
    pending: &Arc<Mutex<Vec<PendingCommand>>>,
    source: Option<UiInstanceId>,
    body: &str,
) -> Option<(u64, Value)> {
    let transport: Value = match serde_json::from_str(body) {
        Ok(value) => value,
        Err(error) => {
            log::warn!("invalid Web UI IPC: {error}");
            return None;
        }
    };
    let request_id = transport.get("request_id").and_then(Value::as_u64);
    let Some(request_id) = request_id else {
        log::warn!("Web UI IPC was rejected: request_id is missing or not an unsigned integer");
        return None;
    };
    // Admission, submission and append are one transaction. Immediate failures
    // consume the same bounded observer slots as accepted business commands.
    let mut pending = pending
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if pending.len() >= crate::registry::MAX_PREPARED_COMMANDS {
        let command_name = transport
            .get("command")
            .and_then(|command| command.get("command"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        return Some((
            request_id,
            json!({
                "version": 1,
                "command": command_name,
                "ok": false,
                "error": {"code": "command_queue_full", "message": "command queue is full"},
            }),
        ));
    }
    let command = transport.get("command").cloned();
    let Some(command) = command else {
        log::warn!("Web UI IPC request {request_id} was rejected: command is missing");
        pending.push(PendingCommand {
            request_id,
            command_name: String::new(),
            submitted_at: Instant::now(),
            timeout_logged: false,
            call: None,
            immediate: Some(json!({
                "version": 1,
                "command": "",
                "ok": false,
                "error": {
                    "code": "invalid_command_envelope",
                    "message": "transport command is required",
                },
            })),
        });
        return None;
    };
    let command_name = command.get("command");
    let command_name = command_name
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_owned();
    if command_name == "ui.open" {
        log::debug!(
            "Web UI IPC request {request_id}: enqueueing ui.open with arguments {:?}",
            {
                let arguments = command.get("arguments");
                arguments
            }
        );
    }
    let immediate_error = |code: &str, message: &str| json!({"version":1,"command":command_name,"ok":false,"error":{"code":code,"message":message}});
    let submitted_at = Instant::now();
    let item = match io {
        Some(io) => match io.submit(
            command,
            source.map_or(UiCommandSource::Host, UiCommandSource::WebView),
        ) {
            Ok(call) => PendingCommand {
                request_id,
                command_name: command_name.clone(),
                submitted_at,
                timeout_logged: false,
                call: Some(call),
                immediate: None,
            },
            Err(JsonSubmitError::Full) => PendingCommand {
                request_id,
                command_name: command_name.clone(),
                submitted_at,
                timeout_logged: false,
                call: None,
                immediate: Some(immediate_error(
                    "command_queue_full",
                    "command queue is full",
                )),
            },
            Err(JsonSubmitError::Disconnected) => PendingCommand {
                request_id,
                command_name: command_name.clone(),
                submitted_at,
                timeout_logged: false,
                call: None,
                immediate: Some(immediate_error(
                    "internal_command_error",
                    "command endpoint disconnected",
                )),
            },
            Err(JsonSubmitError::InputTooLarge { command }) => PendingCommand {
                request_id,
                command_name: command_name.clone(),
                submitted_at,
                timeout_logged: false,
                call: None,
                immediate: Some(json!({
                    "version": 1,
                    "command": command,
                    "ok": false,
                    "error": {
                        "code": "command_input_too_large",
                        "message": "command input exceeds 64 KiB",
                    },
                })),
            },
        },
        None => PendingCommand {
            request_id,
            command_name: command_name.clone(),
            submitted_at,
            timeout_logged: false,
            call: None,
            immediate: Some(immediate_error(
                "internal_command_error",
                "command endpoint is unavailable",
            )),
        },
    };
    log::trace!("Web UI IPC request {request_id}: transport submission completed");
    pending.push(item);
    log::trace!("Web UI IPC request {request_id}: queued for response polling");
    None
}

#[cfg(target_os = "windows")]
pub(crate) fn resolve_pending_commands(
    pending: &Arc<Mutex<Vec<PendingCommand>>>,
    mut resolve: impl FnMut(u64, &str, Value),
) -> bool {
    let resolved = {
        let mut pending = pending
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut resolved = Vec::new();
        let mut index = 0;
        while index < pending.len() {
            let result = match pending[index].immediate.take() {
                Some(result) => Some(result),
                None => match pending[index].call.as_ref().map(RequestCall::poll) {
                    Some(ResponsePoll::Ready(result)) => Some(result),
                    Some(ResponsePoll::Disconnected) => Some(json!({
                        "version": 1,
                        "command": pending[index].command_name,
                        "ok": false,
                        "error": {
                            "code": "internal_command_error",
                            "message": "command response channel disconnected",
                        },
                    })),
                    Some(ResponsePoll::Pending) | None => None,
                },
            };
            let Some(result) = result else {
                if !pending[index].timeout_logged
                    && pending[index].submitted_at.elapsed() >= Duration::from_secs(2)
                {
                    log::warn!(
                        "Web UI IPC request {} ({}) has waited over two seconds for a client-command response",
                        pending[index].request_id,
                        pending[index].command_name
                    );
                    pending[index].timeout_logged = true;
                }
                index += 1;
                continue;
            };
            let command = pending.swap_remove(index);
            resolved.push((command.request_id, command.command_name, result));
        }
        resolved
    };
    let resolved_any = !resolved.is_empty();
    for (request_id, command_name, result) in resolved {
        // WebView2 script evaluation is an external, reentrant boundary. A
        // Promise continuation may synchronously post the next IPC message,
        // which needs the same pending-command mutex.
        resolve(request_id, &command_name, result);
    }
    resolved_any
}

#[cfg(target_os = "windows")]
pub(crate) fn resolve_webui_commands(
    mut executor: Option<bevy::ecs::system::NonSendMut<UiNavigationExecutor>>,
) {
    let Some(executor) = executor.as_deref_mut() else {
        return;
    };
    for overlay in executor.native.live_views(&executor.lifetime) {
        let _ = resolve_pending_commands(&overlay.pending, |request_id, command_name, result| {
            let succeeded = result.get("ok").and_then(Value::as_bool);
            if succeeded == Some(false) {
                log::warn!("Web UI IPC request {request_id} ({command_name}) failed: {result}");
            } else if command_name == "ui.open" {
                log::debug!("Web UI IPC request {request_id}: ui.open completed with {result}");
            }
            let script = response_script(request_id, &result);
            if let Err(error) =
                evaluate_script_transaction(&overlay.webview, &overlay.scripts, &script)
            {
                log::error!("cannot resolve Web UI command: {error}");
            }
        });
    }
}

pub(crate) fn bridge_focus_request(body: &str) -> bool {
    serde_json::from_str::<Value>(body)
        .ok()
        .is_some_and(|value| {
            let request = value.get("roundo_focus_request");
            request.and_then(Value::as_bool) == Some(true)
        })
}

pub(crate) fn bridge_handshake(body: &str) -> bool {
    serde_json::from_str::<Value>(body)
        .ok()
        .is_some_and(|value| {
            let readiness = value.get("roundo_bridge_ready");
            readiness.and_then(Value::as_bool) == Some(true)
        })
}

pub(crate) fn response_script(request_id: u64, result: &Value) -> String {
    format!(
        "window.__roundoResolve({}, {});",
        request_id,
        serde_json::to_string(result).expect("JSON result is serializable")
    )
}

pub(crate) fn data_sync_script(resource: &str, data: &Value) -> String {
    format!(
        "window.__roundoSync({}, {});",
        serde_json::to_string(resource).expect("Client Data resource name serializes"),
        serde_json::to_string(data).expect("Client Data snapshot serializes")
    )
}

#[cfg(target_os = "windows")]
const WEBUI_BRIDGE_SCRIPT: &str = r#"(() => {
  const MAX_IN_FLIGHT = 64;
  const MAX_ACTIVATION_QUEUE = 64;
  const MAX_SUBSCRIPTIONS = 64;
  let next = 1;
  let active = false;
  const pending = new Map();
  const queued = [];
  const subscriptions = new Map();
  const snapshots = new Map();
  const commandName = command => typeof command?.command === 'string' ? command.command : '';
  const commandError = (command, code, message) => ({version:1,command:commandName(command),ok:false,error:{code,message}});
  const post = body => {
    if (active) {
      window.ipc.postMessage(body);
      return true;
    }
    if (queued.length >= MAX_ACTIVATION_QUEUE) return false;
    queued.push(body);
    return true;
  };
  const subscriptionBody = (resource, subscribed) => JSON.stringify({roundo_subscription:{version:1,resource,subscribed}});
  const postSubscription = (resource, subscribed) => {
    if (!active && !subscribed) {
      const prior = subscriptionBody(resource, true);
      const index = queued.indexOf(prior);
      if (index !== -1) {
        queued.splice(index, 1);
        return true;
      }
    }
    return post(subscriptionBody(resource, subscribed));
  };
  window.__roundoResolve = (id, value) => {
    const resolve = pending.get(id);
    if (resolve) {
      pending.delete(id);
      resolve(value);
    }
  };
  const failQueuedCommand = (body, error) => {
    try {
      const envelope = JSON.parse(body);
      if (Number.isSafeInteger(envelope.request_id)) {
        window.__roundoResolve(envelope.request_id, commandError(envelope.command, 'internal_command_error', String(error)));
      }
    } catch (_) {}
  };
  window.__roundoActivate = () => {
    if (active) return;
    active = true;
    for (const body of queued.splice(0)) {
      try { window.ipc.postMessage(body); }
      catch (error) { failQueuedCommand(body, error); }
    }
  };
  window.__roundoEvent = (resource, data) => {
    const listeners = subscriptions.get(resource);
    if (!listeners) return;
    for (const listener of [...listeners]) {
      try { listener(data); }
      catch (error) { console.error('Roundo subscriber failed', resource, error); }
    }
  };
  window.__roundoSync = (resource, data) => {
    snapshots.set(resource, data);
    window.__roundoEvent(resource, data);
  };
  const allocateRequestId = () => {
    for (let attempt = 0; attempt < MAX_IN_FLIGHT; attempt++) {
      const id = next;
      next = next >= Number.MAX_SAFE_INTEGER ? 1 : next + 1;
      if (!pending.has(id)) return id;
    }
    throw new Error('Roundo request ID space is exhausted');
  };
  window.roundo = {
    execute(command) {
      if (pending.size >= MAX_IN_FLIGHT) {
        return Promise.resolve(commandError(command, 'command_queue_full', 'Web UI in-flight command limit reached'));
      }
      return new Promise((resolve, reject) => {
        const id = allocateRequestId();
        pending.set(id, resolve);
        let body;
        try { body = JSON.stringify({request_id:id,command}); }
        catch (error) { pending.delete(id); reject(error); return; }
        try {
          if (!post(body)) {
            pending.delete(id);
            resolve(commandError(command, 'command_queue_full', 'Web UI activation queue is full'));
          }
        } catch (error) {
          pending.delete(id);
          reject(error);
        }
      });
    },
    subscribe(resource, listener) {
      if (typeof resource !== 'string' || typeof listener !== 'function') {
        throw new TypeError('roundo.subscribe requires a resource name and listener');
      }
      let listeners = subscriptions.get(resource);
      if (!listeners) {
        if (subscriptions.size >= MAX_SUBSCRIPTIONS) throw new RangeError('Roundo subscription limit reached');
        listeners = new Set();
        subscriptions.set(resource, listeners);
      }
      const first = listeners.size === 0;
      listeners.add(listener);
      if (first) {
        let accepted = false;
        try { accepted = postSubscription(resource, true); }
        catch (error) {
          listeners.delete(listener);
          subscriptions.delete(resource);
          throw error;
        }
        if (!accepted) {
          listeners.delete(listener);
          subscriptions.delete(resource);
          throw new Error('Roundo activation queue is full');
        }
      } else if (snapshots.has(resource)) {
        try { listener(snapshots.get(resource)); }
        catch (error) { console.error('Roundo Client Data subscriber failed', resource, error); }
      }
      let live = true;
      return () => {
        if (!live) return;
        live = false;
        listeners.delete(listener);
        if (!listeners.size) {
          subscriptions.delete(resource);
          snapshots.delete(resource);
          try {
            if (!postSubscription(resource, false)) {
              console.error('Roundo Client Data unsubscribe queue is full', resource);
            }
          } catch (error) { console.error('Roundo Client Data unsubscribe failed', resource, error); }
        }
      };
    }
  };
  window.ipc.postMessage(JSON.stringify({roundo_bridge_ready:true}));
  Object.defineProperty(window, '__roundoBridgeHandshakeSent', {value: true, writable: false, configurable: false});
})();"#;

pub(crate) fn webui_initialization_script(definition: &str) -> String {
    let prefix = serde_json::to_string(&format!("/{definition}/"))
        .expect("Definition path prefix serializes");
    format!(
        "{WEBUI_BRIDGE_SCRIPT}\n(() => {{ const prefix={prefix}; const allowed=(value)=>{{ try {{ const url=new URL(value,location.href); return url.href==='about:blank'||(url.origin===location.origin&&url.pathname.startsWith(prefix)); }} catch (_) {{ return false; }} }}; const check=(frame)=>{{ const value=frame.getAttribute('src'); if(value&&!allowed(value)){{ console.error('Roundo blocked cross-Definition iframe',value); frame.removeAttribute('src'); }} }}; new MutationObserver(records=>{{ for(const record of records){{ if(record.target instanceof HTMLIFrameElement)check(record.target); for(const node of record.addedNodes){{ if(node instanceof HTMLIFrameElement)check(node); if(node.querySelectorAll)node.querySelectorAll('iframe').forEach(check); }} }} }}).observe(document,{{subtree:true,childList:true,attributes:true,attributeFilter:['src']}}); window.addEventListener('DOMContentLoaded',()=>document.querySelectorAll('iframe').forEach(check),{{once:true}}); }})();"
    )
}
