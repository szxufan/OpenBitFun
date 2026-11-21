// Read-only diagnostics of production singleton exports. No product state is injected.
export async function installObserver(input) {
  const { ids, modules } = input;
  if (window.__acpTraceInstalled) return;
  const f = await import(modules.manager);
  const r = await import(modules.state);
  const g = await import(modules.gate);
  const manager = Object.values(f).find(v => typeof v === 'function' && v.getInstance && v.prototype.sendMessage);
  const machines = Object.values(r).find(v => v && typeof v === 'object' && v.getCurrentState && v.getOrCreate);
  const c = manager.getInstance().context;
  const trace = [];
  window.__acpTrace = trace;
  const push = value => {
    trace.push({ at: Date.now(), ...value });
    if (trace.length > 12000) trace.shift();
  };
  const take = id => {
    const s = c.flowChatStore.getState().sessions.get(id);
    const mc = machines.get(id);
    const t = s?.dialogTurns.at(-1);
    return {
      id, state: mc?.getCurrentState(), version: mc?.getContext().version,
      currentTurn: mc?.getContext().currentDialogTurnId,
      gap: g[modules.gap]('local', id), attach: g[modules.attach]('local', id),
      cancelGate: c.userCancelledSessionIds.has(id), history: s?.historyState,
      lastTurnId: t?.id, lastTurnStatus: t?.status,
      rounds: t?.modelRounds?.map(x => ({ id: x.id, status: x.status,
        items: x.items?.map(n => ({ id: n.id, type: n.type })) })),
      queue: Object.keys(localStorage)
        .filter(k => k.startsWith('openbitfun.flowChat.pendingQueue.v1.') && k.includes(encodeURIComponent(id)))
        .map(k => JSON.parse(localStorage.getItem(k)).map(x => ({
          id: x.id, content: x.content, status: x.status, retry: x.retryCount,
        }))),
    };
  };
  g[modules.subscribeGap]((surface, id) => push({ type: 'gap', surface, id,
    stack: new Error('gap origin').stack, snapshot: take(id) }));
  machines.subscribeGlobal((id) => {
    if (ids.includes(id)) push({ type: 'machine', id, snapshot: take(id) });
  });
  let prev = '';
  setInterval(() => {
    const values = ids.map(take);
    const signature = JSON.stringify(values);
    if (signature !== prev) {
      prev = signature;
      push({ type: 'poll', sessions: values });
    }
  }, 80);
  window.__acpDiagnosticSnapshot = () => ids.map(take);
  window.__acpTraceInstalled = true;
  return { installed: true };
}
