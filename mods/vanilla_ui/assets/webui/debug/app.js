'use strict';

import {createDebugTransport} from './transport.js';

// Pure attribute validation; request correlation lives in the shared adapter.
function numericValue(text, kind) {
  if (String(text).trim() === '') throw new Error('属性值不能为空');
  const value = Number(text);
  if (!Number.isFinite(value)) throw new Error('属性值必须是有限数');
  if (kind === 'integer' && (!Number.isInteger(value) || value < -2147483648 || value > 2147483647)) throw new Error('整数须在 i32 范围内');
  if (kind === 'unorm' && (value < 0 || value > 1)) throw new Error('比率须在 [0, 1] 内');
  return value;
}
export function mount(root) {
  const $ = id => root.querySelector('#' + CSS.escape(id));
  let selected = null, bodySnapshot = null, epoch = null, busy = false, nextOffset = null;
  let active = false, destroyed = false;
  let bodies = [], lives = [], dataTypes = [];
  let bodySummaries = new Map();
  let selectionGeneration = 0;
  const labels = ['生命值', '意图值', '执行值', '记录值', '连续率', '变化率', '周期率', '永恒率'];
  const names = ['health','intent','execution','record','continuity','change','cycle','eternity'];
  const dataNames = {
    'roundo.creature.movement-command.v1':'移动意图',
    'roundo.creature.gaze-command-state.v1':'视线目标',
    'roundo.creature.motion-experience.v1':'运动经历',
    'roundo.creature.gaze-command.v1':'视线增量',
    'roundo.creature.jump-command.v1':'跳跃请求'
  };
  const roleInfo = {
    intent:['意图体', '表达想做什么：向生命体发布意图，不保证意图被兑现。'],
    execution:['执行体', '根据自身能力和环境产生实际行为；角色标签本身不证明本次行为已经发生。'],
    record:['记录体', '读取协作数据，自行决定整理和保留哪些记录；允许不读取或不生成记录。']
  };
  const publicDuty = '世界级公共保管体：承接意外释放物品的保管关系，不要求参与生命体，也不代表地面上出现掉落物。正常管理路径不能单独删除，随整个世界退出。';
  const actionInfo = {
    declare_reader:['声明读取资格', '为当前概念体声明读取这类数据的资格；声明不限于当前生命体，不等于批准或接通。'],
    declare_writer:['声明写入资格', '为当前概念体声明写入这类数据的资格；声明不限于当前生命体，不会创建状态或接通写入。'],
    approve_reader:['批准读取', '由所选生命体批准当前概念体读取；这一步会实际授予批准，但尚未注册读取关系。'],
    approve_writer:['批准写入', '由所选生命体批准事件写入者或状态的原始写入者；不自动接通或更换现有状态来源。'],
    register_reader:['接通读取', '在声明、批准及数据接口满足要求后，注册实际读取关系。'],
    register_writer:['接通写入 / 重接原来源', '事件：注册写入者。状态：只重新接通原始写入者，不创建新状态，也不能更换来源。'],
    unregister_reader:['断开读取', '注销当前读取关系，保留声明与批准；不影响其他读取者。'],
    unregister_writer:['断开写入', '状态：保留当前值、历史和读取者，只冻结写入。事件：不撤回已经接受的事件。声明与批准保留。'],
    create_slot:['创建空状态并接通写入', '仅在状态尚不存在且规则允许空初值时创建；固定当前概念体为不可更换的原始写入者，并注册写入。不会覆盖已有状态。']
  };
  const dataLabel = (key, shape) => `${dataNames[key] || key} · ${shape === 'state' ? '状态' : shape === 'event' ? '事件' : shape}`;
  function message(text, error = false) { $('message').textContent = text; $('message').classList.toggle('error', error); }
  function reset() {
    selected = null; bodySnapshot = null; bodies = []; lives = []; dataTypes = []; bodySummaries.clear(); nextOffset = null;
    $('body-identity').hidden = true; $('body-duty').textContent = ''; $('body-recipe').replaceChildren(); $('identity-evidence').textContent = '';
    selectionGeneration++;
    $('body-list').replaceChildren(); $('life-list').replaceChildren(); $('supported-data').replaceChildren();
    $('life').replaceChildren(); $('data-type').replaceChildren();
    $('counts').textContent = '会话已改变，请刷新列表';
    $('empty-selection').hidden = false;
    relationState();
    $('selection').textContent = '未选择概念体'; $('attributes-form').hidden = true;
    $('relation-editor').hidden = true; $('create-form').hidden = true; $('inspect').disabled = true; $('more').hidden = true;
    message('会话状态已更新。请刷新列表；旧会话选择器与未提交草稿已清除。');
  }
  const transport = createDebugTransport(c => roundo.execute(c), (name, listener) => roundo.subscribe(name, listener), value => { epoch = value; reset();
    if (value) queueMicrotask(() => { if (active && !destroyed && !busy && epoch === value) run(() => list()); });
  });
  async function run(task, success) {
    if (busy || destroyed) return;
    busy = true; $('bodies').inert = true; root.classList.add('busy'); $('activity').textContent = '等待服务端结果…';
    try { await task(); if (!destroyed && success) message(success); }
    catch (error) { if (!destroyed) message(error.message, true); }
    finally { busy = false; $('bodies').inert = false; root.classList.remove('busy'); $('activity').textContent = '就绪'; }
  }
  function summaryFor(id) {
    return bodySummaries.get(id) || (bodySnapshot?.body === id ? bodySnapshot.summary : null);
  }
  function bodyTitle(id) {
    const summary = summaryFor(id);
    if (!summary) return '身份信息未提供';
    const titles = summary.roles.map(role => roleInfo[role]?.[0] || role);
    if (summary.is_public) titles.push('公共保管体（Public）');
    return titles.join(' / ') || '未标注角色的概念体';
  }
  const bodyLabel = id => `${bodyTitle(id)} · ${id}`;
  function bodyDuty(id) {
    const summary = summaryFor(id);
    if (!summary) return '服务端未提供身份描述；不能根据编号、属性值或数量判断角色。';
    const duties = summary.roles.map(role => roleInfo[role]?.[1] || `未适配的角色标签：${role}`);
    if (summary.is_public) duties.push(publicDuty);
    return duties.join(' ') || '未发现当前编辑器识别的意图、执行或记录角色标签。这不表示它没有其他能力；基础属性也不会自动赋予角色。';
  }
  function selectLife(id) {
    if (!lives.includes(id)) { lives.push(id); options($('life'), lives, value => value); }
    $('life').value = id; relationState();
  }
  function bodyButton(id, compact = false) {
    const button = document.createElement('button'); button.type = 'button'; button.className = 'body-choice';
    const title = document.createElement('strong'); title.textContent = bodyTitle(id);
    const identity = document.createElement('span'); identity.className = 'object-id';
    const summary = summaryFor(id);
    identity.textContent = `${id}${summary?.recipe_life ? ` · 配方 ${summary.recipe_life}` : ''}`;
    button.append(title, identity);
    if (!compact) {
      const duty = document.createElement('span'); duty.className = 'object-duty';
      duty.textContent = summary?.is_public ? '世界设施 · 公共物品保管' : summary?.roles.length
        ? summary.roles.map(role => ({intent:'发布意图', execution:'产生行为', record:'选择和保留记录'}[role] || role)).join(' / ')
        : '点击查看身份与数据关系';
      button.append(duty);
    }
    button.classList.toggle('selected', selected === id); button.setAttribute('aria-pressed', String(selected === id));
    button.addEventListener('click', () => run(() => inspect(id)));
    return button;
  }
  function renderLives() {
    const life = $('life').value;
    const summaries = [...bodySummaries.values()];
    $('life-list').replaceChildren();
    for (const id of lives) {
      const button = document.createElement('button'); button.type = 'button';
      button.textContent = summaries.some(summary => summary.recipe_life === id) ? `运动协作 · ${id}` : `生命体 · ${id}`;
      button.classList.toggle('selected', life === id); button.setAttribute('aria-pressed', String(life === id));
      button.addEventListener('click', () => selectLife(id));
      $('life-list').append(button);
    }
    if (!lives.length) $('life-list').textContent = epoch ? '尚未加载生命体；可创建或加载更多。' : '连接服务端后可读取或创建。';
    $('life-selection').textContent = life ? `生命体 · ${life}` : '尚未选择生命体';
    const members = summaries.filter(summary => summary.recipe_life === life);
    $('life-note').textContent = !life ? '在左侧选择生命体，或创建一个没有参与者的协作空间。'
      : members.length ? `已加载 ${members.length} 个运动配方成员。点击查看各自职责与实际读写关系。`
      : '已选为关系配置目标；当前已加载的身份描述未报告运动配方成员，不表示没有数据参与者。';
    $('life-members').replaceChildren();
    for (const member of members) $('life-members').append(bodyButton(member.body, true));
    $('member-note').hidden = members.length === 0;
  }
  function renderList() {
    const filter = $('search').value.trim().toLowerCase();
    const visible = bodies.filter(id => `${bodyLabel(id)} ${bodyDuty(id)} ${summaryFor(id)?.recipe_life || ''}`.toLowerCase().includes(filter));
    $('body-list').replaceChildren();
    for (const infrastructure of [false, true]) {
      const group = visible.filter(id => Boolean(summaryFor(id)?.is_public) === infrastructure);
      if (infrastructure && group.length) {
        const heading = document.createElement('h4'); heading.textContent = '世界设施'; $('body-list').append(heading);
      }
      for (const id of group) $('body-list').append(bodyButton(id));
    }
    if (!visible.length) {
      const hint = document.createElement('p'); hint.textContent = !epoch ? '连接服务端后可读取或创建'
        : bodies.length ? '没有匹配的已加载概念体；可换关键词或加载更多。' : '当前尚无已加载概念体';
      $('body-list').append(hint);
    }
  }
  function renderIdentity() {
    $('body-identity').hidden = false;
    $('body-duty').textContent = bodyDuty(selected);
    const summary = summaryFor(selected);
    $('body-recipe').replaceChildren();
    if (summary?.recipe_life) {
      const button = document.createElement('button'); button.type = 'button'; button.textContent = `查看运动配方 ${summary.recipe_life}`;
      button.addEventListener('click', () => selectLife(summary.recipe_life)); $('body-recipe').append(button);
      const note = document.createElement('span'); note.className = 'muted'; note.textContent = ' 配方关联不是独占归属；实际读写权限见下方。';
      $('body-recipe').append(note);
    } else $('body-recipe').textContent = summary ? '当前描述未报告运动配方关联；是否参与其他数据协作，请查看下方实际关系。' : '配方关联：服务端未提供描述。';
    $('identity-evidence').textContent = summary
      ? `身份来自服务端当前的原生角色标签${summary.is_public ? '及经库存接口核验的 Public 身份' : ''}；配方关联来自当前存活生命体的运动组合登记。body 编号仅用于诊断定位，不是名称、出生顺序、角色或权限。`
      : '未收到只读身份描述，界面不推断角色。更新服务端并重新连接后可读取新描述。';
  }
  function options(element, values, label) {
    const previous = element.value; element.replaceChildren();
    for (const value of values) { const option = document.createElement('option'); option.value = value; option.textContent = label(value); element.append(option); }
    if (values.includes(previous)) element.value = previous;
  }
  async function list(append = false) {
    const offset = append ? nextOffset : 0;
    if (offset === null) return;
    const reply = await transport.request({list:{offset}});
    const data = reply.objects;
    if (!data) throw new Error('服务端未返回对象列表');
    bodies = [...new Set([...(append ? bodies : []), ...data.bodies])];
    lives = [...new Set([...(append ? lives : []), ...data.lives])];
    if (!append) bodySummaries.clear();
    for (const summary of data.body_summaries || []) bodySummaries.set(summary.body, summary);
    dataTypes = data.data_types; nextOffset = data.next_offset;
    $('counts').textContent = `已加载 ${bodies.length} / ${data.total_bodies} 概念体，${lives.length} / ${data.total_lives} 生命体${data.truncated ? '（诊断索引已达上限，结果不完整）' : ''}`;
    $('more').hidden = nextOffset === null;
    options($('life'), lives, id => id);
    options($('data-type'), dataTypes.map(t => t.key), key => dataLabel(key, dataTypes.find(t => t.key === key).shape));
    $('supported-data').replaceChildren();
    for (const type of dataTypes) {
      const item = document.createElement('li'); item.textContent = `${dataLabel(type.key, type.shape)} · ${type.key}`;
      $('supported-data').append(item);
    }
    renderList(); relationState();
    if (selected && !append && !bodies.includes(selected)) {
      // A selected object may be on another page; inspect resolves actual liveness.
      $('snapshot-note').textContent = '当前列表页未包含所选体；刷新快照以检查其存活状态。';
    }
  }
  function inputFor(kind, value) {
    const input = document.createElement('input'); input.type = 'number'; input.required = true;
    input.step = kind === 'integer' ? '1' : 'any'; input.min = kind === 'integer' ? '-2147483648' : '0';
    input.max = kind === 'integer' ? '2147483647' : '1'; input.value = String(value); return input;
  }
  function renderBody(data) {
    $('selection').textContent = bodyLabel(selected); renderIdentity();
    $('attributes').replaceChildren();
    for (const attribute of data.attributes) {
      const label = document.createElement('label');
      const title = document.createElement('span'); title.textContent = labels[attribute.id-1] || attribute.name; title.title = attribute.name; label.append(title);
      if (!attribute.writable || !attribute.value) { const text = document.createElement('span'); text.textContent = '未适配类型 · 只读'; label.append(text); }
      else {
        const kind = Object.hasOwn(attribute.value, 'integer') ? 'integer' : 'unorm';
        const input = inputFor(kind, attribute.value[kind]); const row = document.createElement('div'); row.className = 'field-actions';
        const save = document.createElement('button'); save.type = 'button'; save.textContent = '提交';
        save.addEventListener('click', () => run(async () => {
          const body = selected; const value = numericValue(input.value, kind);
          await transport.request({set_attribute:{body, attribute:attribute.id, value:{[kind]:value}}});
          await inspect(body);
        }, '属性已由服务端修改，并重新读取当前值。'));
        row.append(input, save); label.append(row);
      }
      $('attributes').append(label);
    }
    if (!data.attributes.length) $('attributes').textContent = '该概念体没有已注册到集中目录的属性；不会自动补零。';
    $('relations').replaceChildren();
    const active = data.relations.filter(r => r.reader_registered || r.writer_registered || r.immutable_origin);
    const table = document.createElement('table');
    const heading = document.createElement('tr');
    for (const text of ['生命体','数据','当前概念体的关系']) { const cell = document.createElement('th'); cell.textContent = text; heading.append(cell); } table.append(heading);
    for (const relation of active) {
      const row = document.createElement('tr');
      const relationships = [
        relation.reader_registered ? '读取已接通' : relation.reader_declared ? '已声明读取，未接通' : '',
        relation.writer_registered ? '写入已接通' : relation.writer_declared ? '已声明写入，未接通' : '',
        relation.immutable_origin ? '固定状态来源' : ''
      ].filter(Boolean).join('；');
      for (const text of [relation.life, dataLabel(relation.data_type, relation.shape), relationships]) {
        const cell = document.createElement('td'); cell.textContent = text; cell.title = relation.data_type; row.append(cell);
      }
      const choose = document.createElement('button'); choose.type = 'button'; choose.textContent = '配置';
      choose.addEventListener('click', () => {
        $('data-type').value = relation.data_type; selectLife(relation.life);
      });
      const cell = document.createElement('td'); cell.append(choose); row.append(cell); table.append(row);
    }
    const actionHeading = document.createElement('th'); actionHeading.textContent = '操作'; heading.append(actionHeading);
    if (active.length) $('relations').append(table); else $('relations').textContent = '本次快照覆盖的数据中，未发现已接通或固定来源关系。声明和批准不等于接通；请在下方选择数据查看或配置。';
    if (data.relations_truncated) { const warning = document.createElement('p'); warning.textContent = '关系快照达到 1024 条上限，以上结果不完整；未列出的关系不能判定为不存在。'; $('relations').append(warning); }
    $('snapshot-note').textContent = `服务端快照 · ${new Date().toLocaleTimeString()}；修改后重新读取，外部变动请手动刷新。`;
    $('attributes-form').hidden = false; $('relation-editor').hidden = false; $('create-form').hidden = true; $('empty-selection').hidden = true;
    relationState();
  }
  async function inspect(id) {
    const version = ++selectionGeneration;
    const data = await transport.request({inspect:{body:id}});
    if (version !== selectionGeneration) return;
    if (!data.body) throw new Error('服务端未返回概念体快照');
    selected = id; bodySnapshot = data.body;
    if (bodySnapshot.summary) bodySummaries.set(id, bodySnapshot.summary);
    else bodySummaries.delete(id);
    $('inspect').disabled = false; renderList(); renderBody(bodySnapshot);
  }
  function relationState() {
    renderLives();
    const type = $('data-type').value, life = $('life').value;
    const shape = dataTypes.find(t => t.key === type)?.shape;
    const r = bodySnapshot?.relations.find(r => r.life === life && r.data_type === type);
    const writer = $('relation-direction').value === 'writer';
    const ready = Boolean(selected && life && shape);
    $('relation-preview').textContent = ready ? (writer
      ? `${bodyLabel(selected)} → 写入「${dataLabel(type, shape)}」→ ${life}`
      : `${life} → 提供「${dataLabel(type, shape)}」→ ${bodyLabel(selected)} 读取`) : '请选择生命体、概念体和数据，再配置读写关系。';
    $('data-description').textContent = shape === 'state'
      ? '状态：当前值持续保留至替换。读取前须已创建状态存储；原始写入者固定，断开后只能由原来源重接。已有状态不要再次创建。'
      : shape === 'event' ? '事件：有期限的一次性消息，各读取者独立读取；发布不保证有人执行，过期不等于取消已经发生的行为。' : '';
    $('relation-state').textContent = r
      ? `快照：读取资格${r.reader_declared?'已声明':'未声明'}，读取${r.reader_registered?'已接通':'未接通'}；写入资格${r.writer_declared?'已声明':'未声明'}，写入${r.writer_registered?'已接通':'未接通'}。${r.immutable_origin?'当前概念体是固定状态来源。':''}批准状态：快照未提供。`
      : '快照未提供此接口的关系证据，不能据此判断批准状态或状态是否已创建。原生接口缺失时服务端会拒绝，不会自动安装。';
    $('relation-action').querySelector('[value="create_slot"]').disabled = shape !== 'state';
    if (shape !== 'state' && $('relation-action').value === 'create_slot') $('relation-action').value = 'declare_writer';
    $('relation-guide').replaceChildren();
    const writeAction = shape === 'state' && !r?.immutable_origin ? 'create_slot' : 'register_writer';
    const actions = writer ? ['declare_writer', 'approve_writer', writeAction] : ['declare_reader', 'approve_reader', 'register_reader'];
    actions.forEach((action, index) => {
      const step = document.createElement('section'); step.className = 'step';
      const title = document.createElement('h4'); title.textContent = `${index + 1}. ${actionInfo[action][0]}`;
      const note = document.createElement('p'); note.textContent = actionInfo[action][1];
      const button = document.createElement('button'); button.type = 'button'; button.textContent = `执行第 ${index + 1} 步`;
      button.disabled = !ready;
      button.addEventListener('click', () => run(() => applyRelation(action)));
      step.append(title, note, button); $('relation-guide').append(step);
    });
    actionDescription();
  }
  function actionDescription() {
    const action = $('relation-action').value;
    $('action-description').textContent = actionInfo[action]?.[1] || '';
  }
  async function applyRelation(action) {
    const body = selected, life = $('life').value, data_type = $('data-type').value;
    if (!body || !life || !data_type) throw new Error('请选择概念体、生命体和数据');
    await transport.request({relation:{body, life, data_type, action}});
    const result = `${bodyLabel(body)} · ${life} · ${dataNames[data_type] || data_type}：${actionInfo[action][0]}已成功。其他步骤未自动执行。`;
    try { await inspect(body); }
    catch (error) { throw new Error(`${result}\n但刷新快照失败：${error.message}。请手动刷新，不要据此重复已成功操作。`); }
    message(result);
  }
  $('refresh').addEventListener('click', () => run(() => list()));
  $('more').addEventListener('click', () => run(() => list(true)));
  $('inspect').addEventListener('click', () => selected && run(() => inspect(selected)));
  $('search').addEventListener('input', renderList);
  $('new-body').addEventListener('click', () => {
    if (busy) return;
    $('create-form').hidden = false; $('attributes-form').hidden = true; $('relation-editor').hidden = true; $('empty-selection').hidden = true;
    $('body-identity').hidden = true; $('inspect').disabled = true; $('selection').textContent = '创建概念体';
  });
  $('cancel-create').addEventListener('click', () => {
    $('create-form').hidden = true; $('inspect').disabled = !bodySnapshot;
    if (bodySnapshot) renderBody(bodySnapshot);
    else { $('selection').textContent = '未选择概念体'; $('empty-selection').hidden = false; }
  });
  for (let i = 0; i < names.length; i++) {
    const label = document.createElement('label'); label.textContent = labels[i];
    const input = inputFor(i < 4 ? 'integer' : 'unorm', 0); input.name = names[i]; label.append(input); $('create-fields').append(label);
  }
  $('create-form').addEventListener('invalid', event => {
    const details = event.target.closest('details'); if (details) details.open = true;
  }, true);
  $('create-form').addEventListener('submit', event => { event.preventDefault(); run(async () => {
    const values = {};
    for (let i = 0; i < names.length; i++) values[names[i]] = numericValue($('create-form').elements.namedItem(names[i]).value, i < 4 ? 'integer' : 'unorm');
    const reply = await transport.request({create_body:{values}});
    if (!reply.created?.body) throw new Error('创建回执未包含概念体选择器');
    await list(); await inspect(reply.created.body);
  }, '概念体已创建。可选择生命体，在下方逐步配置读写关系；基础属性不赋予角色或行为。'); });
  $('new-life').addEventListener('click', () => run(async () => {
    const reply = await transport.request('create_life');
    if (!reply.created?.life) throw new Error('创建回执未包含生命体选择器');
    const id = reply.created.life; await list();
    if (!lives.includes(id)) { lives.push(id); options($('life'), lives, value => value); }
    $('life').value = id; relationState();
    if (selected) await inspect(selected);
  }, '生命体已创建并选中：预设数据接入规则，但没有参与者或状态存储。请选择概念体配置协作；这不会自动生成可行动生物。'));
  $('relation-form').addEventListener('submit', event => { event.preventDefault(); run(() => applyRelation($('relation-action').value)); });
  $('life').addEventListener('change', relationState); $('data-type').addEventListener('change', relationState);
  $('relation-direction').addEventListener('change', relationState); $('relation-action').addEventListener('change', actionDescription);
  $('attributes-form').addEventListener('submit', event => event.preventDefault());
  renderList(); relationState();
  return {
    activate() { active = true; if (epoch && !busy) run(() => list()); },
    deactivate() { active = false; },
    destroy() { destroyed = true; active = false; transport.destroy(); }
  };
}
