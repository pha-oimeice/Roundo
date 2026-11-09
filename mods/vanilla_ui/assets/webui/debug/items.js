'use strict';

import {createDebugTransport} from './transport.js';

export function mount(root) {
  const $ = id => root.querySelector('#' + CSS.escape(id));
  const inventories = new Map();
  const roles = {intent:'意图体', execution:'执行体', record:'记录体'};
  let epoch = null, revision = 0, busy = false, destroyed = false, active = false;
  let nextOffset = null, total = 0, truncated = false;
  let listedInventory = null, listedItems = [], selectedItem = null, itemTotal = 0;
  let itemOffset = 0, nextItemOffset = null, previousItemOffsets = [];

  function message(text, error = false) {
    $('message').textContent = text;
    $('message').classList.toggle('error', error);
  }
  const ownerLabel = owner => `${owner.is_public ? '公共保管体（Public）' : owner.roles.map(role => roles[role] || role).join(' / ') || '未标注角色的概念体'} · ${owner.body}`;
  const inventoryLabel = entry => `${entry.is_public ? 'Public 物品栏' : entry.inventory} · ${ownerLabel(entry.owner)}`;
  function reach(kind) {
    if (!$(`enable-${kind}`).checked) return null;
    const input = $(`${kind}-reach`);
    const value = input.value === '' ? NaN : Number(input.value);
    // Match the native f32 source validator, including overflow/underflow.
    if (!Number.isFinite(Math.fround(value)) || Math.fround(value) <= 0) throw new Error('已勾选的功能必须填写可表示为有限正数的距离。');
    return value;
  }
  function updateSelection() {
    const selected = inventories.get($('item-inventory').value);
    $('inventory-owner').textContent = selected ? ownerLabel(selected.owner) : '尚未选择。';
    $('inventory-custody').textContent = selected ? `${selected.is_public ? 'Public 兜底保管' : selected.inventory} · 快照中直接保管 ${selected.item_count} 件物品。` : '';
    $('create').disabled = !epoch || !selected;
    $('refresh-items').disabled = !epoch || !selected;
    let functions;
    try {
      const placement = reach('placement'), destruction = reach('destruction');
      functions = [placement === null ? null : `放置方块（距离 ${placement}）`, destruction === null ? null : `破坏方块（距离 ${destruction}）`].filter(value => value !== null).join('；') || '不装配功能';
    } catch (error) { functions = error.message; }
    $('composition-preview').textContent = selected ? `${inventoryLabel(selected)}\n${functions}。创建新实例，不修改已有物品。` : '尚未选择归属物品栏。';
  }
  function renderInventories() {
    const select = $('item-inventory'), previous = select.value;
    select.replaceChildren();
    const placeholder = document.createElement('option'); placeholder.value = ''; placeholder.textContent = '请选择物品栏'; select.append(placeholder);
    for (const entry of inventories.values()) {
      const option = document.createElement('option'); option.value = entry.inventory; option.textContent = inventoryLabel(entry); select.append(option);
    }
    if (inventories.has(previous)) select.value = previous;
    $('more-inventories').hidden = nextOffset === null;
    $('inventory-counts').textContent = epoch ? `已加载 ${inventories.size} / ${total} 个物品栏${truncated ? '（诊断索引截断，未展示项不能判定为不存在）' : ''}。` : '连接服务端后读取物品栏。';
    if (listedInventory !== null && listedInventory !== select.value) clearItems();
    updateSelection();
  }
  function selectItem(item) {
    selectedItem = item;
    $('confirm-delete').checked = false;
    renderItemSelection();
  }
  function renderItemSelection() {
    $('delete-item').hidden = selectedItem === null;
    $('selected-item').textContent = selectedItem === null ? '' : `物品 ID：${selectedItem.item}`;
    $('delete').disabled = selectedItem === null || !$('confirm-delete').checked;
    for (const button of $('item-list').querySelectorAll('button')) {
      const selected = button.dataset.selector === selectedItem?.selector;
      button.classList.toggle('selected', selected);
      button.setAttribute('aria-pressed', String(selected));
    }
  }
  function renderItems() {
    $('item-list').replaceChildren();
    for (const item of listedItems) {
      const button = document.createElement('button');
      button.type = 'button'; button.dataset.selector = item.selector; button.textContent = item.item;
      button.addEventListener('click', () => { if (!busy) selectItem(item); });
      $('item-list').append(button);
    }
    $('item-counts').textContent = listedInventory === null ? '选择物品栏后读取物品。'
      : `服务端快照 · 共 ${itemTotal} 件，本页 ${listedItems.length} 件${listedItems.length ? `，从第 ${itemOffset + 1} 件起` : ''}。`;
    if (listedInventory !== null && !listedItems.length) $('item-list').textContent = itemTotal ? '本页没有物品，请刷新。' : '物品栏为空。';
    $('previous-items').hidden = previousItemOffsets.length === 0;
    $('next-items').hidden = nextItemOffset === null;
    renderItemSelection();
  }
  function clearItems() {
    listedInventory = null; listedItems = []; selectedItem = null; itemTotal = 0;
    itemOffset = 0; nextItemOffset = null; previousItemOffsets = [];
    $('confirm-delete').checked = false;
    renderItems();
  }
  function lock() { $('item-workspace').inert = busy || !epoch; }
  function reset() {
    inventories.clear(); nextOffset = null; total = 0; truncated = false;
    clearItems(); $('create-panel').open = false;
    $('created-item').hidden = true;
    $('created-id').textContent = ''; $('created-owner').textContent = ''; $('created-functions').textContent = '';
    for (const kind of ['placement', 'destruction']) {
      $(`enable-${kind}`).checked = false;
      const input = $(`${kind}-reach`); input.value = ''; input.disabled = true; input.required = false;
    }
    renderInventories(); lock();
    message(epoch ? '正在读取物品栏，请选择要管理的物品栏。' : '尚未连接服务端。');
  }
  const transport = createDebugTransport(c => roundo.execute(c), (name, listener) => roundo.subscribe(name, listener), value => {
    epoch = value; revision++; reset();
    queueMicrotask(() => { if (active && !busy && !destroyed && epoch) run(refresh); });
  });
  async function request(operation) {
    const current = revision;
    const data = await transport.request(operation);
    if (destroyed || current !== revision) throw new Error('会话已改变，旧结果不再用于当前界面。');
    return data;
  }
  async function run(task) {
    if (busy || destroyed || !epoch) return;
    const current = revision;
    busy = true; lock(); $('activity').textContent = '等待服务端结果…';
    try { await task(); }
    catch (error) { if (!destroyed && current === revision) message(error.message || String(error), true); }
    finally {
      busy = false;
      if (!destroyed) {
        lock(); $('activity').textContent = '就绪';
        // Reconnect reloads snapshots; never replay a creation or deletion.
        if (current !== revision && epoch && active) queueMicrotask(() => { if (active && !destroyed) run(refresh); });
      }
    }
  }
  async function loadInventories(append = false) {
    const offset = append ? nextOffset : 0;
    if (offset === null) return;
    const data = (await request({inventory:{list:{body:null, offset}}})).inventories;
    if (!data) throw new Error('服务端未返回物品栏列表。');
    if (!append) inventories.clear();
    for (const entry of data.inventories) inventories.set(entry.inventory, entry);
    nextOffset = data.next_offset; total = data.total; truncated = data.truncated;
    renderInventories();
  }
  async function loadItems(offset = 0, history = []) {
    const inventory = $('item-inventory').value;
    if (!inventories.has(inventory)) { clearItems(); return; }
    selectItem(null);
    const data = (await request({item:{list:{inventory, offset}}})).items;
    if (!data || data.inventory.inventory !== inventory) throw new Error('服务端未返回所选物品栏的物品列表。');
    listedInventory = inventory; listedItems = data.items; itemTotal = data.total_items;
    itemOffset = offset; nextItemOffset = data.next_offset; previousItemOffsets = history;
    inventories.set(inventory, data.inventory);
    renderInventories(); renderItems();
  }
  async function refresh() {
    await loadInventories();
    await loadItems();
    message(listedInventory === null ? '请选择要管理的物品栏。' : '物品列表已刷新。');
  }
  async function refreshAfterMutation(success) {
    message(success);
    try { await loadItems(); }
    catch (error) { throw new Error(`${success}\n但刷新物品列表失败：${error.message}。请手动刷新，不要重复提交已成功操作。`); }
  }
  function renderCreated(created) {
    inventories.set(created.inventory.inventory, created.inventory);
    renderInventories();
    $('created-id').textContent = created.item;
    $('created-owner').textContent = `归属：${inventoryLabel(created.inventory)}`;
    $('created-functions').textContent = [created.placement_reach === null ? null : `放置方块 · 距离 ${created.placement_reach}`, created.destruction_reach === null ? null : `破坏方块 · 距离 ${created.destruction_reach}`].filter(value => value !== null).join('；') || '未装配功能。';
    $('created-item').hidden = false;
  }
  $('item-inventory').addEventListener('change', () => {
    clearItems(); updateSelection();
    run(async () => { await loadItems(); message(listedInventory === null ? '请选择物品栏。' : '选择物品可执行删除。'); });
  });
  $('refresh-inventories').addEventListener('click', () => run(refresh));
  $('more-inventories').addEventListener('click', () => run(() => loadInventories(true)));
  $('refresh-items').addEventListener('click', () => run(() => loadItems()));
  $('next-items').addEventListener('click', () => {
    if (nextItemOffset !== null) run(() => loadItems(nextItemOffset, [...previousItemOffsets, itemOffset]));
  });
  $('previous-items').addEventListener('click', () => {
    const history = [...previousItemOffsets], offset = history.pop();
    if (offset !== undefined) run(() => loadItems(offset, history));
  });
  $('confirm-delete').addEventListener('change', renderItemSelection);
  $('cancel-delete').addEventListener('click', () => selectItem(null));
  $('delete-item').addEventListener('submit', event => {
    event.preventDefault();
    const target = selectedItem, inventory = listedInventory;
    if (!target || !inventory || inventory !== $('item-inventory').value || !$('confirm-delete').checked) return;
    run(async () => {
      selectItem(null);
      const data = await request({item:{remove:{inventory, selector:target.selector}}});
      if (data.item_removed?.selector !== target.selector || data.item_removed.item !== target.item) {
        clearItems();
        throw new Error('未收到匹配的删除完成回执；请刷新核实，不要重复提交。');
      }
      clearItems(); $('created-item').hidden = true;
      await refreshAfterMutation(`物品 ${target.item} 的所选实例已删除。`);
    });
  });
  for (const kind of ['placement', 'destruction']) {
    $(`enable-${kind}`).addEventListener('change', () => {
      const enabled = $(`enable-${kind}`).checked;
      const input = $(`${kind}-reach`); input.disabled = !enabled; input.required = enabled;
      updateSelection();
    });
    $(`${kind}-reach`).addEventListener('input', updateSelection);
  }
  $('create-item').addEventListener('submit', event => {
    event.preventDefault();
    run(async () => {
      const inventory = $('item-inventory').value;
      if (!inventories.has(inventory)) throw new Error('请先选择已有物品栏。');
      const data = await request({create_item:{inventory, placement_reach:reach('placement'), destruction_reach:reach('destruction')}});
      if (!data.item_created) throw new Error('服务端未返回物品创建回读；请检查目标物品栏，不要重复提交。');
      renderCreated(data.item_created);
      await refreshAfterMutation(`物品 ${data.item_created.item} 已创建到所选物品栏。`);
    });
  });
  reset();
  return {
    activate() { active = true; if (epoch && !busy) run(refresh); },
    deactivate() { active = false; },
    destroy() { destroyed = true; active = false; transport.destroy(); }
  };
}
