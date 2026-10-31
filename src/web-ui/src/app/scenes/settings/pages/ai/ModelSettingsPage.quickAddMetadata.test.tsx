// @vitest-environment jsdom

import React, { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { aiApi, systemAPI } from '@/infrastructure/api';
import { agentAPI } from '@/infrastructure/api/service-api/AgentAPI';
import { configManager } from '@/infrastructure/config/services/ConfigManager';
import { i18nService } from '@/infrastructure/i18n';
import { activateSurface } from '@/infrastructure/peer-device/deviceSurface';
import ModelSettingsPage from './ModelSettingsPage';

globalThis.IS_REACT_ACT_ENVIRONMENT = true;

const remoteModels = [
  {
    id: 'glm-5.3',
    display_name: undefined,
    context_length: 1048576,
    max_output_tokens: 131072,
    reasoning_options: ['low', 'high', 'max'],
    supported_endpoint_types: ['openai'],
    support_image_input: true,
    support_video_input: true,
  },
  {
    id: 'plain-model',
    supported_endpoint_types: ['openai'],
  },
];

describe('quick-add remote metadata prefill', () => {
  let container: HTMLDivElement;
  let root: Root;
  const copy = (key: string) => String(i18nService.getI18nInstance().t(key, { ns: 'settings/models' }));
  const button = (key: string) => [...container.querySelectorAll<HTMLButtonElement>('button')]
    .find(element => element.textContent === copy(key))!;

  beforeAll(async () => {
    await Promise.all((['settings', 'settings/models', 'settings/default-model', 'settings/media', 'components'] as const).map(
      namespace => i18nService.loadNamespace(namespace),
    ));
  });

  beforeEach(() => {
    activateSurface('local');
    configManager.clearCache();
    const values: Record<string, unknown> = {
      'ai.models': [], 'ai.default_models': {},
      'ai.proxy': { enabled: false, url: '', username: '', password: '' },
      'ai.stream_idle_timeout_secs': 120, 'ai.stream_ttft_timeout_secs': 60,
    };
    vi.spyOn(configManager, 'getOptionalConfig').mockImplementation(
      <T,>(path: string) => Promise.resolve(values[path] as T),
    );
    vi.spyOn(configManager, 'getConfig').mockImplementation(
      <T,>(path?: string) => Promise.resolve(values[path!] as T),
    );
    vi.spyOn(aiApi, 'getModelCatalog').mockImplementation(() => new Promise(() => {}));
    vi.spyOn(aiApi, 'getLocalModelsDevCatalogs').mockImplementation(() => new Promise(() => {}));
    vi.spyOn(aiApi, 'getModelsDevCatalogStatus').mockImplementation(() => new Promise(() => {}));
    vi.spyOn(aiApi, 'listSubscriptionAccounts').mockResolvedValue([]);
    vi.spyOn(aiApi, 'onModelCatalogUpdated').mockReturnValue(() => {});
    vi.spyOn(agentAPI, 'getModelRoutingStatus').mockResolvedValue({ apiVersion: 1, available: true,
      mode: 'manual', defaultMode: 'manual', ready: false, issue: null, connected: null, summaryEnabled: true });
    vi.spyOn(aiApi, 'listModelsByConfig').mockResolvedValue(remoteModels);
    vi.spyOn(aiApi, 'testAIConfigConnection').mockImplementation(() => new Promise(() => {}));
    container = document.createElement('div');
    document.body.append(container);
    root = createRoot(container);
  });

  afterEach(async () => {
    act(() => root.unmount());
    await i18nService.applyPersistedLanguage('en-US');
    container.remove();
    vi.restoreAllMocks();
    activateSurface('local');
  });

  const editor = () => document.querySelector<HTMLElement>(
    '.openbitfun-model-settings__editor-dialog:not([data-state="exiting"])',
  );
  const inputValue = async (input: HTMLInputElement, value: string) => act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!.call(input, value);
    input.dispatchEvent(new Event('input', { bubbles: true }));
  });

  it('prefills context, output cap, reasoning, and category from the models endpoint', async () => {
    await i18nService.applyPersistedLanguage('en-US');
    const update = vi.spyOn(configManager, 'updateConfig');
    await act(async () => root.render(<ModelSettingsPage />));
    await act(async () => button('actions.addModel').click());
    await act(async () => container.querySelector<HTMLButtonElement>('button[data-provider-id="custom"]')!.click());

    await inputValue(editor()!.querySelector<HTMLInputElement>('[data-testid="settings-model-provider-name-input"]')!, 'LJP');
    await inputValue(editor()!.querySelector<HTMLInputElement>('[data-testid="settings-model-base-url-input"]')!, 'https://ljp.example.test/v1');
    await inputValue(editor()!.querySelector<HTMLInputElement>('[data-testid="settings-model-api-key-input"]')!, 'fixture-key');

    await act(async () => editor()!.querySelector<HTMLButtonElement>('[data-testid="settings-model-select"]')!.click());
    await act(async () => {});

    const options = [...document.querySelectorAll<HTMLButtonElement>('[data-testid="settings-model-option"]')];
    expect(options.map(option => option.getAttribute('data-model-id'))).toEqual(['glm-5.3', 'plain-model']);
    // Advertised metadata renders on the row so users can see what the
    // provider's /models endpoint actually returned.
    expect(options[0].getAttribute('data-model-meta')).toContain('1M');
    expect(options[0].textContent).toContain('low/high/max');
    expect(options[1].getAttribute('data-model-meta')).toBeNull();
    await act(async () => options.find(option => option.getAttribute('data-model-id') === 'glm-5.3')!.click());
    const done = [...document.querySelectorAll<HTMLButtonElement>('[role="dialog"] button')]
      .find(element => element.textContent === copy('actions.done'))!;
    await act(async () => done.click());

    const save = editor()!.querySelector<HTMLButtonElement>('[data-testid="settings-model-save-btn"]')!;
    await act(async () => save.click());
    await act(async () => {});

    expect(update).toHaveBeenCalled();
    const saved = update.mock.calls[0][1]([] as never as never[]) as unknown as Array<Record<string, unknown>>;
    const glm = saved.at(-1)!;
    expect(glm.model_name).toBe('glm-5.3');
    expect(glm.context_window).toBe(1048576);
    expect(glm.max_tokens).toBe(131072);
    expect(glm.category).toBe('multimodal');
    const reasoning = glm.reasoning as { presets?: Array<{ id: string }>; default_preset?: string };
    expect(reasoning.presets?.map(preset => preset.id)).toEqual(['low', 'high', 'max']);
    expect(reasoning.default_preset).toBeUndefined();
  });
});
