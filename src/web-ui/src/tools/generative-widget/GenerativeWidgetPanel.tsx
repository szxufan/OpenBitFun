import React, { useState } from 'react';
import { useI18n } from '@/infrastructure/i18n';
import {
  type GenerativeWidgetOrigin,
} from '@/flow_chat/utils/generativeWidgetProjection';
import { useGenerativeWidgetProjection } from '@/flow_chat/hooks/useGenerativeWidgetProjection';
import GenUIApplication from '@/tools/generative-ui/GenUIApplication';
import { isGenUIAppEnvelope, type GenUIAppEnvelope, type GenUIAppState } from '@/tools/generative-ui/genUITypes';
import { resolveGenUIInteractionState } from '@/tools/generative-ui/genUIInteractionState';
import type { GenUIVerification } from '@/tools/generative-ui/genUIVerification';
import { genUIContentKey } from '@/tools/generative-ui/genUIState';
import './GenerativeWidgetPanel.scss';

export interface GenerativeWidgetPanelProps {
  app?: GenUIAppEnvelope;
  appState?: GenUIAppState;
  origin?: GenerativeWidgetOrigin;
  presentation?: 'panel' | 'inline';
  onExpand?: (value: { origin: GenerativeWidgetOrigin; app: GenUIAppEnvelope; appState?: GenUIAppState; widgetId: string }) => void;
}

export const GenerativeWidgetPanel: React.FC<GenerativeWidgetPanelProps> = ({
  app: suppliedApp,
  appState: suppliedState,
  origin,
  presentation = 'panel',
  onExpand,
}) => {
  const { t } = useI18n('flow-chat');
  const projection = useGenerativeWidgetProjection(origin);
  const app = origin ? projection?.app : (isGenUIAppEnvelope(suppliedApp) ? suppliedApp : undefined);
  const appState = origin ? projection?.appState : suppliedState;
  const [verification,setVerification]=useState<GenUIVerification>();
  const interactionState = resolveGenUIInteractionState({
    app,
    toolStatus: projection?.status,
    isStreaming: projection?.isStreaming,
    isFailed: projection?.isFailed,
    isCancelled: projection?.status === 'cancelled',
    isRejected: projection?.status === 'rejected',
    ownerReady: true,
    ownerError: projection?.readOnlyReason,
    deliveryAccepted: false,
    requiresDelivery: false,
  });
  const terminalStatus = t(`toolCards.generativeUI.state.${interactionState}`);
  const showToolbar = presentation === 'panel' || Boolean(projection?.origin && onExpand);

  if (projection?.isFailed || projection?.status === 'rejected') return null;
  const currentVerification = app && verification?.definitionKey === genUIContentKey(app) ? verification : undefined;
  const source=app && 'source' in app ? app.source:undefined;
  const unknownVersion=!!app && app.schemaVersion!==2;
  const fatalRender=unknownVersion || currentVerification?.render==='failed';
  const archivedSource=source && typeof source==='object' && 'content' in source && typeof source.content==='string'?source.content:JSON.stringify(app,null,2);
  if (app) {
    return (
      <div
        className={`openbitfun-generative-widget-panel openbitfun-generative-widget-panel--app openbitfun-generative-widget-panel--${presentation}`}
        data-openbitfun-component="generative-widget"
        data-openbitfun-part="application"
        data-openbitfun-state={interactionState}
      >
        {showToolbar && !fatalRender ? <div className="openbitfun-generative-widget-panel__toolbar" data-openbitfun-component="generative-widget" data-openbitfun-part="toolbar">
          <span className="openbitfun-generative-widget-panel__title">{app.title}</span>
          <span className="openbitfun-generative-widget-panel__status" role="status" aria-live="polite">{terminalStatus}</span>
          {presentation === 'inline' && projection?.origin && onExpand ? <button
            type="button"
            className="openbitfun-generative-widget-panel__expand"
            onClick={() => onExpand({ origin: projection.origin, app, appState, widgetId: projection.widgetId })}
            aria-label="Open expanded GenUI view"
          >↗</button> : null}
        </div> : null}
        {projection?.readOnlyReason ? <p role="status">{projection.readOnlyReason}</p> : null}
        {fatalRender && <details><summary>{app.title}</summary><pre>{archivedSource}</pre></details>}
        <fieldset disabled hidden={fatalRender} className="openbitfun-generative-widget-panel__app" data-openbitfun-component="generative-widget" data-openbitfun-part="app">
          {!unknownVersion && <GenUIApplication
            key={JSON.stringify([projection?.origin ?? origin, app.id])}
            definition={app}
            streaming={projection?.isStreaming}
            initialState={appState}
            onVerification={setVerification}
          />}
        </fieldset>
      </div>
    );
  }

  return (
    <div className="openbitfun-generative-widget-panel openbitfun-generative-widget-panel--empty"
      data-openbitfun-component="generative-widget" data-openbitfun-part="empty"
      data-openbitfun-state={interactionState}>
      {interactionState === 'starting' ? <div className="openbitfun-generative-widget-panel__skeleton" aria-label={terminalStatus} aria-busy="true">
        <span /><span /><span />
      </div> : <div className="openbitfun-generative-widget-panel__empty-copy">
        {projection?.readOnlyReason ?? terminalStatus}
      </div>}
    </div>
  );
};
export default GenerativeWidgetPanel;
