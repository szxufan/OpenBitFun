// Presentation-only bridge inside the opaque frame. Native capabilities stay in the parent.
(() => {
  const initial = document.getElementById('codex-visualization-widget-state');
  const pending = new Map();
  let sequence = 0;
  let port;
  const queued = [];
  function send(message) {
    if (port) port.postMessage(message);
    else queued.push(message);
  }
  const api = {
    widgetState: JSON.parse(initial.textContent).widgetState,
    setWidgetState(state) {
      const serialized = JSON.stringify(state);
      const snapshot = JSON.parse(serialized);
      const id = ++sequence;
      return new Promise((resolve, reject) => {
        pending.set(id, { resolve, reject, snapshot });
        send({ type: 'widget-state-write', id, state: serialized });
      });
    },
    sendFollowUpMessage({ prompt, title }) { send({ type: 'follow-up', prompt, title }); },
    openExternal({ href }) { send({ type: 'open-external', href }); },
    notifyIntrinsicHeight(height) { send({ type: 'height', height }); },
  };
  window.openai = api;
  addEventListener('message', (event) => {
    if (port || event.source !== parent || event.data?.type !== 'codex-visualization-initialize' || !event.ports[0]) return;
    port = event.ports[0];
    port.onmessage = ({ data }) => {
      if (data?.type !== 'widget-state-result') return;
      const request = pending.get(data.id);
      if (!request) return;
      pending.delete(data.id);
      if (!data.ok) { request.reject(Error('Figure state could not be saved')); return; }
      api.widgetState = request.snapshot;
      dispatchEvent(new CustomEvent('openai:set_globals', { detail: { globals: { widgetState: api.widgetState } } }));
      request.resolve();
    };
    queued.splice(0).forEach(message => port.postMessage(message));
    const report = () => api.notifyIntrinsicHeight(document.body.getBoundingClientRect().height);
    new ResizeObserver(report).observe(document.body);
    report();
  });
  function tabsFor(tab) {
    return [...tab.closest('[role=tablist]').querySelectorAll('[role=tab]')];
  }
  function select(tab) {
    for (const item of tabsFor(tab)) {
      const panel = document.getElementById(item.getAttribute('aria-controls'));
      if (!panel) continue;
      panel.hidden = item !== tab;
      item.tabIndex = item === tab ? 0 : -1;
      item.setAttribute('aria-selected', String(item === tab));
      item.classList.toggle('active', item === tab);
    }
  }
  document.addEventListener('click', (event) => {
    const tab = event.target.closest?.('[role=tablist] [role=tab][aria-controls]');
    if (tab) select(tab);
  });
  document.addEventListener('keydown', (event) => {
    const tab = event.target.closest?.('[role=tablist] [role=tab][aria-controls]');
    if (!tab) return;
    const tabs = tabsFor(tab);
    const index = tabs.indexOf(tab);
    const next = { ArrowLeft: (index + tabs.length - 1) % tabs.length, ArrowRight: (index + 1) % tabs.length, Home: 0, End: tabs.length - 1 }[event.key];
    if (next === undefined) return;
    event.preventDefault();
    tabs[next].focus();
    tabs[next].click();
  });
  document.addEventListener('DOMContentLoaded', () => {
    window.lucide?.createIcons();
    for (const list of document.querySelectorAll('[role=tablist]')) {
      const tab = list.querySelector('[role=tab][aria-selected=true][aria-controls], [role=tab].active[aria-controls]') ?? list.querySelector('[role=tab][aria-controls]');
      if (tab) select(tab);
    }
  });
})();
