'use strict';

// Internal views share this document and background. Only whole WebUIs use host navigation.
const $ = id => document.getElementById(id);
const content = $('page-content');
const declarations = new Map();
for (const declaration of document.querySelectorAll('template[data-page]')) {
  const name = declaration.dataset.page;
  if (!name || declarations.has(name) || !declaration.dataset.source || !declaration.dataset.title) {
    throw new Error('调试子界面声明缺失或重复');
  }
  declarations.set(name, declaration);
}
const directory = {
  name: content.firstElementChild.dataset.page,
  title: content.firstElementChild.dataset.title,
  root: content.firstElementChild,
  controller: null,
  streams: []
};
const pages = new Map([[directory.name, directory]]);
const history = [directory];
const unsubs = [];
const loading = new AbortController();
let current = directory, navigating = false, destroyed = false;

function message(text) {
  $('navigation-message').textContent = text;
  $('navigation-message').hidden = false;
}
function clearMessage() { $('navigation-message').hidden = true; }
async function loadPage(name) {
  if (pages.has(name)) return pages.get(name);
  const declaration = declarations.get(name);
  if (!declaration) throw new Error(`未声明的调试子界面：${name}`);
  const response = await fetch(declaration.dataset.source, {signal:loading.signal});
  if (!response.ok) throw new Error(`调试子界面加载失败：${name}（${response.status}）`);
  const fragment = document.createElement('template');
  fragment.innerHTML = await response.text();
  const root = fragment.content.firstElementChild;
  if (fragment.content.children.length !== 1 || !root?.matches('section.debug-page') || root.querySelector('script')) {
    throw new Error(`调试子界面须为不含脚本的单个内容片段：${name}`);
  }
  const module = declaration.dataset.controller ? await import(declaration.dataset.controller) : null;
  if (module && typeof module.mount !== 'function') throw new Error(`调试子界面缺少初始化入口：${name}`);
  if (destroyed) throw new Error('调试界面已关闭');
  const page = {
    name,
    title: declaration.dataset.title,
    root,
    controller: module ? module.mount(root) : null,
    streams: []
  };
  pages.set(name, page);
  return page;
}
function deactivate(page) {
  page.controller?.deactivate();
  for (const unsubscribe of page.streams) unsubscribe();
  page.streams = [];
}
function activate(page) {
  page.controller?.activate();
  for (const node of page.root.querySelectorAll('[data-stream]')) {
    page.streams.push(roundo.subscribe(node.dataset.stream, value => {
      node.textContent = JSON.stringify(value, null, 2);
    }));
  }
}
async function showPage(name, backwards = false) {
  if (navigating || destroyed || name === current.name) return;
  navigating = true;
  clearMessage();
  try {
    // Keep the current content attached until the next view is loaded and initialized.
    const next = await loadPage(name);
    if (destroyed) return;
    deactivate(current);
    content.replaceChildren(next.root);
    current = next;
    if (backwards) history.pop();
    else history.push(next);
    $('page-title').textContent = next.title;
    document.title = `Roundo · ${next.title}`;
    $('back').hidden = history.length === 1;
    if (history.length > 1) $('back').textContent = `返回${history[history.length - 2].title}`;
    activate(next);
    $('page-title').focus();
  } catch (error) {
    if (!destroyed) message(error.message || String(error));
  } finally { navigating = false; }
}
async function hostNavigate(command, args = {}) {
  if (navigating || destroyed) return;
  navigating = true;
  clearMessage();
  try {
    const result = await roundo.execute({version:1, command, arguments:args});
    if (!result.ok) message(`[${result.error.code}] ${result.error.message}`);
  } catch (error) {
    if (!destroyed) message(error.message || String(error));
  } finally { navigating = false; }
}
function back() {
  if (history.length > 1) showPage(history[history.length - 2].name, true);
  else hostNavigate('ui.back');
}
$('back').addEventListener('click', back);
$('close').addEventListener('click', () => hostNavigate('ui.back'));
$('bindings').addEventListener('click', () => hostNavigate('ui.open', {import:'settings'}));
content.addEventListener('click', event => {
  const entry = event.target.closest('[data-view]');
  if (entry) showPage(entry.dataset.view);
});
unsubs.push(roundo.subscribe('client.input', events => {
  if (events.some(event => event.function === 'roundo.debug' && event.state === 'pressed')) back();
}));
unsubs.push(roundo.subscribe('client.connection', value => {
  const state = value.status.status;
  $('connection').textContent = state === 'connected' ? '已连接' : `未连接 · ${state}`;
}));
unsubs.push(roundo.subscribe('client.bindings', value => {
  const keys = value.bindings.filter(b => value.supported_slots.some(slot => slot.slot === b.slot && slot.functions.includes('roundo.debug'))).map(b => value.supported_keys.find(k => k.key === b.key)?.display_name || b.key);
  $('shortcut').textContent = `调试快捷键：${keys.join(' / ') || '已解绑'}。可在设置中修改；输入框内不触发。`;
}));
window.addEventListener('roundo:destroying', () => {
  destroyed = true;
  loading.abort();
  deactivate(current);
  for (const page of pages.values()) page.controller?.destroy();
  for (const unsubscribe of unsubs) unsubscribe();
}, {once:true});
