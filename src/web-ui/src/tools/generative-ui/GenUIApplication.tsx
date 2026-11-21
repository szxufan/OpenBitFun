import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import type { GenUIRenderDiagnostic } from './genUIRenderDiagnostics';
import { isFatalGenUIError } from './genUIRenderDiagnostics';
import type { GenUIAppDefinition, GenUIAppEnvelope, GenUIAppState } from './genUITypes';
import { designSpecForApp, libraryFor, preflightGenUIApp } from './genUIContract';
import { recordedGenUIState, genUIContentKey, validateGenUIState } from './genUIState';
import type { GenUIVerification } from './genUIVerification';
import './GenUIApplication.scss';
import GenUIHostSurface from './GenUIHostSurface';
import StaticHtmlDocument from '@/shared/ui/static-document/StaticHtmlDocument';
import GenUIOpenUIDocument from './genUIOpenUI';
import { inspectGenUIHtmlSource } from './genUIHtmlSource';
import { GenUIStream } from './genUIStream';
import { genUIProduct } from './genUIProduct';

interface HistoryProps {
  definition: GenUIAppEnvelope;
  initialState?: GenUIAppState;
  streaming?: boolean;
  onVerification?: (report: GenUIVerification) => void;
}

export default function GenUIApplication(props: HistoryProps) {
  return <Application key={props.definition.id} {...props} />;
}

function Application({ definition, initialState, streaming = false, onVerification }: HistoryProps) {
  const stream = useRef(new GenUIStream());
  const definitionKey = genUIContentKey(definition);
  const app = useMemo(() => (streaming ? stream.current.project(definition, genUIProduct.catalog) : definition as GenUIAppDefinition), [definition, streaming]);
  const preflight = useMemo(() => streaming ? { accepted: true, issues: [] } : preflightGenUIApp(app, false, genUIProduct.catalog), [app, streaming]);
  const designSpec = designSpecForApp(app, genUIProduct.catalog);
  const library = libraryFor(app, genUIProduct.catalog);
  const source = app.source;
  const sourceIssues = useMemo(() => source?.kind === 'html-document' ? inspectGenUIHtmlSource(source, designSpec) : [], [source, designSpec]);
  const state = useMemo(() => recordedGenUIState(app, initialState), [app, initialState]);
  const [openUIReport, setOpenUIReport] = useState<{ key: string; errors: GenUIRenderDiagnostic[] }>();
  const receiveOpenUIErrors = useCallback((errors: GenUIRenderDiagnostic[]) => {
    setOpenUIReport({ key: definitionKey, errors });
  }, [definitionKey]);
  const openUIErrors = !streaming && openUIReport?.key === definitionKey ? openUIReport.errors : [];
  const fatalErrors = openUIErrors.filter(isFatalGenUIError);
  const onVerificationRef = useRef(onVerification);
  useEffect(() => { onVerificationRef.current = onVerification; }, [onVerification]);
  const stateIssues = preflight.accepted ? validateGenUIState(app, state) : [];
  const renderErrors = !library ? [`GenUI library ${app.libraryId}@${app.libraryRevision} is unavailable`] : !source ? ['GenUI source is required; semantic graph documents were removed'] : sourceIssues.length ? sourceIssues : fatalErrors.map(error => error.message);
  const render: GenUIVerification['render'] = renderErrors.length ? 'failed' : streaming || (source?.kind === 'openui-lang' && openUIReport?.key !== definitionKey) ? 'unverified' : 'passed';
  const checks: GenUIVerification['checks'] = renderErrors.length || stateIssues.length ? 'failed' : source?.kind === 'html-document' ? 'passed' : streaming ? 'unverified' : 'passed';
  const report: GenUIVerification = {
    appId: app.id,
    definitionKey,
    state,
    checks,
    preflight: streaming ? 'unverified' : preflight.accepted && !stateIssues.length ? 'passed' : 'failed',
    render,
    interaction: 'unverified',
    persistence: 'unverified',
    reopen: 'unverified',
    issues: [...preflight.issues, ...stateIssues, ...renderErrors.map(message => ({ path: 'source', message, code: 'render', severity: 'error' as const })), ...openUIErrors.filter(error => !isFatalGenUIError(error)).map(error => ({ path: error.path ?? error.statementId ?? 'source', message: error.message, code: error.code, severity: 'warning' as const }))],
    diagnostics: openUIErrors,
  };
  const reportKey = JSON.stringify(report);
  useEffect(() => { onVerificationRef.current?.(JSON.parse(reportKey)); }, [reportKey]);
  if (!source) {
    return <GenUIHostSurface specification={designSpec}><div className="gen-ui-application" data-openbitfun-component="gen-ui-application" data-openbitfun-part="root" data-openbitfun-state="rejected"><div data-openbitfun-component="gen-ui-application" data-openbitfun-part="content">Semantic graph GenUI documents are no longer supported.</div></div></GenUIHostSurface>;
  }
  return <GenUIHostSurface specification={designSpec}>
    <div className={'gen-ui-application gen-ui-application--' + (genUIProduct.adapters[app.libraryId]?.adapterKind ?? 'unknown') + (designSpec?.shell.kind === 'phone' ? ' gen-ui-application--phone' : '') + (source.kind === 'html-document' ? ' gen-ui-application--document' : '')} data-openbitfun-component="gen-ui-application" data-openbitfun-part="root" data-gen-ui-app={app.id} data-gen-ui-library={app.libraryId} data-gen-ui-library-revision={app.libraryRevision} data-gen-ui-source-kind={source.kind} data-openbitfun-state={render === 'failed' || report.preflight === 'failed' ? 'rejected' : render === 'passed' ? 'rendered' : undefined} data-gen-ui-preflight={report.preflight} data-gen-ui-postcheck={report.render} data-gen-ui-save="unverified">
      <div data-openbitfun-component="gen-ui-application" data-openbitfun-part="content">{library && (source.kind === 'html-document'
        ? <StaticHtmlDocument source={source} specification={designSpec} />
        : <GenUIOpenUIDocument source={source} library={genUIProduct.libraries[app.libraryId]}
            initialState={state} isStreaming={streaming} onError={receiveOpenUIErrors} />
      )}</div>
    </div>
  </GenUIHostSurface>;
}
