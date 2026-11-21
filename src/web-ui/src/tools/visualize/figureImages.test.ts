// @vitest-environment jsdom
import { afterEach, describe, expect, it, vi } from 'vitest';
import { localizeFigureImages } from './figureImages';
import { snapshotFigureScripts } from './figureResources';
import type { Declarations } from './visualBindings';

const url = 'https://images.example.test/diagram.png';
const declarations: Declarations = { photo: { type: 'image', url }, rows: { type: 'json', value: [3] } };

afterEach(() => vi.unstubAllGlobals());

describe('private figure images', () => {
  it('loads only the declared image, hands the guest local bytes, and reuses the checked snapshot', async () => {
    const fetchImage = vi.fn().mockResolvedValue(new Response(new Uint8Array([137, 80, 78, 71]), {
      headers: { 'content-type': 'image/png' },
    }));
    vi.stubGlobal('fetch', fetchImage);
    const input = { rows: { kind: 'json' as const, value: [3] } };
    const checked = await localizeFigureImages(declarations, input);
    expect(fetchImage).toHaveBeenCalledWith(url, expect.objectContaining({
      mode: 'cors', credentials: 'omit', redirect: 'error', referrerPolicy: 'no-referrer',
    }));
    expect(checked.photo).toEqual({ kind: 'image', value: 'data:image/png;base64,iVBORw==' });
    expect(await localizeFigureImages(declarations, checked)).toEqual(checked);
    expect(fetchImage).toHaveBeenCalledTimes(1);
    expect(checked.rows).toBe(input.rows);
  });

  it.each([['text/html', 3], ['image/png', 256 * 1024 + 1]])('rejects unsupported or oversized response bytes', async (type, size) => {
    vi.stubGlobal('fetch', vi.fn().mockResolvedValue(new Response(new Uint8Array(Number(size)), {
      headers: { 'content-type': String(type) },
    })));
    await expect(localizeFigureImages(declarations, {})).rejects.toThrow('Could not load the declared image');
  });
});

describe('figure script snapshots', () => {
  const url = 'https://cdn.jsdelivr.net/npm/example@1/dist/plot.js';
  const script = '<script src="' + url + '"></script>';

  it('snapshots a fixed public dependency once and reuses its bytes', async () => {
    const load = vi.fn().mockResolvedValue(new Response('window.draw = () => 1;', {
      headers: { 'content-type': 'application/javascript' },
    }));
    vi.stubGlobal('fetch', load);
    const source = await snapshotFigureScripts(script.repeat(2), {});
    expect(source.match(/data:application\/javascript;base64,/g)).toHaveLength(2);
    expect(await snapshotFigureScripts(source, {})).toBe(source);
    expect(load).toHaveBeenCalledTimes(1);
    expect(load).toHaveBeenCalledWith(url, expect.objectContaining({ credentials: 'omit', redirect: 'error' }));
  });

  it('restricts this loader to public declarations and supported script origins', async () => {
    const load = vi.fn();
    vi.stubGlobal('fetch', load);
    expect(await snapshotFigureScripts(script, declarations)).toBe(script);
    const other = script.replace('cdn.jsdelivr.net', 'other.example.test');
    expect(await snapshotFigureScripts(other, {})).toBe(other);
    expect(load).not.toHaveBeenCalled();
  });
});
