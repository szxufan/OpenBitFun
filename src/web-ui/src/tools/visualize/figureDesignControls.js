// Authored controls are registered with FlowChat. Their callback targets never leave this frame.
(() => {
  let nextId = 0;
  const groups = new Set();
  function syncVisibility() {
    for (const group of groups) group.publish();
  }
  new MutationObserver(syncVisibility).observe(document.body, {
    subtree: true, childList: true, attributes: true, attributeFilter: ['hidden', 'class', 'style'],
  });
  addEventListener('openai:set_globals', event => {
    const annotation = event.detail?.globals?.visualizationAnnotation;
    if (!annotation) return;
    for (const group of groups) {
      const edits = annotation.changes?.find(change => change.registrationId === group.id)?.annotationControlChanges ?? [];
      let changed = false;
      for (const { control, object, property } of group.entries) {
        const value = edits.find(edit => edit.callback === control.callback)?.value ?? control.currentValue;
        if (object[property] !== value) {
          object[property] = value;
          changed = true;
        }
      }
      if (changed) group.onChange?.();
    }
  });
  window.Tweak = class {
    constructor({ container, onChange } = {}) {
      this.container = typeof container === 'string' ? document.querySelector(container) : container ?? document.body;
      this.id = 'figure-controls-' + (++nextId);
      this.entries = [];
      this.onChange = onChange;
      groups.add(this);
    }
    add(object, property, options) {
      const control = { label: property, ...options, callback: this.id + '-' + this.entries.length, currentValue: object[property] };
      this.entries.push({ control, object, property });
      this.publish();
      return this;
    }
    addSlider(object, property, { min = 0, max = 100, step = 1, ...rest } = {}) {
      return this.add(object, property, { ...rest, type: 'range', min, max, step });
    }
    addToggle(object, property, options) { return this.add(object, property, { ...options, type: 'toggle' }); }
    addColorPicker(object, property, options) { return this.add(object, property, { ...options, type: 'color' }); }
    addSelect(object, property, { options, ...rest }) {
      const choices = options.map(choice => typeof choice === 'string' ? { label: choice, value: choice } : choice);
      return this.add(object, property, { ...rest, type: 'select', options: choices });
    }
    publish() {
      const el = this.container;
      const target = { id: this.id, label: el.getAttribute('aria-label') || el.id || document.title, visible: el.getClientRects().length > 0 };
      const controls = this.entries.map(entry => entry.control);
      const signature = JSON.stringify([target, controls]);
      if (this.last === signature) return;
      this.last = signature;
      window.openai.callTool('__codex_visualization_annotations__', {
        type: 'register', registrationId: this.id, targets: [target], annotationControls: { controls },
      }).catch(() => {}); // Background inspection has no interactive controls panel.
    }
    dispose() {
      groups.delete(this);
      window.openai.callTool('__codex_visualization_annotations__', { type: 'dispose', registrationId: this.id }).catch(() => {});
    }
  };
})();
