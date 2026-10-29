// @vitest-environment jsdom
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import type { AIModelConfig, MediaConfig } from '../../types';
import { activateSurface } from '@/infrastructure/peer-device/deviceSurface';
import { ImageGenerationModelCard } from './ImageGenerationSection';
import { useImageGenerationSettings, type ImageGenerationSettings } from './useImageGenerationSettings';
import DefaultModelConfig from '../DefaultModelConfig';

globalThis.IS_REACT_ACT_ENVIRONMENT = true;
const mocks = vi.hoisted(() => ({ get: vi.fn(), getModels: vi.fn(), update: vi.fn(), success: vi.fn(), error: vi.fn(), test: vi.fn() }));
vi.mock('../../services/ConfigManager', () => ({ configManager: {
  hasCachedConfig: () => false, getCachedConfig: () => undefined,
  getConfig: mocks.get, getOptionalConfig: mocks.getModels, updateConfig: mocks.update, watch: () => () => {},
} }));
vi.mock('@/shared/notification-system', () => ({ useNotification: () => ({ success: mocks.success, error: mocks.error }), notificationService: { success: mocks.success, error: mocks.error } }));
vi.mock('../ModelSelectPresentation', () => ({ useModelSelectPresentation: () => ({ buildModelOption: (model: AIModelConfig) => ({ label: model.model_name, value: model.id }) }) }));
vi.mock('react-i18next', async original => ({ ...await original<typeof import('react-i18next')>(), useTranslation: () => { const t = (key: string) => key; return { t, i18n: { language: 'en' }, res: { t } }; } }));
vi.mock('@/infrastructure/api/service-api/MediaApi', () => ({ mediaAPI: { testImageGenerationService: mocks.test } }));
vi.mock('@/infrastructure/i18n', () => ({ useI18n: () => ({ t: (key: string) => key, formatNumber: String }) }));
vi.mock('@/infrastructure/confirm-dialog', () => ({ confirmDialog: vi.fn(async () => true) }));

const subscription: AIModelConfig = {
  id: 'configured-codex', name: 'My subscription', provider: 'openai', model_name: 'account-chat-model',
  base_url: 'https://chatgpt.com/backend-api/codex', api_key: 'unused-chat-secret', enabled: true,
  auth: { type: 'subscription', provider: 'codex' }, category: 'general_chat', capabilities: ['text_chat'],
};
const apiImageModel: AIModelConfig = {
  id: 'gateway-image', name: 'Image gateway', provider: 'openai', model_name: 'my-image-model',
  base_url: 'https://images.example/v1', request_url: 'https://images.example/v1/chat/completions',
  api_key: 'image-secret', auth: { type: 'api_key' }, enabled: true,
  category: 'image_generation', capabilities: ['image_generation'],
};
const legacy: MediaConfig = { image_services: [{ id: 'legacy', name: 'Saved image', dialect: 'openai_images',
  base_url: 'https://images.example/v1', request_url: 'https://images.example/v1/images/generations',
  model_name: 'image-model', api_key: 'saved-image-key', enabled: true, params: { size: '1024x1024', n: 1, seed: 42 },
  custom_headers: { 'X-Tenant': 'kept' }, auth: { type: 'api_key' } }], default_image_service: 'legacy' };

describe('image models in model settings', () => {
  let host: HTMLDivElement;
  let root: Root;
  let media: MediaConfig;
  let settings: ImageGenerationSettings;
  function Fixture({ models = [subscription] }: { models?: AIModelConfig[] }) {
    settings = useImageGenerationSettings();
    return <>
      <DefaultModelConfig compact models={models} defaultModels={{}} loading={false} disabled={false}
        onDefaultModelsChange={() => {}} />
      {settings.media.image_services.map(service => <ImageGenerationModelCard key={service.id} settings={settings} service={service}
        actions={<button type="button" data-testid="legacy-remove" onClick={() => void settings.remove(service)}>remove</button>} />)}
    </>;
  }
  const render = async (models?: AIModelConfig[]) => act(async () => {
    if (models) mocks.getModels.mockResolvedValue(models);
    root.render(<Fixture models={models} />);
  });
  const click = async (element: HTMLElement) => act(async () => { element.focus(); element.click(); });
  const choose = async (testId: string, label: string) => {
    const control = document.querySelector<HTMLElement>(`[data-testid="${testId}"]`)!;
    await click(control.matches('button') ? control : control.querySelector<HTMLElement>('button[data-openbitfun-part="trigger"]')!);
    const option = [...document.querySelectorAll<HTMLElement>('[role="option"]')].find(element => element.textContent?.includes(label));
    expect(option, `option ${label}`).toBeTruthy(); await click(option!);
  };

  beforeEach(() => {
    activateSurface('local');
    media = JSON.parse(JSON.stringify(legacy));
    mocks.get.mockReset().mockImplementation(async () => media);
    mocks.getModels.mockReset().mockResolvedValue([subscription]);
    mocks.update.mockReset().mockImplementation(async (_path, update) => { media = update(media); return media; });
    mocks.success.mockReset(); mocks.error.mockReset(); mocks.test.mockReset();
    host = document.createElement('div'); document.body.append(host); root = createRoot(host);
  });
  afterEach(() => { act(() => root.unmount()); host.remove(); activateSurface('local'); });

  it('keeps a consistent legacy default without rewriting it on load', async () => {
    await render();
    expect(settings.selectedId).toBe('legacy');
    expect(mocks.update).not.toHaveBeenCalled();
  });

  it('persists the first enabled legacy service when no default is stored', async () => {
    media = { image_services: [{ ...legacy.image_services[0], id: 'only-service' }] };
    await render([]);
    expect(media.default_image_service).toBe('only-service');
    expect(settings.selectedId).toBe('only-service');
    expect(mocks.update).toHaveBeenCalledTimes(1);
    expect(mocks.success).not.toHaveBeenCalled();
  });

  it('selects the image slot from provider-attached models and writes ai.default_models', async () => {
    await render([subscription, apiImageModel]);
    await choose('image-service-default-select', 'my-image-model');
    expect(mocks.update).toHaveBeenCalledTimes(1);
    expect(mocks.update.mock.calls[0][0]).toBe('ai.default_models');
  });

  it('derives the image service from the model provider connection when selected as configured default', async () => {
    await render([subscription, apiImageModel]);
    await act(async () => { await settings.selectDefault('configured:gateway-image'); });
    const mirrored = media.image_services.find(service => service.id === 'configured-image:gateway-image');
    expect(mirrored).toMatchObject({
      dialect: 'openai_images', base_url: 'https://images.example/v1', model_name: 'my-image-model',
      api_key: 'image-secret', enabled: true,
    });
    // The chat request URL must not leak into the image service override.
    expect(mirrored?.request_url).toBeUndefined();
    expect(media.default_image_service).toBe('configured-image:gateway-image');
  });

  it('excludes plain chat models from the image import candidates', async () => {
    await render([subscription, apiImageModel]);
    expect(settings.configuredModels.map(model => model.id)).toContain('gateway-image');
    await act(async () => { await settings.selectDefault('configured:configured-codex'); });
    expect(media.image_services.at(-1)).toMatchObject({ dialect: 'chatgpt_codex', api_key: '', auth: subscription.auth });
  });

  it('lists codex subscription models and api-key image models as configured candidates', async () => {
    await render([subscription, apiImageModel]);
    const candidates = settings.configuredModels.map(model => model.id);
    expect(candidates).toEqual(['configured-codex', 'gateway-image']);
  });

  it('reuses an existing equivalent mirror when its configured model is selected', async () => {
    const { imageServiceFromSubscriptionModel } = await import('./imageModels');
    const mirrored = { ...imageServiceFromSubscriptionModel(apiImageModel), id: 'manually-imported', enabled: false };
    media = { ...legacy, image_services: [...legacy.image_services, mirrored] };
    await render([subscription, apiImageModel]);
    await act(async () => { await settings.selectDefault('configured:gateway-image'); });
    expect(media.default_image_service).toBe('manually-imported');
    expect(media.image_services).toHaveLength(2);
    expect(media.image_services.find(service => service.id === mirrored.id)).toMatchObject({ enabled: true });
  });

  it('lets a legacy card be removed and keeps its parameters for other services', async () => {
    await render([]);
    const removeButton = host.querySelector<HTMLButtonElement>('[data-testid="legacy-remove"]')!;
    await click(removeButton);
    expect(media.image_services).toEqual([]);
    expect(media.default_image_service).toBeUndefined();
  });

  it('resets the list and closes drafts while the next execution host loads', async () => {
    await render();
    const pending = new Promise<MediaConfig>(() => {});
    mocks.get.mockImplementation(async () => pending);
    await act(async () => { activateSurface('peer-device'); });
    expect(settings.media.image_services).toEqual([]);
    expect(host.textContent).not.toContain('Saved image');
  });

  it('does not apply an old host save result or success notification after activation changes', async () => {
    await render();
    const oldSave = (() => { let resolve!: (value: MediaConfig) => void; const promise = new Promise<MediaConfig>(done => { resolve = done; }); return { promise, resolve }; })();
    mocks.update.mockReturnValueOnce(oldSave.promise);
    let save!: Promise<void>;
    await act(async () => { save = settings.selectDefault('configured:gateway-image'); });
    mocks.get.mockResolvedValue({ image_services: [] });
    await act(async () => { activateSurface('next-host'); });
    await act(async () => { oldSave.resolve(legacy); await save; });
    expect(settings.media.image_services).toEqual([]);
    expect(mocks.success).not.toHaveBeenCalled();
  });

  it('does not offer previous device credentials while the next device model list loads', async () => {
    await render();
    const nextModels = (() => { let resolve!: (value: AIModelConfig[]) => void; const promise = new Promise<AIModelConfig[]>(done => { resolve = done; }); return { promise, resolve }; })();
    mocks.getModels.mockReturnValueOnce(nextModels.promise);
    mocks.get.mockResolvedValue({ image_services: [] });
    await act(async () => { activateSurface('next-host'); });
    expect(settings.credentials).toEqual([]);
    await act(async () => { await settings.selectDefault('configured:gateway-image'); });
    expect(mocks.update).not.toHaveBeenCalled();
    await act(async () => { nextModels.resolve([]); });
    expect(settings.configuredModels).toEqual([]);
  });
});
