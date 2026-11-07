'use strict';

// All debug pages use ordinary UI imports and the host's navigation lifetime.
(() => {
  const $ = id => document.getElementById(id);
  const command = (name, args = {}) => roundo.execute({version:1, command:name, arguments:args});
  const unsubs = [];
  let navigating = false;
  function message(text) {
    $('message').textContent = text;
    $('message').classList.add('error');
  }
  async function navigate(name, args) {
    if (navigating) return;
    navigating = true;
    try {
      const result = await command(name, args);
      if (!result.ok) message(`[${result.error.code}] ${result.error.message}`);
    } catch (error) { message(error.message || String(error)); }
    finally { navigating = false; }
  }
  const back = () => navigate('ui.back');
  $('close').addEventListener('click', back);
  $('bindings').addEventListener('click', () => navigate('ui.open', {import:'settings'}));
  for (const entry of document.querySelectorAll('[data-import]')) {
    entry.addEventListener('click', () => navigate('ui.open', {import:entry.dataset.import}));
  }
  unsubs.push(roundo.subscribe('client.input', events => {
    if (events.some(event => event.function === 'roundo.debug' && event.state === 'pressed')) back();
  }));
  unsubs.push(roundo.subscribe('client.connection', value => {
    const state = value.status.status;
    $('connection').textContent = state === 'connected' ? '已连接' : `未连接 · ${state}`;
    if ($('runtime-connection')) $('runtime-connection').textContent = JSON.stringify(value, null, 2);
  }));
  for (const [stream, id] of [['client.hud', 'runtime-hud'], ['client.control', 'runtime-control']]) {
    if ($(id)) unsubs.push(roundo.subscribe(stream, value => $(id).textContent = JSON.stringify(value, null, 2)));
  }
  unsubs.push(roundo.subscribe('client.bindings', value => {
    const keys = value.bindings.filter(b => value.supported_slots.some(slot => slot.slot === b.slot && slot.functions.includes('roundo.debug'))).map(b => value.supported_keys.find(k => k.key === b.key)?.display_name || b.key);
    $('shortcut').textContent = `调试快捷键：${keys.join(' / ') || '已解绑'}。可在设置中修改；输入框内不触发。`;
  }));
  window.addEventListener('roundo:destroying', () => {
    for (const unsubscribe of unsubs) unsubscribe();
  }, {once:true});
})();
