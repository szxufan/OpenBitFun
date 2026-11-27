// @vitest-environment jsdom
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { activateSurface } from '@/infrastructure/peer-device/deviceSurface';
import type { AIModelConfig } from '@/infrastructure/config/types';
import BtwSettingsSection from './BtwSettingsSection';
import { BTW_CLOSE_CONFIRMATION_STORAGE_KEY, shouldConfirmBtwClose } from '@/infrastructure/preferences/btwCloseConfirmation';

globalThis.IS_REACT_ACT_ENVIRONMENT = true;
const mocks = vi.hoisted(() => ({ supported: true, get: vi.fn(), update: vi.fn(), list: vi.fn() }));
vi.mock('@/infrastructure/config/services/btwModelDefaults', async original => ({
  ...await original<typeof import('@/infrastructure/config/services/btwModelDefaults')>(),
  supportsBtwModelDefaults: () => mocks.supported,
}));
vi.mock('@/infrastructure/config/services/ConfigManager', () => ({ configManager: {
  getConfig: mocks.get, getOptionalConfig: async () => models, updateConfig: mocks.update, watch: () => () => {},
} }));
vi.mock('@/infrastructure/api/service-api/ModelPoolAPI', async original => ({
  ...await original<typeof import('@/infrastructure/api/service-api/ModelPoolAPI')>(),
  supportsBtwModelPools: () => true, modelPoolAPI: { list: mocks.list },
}));
vi.mock('react-i18next', async original => ({
  ...await original<typeof import('react-i18next')>(), useTranslation: () => ({ t: (key: string) => key }),
}));
vi.mock('@openbitfun/ui', async original => ({
  ...await original<typeof import('@openbitfun/ui')>(),
  Combobox: ({ value, options, disabled, onValueChange, 'aria-label': label }: {
    value: string; disabled: boolean; options: { value: string; label: string; disabled?: boolean }[];
    onValueChange: (value: string) => void; 'aria-label': string;
  }) => <select aria-label={label} value={value} disabled={disabled} onChange={event => onValueChange(event.target.value)}>
    {options.map(option => <option key={option.value} value={option.value} disabled={option.disabled}>{option.label}</option>)}
  </select>,
}));

const models: AIModelConfig[] = [
  { id: 'alpha', name: 'Alpha', model_name: 'alpha', provider: 'openai', base_url: 'https://example.test', enabled: true, capabilities: ['text_chat'] },
  { id: 'disabled', name: 'Disabled', model_name: 'disabled', provider: 'openai', base_url: 'https://example.test', enabled: false, capabilities: ['text_chat'] },
  { id: 'audio', name: 'Audio', model_name: 'audio', provider: 'openai', base_url: 'https://example.test', enabled: true, capabilities: ['speech_recognition'] },
];

describe('BTW settings', () => {
  let root: Root;
  let host: HTMLDivElement;
  let config: unknown;
  const select = () => host.querySelector<HTMLSelectElement>('select')!;
  const render = () => act(async () => { root.render(<BtwSettingsSection />); });
  const choose = (value: string) => act(async () => {
    select().value = value;
    select().dispatchEvent(new Event('change', { bubbles: true }));
  });
  beforeEach(() => {
    activateSurface('local'); mocks.supported = true;
    localStorage.removeItem(BTW_CLOSE_CONFIRMATION_STORAGE_KEY);
    config = { model_selection: { kind: 'inherit' }, extension: 'keep' };
    mocks.get.mockReset().mockImplementation(async () => config);
    mocks.update.mockReset().mockImplementation(async (_path, update) => { config = update(config); return config; });
    mocks.list.mockReset().mockResolvedValue({ version: 1, pools: [
      { id: 'pool', name: 'Side pool', strategy: 'balanced', member_model_ids: ['alpha'] },
    ] });
    host = document.createElement('div'); document.body.append(host); root = createRoot(host);
  });
  afterEach(() => { act(() => root.unmount()); host.remove(); activateSurface('local'); });

  it('groups the host model and existing controller close preference under one side-question section', async () => {
    localStorage.setItem(BTW_CLOSE_CONFIRMATION_STORAGE_KEY, 'true');
    await render();
    const section = host.querySelector('#settings-section-btw')!;
    const toggle = section.querySelector<HTMLInputElement>('[role="switch"]')!;
    expect(section.contains(select())).toBe(true);
    expect(toggle.checked).toBe(false);
    await act(async () => toggle.click());
    expect(shouldConfirmBtwClose()).toBe(true);
    expect(mocks.update).not.toHaveBeenCalled();
  });

  it('defaults to inheritance, filters models and saves a concrete model or pool without dropping extensions', async () => {
    await render();
    expect(select().value).toBe('inherit');
    expect([...select().options].map(option => option.value)).toEqual(['inherit', 'model:alpha', 'pool:pool']);
    await choose('model:alpha');
    expect(config).toEqual({ model_selection: { kind: 'model', model_ref: 'alpha' }, extension: 'keep' });
    await choose('pool:pool');
    expect(config).toEqual({ model_selection: { kind: 'pool', pool_id: 'pool' }, extension: 'keep' });
    await choose('inherit');
    expect(config).toEqual({ model_selection: { kind: 'inherit' }, extension: 'keep' });
  });

  it.each([
    { model_selection: { kind: 'model', model_ref: 'deleted' } },
    { model_selection: { kind: 'future' } },
    null,
  ])('keeps unavailable saved choices visible and allows an explicit repair: %j', async value => {
    config = value;
    await render();
    expect(select().selectedOptions[0].disabled).toBe(true);
    expect(mocks.update).not.toHaveBeenCalled();
    await choose('inherit');
    expect(config).toEqual({ model_selection: { kind: 'inherit' } });
  });

  it('locks failed reads and retains the saved choice after failed writes', async () => {
    mocks.get.mockRejectedValueOnce(new Error('offline'));
    await render();
    expect(select().disabled).toBe(true);
    expect(host.textContent).toContain('btwModel.failed');
    await act(async () => host.querySelector<HTMLButtonElement>('button')!.click());
    expect(select().disabled).toBe(false);
    mocks.update.mockRejectedValueOnce(new Error('write failed'));
    await choose('model:alpha');
    expect(config).toEqual({ model_selection: { kind: 'inherit' }, extension: 'keep' });
    expect(select().value).toBe('inherit');
  });

  it('does not read or write the new setting on older hosts', async () => {
    mocks.supported = false;
    await render();
    expect(host.textContent).toContain('btwModel.unsupported');
    expect(mocks.get).not.toHaveBeenCalled();
    expect(mocks.update).not.toHaveBeenCalled();
    const toggle = host.querySelector<HTMLInputElement>('[role="switch"]')!;
    expect(toggle.disabled).toBe(false);
    await act(async () => toggle.click());
    expect(shouldConfirmBtwClose()).toBe(false);
  });

  it('ignores a departed host response after a surface switch', async () => {
    let resolve!: (value: unknown) => void;
    mocks.get.mockImplementationOnce(() => new Promise(done => { resolve = done; }));
    await render();
    config = { model_selection: { kind: 'pool', pool_id: 'pool' } };
    await act(async () => { activateSurface('peer-device'); });
    expect(select().value).toBe('pool:pool');
    await act(async () => { resolve({ model_selection: { kind: 'model', model_ref: 'alpha' } }); });
    expect(select().value).toBe('pool:pool');
  });
});
