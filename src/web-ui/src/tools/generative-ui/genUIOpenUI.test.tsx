/** @vitest-environment jsdom */
import React, { act } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import GenUIOpenUIDocument from './genUIOpenUI';
import { zodForSchema } from './genUIOpenUISchema';
import GenUIApplication from './GenUIApplication';
import { genUIProduct } from './genUIProduct';
import { genUICatalog } from './genUIContract';
import { oiComponents } from './components/oiComponents';
import type { GenUIAppDefinition } from './genUITypes';

describe('OpenUI source adapter', () => {
  let container: HTMLDivElement;
  let root: Root;

  beforeEach(() => {
    container = document.createElement('div');
    document.body.appendChild(container);
    root = createRoot(container);
  });

  afterEach(async () => {
    await act(async () => root.unmount());
    container.remove();
  });

  it('accepts JSON numeric equivalents for boolean component props', () => {
    const booleanProp = zodForSchema({ type: 'boolean' });
    expect(booleanProp.parse(0)).toBe(0);
    expect(booleanProp.parse(1)).toBe(1);
    expect(booleanProp.parse(false)).toBe(false);
    expect(() => booleanProp.parse(2)).toThrow();
  });

  it('uses the selected product library for OpenUI functions', async () => {
    await act(async () => root.render(<GenUIOpenUIDocument
      source={{ kind: 'openui-lang', content: 'root = Col([total, sine])\ntotal = Text(@Sum([2, 3]))\nsine = Text(@Round(@Sin(1.5707963267948966), 2))' }}
      library={genUIProduct.libraries['openbitfun.composable.v1']}
    />));
    expect(container.textContent).toContain('5');
    expect(container.textContent).toContain('1');
  });

  it('keeps read-only completed documents out of streaming mode', async () => {
    await act(async () => root.render(<GenUIOpenUIDocument
      source={{ kind: 'openui-lang', content: 'root = Col([button, html])\nbutton = Button("Continue")\nhtml = HtmlView("<p>Ready</p>")' }}
      library={genUIProduct.libraries['openbitfun.composable.v1']}
    />));

    expect(container.querySelector<HTMLButtonElement>('[data-openbitfun-component="button"]')?.disabled).toBe(true);
    expect(container.querySelector('.gen-ui-oi-html')).not.toBeNull();
  });

  it('keeps completed statements mounted when a later statement is incomplete', async () => {
    const onError = vi.fn();
    await act(async () => root.render(<GenUIOpenUIDocument
      source={{ kind: 'openui-lang', content: 'root = Col([text])\ntext = Text("Already visible")\nbroken = Text(' }}
      library={genUIProduct.libraries['openbitfun.composable.v1']}
      isStreaming
      onError={onError}
    />));
    expect(container.textContent).toContain('Already visible');
    expect(container.querySelector('.gen-ui-oi-col')).not.toBeNull();
  });

  it('defers HtmlView execution until the OpenUI source stream completes', async () => {
    await act(async () => root.render(<GenUIOpenUIDocument
      source={{ kind: 'openui-lang', content: 'root = HtmlView("<script>window.bad = true</script><p>Ready</p>")' }}
      library={genUIProduct.libraries['openbitfun.composable.v1']}
      isStreaming
    />));
    expect(container.querySelector('.gen-ui-oi-html')).toBeNull();
    expect(container.querySelector('.gen-ui-oi-html-slot')).not.toBeNull();
  });

  it.each(['openui.v1', 'openbitfun.composable.v1'])('preserves the saved video without starting its player in %s', async libraryId => {
    await act(async () => root.render(<GenUIOpenUIDocument
      source={{ kind: 'openui-lang', content: 'root = YouTube("abcdefghijk", "Recorded video")' }}
      library={genUIProduct.libraries[libraryId]}
    />));
    expect(container.querySelector('a')?.href).toBe('https://www.youtube.com/watch?v=abcdefghijk');
    expect(container.textContent).toContain('Recorded video');
    expect(container.querySelector('iframe')).toBeNull();
  });

  it.each(['openui.v1', 'openbitfun.composable.v1'])('replays nested HTML as static content in %s', async libraryId => {
    await act(async () => root.render(<GenUIOpenUIDocument
      source={{ kind: 'openui-lang', content: 'root = HtmlView("<p>Recorded diagram</p><script>window.archived = true</script>", 360, "Diagram")' }}
      library={genUIProduct.libraries[libraryId]}
    />));
    const frame = container.querySelector('iframe');
    expect(frame?.getAttribute('sandbox')).toBe('');
    expect(frame?.srcdoc).toContain('Recorded diagram');
    expect(frame?.srcdoc).toContain("script-src 'none'");
    expect(frame?.srcdoc).not.toContain('window.archived');
  });

  it('replays a streamed input with its saved value and editing disabled', async () => {
    const definition: GenUIAppDefinition = {
      schemaVersion: 2,
      id: 'streaming-app',
      title: 'Streaming app',
      template: 'openbitfun.app',
      libraryId: 'openbitfun.composable.v1',
      libraryRevision: 2,
      designSpec: { id: 'openbitfun.desktop.v1', revision: 1 },
      source: {
        kind: 'openui-lang',
        content: 'root = Col([input])\ninput = Input($name, "Name", "text")',
      },
      state: { name: { type: 'string', default: 'Initial' } },
    };

    await act(async () => root.render(<GenUIApplication
      definition={definition}
      streaming
      initialState={{ name: 'Initial' }}
    />));

    const input = container.querySelector<HTMLInputElement>('input[aria-label="Name"]');
    expect(input?.disabled).toBe(true);
    expect(input?.value).toBe('Initial');
  });

  it('renders SegmentedControl with owner state defaults', async () => {
    await act(async () => root.render(<GenUIOpenUIDocument
      source={{ kind: 'openui-lang', content: 'root = SegmentedControl($stage, [{"value":"探索","label":"探索"},{"value":"建设","label":"建设"}])' }}
      library={genUIProduct.libraries['openbitfun.composable.v1']}
      initialState={{ stage: '探索' }}
    />));
    expect(container.textContent).toContain('探索');
  });

  it('renders the Open Intelligent UI layout and title primitives', async () => {
    await act(async () => root.render(<GenUIOpenUIDocument
      source={{ kind: 'openui-lang', content: 'root = Col([heading, body])\nheading = Title("Overview")\nbody = Row([Text("A"), Box([Text("B")])])' }}
      library={genUIProduct.libraries['openbitfun.composable.v1']}
    />));
    expect(container.querySelector('.gen-ui-oi-col')).not.toBeNull();
    expect(container.querySelector('.gen-ui-oi-row')).not.toBeNull();
    expect(container.querySelector('.gen-ui-oi-box')).not.toBeNull();
    expect(container.querySelector('.gen-ui-oi-title')?.textContent).toBe('Overview');
  });

  it('uses the vendored upstream library and CSS for the Open Intelligent UI profile', async () => {
    await act(async () => root.render(<GenUIOpenUIDocument
      source={{ kind: 'openui-lang', content: 'root = Col([heading, body])\nheading = Title("Open Intelligent UI")\nbody = Text("Upstream styles")' }}
      library={genUIProduct.libraries['openui.v1']}
    />));
    expect(genUIProduct.libraries['openui.v1'].nativeLibrary).toBeDefined();
    expect(container.querySelector('.iui-col')).not.toBeNull();
    expect(container.querySelector('.iui-title')?.textContent).toBe('Open Intelligent UI');
    expect(container.querySelector('.gen-ui-oi-col')).toBeNull();
  });

  it('mounts OpenBitFun controls through @openbitfun/ui while keeping GenUI visuals local', async () => {
    await act(async () => root.render(<GenUIOpenUIDocument
      source={{ kind: 'openui-lang', content: 'root = Col([card, chart])\ncard = Card([Button("Continue"), Input($name, "Name", "text")], "soft")\nchart = Chart("line", [{label: "A", value: 1}, {label: "B", value: 2}], "label", ["value"])' }}
      library={genUIProduct.libraries['openbitfun.composable.v1']}
      initialState={{ name: 'OpenBitFun' }}
    />));
    expect(container.querySelector('[data-openbitfun-component="card"]')).not.toBeNull();
    expect(container.querySelector('[data-openbitfun-component="button"]')).not.toBeNull();
    expect(container.querySelector('[data-openbitfun-component="input"]')).not.toBeNull();
    expect(container.querySelector('[data-chart-type="line"]')).not.toBeNull();
  });

  it('keeps the transient OpenUI tree free of inspector boundaries', async () => {
    await act(async () => root.render(<GenUIOpenUIDocument
      source={{ kind: 'openui-lang', content: 'root = Col([drawing])\ndrawing = Svg([circle], "0 0 20 20")\ncircle = Circle(10, 10, 5, "accent")' }}
      library={genUIProduct.libraries['openbitfun.composable.v1']}
    />));
    expect(container.querySelector('[data-gen-ui-node]')).toBeNull();
    expect(container.querySelector('svg > circle')).not.toBeNull();
  });

  it('renders OpenUI chart variants through their native visual branches', async () => {
    for (const type of ['line', 'pie', 'scatter'] as const) {
      await act(async () => root.render(<GenUIOpenUIDocument
        source={{ kind: 'openui-lang', content: `root = Chart("${type}", [{label: "A", x: 1, value: 2}, {label: "B", x: 2, value: 3}], "${type === 'scatter' ? 'x' : 'label'}", ["value"])` }}
        library={genUIProduct.libraries['openbitfun.composable.v1']}
      />));
      expect(container.querySelector(`[data-chart-type="${type}"]`)).not.toBeNull();
    }
  });

  it('allows the transient unhydrated SegmentedControl value to settle on its first option', () => {
    const render = oiComponents.SegmentedControl.render;
    expect(() => render({
      view: {
        props: {
          label: 'Stage',
          value: undefined,
          options: [{ value: '探索', label: '探索' }, { value: '建设', label: '建设' }],
        },
      } as never,
      slots: {},
      disabled: false,
      emit: vi.fn(),
    })).not.toThrow();
  });

  it('still rejects duplicate SegmentedControl options', () => {
    const render = oiComponents.SegmentedControl.render;
    expect(() => render({
      view: {
        props: {
          label: 'Stage',
          value: '探索',
          options: [{ value: '探索', label: '探索' }, { value: '探索', label: 'Duplicate' }],
        },
      } as never,
      slots: {},
      disabled: false,
      emit: vi.fn(),
    })).toThrow('SegmentedControl requires unique options');
  });
});
