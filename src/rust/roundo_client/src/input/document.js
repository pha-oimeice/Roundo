// Trusted device adapter owned exclusively by the client input module.
// Mod documents receive semantic events, never implement physical-key listeners.
(() => {
  if (window !== window.top) return;
  let bindings = [];
  const heldKeys = new Set();
  const heldButtons = new Set();
  const code = key => key.startsWith('mouse_') ? null : key.split('_').map(part => part[0].toUpperCase() + part.slice(1)).join('');
  const buttons = {mouse_left: 0, mouse_middle: 1, mouse_right: 2};
  roundo.subscribe('client.bindings', snapshot => { bindings = snapshot.bindings || []; });
  const editing = target => target?.isContentEditable || target?.closest?.('input,textarea,select,[contenteditable="true"]');
  const send = payload => {
    try { window.ipc.postMessage(JSON.stringify({roundo_host_message: payload})); }
    catch (error) { console.error('Roundo input forwarding failed', error); }
  };
  for (const type of ['keydown', 'keyup']) {
    window.addEventListener(type, event => {
      if (!event.isTrusted) return;
      const escape = event.code === 'Escape';
      if (type === 'keyup') {
        // Release a key admitted by this document even when focus moved to an editor.
        if (!heldKeys.delete(event.code)) return;
      } else {
        if (event.repeat || event.isComposing || !document.hasFocus()) return;
        if (!escape && editing(event.target)) return;
        if (!escape && !bindings.some(binding => code(binding.key) === event.code)) return;
        heldKeys.add(event.code);
      }
      // Ordinary device forwarding must not disable native UI controls (for
      // example keyboard activation of a focused button). Only Esc is reserved.
      if (escape) {
        event.preventDefault();
        event.stopImmediatePropagation();
      }
      send({kind: 'key', code: event.code, pressed: type === 'keydown'});
    }, true);
  }
  for (const type of ['mousedown', 'mouseup']) {
    window.addEventListener(type, event => {
      if (!event.isTrusted) return;
      if (type === 'mouseup') {
        if (!heldButtons.delete(event.button)) return;
      } else {
        if (!document.hasFocus() || editing(event.target)) return;
        if (!bindings.some(binding => buttons[binding.key] === event.button)) return;
        heldButtons.add(event.button);
      }
      send({kind: 'button', button: event.button, pressed: type === 'mousedown'});
    }, true);
  }
  let motion = null;
  function releaseHeld() {
    const keys = [...heldKeys], buttons = [...heldButtons];
    heldKeys.clear();
    heldButtons.clear();
    if (motion) { motion.x = 0; motion.y = 0; }
    for (const code of keys) send({kind: 'key', code, pressed: false});
    for (const button of buttons) send({kind: 'button', button, pressed: false});
  }
  window.addEventListener('blur', releaseHeld);
  window.addEventListener('pagehide', releaseHeld);
  document.addEventListener('visibilitychange', () => { if (document.hidden) releaseHeld(); });
  window.addEventListener('roundo:focus', event => { if (event.detail?.state === 'blurred') releaseHeld(); });
  window.addEventListener('roundo:visibility', event => { if (event.detail?.state === 'hidden') releaseHeld(); });
  window.addEventListener('roundo:destroying', releaseHeld, {once:true});
  window.addEventListener('mousemove', event => {
    if (!event.isTrusted || !document.hasFocus() || editing(event.target)
        || !bindings.some(binding => binding.key === 'mouse_motion')) return;
    if (!motion) {
      motion = {x: 0, y: 0};
      requestAnimationFrame(() => {
        const delta = motion;
        motion = null;
        if (document.hasFocus() && (delta.x || delta.y)) send({kind: 'motion', ...delta});
      });
    }
    motion.x += event.movementX;
    motion.y += event.movementY;
  }, true);
  window.addEventListener('pointerdown', event => {
    if (!event.isTrusted) return;
    try { window.ipc.postMessage(JSON.stringify({roundo_focus_request:true})); }
    catch (error) { console.error('Roundo focus request failed', error); }
  }, true);
})();
