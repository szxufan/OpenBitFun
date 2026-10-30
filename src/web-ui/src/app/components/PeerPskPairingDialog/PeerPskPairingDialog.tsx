import React, { useCallback, useEffect, useState } from 'react';
import {
  Button,
  Dialog,
  DialogBody,
  DialogClose,
  DialogHeader,
  DialogHeading,
  DialogTitle,
  Switch,
} from '@openbitfun/ui';
import { QRCodeSVG } from 'qrcode.react';
import { useI18n } from '@/infrastructure/i18n/hooks/useI18n';
import { createLogger } from '@/shared/utils/logger';
import {
  remoteConnectAPI,
  type PeerPskOffer,
  type PeerPskStatusEntry,
} from '@/infrastructure/api/service-api/RemoteConnectAPI';
import './PeerPskPairingDialog.scss';

const log = createLogger('PeerPskPairingDialog');

export interface PeerPskPairingTarget {
  deviceId: string;
  deviceName: string;
}

interface PeerPskPairingDialogProps {
  open: boolean;
  onClose: () => void;
  /** Device to generate a pairing code for; omit to show code entry only. */
  offerTarget?: PeerPskPairingTarget | null;
}

/**
 * Out-of-band PSK device pairing. Pairing upgrades a same-account peer link
 * from relay-directory trust to a shared secret transferred by the user, so a
 * malicious relay cannot impersonate either device. Traffic keeps the legacy
 * key until the peer completes the pairing.
 */
export const PeerPskPairingDialog: React.FC<PeerPskPairingDialogProps> = ({
  open,
  onClose,
  offerTarget,
}) => {
  const { t } = useI18n('common');
  const [offer, setOffer] = useState<PeerPskOffer | null>(null);
  const [offerError, setOfferError] = useState<string | null>(null);
  const [code, setCode] = useState('');
  const [completing, setCompleting] = useState(false);
  const [completeResult, setCompleteResult] = useState<string | null>(null);
  const [completeError, setCompleteError] = useState<string | null>(null);
  const [entries, setEntries] = useState<PeerPskStatusEntry[]>([]);
  const [requirePaired, setRequirePaired] = useState<boolean | null>(null);

  const refreshEntries = useCallback(() => {
    void remoteConnectAPI.listPeerPsks()
      .then(setEntries)
      .catch(error => log.warn('Could not list device pairings', error));
  }, []);

  useEffect(() => {
    if (!open) return undefined;
    setOffer(null);
    setOfferError(null);
    setCode('');
    setCompleteResult(null);
    setCompleteError(null);
    refreshEntries();
    void remoteConnectAPI.getPeerPskPolicy()
      .then(policy => setRequirePaired(policy.require_paired_controller))
      .catch(error => log.warn('Could not read pairing policy', error));
    return remoteConnectAPI.onPeerPskStatusChanged(() => refreshEntries());
  }, [open, refreshEntries]);

  const toggleRequirePaired = useCallback(async (value: boolean) => {
    setRequirePaired(value);
    try {
      const policy = await remoteConnectAPI.setPeerPskPolicy(value);
      setRequirePaired(policy.require_paired_controller);
    } catch (error) {
      log.error('Could not set pairing policy', error);
      void remoteConnectAPI.getPeerPskPolicy()
        .then(policy => setRequirePaired(policy.require_paired_controller))
        .catch(() => setRequirePaired(null));
    }
  }, []);

  useEffect(() => {
    if (!open || !offerTarget) return;
    let cancelled = false;
    setOffer(null);
    setOfferError(null);
    void remoteConnectAPI.createPeerPskOffer(offerTarget.deviceId, offerTarget.deviceName)
      .then(value => { if (!cancelled) setOffer(value); })
      .catch(error => {
        if (cancelled) return;
        log.error('Could not create device pairing offer', error);
        setOfferError(String((error as Error)?.message ?? error));
      });
    return () => { cancelled = true; };
  }, [open, offerTarget]);

  const submitCode = useCallback(async () => {
    if (!code.trim() || completing) return;
    setCompleting(true);
    setCompleteResult(null);
    setCompleteError(null);
    try {
      const result = await remoteConnectAPI.completePeerPsk(code.trim());
      setCompleteResult(result.confirmed
        ? t('accountLogin.pskPairing.completeConfirmed', { name: result.peerDeviceId })
        : t('accountLogin.pskPairing.completePending', { name: result.peerDeviceId }));
      setCode('');
      refreshEntries();
    } catch (error) {
      log.error('Could not complete device pairing', error);
      setCompleteError(String((error as Error)?.message ?? error));
    } finally {
      setCompleting(false);
    }
  }, [code, completing, refreshEntries, t]);

  const removeEntry = useCallback(async (peerDeviceId: string) => {
    try {
      await remoteConnectAPI.removePeerPsk(peerDeviceId);
      refreshEntries();
    } catch (error) {
      log.error('Could not remove device pairing', error);
    }
  }, [refreshEntries]);

  return (
    <Dialog
      open={open}
      onOpenChange={(nextOpen) => { if (!nextOpen) onClose(); }}
      size="lg"
      aria-label={t('accountLogin.pskPairing.title')}
      className="peer-psk-pairing-dialog"
    >
      <DialogHeader>
        <DialogTitle>
          <DialogHeading>
            {offerTarget
              ? t('accountLogin.pskPairing.titleWithDevice', { name: offerTarget.deviceName })
              : t('accountLogin.pskPairing.title')}
          </DialogHeading>
        </DialogTitle>
        <DialogClose />
      </DialogHeader>
      <DialogBody>
        {offerTarget && (
          <section
            className="peer-psk-pairing-dialog__section"
            data-testid="peer-psk-offer"
            data-openbitfun-component="peer-psk-pairing-dialog"
            data-openbitfun-part="offer"
          >
            <h4>{t('accountLogin.pskPairing.offerHeading')}</h4>
            <p className="peer-psk-pairing-dialog__hint">
              {t('accountLogin.pskPairing.offerHint')}
            </p>
            {offerError && <p className="peer-psk-pairing-dialog__error">{offerError}</p>}
            {offer && (
              <div className="peer-psk-pairing-dialog__offer">
                <QRCodeSVG value={offer.qrUrl} size={150} level="M" includeMargin />
                <textarea
                  className="peer-psk-pairing-dialog__code"
                  readOnly
                  value={offer.code}
                  onFocus={event => event.currentTarget.select()}
                  data-testid="peer-psk-offer-code"
                  data-openbitfun-component="peer-psk-pairing-dialog"
                  data-openbitfun-part="offerCode"
                />
                <div className="peer-psk-pairing-dialog__offer-actions">
                  <span className="peer-psk-pairing-dialog__psk-id">
                    {t('accountLogin.pskPairing.pskId', { id: offer.pskId })}
                  </span>
                  <Button
                    size="sm"
                    variant="secondary"
                    onClick={() => { void navigator.clipboard?.writeText(offer.code); }}
                  >
                    {t('accountLogin.pskPairing.copyCode')}
                  </Button>
                </div>
              </div>
            )}
          </section>
        )}
        <section
          className="peer-psk-pairing-dialog__section"
          data-testid="peer-psk-complete"
          data-openbitfun-component="peer-psk-pairing-dialog"
          data-openbitfun-part="complete"
        >
          <h4>{t('accountLogin.pskPairing.completeHeading')}</h4>
          <p className="peer-psk-pairing-dialog__hint">
            {t('accountLogin.pskPairing.completeHint')}
          </p>
          <textarea
            className="peer-psk-pairing-dialog__code"
            value={code}
            onChange={event => setCode(event.target.value)}
            placeholder={t('accountLogin.pskPairing.codePlaceholder')}
            data-testid="peer-psk-complete-input"
            data-openbitfun-component="peer-psk-pairing-dialog"
            data-openbitfun-part="completeCode"
          />
          {completeError && (
            <p className="peer-psk-pairing-dialog__error" data-testid="peer-psk-complete-error">
              {completeError}
            </p>
          )}
          {completeResult && <p className="peer-psk-pairing-dialog__ok">{completeResult}</p>}
          <Button
            size="sm"
            variant="primary"
            disabled={!code.trim() || completing}
            loading={completing}
            onClick={() => void submitCode()}
            data-testid="peer-psk-complete-submit"
          >
            {t('accountLogin.pskPairing.completeSubmit')}
          </Button>
        </section>
        <section
          className="peer-psk-pairing-dialog__section"
          data-testid="peer-psk-policy"
          data-openbitfun-component="peer-psk-pairing-dialog"
          data-openbitfun-part="policy"
        >
          <h4>{t('accountLogin.pskPairing.policyHeading')}</h4>
          <p className="peer-psk-pairing-dialog__hint">
            {t('accountLogin.pskPairing.policyHint')}
          </p>
          <Switch
            checked={requirePaired === true}
            disabled={requirePaired === null}
            onCheckedChange={value => void toggleRequirePaired(value)}
            data-testid="peer-psk-policy-toggle"
            aria-label={t('accountLogin.pskPairing.policyHeading')}
          />
        </section>
        <section
          className="peer-psk-pairing-dialog__section"
          data-testid="peer-psk-list"
          data-openbitfun-component="peer-psk-pairing-dialog"
          data-openbitfun-part="list"
        >
          <h4>{t('accountLogin.pskPairing.listHeading')}</h4>
          {entries.length === 0 && (
            <p className="peer-psk-pairing-dialog__hint">
              {t('accountLogin.pskPairing.listEmpty')}
            </p>
          )}
          {entries.map(entry => (
            <div
              key={entry.peer_device_id}
              className="peer-psk-pairing-dialog__entry"
              data-openbitfun-component="peer-psk-pairing-dialog"
              data-openbitfun-part="entry"
            >
              <span className="peer-psk-pairing-dialog__entry-name">
                {entry.peer_name || entry.peer_device_id}
              </span>
              <span className="peer-psk-pairing-dialog__entry-status">
                {entry.status === 'paired'
                  ? t('accountLogin.pskPairing.statusPaired')
                  : t('accountLogin.pskPairing.statusOffered')}
              </span>
              <Button
                size="sm"
                variant="text"
                onClick={() => void removeEntry(entry.peer_device_id)}
              >
                {t('accountLogin.pskPairing.remove')}
              </Button>
            </div>
          ))}
        </section>
      </DialogBody>
    </Dialog>
  );
};

export default PeerPskPairingDialog;
