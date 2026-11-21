import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { startNativeWindowDragging } from './window';

const nativeWindow = vi.hoisted(() => ({
  isMaximized: vi.fn(async () => false),
  isFullscreen: vi.fn(async () => false),
  startDragging: vi.fn(async () => {}),
}));
const getCurrentWindow = vi.hoisted(() => vi.fn(() => nativeWindow));
vi.mock('@tauri-apps/api/window', () => ({ getCurrentWindow }));

describe('native window dragging', () => {
  beforeEach(() => {
    vi.clearAllMocks();
    vi.stubGlobal('window', {
      __TAURI_INTERNALS__: {
        invoke: vi.fn(),
        metadata: { currentWindow: { label: 'main' } },
      },
    });
  });

  afterEach(() => vi.unstubAllGlobals());

  it.each(['Win32', 'Linux x86_64', 'MacIntel'].flatMap(platform => [
    { platform, maximized: false, fullscreen: false },
    { platform, maximized: true, fullscreen: false },
    { platform, maximized: false, fullscreen: true },
    { platform, maximized: true, fullscreen: true },
  ]))('uses local $platform policy for maximized=$maximized/fullscreen=$fullscreen', async ({ platform, maximized, fullscreen }) => {
    vi.stubGlobal('navigator', { platform, userAgent: '' });
    nativeWindow.isMaximized.mockResolvedValue(maximized);
    nativeWindow.isFullscreen.mockResolvedValue(fullscreen);

    await startNativeWindowDragging();

    expect(getCurrentWindow).toHaveBeenCalledOnce();
    expect(nativeWindow.isMaximized).toHaveBeenCalledOnce();
    expect(nativeWindow.isFullscreen).toHaveBeenCalledOnce();
    expect(nativeWindow.startDragging).toHaveBeenCalledTimes(
      !fullscreen && (!maximized || platform === 'Win32') ? 1 : 0,
    );
  });

  it.each([
    { name: 'server', localWindow: undefined },
    { name: 'browser', localWindow: {} },
    { name: 'host without current-window metadata', localWindow: { __TAURI_INTERNALS__: { invoke: vi.fn() } } },
  ])('does not call native APIs in a $name', async ({ localWindow }) => {
    vi.stubGlobal('window', localWindow);
    vi.stubGlobal('navigator', { platform: 'Win32', userAgent: 'Windows' });

    await startNativeWindowDragging();

    expect(getCurrentWindow).not.toHaveBeenCalled();
    expect(nativeWindow.startDragging).not.toHaveBeenCalled();
  });
});
