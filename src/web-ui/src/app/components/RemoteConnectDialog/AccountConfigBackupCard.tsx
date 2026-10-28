/**
 * Manual config backup card for the signed-in Remote Connect account panel.
 *
 * Export encrypts this device's config with a fresh one-time backup code and
 * stores it on the relay; import on any same-account device pastes that code.
 * Nothing syncs automatically — both directions are explicit user actions.
 */

import React, { useCallback, useEffect, useRef, useState } from 'react';
import { Alert, Button, IconButton, Input } from '@openbitfun/ui';
import { Check, Copy, DatabaseBackup, Download, Trash2, Upload } from 'lucide-react';
import { useI18n } from '@/infrastructure/i18n';
import { confirmDanger } from '@/infrastructure/confirm-dialog';
import {
  remoteConnectAPI,
  type AccountConfigBackupStatus,
} from '@/infrastructure/api/service-api/RemoteConnectAPI';
import { useNotification } from '@/shared/notification-system';
import { createLogger } from '@/shared/utils/logger';

const log = createLogger('AccountConfigBackupCard');

const AccountConfigBackupCard: React.FC = () => {
  const { t, formatRelativeTime } = useI18n('common');
  const { success } = useNotification();
  const [status, setStatus] = useState<AccountConfigBackupStatus | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [exporting, setExporting] = useState(false);
  const [importing, setImporting] = useState(false);
  const [deleting, setDeleting] = useState(false);
  const [importCode, setImportCode] = useState('');
  const [importError, setImportError] = useState<string | null>(null);
  const [exportedCode, setExportedCode] = useState<string | null>(null);
  const [codeCopied, setCodeCopied] = useState(false);
  const mountedRef = useRef(true);

  useEffect(() => {
    mountedRef.current = true;
    return () => { mountedRef.current = false; };
  }, []);

  const refreshStatus = useCallback(async () => {
    try {
      const next = await remoteConnectAPI.accountConfigBackupStatus();
      if (!mountedRef.current) return;
      setStatus(next);
      setLoadError(null);
    } catch (error) {
      log.error('accountConfigBackupStatus failed', error);
      if (mountedRef.current) setLoadError('accountLogin.configBackup.loadFailed');
    }
  }, []);

  useEffect(() => { void refreshStatus(); }, [refreshStatus]);

  const handleExport = async () => {
    if (status?.exists) {
      const confirmed = await confirmDanger(
        t('accountLogin.configBackup.overwriteTitle'),
        t('accountLogin.configBackup.overwriteWarning'),
        {
          confirmText: t('accountLogin.configBackup.exportAction'),
          cancelText: t('accountLogin.cancel'),
        },
      );
      if (!confirmed) return;
    }
    setExporting(true);
    setExportedCode(null);
    setCodeCopied(false);
    try {
      const result = await remoteConnectAPI.accountConfigBackupExport();
      if (!mountedRef.current) return;
      setExportedCode(result.code);
      success(t('accountLogin.configBackup.exportSuccess'));
      await refreshStatus();
    } catch (error) {
      log.error('accountConfigBackupExport failed', error);
      if (mountedRef.current) setLoadError('accountLogin.configBackup.exportFailed');
    } finally {
      if (mountedRef.current) setExporting(false);
    }
  };

  const handleCopyCode = async () => {
    if (!exportedCode) return;
    try {
      await navigator.clipboard.writeText(exportedCode);
      setCodeCopied(true);
    } catch (error) {
      log.error('copy backup code failed', error);
    }
  };

  const handleImport = async () => {
    const code = importCode.trim();
    if (!code) return;
    const confirmed = await confirmDanger(
      t('accountLogin.configBackup.importTitle'),
      t('accountLogin.configBackup.importWarning'),
      {
        confirmText: t('accountLogin.configBackup.importAction'),
        cancelText: t('accountLogin.cancel'),
      },
    );
    if (!confirmed) return;
    setImporting(true);
    setImportError(null);
    try {
      const result = await remoteConnectAPI.accountConfigBackupImport(code);
      if (!mountedRef.current) return;
      if (result.success) {
        setImportCode('');
        success(t('accountLogin.configBackup.importSuccess'));
      } else {
        // Validation problems are data, not exceptions; surface them joined.
        setImportError(result.errors.join('\n') || t('accountLogin.configBackup.importFailed'));
      }
    } catch (error) {
      log.error('accountConfigBackupImport failed', error);
      if (mountedRef.current) {
        // Decryption and relay errors are loud; raw text stays in the log.
        setImportError('accountLogin.configBackup.importFailed');
      }
    } finally {
      if (mountedRef.current) setImporting(false);
    }
  };

  const handleDelete = async () => {
    const confirmed = await confirmDanger(
      t('accountLogin.configBackup.deleteTitle'),
      t('accountLogin.configBackup.deleteWarning'),
      {
        confirmText: t('accountLogin.configBackup.deleteAction'),
        cancelText: t('accountLogin.cancel'),
      },
    );
    if (!confirmed) return;
    setDeleting(true);
    try {
      await remoteConnectAPI.accountConfigBackupDelete();
      if (!mountedRef.current) return;
      setExportedCode(null);
      success(t('accountLogin.configBackup.deleteSuccess'));
      await refreshStatus();
    } catch (error) {
      log.error('accountConfigBackupDelete failed', error);
      if (mountedRef.current) setLoadError('accountLogin.configBackup.deleteFailed');
    } finally {
      if (mountedRef.current) setDeleting(false);
    }
  };

  if (status && !status.supportedByRelay) {
    return (
      <section className="account-panel__section" aria-labelledby="account-panel-config-backup-title">
        <div className="account-panel__section-heading">
          <span id="account-panel-config-backup-title" className="account-panel__section-title">
            {t('accountLogin.configBackup.title')}
          </span>
        </div>
        <div className="account-panel__card account-panel__backup-card">
          <Alert tone="info" message={t('accountLogin.configBackup.relayUnsupported')} />
        </div>
      </section>
    );
  }

  return (
    <section className="account-panel__section" aria-labelledby="account-panel-config-backup-title">
      <div className="account-panel__section-heading">
        <span id="account-panel-config-backup-title" className="account-panel__section-title">
          {t('accountLogin.configBackup.title')}
        </span>
      </div>
      <div className="account-panel__card account-panel__backup-card" data-openbitfun-component="remote-account-panel" data-openbitfun-part="configBackupCard">
        <p className="account-panel__backup-intro">{t('accountLogin.configBackup.intro')}</p>

        {loadError && <Alert tone="error" message={t(loadError)} />}

        {status?.exists && status.updatedAt != null && (
          <p className="account-panel__backup-state">
            {t('accountLogin.configBackup.existing', {
              time: formatRelativeTime(status.updatedAt * 1000),
            })}
          </p>
        )}
        {status && !status.exists && (
          <p className="account-panel__backup-state">{t('accountLogin.configBackup.none')}</p>
        )}

        {exportedCode && (
          <div className="account-panel__backup-code-block" role="status">
            <p className="account-panel__backup-code-warning">
              {t('accountLogin.configBackup.codeOnceWarning')}
            </p>
            <div className="account-panel__backup-code-line">
              <code className="account-panel__backup-code">{exportedCode}</code>
              <IconButton
                aria-label={t(codeCopied ? 'accountLogin.configBackup.codeCopied' : 'accountLogin.configBackup.copyCode')}
                icon={codeCopied ? <Check size={14} /> : <Copy size={14} />}
                onClick={() => void handleCopyCode()}
                size="sm"
                variant="quiet"
              />
            </div>
          </div>
        )}

        <div className="account-panel__backup-actions">
          <Button
            variant="outline"
            size="sm"
            leadingIcon={<Upload size={14} />}
            onClick={() => void handleExport()}
            disabled={exporting || importing || deleting}
            loading={exporting}
          >
            {t('accountLogin.configBackup.exportAction')}
          </Button>
          <Button
            variant="outline"
            size="sm"
            leadingIcon={<Trash2 size={14} />}
            onClick={() => void handleDelete()}
            disabled={!status?.exists || exporting || importing || deleting}
            loading={deleting}
          >
            {t('accountLogin.configBackup.deleteAction')}
          </Button>
        </div>

        <div className="account-panel__backup-import">
          <Input
            className="account-panel__backup-code-input"
            type="text"
            value={importCode}
            onChange={e => { setImportCode(e.target.value); setImportError(null); }}
            onKeyDown={event => {
              if (event.key === 'Enter') { event.preventDefault(); void handleImport(); }
            }}
            placeholder={t('accountLogin.configBackup.codePlaceholder')}
            aria-label={t('accountLogin.configBackup.codePlaceholder')}
            disabled={importing || exporting || deleting}
          />
          <Button
            variant="primary"
            size="sm"
            leadingIcon={<Download size={14} />}
            onClick={() => void handleImport()}
            disabled={!importCode.trim() || importing || exporting || deleting}
            loading={importing}
          >
            {t('accountLogin.configBackup.importAction')}
          </Button>
        </div>
        {importError && <Alert tone="error" message={importError} />}
        <p className="account-panel__backup-note">
          <DatabaseBackup size={14} aria-hidden="true" />
          {t('accountLogin.configBackup.importNote')}
        </p>
      </div>
    </section>
  );
};

export default AccountConfigBackupCard;
