/**
 * Cross-account shared-host card for the signed-in Remote Connect panel.
 *
 * Two roles on one account:
 *  - "Shared with me": machines another account granted to this one. Each row
 *    can be paired (paste the host's out-of-band PSK code, scoped to the grant)
 *    and entered as a Peer Device surface via the composite grant target.
 *  - "Shared by me": grants this machine offered out. Mint a pairing code for a
 *    grantee and revoke grants.
 *
 * A grant never widens the account device directory; entering a shared host uses
 * the host-computed composite `target_key`, so routing and surface identity stay
 * account-scoped. The relay only decides reachability — the shared host is the
 * authoritative policy point, and every refusal surfaces loudly, never as a
 * silent local fallback.
 */

import React, { useCallback, useEffect, useRef, useState } from 'react';
import { Alert, Button, IconButton, Input } from '@openbitfun/ui';
import { Check, Copy, KeyRound, Share2, Trash2 } from 'lucide-react';
import { useI18n } from '@/infrastructure/i18n';
import { confirmDanger } from '@/infrastructure/confirm-dialog';
import {
  remoteConnectAPI,
  type DeviceGrantList,
} from '@/infrastructure/api/service-api/RemoteConnectAPI';
import { usePeerDeviceMode } from '@/infrastructure/peer-device/peerDeviceContextState';
import { useNotification } from '@/shared/notification-system';
import { createLogger } from '@/shared/utils/logger';

const log = createLogger('SharedHostsCard');

interface SharedHostsCardProps {
  /** Called after entering a shared host so the dialog can close. */
  onEntered: () => void;
}

const SharedHostsCard: React.FC<SharedHostsCardProps> = ({ onEntered }) => {
  const { t } = useI18n('common');
  const { success } = useNotification();
  const { switchToDevice } = usePeerDeviceMode();

  const [supported, setSupported] = useState<boolean | null>(null);
  const [list, setList] = useState<DeviceGrantList | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [busyTarget, setBusyTarget] = useState<string | null>(null);

  // "Shared by me" offer form.
  const [grantee, setGrantee] = useState('');
  const [scope, setScope] = useState<'drive' | 'observe'>('drive');
  const [inviteSecret, setInviteSecret] = useState<string | null>(null);
  const [pairCode, setPairCode] = useState<string | null>(null);

  // "Shared with me" accept + pairing form.
  const [acceptGrant, setAcceptGrant] = useState('');
  const [acceptSecret, setAcceptSecret] = useState('');
  const [pasteCode, setPasteCode] = useState<Record<string, string>>({});

  const mountedRef = useRef(true);
  useEffect(() => {
    mountedRef.current = true;
    return () => { mountedRef.current = false; };
  }, []);

  const refresh = useCallback(async () => {
    setLoading(true);
    try {
      const capabilities = await remoteConnectAPI.accountRelayCapabilities();
      if (!mountedRef.current) return;
      const hasGrants = capabilities.includes('device_grants_v1');
      setSupported(hasGrants);
      if (!hasGrants) {
        setList(null);
        setLoadError(null);
        return;
      }
      const next = await remoteConnectAPI.peerGrantList();
      if (!mountedRef.current) return;
      setList(next);
      setLoadError(null);
    } catch (error) {
      log.error('shared hosts refresh failed', error);
      if (mountedRef.current) setLoadError(t('accountLogin.sharedHosts.loadFailed'));
    } finally {
      if (mountedRef.current) setLoading(false);
    }
  }, [t]);

  useEffect(() => { void refresh(); }, [refresh]);

  const runBusy = useCallback(async <T,>(target: string, action: () => Promise<T>): Promise<T | null> => {
    setBusyTarget(target);
    try {
      return await action();
    } catch (error) {
      log.error(`shared host action failed: ${target}`, error);
      if (mountedRef.current) setLoadError(t('accountLogin.sharedHosts.actionFailed'));
      return null;
    } finally {
      if (mountedRef.current) setBusyTarget(null);
    }
  }, [t]);

  const handleOffer = useCallback(async () => {
    const granteeId = grantee.trim();
    if (!granteeId) return;
    const result = await runBusy('offer', () => remoteConnectAPI.peerGrantOffer(granteeId, scope));
    if (!result) return;
    setInviteSecret(result.inviteSecret);
    setGrantee('');
    success(t('accountLogin.sharedHosts.offerSuccess'));
    await refresh();
  }, [grantee, refresh, runBusy, scope, success, t]);

  const handleRevoke = useCallback(async (grantId: string) => {
    const confirmed = await confirmDanger(
      t('accountLogin.sharedHosts.revokeTitle'),
      t('accountLogin.sharedHosts.revokeWarning'),
      { confirmText: t('accountLogin.sharedHosts.revokeAction'), cancelText: t('accountLogin.cancel') },
    );
    if (!confirmed) return;
    const ok = await runBusy(grantId, () => remoteConnectAPI.peerGrantRevoke(grantId));
    if (ok === null) return;
    success(t('accountLogin.sharedHosts.revokeSuccess'));
    await refresh();
  }, [refresh, runBusy, success, t]);

  const handleOfferPairCode = useCallback(async (grantId: string) => {
    const offer = await runBusy(grantId, () => remoteConnectAPI.createPeerPskOffer('', undefined, grantId));
    if (!offer) return;
    setPairCode(offer.code);
    success(t('accountLogin.sharedHosts.pairCodeReady'));
  }, [runBusy, success, t]);

  const handleAccept = useCallback(async () => {
    const id = acceptGrant.trim();
    const secret = acceptSecret.trim();
    if (!id || !secret) return;
    const result = await runBusy('accept', () => remoteConnectAPI.peerGrantAccept(id, secret));
    if (!result) return;
    setAcceptGrant('');
    setAcceptSecret('');
    success(t('accountLogin.sharedHosts.acceptSuccess'));
    await refresh();
  }, [acceptGrant, acceptSecret, refresh, runBusy, success, t]);

  const handleCompletePairing = useCallback(async (targetKey: string, hostDeviceId: string, grantId: string) => {
    const code = (pasteCode[hostDeviceId] ?? '').trim();
    if (!code) return;
    const result = await runBusy(targetKey, () => remoteConnectAPI.completePeerPsk(code, grantId));
    if (!result) return;
    setPasteCode(prev => ({ ...prev, [hostDeviceId]: '' }));
    success(result.confirmed
      ? t('accountLogin.sharedHosts.pairSuccess')
      : t('accountLogin.sharedHosts.pairPending'));
  }, [pasteCode, runBusy, success, t]);

  const handleEnter = useCallback(async (targetKey: string, deviceName: string) => {
    if (!targetKey) return;
    setBusyTarget(targetKey);
    try {
      const outcome = await switchToDevice(targetKey, deviceName);
      if (outcome === 'activated') onEntered();
    } catch (error) {
      log.error('enter shared host failed', error);
      if (mountedRef.current) setLoadError(t('accountLogin.sharedHosts.enterFailed'));
    } finally {
      if (mountedRef.current) setBusyTarget(null);
    }
  }, [onEntered, switchToDevice, t]);

  const handleCopy = useCallback(async (value: string) => {
    try {
      await navigator.clipboard.writeText(value);
      success(t('accountLogin.sharedHosts.copied'));
    } catch (error) {
      log.error('copy shared host secret failed', error);
    }
  }, [success, t]);

  if (supported === false) {
    return (
      <section className="account-panel__section" aria-labelledby="account-panel-shared-hosts-title">
        <div className="account-panel__section-heading">
          <span id="account-panel-shared-hosts-title" className="account-panel__section-title">
            {t('accountLogin.sharedHosts.title')}
          </span>
        </div>
        <div className="account-panel__card">
          <Alert tone="info" message={t('accountLogin.sharedHosts.relayUnsupported')} />
        </div>
      </section>
    );
  }

  const received = list?.received ?? [];
  const owned = (list?.owned ?? []).filter(grant => grant.state !== 'revoked');

  return (
    <section className="account-panel__section" aria-labelledby="account-panel-shared-hosts-title">
      <div className="account-panel__section-heading">
        <span id="account-panel-shared-hosts-title" className="account-panel__section-title">
          {t('accountLogin.sharedHosts.title')}
        </span>
        <Button variant="text" size="xs" onClick={() => void refresh()} disabled={loading}>
          {t('accountLogin.sharedHosts.refresh')}
        </Button>
      </div>
      <div className="account-panel__card" data-openbitfun-component="remote-account-panel" data-openbitfun-part="sharedHostsCard">
        <p className="account-panel__backup-intro">{t('accountLogin.sharedHosts.intro')}</p>
        {loadError && <Alert tone="error" message={loadError} />}

        {/* Shared with me — enter a granted host as a peer surface. */}
        <h4 className="account-panel__section-subtitle">{t('accountLogin.sharedHosts.receivedTitle')}</h4>
        {received.length === 0 && <p className="account-panel__backup-state">{t('accountLogin.sharedHosts.noReceived')}</p>}
        {received.map(entry => {
          const key = entry.target_key ?? '';
          const name = entry.device_name || entry.grant.device_id;
          const busy = busyTarget === key;
          const canEnter = Boolean(key) && entry.online && entry.compatible;
          return (
            <div className="account-panel__shared-row" key={entry.grant.grant_id}>
              <span className="account-panel__device-name">{name}</span>
              <span className="account-panel__device-status" data-openbitfun-state={entry.online ? 'online' : 'offline'}>
                {entry.online ? t('accountLogin.online') : t('accountLogin.offline')}
                {entry.grant.scope === 'observe' ? ` · ${t('accountLogin.sharedHosts.scopeObserve')}` : ''}
              </span>
              <div className="account-panel__shared-row-actions">
                <Input
                  className="account-panel__backup-code-input"
                  value={pasteCode[entry.grant.device_id] ?? ''}
                  onChange={e => setPasteCode(prev => ({ ...prev, [entry.grant.device_id]: e.target.value }))}
                  placeholder={t('accountLogin.sharedHosts.pairCodePlaceholder')}
                  aria-label={t('accountLogin.sharedHosts.pairCodePlaceholder')}
                  disabled={busy}
                />
                <IconButton
                  aria-label={t('accountLogin.sharedHosts.pairAction')}
                  icon={<KeyRound size={14} />}
                  onClick={() => void handleCompletePairing(key, entry.grant.device_id, entry.grant.grant_id)}
                  disabled={busy || !(pasteCode[entry.grant.device_id] ?? '').trim()}
                  title={t('accountLogin.sharedHosts.pairAction')}
                  size="sm"
                  variant="quiet"
                />
                <Button
                  variant="primary"
                  size="sm"
                  onClick={() => void handleEnter(key, name)}
                  disabled={busy || !canEnter}
                  loading={busy}
                >
                  {t('accountLogin.sharedHosts.enterAction')}
                </Button>
              </div>
            </div>
          );
        })}

        {/* Shared by me — offer this machine and manage outgoing grants. */}
        <h4 className="account-panel__section-subtitle">{t('accountLogin.sharedHosts.shareOutTitle')}</h4>
        <div className="account-panel__shared-row">
          <Input
            className="account-panel__backup-code-input"
            value={grantee}
            onChange={e => setGrantee(e.target.value)}
            placeholder={t('accountLogin.sharedHosts.granteePlaceholder')}
            aria-label={t('accountLogin.sharedHosts.granteePlaceholder')}
            disabled={busyTarget === 'offer'}
          />
          <select
            className="account-panel__shared-scope"
            value={scope}
            onChange={e => setScope(e.target.value === 'observe' ? 'observe' : 'drive')}
            aria-label={t('accountLogin.sharedHosts.scopeLabel')}
          >
            <option value="drive">{t('accountLogin.sharedHosts.scopeDrive')}</option>
            <option value="observe">{t('accountLogin.sharedHosts.scopeObserve')}</option>
          </select>
          <Button
            variant="outline"
            size="sm"
            leadingIcon={<Share2 size={14} />}
            onClick={() => void handleOffer()}
            disabled={!grantee.trim() || busyTarget === 'offer'}
          >
            {t('accountLogin.sharedHosts.offerAction')}
          </Button>
        </div>
        {inviteSecret && (
          <div className="account-panel__backup-code-line" role="status">
            <code className="account-panel__backup-code">{inviteSecret}</code>
            <IconButton
              aria-label={t('accountLogin.sharedHosts.copyInvite')}
              icon={<Copy size={14} />}
              onClick={() => void handleCopy(inviteSecret)}
              size="sm"
              variant="quiet"
            />
          </div>
        )}

        {owned.map(grant => (
          <div className="account-panel__shared-row" key={grant.grant_id}>
            <span className="account-panel__device-name">
              {`${t('accountLogin.sharedHosts.granteeLabel')}: ${grant.grantee_user_id}`}
            </span>
            <span className="account-panel__device-status">
              {grant.scope === 'observe' ? t('accountLogin.sharedHosts.scopeObserve') : t('accountLogin.sharedHosts.scopeDrive')}
            </span>
            <div className="account-panel__shared-row-actions">
              <IconButton
                aria-label={t('accountLogin.sharedHosts.offerPairCodeAction')}
                icon={<KeyRound size={14} />}
                onClick={() => void handleOfferPairCode(grant.grant_id)}
                disabled={busyTarget === grant.grant_id}
                title={t('accountLogin.sharedHosts.offerPairCodeAction')}
                size="sm"
                variant="quiet"
              />
              <IconButton
                aria-label={t('accountLogin.sharedHosts.revokeAction')}
                icon={<Trash2 size={14} />}
                onClick={() => void handleRevoke(grant.grant_id)}
                disabled={busyTarget === grant.grant_id}
                title={t('accountLogin.sharedHosts.revokeAction')}
                size="sm"
                variant="quiet"
              />
            </div>
          </div>
        ))}
        {pairCode && (
          <div className="account-panel__backup-code-block">
            <p className="account-panel__backup-code-warning">{t('accountLogin.sharedHosts.pairCodeHint')}</p>
            <div className="account-panel__backup-code-line">
              <code className="account-panel__backup-code">{pairCode}</code>
              <IconButton
                aria-label={t('accountLogin.sharedHosts.copied')}
                icon={<Check size={14} />}
                onClick={() => void handleCopy(pairCode)}
                size="sm"
                variant="quiet"
              />
            </div>
          </div>
        )}

        {/* Accept an invite received from another machine. */}
        <h4 className="account-panel__section-subtitle">{t('accountLogin.sharedHosts.acceptTitle')}</h4>
        <div className="account-panel__shared-row">
          <Input
            className="account-panel__backup-code-input"
            value={acceptGrant}
            onChange={e => setAcceptGrant(e.target.value)}
            placeholder={t('accountLogin.sharedHosts.acceptGrantPlaceholder')}
            aria-label={t('accountLogin.sharedHosts.acceptGrantPlaceholder')}
            disabled={busyTarget === 'accept'}
          />
          <Input
            className="account-panel__backup-code-input"
            value={acceptSecret}
            onChange={e => setAcceptSecret(e.target.value)}
            placeholder={t('accountLogin.sharedHosts.acceptSecretPlaceholder')}
            aria-label={t('accountLogin.sharedHosts.acceptSecretPlaceholder')}
            disabled={busyTarget === 'accept'}
          />
          <Button
            variant="outline"
            size="sm"
            onClick={() => void handleAccept()}
            disabled={!acceptGrant.trim() || !acceptSecret.trim() || busyTarget === 'accept'}
          >
            {t('accountLogin.sharedHosts.acceptAction')}
          </Button>
        </div>
      </div>
    </section>
  );
};

export default SharedHostsCard;
