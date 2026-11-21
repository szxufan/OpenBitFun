import { SceneVisibilityPage } from './SceneVisibilityPage.mjs';
import { writeFile } from 'node:fs/promises';

export class AcpQueueRecoveryPage extends SceneVisibilityPage {
  async selectSession(id) {
    await this.click('[data-testid="nav-session-item"][data-session-id="' + id + '"]');
    await this.until(() => this.execute(id => [...document.querySelectorAll('[role="tab"][aria-selected="true"]')]
      .some(node => node.dataset.openbitfunValue?.includes(id)), id), 'selected ' + id, 6000);
  }
  async modelReady() {
    await this.until(() => this.execute(() => document.querySelector('[data-testid="chat-model-selector-btn"]')
      ?.innerText.includes('Controlled')), 'ACP model ready', 8000);
  }
  async sendCase(token) {
    await this.enterDraft('ACP_CASE:' + token);
    await this.until(() => this.execute(() => {
      const button = document.querySelector('[data-testid="chat-input-send-btn"]');
      return button && !button.disabled;
    }), 'send enabled', 6000);
    await this.click('[data-testid="chat-input-send-btn"]');
    await this.until(() => this.execute(() => {
      const input = document.querySelector('[data-testid="chat-input-textarea"]');
      return input && !input.textContent.trim();
    }), 'composer cleared after sending ' + token, 6000);
  }
  async promoteQueuedCase(index) {
    const row = '[data-testid="pending-queue-panel"] [data-openbitfun-product-part="item"]';
    const label = await this.execute(({ row, index }) => {
      const item = document.querySelectorAll(row)[index];
      const button = item?.querySelector('[data-openbitfun-product-part="actions"] button');
      if (!button) throw new Error('Missing queued-message send action');
      return button.getAttribute('aria-label');
    }, { row, index });
    await this.click(`${row}:nth-child(${index + 1}) button[aria-label=${JSON.stringify(label)}]`);
  }
  async refreshDocument() {
    const token = await this.execute(() => window.__acpDiagnosticDocument = Math.random().toString());
    await this.request('/session/' + this.session + '/refresh', {});
    await this.until(() => this.execute(token => window.__acpDiagnosticDocument !== token &&
      !![...document.querySelectorAll('[data-testid="nav-session-item"]')]
        .find(node => node.checkVisibility({ checkVisibilityCSS: true })), token)
      .catch(() => false), 'new document ready', 10000);
  }
  async shot(path) {
    const base64 = await this.request('/session/' + this.session + '/screenshot');
    await writeFile(path, Buffer.from(base64, 'base64'));
  }
}
