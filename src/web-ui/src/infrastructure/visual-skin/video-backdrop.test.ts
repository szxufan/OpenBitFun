// @vitest-environment jsdom
import { readFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { compile } from 'sass';
import JSZip from 'jszip';
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { VisualSkinRuntime } from './runtime';
import { VisualSkinPackageParser } from './parser';
import { validateVisualSkinPackage } from './validator';
import type { StoredVisualSkin } from './types';

// A minimal but real MP4 prefix: box size, 'ftyp', brand 'isom'.
const mp4 = new Uint8Array([0, 0, 0, 16, 0x66, 0x74, 0x79, 0x70, 0x69, 0x73, 0x6f, 0x6d, 0, 0, 2, 0]);
const mp4DataUrl = `data:video/mp4;base64,${Buffer.from(mp4).toString('base64')}`;
const parser = new VisualSkinPackageParser();
const manifest = { schema: 'openbitfun.visual-skin', schemaVersion: 1, id: 'film', name: 'Film', version: '1' };
const runtimes: VisualSkinRuntime[] = [];
const tick = () => new Promise(resolve => setTimeout(resolve, 0));

async function zipWith(skin: object, files: Record<string, Uint8Array>) {
  const zip = new JSZip();
  zip.file('skin.json', JSON.stringify(skin));
  for (const [name, bytes] of Object.entries(files)) zip.file(name, bytes);
  return zip.generateAsync({ type: 'arraybuffer', compression: 'STORE' });
}
const videoSkin = (surface: object = {}) => ({
  manifest, modes: { light: {}, dark: {} },
  assets: { film: { mimeType: 'video/mp4', source: { kind: 'path', path: 'assets/film.mp4' } } },
  surfaces: [{ surface: 'shell', backgroundAssetId: 'film', fit: 'contain', backgroundPosition: 'top', ...surface }],
});

async function activate(skin: StoredVisualSkin) {
  const runtime = new VisualSkinRuntime({
    list: async () => ({ skins: [skin], invalidRecords: [] }), get: async () => skin,
    put: async () => {}, delete: async () => {}, getSelection: async () => null, setSelection: async () => {},
  });
  runtimes.push(runtime);
  await runtime.activate('film');
  return runtime;
}

beforeEach(() => {
  let next = 0;
  vi.stubGlobal('URL', Object.assign(URL, { createObjectURL: vi.fn(() => `blob:video-${next++}`), revokeObjectURL: vi.fn() }));
  vi.spyOn(HTMLMediaElement.prototype, 'play').mockResolvedValue(undefined);
  vi.spyOn(HTMLMediaElement.prototype, 'pause').mockImplementation(() => undefined);
  Object.defineProperty(document, 'hidden', { configurable: true, value: false });
  document.body.innerHTML = '<main data-openbitfun-surface="shell"><span data-openbitfun-skin-paint aria-hidden="true"></span></main>';
});
afterEach(() => {
  runtimes.splice(0).forEach(runtime => runtime.dispose());
  document.body.innerHTML = '';
  document.head.querySelectorAll('style').forEach(node => node.remove());
  document.documentElement.removeAttribute('data-color-scheme');
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

describe('mp4 backdrop', () => {
  it('imports an MP4 asset as a data URL', async () => {
    const skin = await parser.parse(await zipWith(videoSkin(), { 'assets/film.mp4': mp4 }));
    expect(skin.assets.film.dataUrl).toMatch(/^data:video\/mp4;base64,/);
  });

  it('rejects bytes that are not an MP4 in both ZIP and embedded JSON packages', async () => {
    await expect(parser.parse(await zipWith(videoSkin(), { 'assets/film.mp4': new Uint8Array(32) }))).rejects.toThrow('not a valid MP4');
    const badJson = new TextEncoder().encode(JSON.stringify({
      manifest,
      modes: { light: {} },
      assets: { film: { mimeType: 'video/mp4', source: { kind: 'data', data: `data:video/mp4;base64,${Buffer.alloc(32).toString('base64')}` } } },
    })).buffer;
    await expect(parser.parse(badJson)).rejects.toThrow('not a valid MP4');
  });

  it('mounts a muted, looping video in the surface paint plane instead of a CSS background', async () => {
    await activate(await parser.parse(await zipWith(videoSkin(), { 'assets/film.mp4': mp4 })));
    const video = document.querySelector<HTMLVideoElement>('[data-openbitfun-skin-paint] > video[data-openbitfun-skin-video]')!;
    expect(video).toBeTruthy();
    expect(video.getAttribute('src')).toMatch(/^blob:video-/);
    expect(video.muted && video.loop && video.autoplay).toBe(true);
    expect(video.controls).toBe(false);
    expect(HTMLMediaElement.prototype.play).toHaveBeenCalled();
    const css = document.querySelector('style[data-openbitfun-skin-runtime]')!.textContent!;
    expect(css).toContain('--openbitfun-skin-surface-video-fit:contain !important;');
    expect(css).toContain('--openbitfun-skin-surface-video-position:top !important;');
    expect(css).not.toContain('--openbitfun-skin-surface-background-image');
    expect(css).not.toContain('--openbitfun-skin-asset-');
    expect(css).toContain('--openbitfun-skin-surface-background-opacity:1 !important;');
    expect(document.querySelector('main')!.getAttribute('data-openbitfun-skin-artwork')).toBe('true');
  });

  it('keeps the video paint plane composited while transcript hover controls change', () => {
    const styles = compile(resolve('src/app/styles/visual-skin.scss')).css;
    const plane = styles.match(/> \[data-openbitfun-skin-paint\]\s*\{[^}]*transform: translateZ\(0\);[^}]*\}/)?.[0];
    expect(plane).toContain('transform: translateZ(0)');
  });

  it('does not blur a rectangular wallpaper patch behind the composer', () => {
    const source = readFileSync(resolve('../../design-system/packages/ui/src/flow-chat/composer/ChatComposer.module.css'), 'utf8');
    const contextRule = source.match(/\.root\[data-has-context="true"\]\s*\{[^}]*\}/)?.[0];
    expect(contextRule).toContain('backdrop-filter: none');
    expect(contextRule).toContain('-webkit-backdrop-filter: none');
    expect(contextRule).not.toContain('blur(');
  });

  it('keeps the same player across repaints, swaps mode-specific videos, and cleans up on deactivate', async () => {
    const modeSkin = await parser.parse(await zipWith({
      manifest,
      modes: {
        light: {},
        dark: {},
        highContrastDark: { surfaces: [{ surface: 'shell', baseToken: 'color.surface.canvas' }] },
      },
      assets: {
        film: { mimeType: 'video/mp4', source: { kind: 'path', path: 'assets/film.mp4' } },
      },
      surfaces: [{ surface: 'shell', backgroundAssetId: 'film', fit: 'fill' }],
    }, { 'assets/film.mp4': mp4 }));
    const runtime = await activate(modeSkin);
    const first = document.querySelector<HTMLVideoElement>('video[data-openbitfun-skin-video]')!;
    expect(document.querySelector('style[data-openbitfun-skin-runtime]')!.textContent).toContain('--openbitfun-skin-surface-video-fit:fill !important;');
    document.documentElement.setAttribute('data-color-scheme', 'dark');
    await tick();
    expect(document.querySelectorAll('video[data-openbitfun-skin-video]')).toHaveLength(1);
    expect(document.querySelector('video[data-openbitfun-skin-video]')).toBe(first);
    document.documentElement.setAttribute('data-contrast', 'high');
    await tick();
    expect(document.querySelector('video[data-openbitfun-skin-video]')).toBeNull();
    expect(first.hasAttribute('src')).toBe(false);
    document.documentElement.removeAttribute('data-contrast');
    await tick();
    expect(document.querySelectorAll('video[data-openbitfun-skin-video]')).toHaveLength(1);
    await runtime.deactivate();
    expect(document.querySelector('video[data-openbitfun-skin-video]')).toBeNull();
  });

  it('suspends playback while the document is hidden and resumes when visible again', async () => {
    await activate(await parser.parse(await zipWith(videoSkin(), { 'assets/film.mp4': mp4 })));
    vi.mocked(HTMLMediaElement.prototype.play).mockClear();
    Object.defineProperty(document, 'hidden', { configurable: true, value: true });
    document.dispatchEvent(new Event('visibilitychange'));
    expect(HTMLMediaElement.prototype.pause).toHaveBeenCalledTimes(1);
    expect(HTMLMediaElement.prototype.play).not.toHaveBeenCalled();
    Object.defineProperty(document, 'hidden', { configurable: true, value: false });
    document.dispatchEvent(new Event('visibilitychange'));
    expect(HTMLMediaElement.prototype.play).toHaveBeenCalledTimes(1);
  });

  it('does not mount a video for a fully transparent backdrop', async () => {
    await activate(await parser.parse(await zipWith(videoSkin({ backgroundOpacity: 0 }), { 'assets/film.mp4': mp4 })));
    expect(document.querySelector('video[data-openbitfun-skin-video]')).toBeNull();
  });

  it('rejects a video asset in component or state slots, which only paint CSS backgrounds', () => {
    for (const patch of [
      { components: [{ component: 'chat-composer', slots: { root: { assetId: 'film' } } }] },
      { components: [{ component: 'chat-composer', states: { hover: { slots: { root: { assetId: 'film' } } } } }] },
      { surfaces: [{ surface: 'shell', states: { empty: { slots: { root: { assetId: 'film' } } } } }] },
    ]) {
      const result = validateVisualSkinPackage({
        manifest, modes: { light: {} },
        assets: { film: { mimeType: 'video/mp4', source: { kind: 'data', data: mp4DataUrl } } },
        ...patch,
      });
      expect(result.valid).toBe(false);
      expect(result.errors.map(error => error.message).join('\n')).toContain('only supported as a surface backgroundAssetId');
    }
  });
});
