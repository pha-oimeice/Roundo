'use strict';

// Shared diagnostic adapter: command admission is not native operation completion.
export function createDebugTransport(execute, subscribe, sessionChanged = () => {}) {
  let snapshot = {epoch:null, requests:[]};
  let generation = 0, destroyed = false;
  const pending = new Map();
  function rejectAll(message) {
    for (const waiter of pending.values()) { clearTimeout(waiter.timer); waiter.reject(new Error(message)); }
    pending.clear();
  }
  function settle(id, waiter) {
    const record = snapshot.requests.find(entry => entry.request_id === id);
    if (!record || record.result === null || record.result === undefined) return;
    pending.delete(id); clearTimeout(waiter.timer);
    if (Object.hasOwn(record.result, 'Ok')) waiter.resolve(record.result.Ok);
    else waiter.reject(new Error(`${record.result.Err.code}: ${record.result.Err.message}`));
  }
  const unsubscribe = subscribe('client.debug', next => {
    if (next.epoch !== snapshot.epoch) {
      generation++;
      rejectAll('连接会话已改变；已接受操作的结果可能未知，请重新读取状态');
      snapshot = next;
      sessionChanged(next.epoch);
    } else snapshot = next;
    for (const [id, waiter] of pending) settle(id, waiter);
  });
  return {
    async request(operation) {
      if (destroyed) throw new Error('调试界面已关闭');
      const original = generation;
      const result = await execute({version:1, command:'debug.request', arguments:{operation}});
      if (!result.ok) throw new Error(`${result.error.code}: ${result.error.message}`);
      if (generation !== original) throw new Error('连接会话已改变，操作结果未知，请刷新核实');
      const id = result.data.request_id;
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => {
          pending.delete(id);
          reject(new Error('诊断回执未送达；操作可能已执行，请刷新核实，勿自动重试'));
        }, 15000);
        const waiter = {resolve, reject, timer};
        pending.set(id, waiter);
        settle(id, waiter); // A receipt may precede the command-acceptance response.
      });
    },
    destroy() {
      if (destroyed) return;
      destroyed = true; generation++;
      rejectAll('调试界面已关闭'); unsubscribe();
    }
  };
}
