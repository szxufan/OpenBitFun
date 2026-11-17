/** @vitest-environment jsdom */
import { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { ResponsesRequestSettingsFields } from './ResponsesRequestSettingsFields';

vi.mock('@/infrastructure/i18n', () => ({ useI18n: () => ({ t: (key: string) => key }) }));
globalThis.IS_REACT_ACT_ENVIRONMENT = true;

describe('Responses connection preferences', () => {
  let root: Root;
  let container: HTMLDivElement;
  const onChange = vi.fn();
  const onReset = vi.fn();
  const select = (name: string) => container.querySelector<HTMLButtonElement>(
    `[role="combobox"][aria-label="advancedSettings.responses.${name}Label"]`,
  )!;

  beforeEach(() => {
    vi.clearAllMocks();
    container = document.createElement('div');
    document.body.appendChild(container);
    root = createRoot(container);
  });
  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
  });

  it('defaults to HTTP/full and disables incremental selection', async () => {
    await act(async () => root.render(<ResponsesRequestSettingsFields onChange={onChange} />));
    expect(select('transport').textContent).toContain('HTTP');
    expect(select('context').disabled).toBe(true);
    expect(select('context').textContent).toContain('advancedSettings.responses.full');
    expect(onChange).not.toHaveBeenCalled();
  });

  it('switching back to HTTP explicitly clears incremental context', async () => {
    await act(async () => root.render(<ResponsesRequestSettingsFields
      value={{ transport: 'websocket', context: 'incremental' }} onChange={onChange} />));
    expect(select('context').disabled).toBe(false);
    await act(async () => select('transport').click());
    const options = [...document.body.querySelectorAll<HTMLElement>('[role="option"]')];
    expect(options.map(option => option.textContent)).toEqual(['HTTP', 'WebSocket']);
    await act(async () => options.find(option => option.textContent === 'HTTP')!.click());
    expect(onChange).toHaveBeenCalledWith({ transport: 'http', context: 'full' });
  });

  it('gates unsupported peers and exposes an explicit inheritance reset', async () => {
    await act(async () => root.render(<ResponsesRequestSettingsFields supported={false}
      value={{ transport: 'websocket', context: 'full' }} onChange={onChange} onReset={onReset} />));
    expect(select('transport').disabled).toBe(true);
    expect(select('context').disabled).toBe(true);
    expect(container.textContent).toContain('advancedSettings.responses.unsupported');
    const reset = () => [...container.querySelectorAll('button')].find(button => button.textContent === 'advancedSettings.responses.reset')!;
    expect(reset().disabled).toBe(true);
    await act(async () => root.render(<ResponsesRequestSettingsFields onChange={onChange} onReset={onReset} />));
    await act(async () => reset().click());
    expect(onReset).toHaveBeenCalledOnce();
    expect(onChange).not.toHaveBeenCalled();
  });
});
