'use strict';

(() => {
  const $ = id => document.getElementById(id);
  const inventories = new Map();
  const roles = {intent:'意图体', execution:'执行体', record:'记录体'};
  let epoch = null, revision = 0, busy = false, destroyed = false;
  let nextOffset = null, total = 0, truncated = false;

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
    let functions;
    try {
      const placement = reach('placement'), destruction = reach('destruction');
      functions = [placement === null ? null : `放置方块（距离 ${placement}）`, destruction === null ? null : `破坏方块（距离 ${destruction}）`].filter(value => value !== null).join('；') || '不装配功能';
    } catch (error) { functions = error.message; }
    $('composition-preview').textContent = selected ? `${inventoryLabel(selected)}\n${functions}。创建新的唯一 ID，不修改已有物品。` : '尚未选择归属物品栏。';
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
    updateSelection();
  }
  function lock() { $('item-workspace').inert = busy || !epoch; }
  function reset() {
    inventories.clear(); nextOffset = null; total = 0; truncated = false;
    $('created-item').hidden = true;
    $('created-id').textContent = ''; $('created-owner').textContent = ''; $('created-functions').textContent = '';
    for (const kind of ['placement', 'destruction']) {
      $(`enable-${kind}`).checked = false;
      const input = $(`${kind}-reach`); input.value = ''; input.disabled = true; input.required = false;
    }
    renderInventories(); lock();
    message(epoch ? '正在读取当前会话的物品栏，请选择归属并组装功能。' : '尚未连接服务端。连接后可管理物品。');
  }
  const transport = createDebugTransport(c => roundo.execute(c), (name, listener) => roundo.subscribe(name, listener), value => {
    epoch = value; revision++; reset();
    queueMicrotask(() => { if (!busy && !destroyed && epoch) run(() => loadInventories()); });
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
        // Reconnect only reloads targets; never replay a creation request.
        if (current !== revision && epoch) queueMicrotask(() => run(() => loadInventories()));
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
  function renderCreated(created) {
    inventories.set(created.inventory.inventory, created.inventory);
    renderInventories();
    $('created-id').textContent = created.item;
    $('created-owner').textContent = `归属：${inventoryLabel(created.inventory)}`;
    $('created-functions').textContent = [created.placement_reach === null ? null : `放置方块 · 距离 ${created.placement_reach}`, created.destruction_reach === null ? null : `破坏方块 · 距离 ${created.destruction_reach}`].filter(value => value !== null).join('；') || '未装配功能。';
    $('created-item').hidden = false;
  }
  $('item-inventory').addEventListener('change', updateSelection);
  $('refresh-inventories').addEventListener('click', () => run(() => loadInventories()));
  $('more-inventories').addEventListener('click', () => run(() => loadInventories(true)));
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
      message(`物品 ${data.item_created.item} 已创建并分配到所选物品栏，功能数据已由服务端回读。`);
    });
  });
  window.addEventListener('roundo:destroying', () => { destroyed = true; transport.destroy(); }, {once:true});
  reset();
})();
