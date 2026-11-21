// Runs inside the opaque visualization frame, before the authored fragment.
// This interface only carries presentation events. It has no tool dispatcher.
(() => {
  const host = window.parent;
  const ownApi = window.openai;
  // Some saved figures write privateContent, then read its fields directly from
  // widgetState. Give those readers a local view without changing serialized
  // snapshots or which fields the host can pass to a follow-up.
  function exposePrivateFields(state) {
    const fields = state?.privateContent;
    if (!fields || typeof fields !== 'object' || Array.isArray(fields) || !Object.isExtensible(state)) return;
    for (const name of Object.keys(fields)) {
      if (name in state || name === 'imageIds' || name === 'prototype') continue;
      Object.defineProperty(state, name, { configurable: true, get: () => fields[name] });
    }
  }
  exposePrivateFields(ownApi?.widgetState);
  addEventListener(
    'openai:set_globals',
    (event) => exposePrivateFields(event.detail?.globals?.widgetState),
    true,
  );
  const send = (message, ports = []) => host.postMessage({ openBitFunVisual: 2, ...message }, '*', ports);
  const data = document.getElementById('openbitfun-visual-data');
  const { declarations, values, figureStyle } = JSON.parse(data.textContent);
  let previousStyle = {};
  function applyAppearance(next) {
    const root = document.documentElement;
    let changed = false;
    for (const name of Object.keys(previousStyle)) {
      if (!(name in next)) { root.style.removeProperty(name); changed = true; }
    }
    for (const [name, value] of Object.entries(next)) {
      if (previousStyle[name] !== value) { root.style.setProperty(name, value); changed = true; }
    }
    root.dataset.theme = next['color-scheme'];
    Object.assign(window.openai, {
      theme: root.dataset.theme, visualizationTheme: root.dataset.theme, visualizationStyleVariables: next,
    });
    previousStyle = next;
    if (changed) dispatchEvent(new Event('openbitfun:figure-appearance'));
  }
  applyAppearance(figureStyle);
  data.remove();
  const sources = new Map();
  const listeners = new Map();
  const api = {
    bindings: Object.freeze(declarations),
    onData(name, callback) {
      if (typeof callback !== 'function') throw new TypeError('onData requires a callback');
      if (!Object.hasOwn(declarations, name)) return () => {};
      if (!listeners.has(name)) listeners.set(name, new Set());
      listeners.get(name).add(callback);
      callback(sources.get(name)?.value);
      return () => listeners.get(name)?.delete(callback);
    },
  };
  function applyValues(changed) {
    const updates = [];
    for (const [name, record] of Object.entries(changed)) {
      if (!Object.hasOwn(declarations, name)) continue;
      const previous = sources.get(name);
      const serialized = JSON.stringify(record);
      if (serialized === previous?.serialized) continue;
      let value = record.value;
      let blobUrl;
      if (record.kind === 'file') {
        const bytes =
          typeof value.base64 === 'string'
            ? Uint8Array.from(atob(value.base64), (c) => c.charCodeAt(0))
            : value.text;
        const blob = new Blob([bytes], { type: value.mimeType });
        blobUrl = URL.createObjectURL(blob);
        value = { name: value.name, blob, url: blobUrl };
      }
      const revision = (previous?.revision ?? 0) + 1;
      sources.set(name, { value, revision, blobUrl, serialized });
      updates.push({ name, revision, state: 'ready', value: { type: record.kind, value } });
      for (const callback of listeners.get(name) ?? []) {
        try {
          callback(value);
        } catch (error) {
          globalThis.reportError(error);
        }
      }
      if (previous?.blobUrl) URL.revokeObjectURL(previous.blobUrl);
    }
    if (!updates.length) return;
    window.openai.visualizationData = { updates };
    dispatchEvent(
      new CustomEvent('openai:set_globals', { detail: { globals: { visualizationData: { updates } } } }),
    );
  }
  ownApi.visualization = api;
  applyValues(values);
  ownApi.callTool = (name, args) => {
    if (name === '__codex_inline_visualization_ready__') return Promise.resolve({});
    if (name === '__codex_dashboard_layout_availability__') {
      send({ type: 'layout', available: args?.available === true });
      return Promise.resolve({});
    }
    if (
      name === '__codex_dashboard_source_inspection__' ||
      name === '__codex_dashboard_source_tab_labels__'
    ) {
      send({ type: name.endsWith('labels__') ? 'source-labels' : 'source-inspect', args });
      return Promise.resolve({});
    }
    if (name !== '__codex_visualization_annotations__') {
      return Promise.reject(new Error('This host does not provide that connector.'));
    }
    return new Promise((resolve, reject) => {
      const channel = new MessageChannel();
      const timeout = setTimeout(() => {
        channel.port1.close();
        reject(new Error('Visualization controls unavailable'));
      }, 5000);
      channel.port1.onmessage = (event) => {
        clearTimeout(timeout);
        channel.port1.close();
        resolve(event.data);
      };
      send({ type: 'annotation', args }, [channel.port2]);
    });
  };
  addEventListener(
    'message',
    (event) => {
      if (event.source !== host || event.data?.openBitFunVisual !== 2) return;
      if (event.data.type !== 'attach' || !event.ports[0]) return;
      event.ports[0].onmessage = ({ data: message }) => {
        if (message.values) applyValues(message.values);
        if (message.figureStyle) applyAppearance(message.figureStyle);
        const globals = message.globals;
        if (!globals) return;
        if (globals.theme) document.documentElement.dataset.theme = globals.theme;
        Object.assign(window.openai, globals);
        dispatchEvent(new CustomEvent('openai:set_globals', { detail: { globals } }));
      };
    },
    { once: false },
  );
  // Module loads may fail after the conversation itself has finished loading.
  addEventListener(
    'error',
    (event) => {
      if (event.target instanceof HTMLLinkElement && ['preload', 'modulepreload'].includes(event.target.rel))
        return;
      if (
        event.target instanceof HTMLScriptElement ||
        event.target instanceof HTMLLinkElement ||
        event instanceof ErrorEvent
      ) {
        send({ type: 'resource-error' });
      }
    },
    true,
  );
  addEventListener('unhandledrejection', () => send({ type: 'resource-error' }));
  // The product's mount and attach do not depend on third-party CDN load events.
  send({ type: 'ready' });
})();
