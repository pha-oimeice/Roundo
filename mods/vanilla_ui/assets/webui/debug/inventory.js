'use strict';

import {createDebugTransport} from './transport.js';

export function mount(root) {
  const $ = id => root.querySelector('#' + CSS.escape(id));
  const owners = new Map(), inventories = new Map(), targets = new Map();
  const roles = {intent:'意图体', execution:'执行体', record:'记录体'};
  let epoch = null, revision = 0, busy = false, destroyed = false, active = false;
  let ownerOffset = null, inventoryOffset = null, targetOffset = 0;
  let selected = null, snapshot = null, selectedItem = null;
  let itemOffset = 0, nextItemOffset = null, previousItemOffsets = [];

  function message(text, error = false) {
    $('message').textContent = text;
    $('message').classList.toggle('error', error);
  }
  const ownerLabel = owner => `${owner.is_public ? '公共保管体（Public）' : owner.roles.map(role => roles[role] || role).join(' / ') || '未标注角色的概念体'} · ${owner.body}`;
  const inventoryLabel = entry => `${entry.is_public ? 'Public 物品栏' : '物品栏'} · ${entry.inventory} · ${ownerLabel(entry.owner)}`;
  function option(select, value, text, disabled = false) {
    const node = document.createElement('option');
    node.value = value; node.textContent = text; node.disabled = disabled; select.append(node);
  }
  function renderOwners() {
    for (const [id, placeholder] of [['owner-filter', '所有概念体'], ['create-owner', '请选择概念体']]) {
      const select = $(id), previous = select.value;
      select.replaceChildren(); option(select, '', placeholder);
      for (const owner of owners.values()) option(select, owner.body, ownerLabel(owner), id === 'create-owner' && owner.is_public);
      if (owners.has(previous)) select.value = previous;
    }
  }
  function remember(entries) {
    for (const entry of entries) owners.set(entry.owner.body, entry.owner);
    renderOwners();
  }
  function renderTargets() {
    const previous = $('target-inventory').value;
    $('target-inventory').replaceChildren(); option($('target-inventory'), '', '请选择目标物品栏');
    for (const entry of targets.values()) {
      if (entry.inventory !== selected) option($('target-inventory'), entry.inventory, inventoryLabel(entry));
    }
    if (targets.has(previous) && previous !== selected) $('target-inventory').value = previous;
    $('load-targets').hidden = targetOffset === null;
    $('target-note').textContent = `已缓存 ${targets.size} 个目标候选（当前物品栏不列为目标）。可加载其他概念体的物品栏；外部变动后请刷新。`;
  }
  function renderList() {
    const search = $('search').value.trim().toLowerCase();
    $('inventory-list').replaceChildren();
    let count = 0;
    for (const entry of inventories.values()) {
      if (!inventoryLabel(entry).toLowerCase().includes(search)) continue;
      count++;
      const button = document.createElement('button'); button.type = 'button'; button.className = 'body-choice';
      button.classList.toggle('selected', selected === entry.inventory);
      button.setAttribute('aria-pressed', String(selected === entry.inventory));
      const title = document.createElement('strong'); title.textContent = entry.is_public ? 'Public 兜底物品栏' : entry.inventory;
      const owner = document.createElement('span'); owner.className = 'object-id'; owner.textContent = ownerLabel(entry.owner);
      const items = document.createElement('span'); items.className = 'object-duty'; items.textContent = `直接保管 ${entry.item_count} 件物品`;
      button.append(title, owner, items);
      button.addEventListener('click', () => run(() => inspect(entry.inventory)));
      $('inventory-list').append(button);
    }
    if (!count) $('inventory-list').textContent = !epoch ? '连接服务端后读取。' : search ? '没有匹配的已加载物品栏。' : '当前查询没有已加载物品栏。';
    $('more-inventories').hidden = inventoryOffset === null;
  }
  function clearSelection() {
    selected = null; snapshot = null; selectedItem = null;
    itemOffset = 0; nextItemOffset = null; previousItemOffsets = [];
    $('selection').textContent = '尚未选择物品栏'; $('inspect').disabled = true;
    $('inventory-detail').hidden = true; $('empty-selection').hidden = false;
    $('confirm-remove').checked = false; $('remove-panel').open = false;
    $('item-list').replaceChildren();
    renderList(); renderTargets();
  }
  function reset() {
    owners.clear(); inventories.clear(); targets.clear();
    ownerOffset = null; inventoryOffset = null; targetOffset = 0;
    $('owner-filter').value = ''; $('create-owner').value = ''; $('search').value = '';
    renderOwners(); clearSelection();
    $('counts').textContent = '会话已改变，旧列表已清除';
    $('owner-counts').textContent = '会话已改变，旧概念体选择已清除';
    $('more-owners').hidden = true;
    message(epoch ? '正在读取当前会话的概念体与物品栏…' : '尚未连接服务端。连接后可查看和管理物品栏。');
  }
  const transport = createDebugTransport(c => roundo.execute(c), (name, listener) => roundo.subscribe(name, listener), value => {
    epoch = value; revision++; reset();
    queueMicrotask(() => { if (active && !busy && !destroyed && epoch) run(loadInitial); });
  });
  async function request(operation, inventory = true) {
    const current = revision;
    const data = await transport.request(inventory ? {inventory:operation} : operation);
    if (destroyed || current !== revision) throw new Error('会话已改变，旧结果不再用于当前界面。');
    return data;
  }
  async function run(task) {
    if (busy || destroyed) return;
    const current = revision;
    busy = true; $('inventory-workspace').inert = true; $('activity').textContent = '等待服务端结果…';
    try { await task(); }
    catch (error) { if (!destroyed && current === revision) message(error.message || String(error), true); }
    finally {
      busy = false;
      if (!destroyed) {
        $('inventory-workspace').inert = false; $('activity').textContent = '就绪';
        // Only a new session triggers a fresh read, never a mutation retry.
        if (current !== revision && epoch && active) queueMicrotask(() => { if (active && !destroyed) run(loadInitial); });
      }
    }
  }
  async function loadOwners(append = false) {
    const offset = append ? ownerOffset : 0;
    if (offset === null) return;
    const reply = await request({list:{offset}}, false);
    const data = reply.objects;
    if (!data) throw new Error('服务端未返回概念体列表');
    if (!append) owners.clear();
    for (const owner of data.body_summaries) owners.set(owner.body, owner);
    // Preserve owners observed in inventory snapshots on other pages.
    remember([...inventories.values(), ...targets.values()]);
    ownerOffset = data.next_offset; $('more-owners').hidden = ownerOffset === null;
    $('owner-counts').textContent = `已加载 ${owners.size} 个概念体；服务端共 ${data.total_bodies} 个${data.truncated ? '（诊断索引截断）' : ''}。Public 不能另建物品栏。`;
  }
  async function loadInventories(append = false) {
    const offset = append ? inventoryOffset : 0;
    if (offset === null) return;
    const data = (await request({list:{body:$('owner-filter').value || null, offset}})).inventories;
    if (!data) throw new Error('服务端未返回物品栏列表');
    if (!append) inventories.clear();
    for (const entry of data.inventories) { inventories.set(entry.inventory, entry); targets.set(entry.inventory, entry); }
    remember(data.inventories); inventoryOffset = data.next_offset;
    $('counts').textContent = `当前查询已加载 ${inventories.size} / ${data.total} 个物品栏${data.truncated ? '（库存或所属概念体超出诊断索引，结果不完整）' : ''}。`;
    renderList(); renderTargets();
  }
  async function loadTargets() {
    if (targetOffset === null) return;
    const data = (await request({list:{body:null, offset:targetOffset}})).inventories;
    if (!data) throw new Error('服务端未返回目标物品栏列表');
    for (const entry of data.inventories) targets.set(entry.inventory, entry);
    targetOffset = data.next_offset; remember(data.inventories); renderTargets();
    if (data.truncated) message('目标物品栏列表达到诊断索引上限，未列出的物品栏不能判定为不存在。');
  }
  async function loadInitial() { await loadOwners(); await loadInventories(); }
  function renderItemSelection() {
    $('selected-item').textContent = selectedItem === null ? '请先选择一件直接成员。' : `已选择物品 ID：${selectedItem}`;
    $('transfer').disabled = selectedItem === null;
    $('spill').disabled = selectedItem === null || snapshot?.summary.is_public;
    for (const button of $('item-list').querySelectorAll('button')) {
      const active = button.dataset.item === selectedItem;
      button.classList.toggle('selected', active); button.setAttribute('aria-pressed', String(active));
    }
  }
  function renderDetail() {
    const entry = snapshot.summary;
    $('selection').textContent = entry.is_public ? `Public 物品栏 · ${selected}` : `物品栏 · ${selected}`;
    $('inspect').disabled = false; $('empty-selection').hidden = true; $('inventory-detail').hidden = false;
    $('inventory-owner').textContent = ownerLabel(entry.owner);
    $('inventory-scope').textContent = entry.is_public ? '世界唯一兜底库存，不能删除。保管关系不等于物品数据访问权。' : '这是该概念体持有的一个独立物品栏，不与它的其他物品栏合并。';
    $('snapshot-note').textContent = `服务端快照 · ${new Date().toLocaleTimeString()}。外部变动请刷新；分页不保证同一时刻。`;
    $('item-counts').textContent = `共 ${snapshot.total_items} 件直接成员；本页 ${snapshot.items.length} 件${snapshot.items.length ? `，从第 ${itemOffset + 1} 件起` : ''}。仅展示 ID，不推断名称或类型。`;
    $('item-list').replaceChildren();
    for (const id of snapshot.items) {
      const button = document.createElement('button'); button.type = 'button'; button.dataset.item = id; button.textContent = id;
      button.addEventListener('click', () => { selectedItem = id; renderItemSelection(); });
      $('item-list').append(button);
    }
    if (!snapshot.items.length) $('item-list').textContent = snapshot.total_items ? '本页没有成员，请刷新当前物品栏。' : '物品栏为空。';
    $('previous-items').hidden = previousItemOffsets.length === 0; $('next-items').hidden = nextItemOffset === null;
    $('remove-panel').hidden = entry.is_public;
    $('confirm-remove').checked = false; $('remove-panel').open = false;
    $('remove-note').textContent = `删除 ${selected}，并将它当前的直接成员转入 Public。不删除所属概念体、物品或物品自身的库存。`;
    renderItemSelection(); renderList(); renderTargets();
  }
  async function inspect(id, offset = 0, history = []) {
    const data = (await request({inspect:{inventory:id, offset}})).inventory;
    if (!data) throw new Error('服务端未返回物品栏详情');
    selected = id; snapshot = data; selectedItem = null;
    itemOffset = offset; nextItemOffset = data.next_offset; previousItemOffsets = history;
    if (inventories.has(id)) inventories.set(id, data.summary);
    targets.set(id, data.summary); remember([data.summary]); renderDetail();
  }
  async function mutate(operation, success, refresh) {
    const data = await request(operation);
    message(success);
    try { await refresh(data); }
    catch (error) { throw new Error(`${success}\n但重新读取失败：${error.message}。请手动刷新，不要重复已成功操作。`); }
  }

  $('refresh').addEventListener('click', () => run(async () => {
    await loadOwners(); await loadInventories(); message('已重新读取概念体与当前查询的物品栏列表；详情请单独刷新。');
  }));
  $('owner-filter').addEventListener('change', () => run(() => loadInventories()));
  $('more-owners').addEventListener('click', () => run(() => loadOwners(true)));
  $('more-inventories').addEventListener('click', () => run(() => loadInventories(true)));
  $('search').addEventListener('input', renderList);
  $('inspect').addEventListener('click', () => selected && run(() => inspect(selected)));
  $('next-items').addEventListener('click', () => selected && nextItemOffset !== null && run(() => inspect(selected, nextItemOffset, [...previousItemOffsets, itemOffset])));
  $('previous-items').addEventListener('click', () => {
    const history = [...previousItemOffsets], offset = history.pop();
    if (selected && offset !== undefined) run(() => inspect(selected, offset, history));
  });
  $('load-targets').addEventListener('click', () => run(loadTargets));
  $('refresh-targets').addEventListener('click', () => run(async () => { targets.clear(); targetOffset = 0; renderTargets(); await loadTargets(); }));
  $('create-inventory').addEventListener('submit', event => {
    event.preventDefault();
    const body = $('create-owner').value;
    if (!body) return;
    run(() => mutate({create:{body}}, '空物品栏已由服务端创建。', async data => {
      if (!data.inventory_created?.inventory) throw new Error('创建结果未包含物品栏选择器');
      await loadInventories(); await inspect(data.inventory_created.inventory);
    }));
  });
  $('transfer-item').addEventListener('submit', event => {
    event.preventDefault();
    const source = selected, item = selectedItem, target = $('target-inventory').value;
    if (!source || item === null || !target) return;
    run(() => mutate({transfer:{source, target, item}}, '所选物品已转入目标物品栏；物品数据未修改。', async () => {
      await loadInventories(); await inspect(source);
    }));
  });
  $('spill').addEventListener('click', () => {
    const inventory = selected, item = selectedItem;
    if (!inventory || item === null) return;
    run(() => mutate({spill:{inventory, item}}, '所选物品的保管关系已转入 Public；未生成空间掉落物。', async () => {
      await loadInventories(); await inspect(inventory);
    }));
  });
  $('remove-inventory').addEventListener('submit', event => {
    event.preventDefault();
    const inventory = selected;
    if (!inventory || !$('confirm-remove').checked || snapshot?.summary.is_public) return;
    run(() => mutate({remove:{inventory}}, '物品栏已删除，其直接成员已转入 Public；所属概念体与物品保留。', async () => {
      inventories.delete(inventory); targets.delete(inventory); clearSelection(); await loadInventories();
    }));
  });
  renderOwners(); clearSelection();
  return {
    activate() { active = true; if (epoch && !busy) run(loadInitial); },
    deactivate() { active = false; },
    destroy() { destroyed = true; active = false; transport.destroy(); }
  };
}
