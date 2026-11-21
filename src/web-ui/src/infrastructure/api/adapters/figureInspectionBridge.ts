import type { WrittenFigureRequest } from '@/tools/visualize/backgroundInspection';
import { getActiveSurfaceId, isLocalSurface } from '@/infrastructure/peer-device/deviceSurface';
import { emit, listen } from '@tauri-apps/api/event';

/** Local to the producing desktop. No relay, peer routing or selected chat is involved. */
export function listenForFigureChecks() {
  return listen<{ id: string; request: WrittenFigureRequest }>('figure-check', async ({ payload }) => {
    const { id, request } = payload;
    if (!/^[0-9a-f-]{36}$/i.test(id) || typeof request?.source !== 'string') return;
    let result;
    try {
      const { inspectWrittenFigure } = await import('@/tools/visualize/backgroundInspection');
      const checked = await inspectWrittenFigure(request, async sessionId => {
        const { flowChatStore } = await import('@/flow_chat/store/FlowChatStore');
        if (isLocalSurface(getActiveSurfaceId())) return flowChatStore.getState().sessions.get(sessionId);
      }, () => emit('figure-check-stage-' + id));
      result = { Ok: checked };
    } catch (e) {
      result = { Err: e instanceof Error ? e.message : String(e) };
    }
    await emit('figure-check-result-' + id, result);
  });
}
