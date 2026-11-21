import {
  Alert,
  Button,
  Checkbox,
  Icon,
  Input,
  Dialog,
  DialogBody,
  DialogClose,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogHeading,
  DialogTitle,
} from '@openbitfun/ui';
import React, { useCallback, useEffect, useRef, useState } from 'react';
import { useI18n } from '@/infrastructure/i18n';
import { createLogger } from '@/shared/utils/logger';
import { Loader2 } from 'lucide-react';
import { dispatchApi } from './dispatchApi';
import type {
  DispatchSelection,
  DispatchSshProbe,
  DispatchTargetOption,
} from './types';
import {
  BASE_DISPATCH_CAPABILITIES,
  DISPATCH_PROTOCOL_VERSION,
} from './dispatchPreflight';
import { configAPI } from '@/infrastructure/api/service-api/ConfigAPI';
import { gitAPI } from '@/infrastructure/api/service-api/GitAPI';
import type { WorktreeSettings } from '@/infrastructure/api/service-api/WorktreeAPI';
import './DispatchTargetDialog.scss';

const log = createLogger('DispatchTargetDialog');
const DIALOG_TITLE_ID = 'dispatch-install-dialog-title';
interface DispatchTargetDialogProps {
  open: boolean;
  target: DispatchTargetOption | null;
  sourceWorkspacePath?: string;
  sourceWorkspaceId?: string;
  onClose: () => void;
  onReady: (selection: DispatchSelection) => void;
}

export const DispatchTargetDialog: React.FC<DispatchTargetDialogProps> = ({
  open,
  target,
  sourceWorkspacePath,
  sourceWorkspaceId,
  onClose,
  onReady,
}) => {
  const { t } = useI18n('common');
  const [includeUncommitted, setIncludeUncommitted] = useState(false);
  const [baseRef, setBaseRef] = useState('HEAD');
  const [baseRefError, setBaseRefError] = useState<string | null>(null);
  const [validatingBaseRef, setValidatingBaseRef] = useState(false);
  const [worktreeSettingsLoading, setWorktreeSettingsLoading] = useState(true);
  const [probe, setProbe] = useState<DispatchSshProbe | null>(null);
  const [probing, setProbing] = useState(false);
  const [probeError, setProbeError] = useState(false);
  const generationRef = useRef(0);
  const includeUncommittedTouchedRef = useRef(false);

  const targetId = target?.connectionId?.trim() ?? '';

  const runProbe = useCallback(async () => {
    if (!targetId || !target || target.kind !== 'ssh') return;
    // The target's own directories are irrelevant now: dispatch checks out its
    // own worktree there, so the probe only reports CLI and model readiness.
    const path = '';
    const generation = ++generationRef.current;
    setProbing(true);
    setProbeError(false);
    try {
      const result = await dispatchApi.probeTarget({ kind: 'ssh', connectionId: targetId, workspacePath: path });
      if (generation === generationRef.current) {
        setProbe(result);
      }
    } catch (nextError) {
      if (generation === generationRef.current) {
        setProbe(null);
        setProbeError(true);
        log.warn('Failed to check dispatch target readiness', {
          targetKind: target.kind,
          targetId,
          error: nextError,
        });
      }
    } finally {
      if (generation === generationRef.current) {
        setProbing(false);
      }
    }
  }, [target, targetId]);

  useEffect(() => {
    if (!open || !targetId) return;
    includeUncommittedTouchedRef.current = false;
    setIncludeUncommitted(false);
    setBaseRef('HEAD');
    setBaseRefError(null);
    setValidatingBaseRef(false);
    setWorktreeSettingsLoading(true);
    setProbe(null);
    setProbeError(false);
    void runProbe();
  }, [open, runProbe, targetId]);

  // Dispatch uses the same baseline creation path as the regular worktree
  // control, so its initial copy-local-changes choice follows that setting.
  useEffect(() => {
    if (!open) return;
    let cancelled = false;
    setWorktreeSettingsLoading(true);
    void configAPI.getConfig('app.worktrees', { skipRetryOnNotFound: true })
      .then(settings => {
        if (!cancelled && !includeUncommittedTouchedRef.current) {
          const configured = settings as Partial<WorktreeSettings> | undefined;
          setIncludeUncommitted(configured?.copyLocalChanges === true);
        }
      })
      .catch(nextError => {
        log.warn('Failed to read worktree settings for dispatch', {
          error: nextError,
        });
        if (!cancelled && !includeUncommittedTouchedRef.current) {
          setIncludeUncommitted(false);
        }
      })
      .finally(() => {
        if (!cancelled) setWorktreeSettingsLoading(false);
      });
    return () => {
      cancelled = true;
    };
  }, [open, targetId]);

  // Retires every in-flight probe and revision check, so a result
  // that lands after the dialog moved on cannot write to a closed dialog.
  const invalidatePendingWork = useCallback(() => {
    generationRef.current += 1;
  }, []);

  useEffect(() => {
    if (!open || !targetId) return;
    return () => {
      invalidatePendingWork();
    };
  }, [invalidatePendingWork, open, targetId]);

  const closeDialog = useCallback(() => {
    invalidatePendingWork();
    onClose();
  }, [invalidatePendingWork, onClose]);

  const protocol = probe?.protocol;
  const requiredCapabilities = [...BASE_DISPATCH_CAPABILITIES];
  const missingCapabilities = protocol
    ? requiredCapabilities.filter(capability => !protocol.capabilities.includes(capability))
    : requiredCapabilities;
  const protocolCompatible =
    protocol?.protocolVersion === DISPATCH_PROTOCOL_VERSION &&
    missingCapabilities.length === 0;
  const cliReady =
    !!probe?.cliInstalled &&
    !!protocol &&
    !probe.protocolError &&
    protocolCompatible;
  // The source workspace is owned by ID; the path only labels its checkout.
  const workspaceReady = !!sourceWorkspaceId?.trim() || !!sourceWorkspacePath?.trim();
  /**
   * Model readiness is deliberately absent: the model is chosen in the
   * composer like any local session, and submission pushes this device's model
   * configuration to a target that cannot serve the choice. Gating here would
   * ask the user to solve a problem the backend already solves.
   */
  const ready = workspaceReady && cliReady;

  const confirmTarget = async () => {
    if (
      !target
      || target.kind !== 'ssh'
      || !targetId
      || !ready
    ) return;
    const normalizedSourcePath = sourceWorkspacePath?.trim() || '';
    const normalizedBaseRef = baseRef.trim() || 'HEAD';
    const generation = generationRef.current;
    setValidatingBaseRef(true);
    setBaseRefError(null);
    try {
      await gitAPI.resolveRevision({ workspaceId: sourceWorkspaceId ?? '', repositoryPath: normalizedSourcePath }, normalizedBaseRef);
    } catch (nextError) {
      if (generation === generationRef.current) {
        log.warn('Failed to resolve dispatch base revision', {
          repositoryPath: normalizedSourcePath,
          revision: normalizedBaseRef,
          error: nextError,
        });
        setBaseRefError(t('dispatch.baseRefInvalid', { ref: normalizedBaseRef }));
      }
      return;
    } finally {
      if (generation === generationRef.current) {
        setValidatingBaseRef(false);
      }
    }
    if (generation !== generationRef.current) return;
    // The target chooses where its worktree lands, so nothing is sent here.
    const request = {
      kind: 'ssh' as const,
      connectionId: targetId,
      workspacePath: '',
    };
    onReady({
      request,
      target: {
        ...request,
        displayName: target.displayName,
      },
      includeUncommitted,
      baseRef: normalizedBaseRef,
      modelCatalog: protocol?.modelCatalog,
      availableModels: protocol?.availableModels,
      defaultModel: protocol?.defaultModel,
    });
  };

  return (
    <Dialog
      open={open}
      onOpenChange={(nextOpen) => { if (!nextOpen) closeDialog(); }}
      size="md"
      aria-labelledby={DIALOG_TITLE_ID}
      data-testid="dispatch-install-dialog"
    >
      <div
        className="dispatch-install-dialog"
        data-openbitfun-component="dispatch-install-dialog"
        data-openbitfun-part="root"
      >
        <DialogHeader
          data-openbitfun-component="dispatch-install-dialog"
          data-openbitfun-part="header"
        >
          <DialogHeading>
            <DialogTitle id={DIALOG_TITLE_ID}>
              {t('dispatch.configureTitle', { target: target?.displayName ?? '' })}
            </DialogTitle>
            <DialogDescription>{t('dispatch.configureSubtitle')}</DialogDescription>
          </DialogHeading>
          <DialogClose />
        </DialogHeader>

        <DialogBody
          className="dispatch-install-dialog__body"
          data-openbitfun-component="dispatch-install-dialog"
          data-openbitfun-part="body"
        >
          {baseRefError ? (
            <Alert
              tone="error"
              message={baseRefError}
              closable
              onClose={() => setBaseRefError(null)}
            />
          ) : null}

          <section className="dispatch-install-dialog__section">
            <h3 className="dispatch-install-dialog__section-title">
              {t('dispatch.readinessTitle')}
            </h3>
            {probing || (!probe && !probeError) ? (
              <div className="dispatch-install-dialog__pending" role="status">
                <Loader2 size={14} className="dispatch-install-dialog__spin" />
                {t('dispatch.checkingTarget')}
              </div>
            ) : null}
            {probe ? (
              <div className="dispatch-install-dialog__checks">
                <div
                  className="dispatch-install-dialog__check-row"
                  data-state={cliReady ? 'ok' : 'blocked'}
                >
                  <span>{t('dispatch.cliStatus')}</span>
                  <strong>
                    {cliReady
                      ? t('dispatch.cliReady', { version: protocol?.cliVersion })
                      : t(probe.cliInstalled ? 'dispatch.cliUpdateRequired' : 'dispatch.cliUnavailable')}
                  </strong>
                </div>
              </div>
            ) : null}
            {probeError || (probe && !cliReady) ? (
              <div className="dispatch-install-dialog__retry">
                {probeError && (
                  <span className="dispatch-install-dialog__hint">
                    {t('dispatch.probeFailed')}
                  </span>
                )}
                <Button
                  variant="outline"
                  size="sm"
                  disabled={probing}
                  onClick={() => void runProbe()}
                  leadingIcon={<Icon name="refresh" size="sm" aria-hidden />}
                >
                  {t('dispatch.retryCheck')}
                </Button>
              </div>
            ) : null}
          </section>

          <section className="dispatch-install-dialog__section">
            <h3 className="dispatch-install-dialog__section-title">
              {t('dispatch.deliveryTitle')}
            </h3>
            <div className="dispatch-install-dialog__field">
              <span className="dispatch-install-dialog__field-label">
                {t('dispatch.baselineSource')}
              </span>
              <code className="dispatch-install-dialog__path">{sourceWorkspacePath}</code>
            </div>
            <span className="dispatch-install-dialog__hint">
              {t('dispatch.baselineDescription')}
            </span>
            <label className="dispatch-install-dialog__field dispatch-install-dialog__base-ref">
              <span className="dispatch-install-dialog__field-label">
                {t('dispatch.baseRef')}
              </span>
              <Input
                type="text"
                size="md"
                value={baseRef}
                disabled={validatingBaseRef}
                spellCheck={false}
                onValueChange={value => {
                  setBaseRef(value);
                  setBaseRefError(null);
                }}
                placeholder="HEAD"
              />
              <span className="dispatch-install-dialog__hint">
                {t('dispatch.baseRefHint')}
              </span>
            </label>
            <Checkbox
              className="dispatch-install-dialog__toggle"
              checked={includeUncommitted}
              disabled={validatingBaseRef}
              onCheckedChange={checked => {
                includeUncommittedTouchedRef.current = true;
                setIncludeUncommitted(checked);
              }}
              label={t('dispatch.includeUncommitted')}
              description={t('dispatch.includeUncommittedHint')}
            />
          </section>
        </DialogBody>

        <div
          className="dispatch-install-dialog__actions"
          data-openbitfun-component="dispatch-install-dialog"
          data-openbitfun-part="actions"
        >
          <DialogFooter>
          <Button
            variant="fill"
            size="sm"
            onClick={closeDialog}
          >
            {t('dispatch.cancel')}
          </Button>
          <Button
            variant="primary"
            size="sm"
            disabled={
              !ready
              || probing
              || validatingBaseRef
              || worktreeSettingsLoading
            }
            onClick={() => void confirmTarget()}
          >
            {validatingBaseRef ? (
              <Loader2 size={14} className="dispatch-install-dialog__spin" />
            ) : null}
            {t('dispatch.useTarget')}
          </Button>
          </DialogFooter>
        </div>
      </div>
    </Dialog>
  );
};
