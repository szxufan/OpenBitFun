import { isWindowsDesktopRuntime, supportsNativeWindowDragging } from './environment';

/** Window chrome always belongs to the controller, including Peer Device Mode. */
export async function startNativeWindowDragging(): Promise<void> {
  if (!supportsNativeWindowDragging()) return;

  const { getCurrentWindow } = await import('@tauri-apps/api/window');
  const appWindow = getCurrentWindow();
  // Read native state at the gesture boundary; React's resize sync is debounced.
  const [isMaximized, isFullscreen] = await Promise.all([
    appWindow.isMaximized(),
    appWindow.isFullscreen(),
  ]);
  // Windows caption dragging restores maximized windows and follows the pointer.
  if (isFullscreen || (isMaximized && !isWindowsDesktopRuntime())) return;

  await appWindow.startDragging();
}
