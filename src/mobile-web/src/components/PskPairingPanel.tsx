import React, { useCallback, useEffect, useState } from 'react';
import { MobileBanner, MobileButton, MobileCard, MobileStatus } from '@openbitfun/ui/mobile';
import { useI18n } from '../i18n';
import type { RelayHttpClient } from '../services/RelayHttpClient';
import type { PeerPskListing } from '../services/PeerPskStore';
import { PSK_KEY_MISMATCH_MESSAGE } from '../../../shared/relay-transport/HostStream';

/**
 * Out-of-band PSK device pairing on the relay-hosted web client.
 *
 * Pairing upgrades a same-account peer link from relay-directory trust to a
 * shared secret the user transfers between devices, so a malicious relay
 * cannot impersonate either device. Hosts that switched on enforcement refuse
 * unpaired controllers entirely, which makes this panel the way a web
 * controller gets in at all.
 */
export const PskPairingPanel: React.FC<{ client: RelayHttpClient }> = ({ client }) => {
  const { t } = useI18n();
  const [pairings, setPairings] = useState<PeerPskListing[]>(() => client.listPskPairings());
  const [offer, setOffer] = useState<{ code: string; pskId: string } | null>(null);
  const [offering, setOffering] = useState(false);
  const [code, setCode] = useState('');
  const [completing, setCompleting] = useState(false);
  const [result, setResult] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [removingId, setRemovingId] = useState<string | null>(null);

  const refresh = useCallback(() => setPairings(client.listPskPairings()), [client]);

  useEffect(() => { refresh(); }, [refresh]);

  const createOffer = useCallback(async () => {
    setOffering(true);
    setError(null);
    setResult(null);
    try {
      const value = await client.createPskOffer();
      setOffer({ code: value.code, pskId: value.pskId });
      refresh();
    } catch (cause) {
      setError((cause as Error)?.message ?? String(cause));
    } finally {
      setOffering(false);
    }
  }, [client, refresh]);

  const submitCode = useCallback(async () => {
    if (!code.trim() || completing) return;
    setCompleting(true);
    setError(null);
    setResult(null);
    try {
      const done = await client.completePskPairing(code.trim());
      setResult(done.confirmed
        ? t('devices.pskCompleteConfirmed', { device: done.peerDeviceId })
        : t('devices.pskCompletePending', { device: done.peerDeviceId }));
      setCode('');
      setOffer(null);
      refresh();
    } catch (cause) {
      const message = (cause as Error)?.message ?? String(cause);
      setError(message === PSK_KEY_MISMATCH_MESSAGE
        ? t('devices.pskMismatch')
        : message);
    } finally {
      setCompleting(false);
    }
  }, [client, code, completing, refresh, t]);

  const remove = useCallback(async (peerDeviceId: string) => {
    setRemovingId(peerDeviceId);
    try {
      await client.removePskPairing(peerDeviceId);
      refresh();
    } catch (cause) {
      setError((cause as Error)?.message ?? String(cause));
    } finally {
      setRemovingId(null);
    }
  }, [client, refresh]);

  const copyCode = useCallback(() => {
    if (!offer) return;
    void navigator.clipboard?.writeText(offer.code).catch(() => undefined);
  }, [offer]);

  return (
    <MobileCard appearance="elevated" className="devices-page__psk-panel" data-testid="psk-pairing-panel">
      <div className="devices-page__psk-heading">
        <h3>{t('devices.pskTitle')}</h3>
        <p className="devices-page__psk-hint">{t('devices.pskHint')}</p>
      </div>

      {error && <MobileBanner tone="danger">{error}</MobileBanner>}
      {result && <MobileStatus title={result} />}

      <div className="devices-page__psk-actions">
        <MobileButton size="sm" appearance="secondary" loading={offering} onClick={() => void createOffer()}>
          {t('devices.pskOffer')}
        </MobileButton>
      </div>

      {offer && (
        <div className="devices-page__psk-offer" data-testid="psk-offer">
          <p className="devices-page__psk-hint">{t('devices.pskOfferHint')}</p>
          <textarea
            className="devices-page__psk-code"
            readOnly
            value={offer.code}
            onFocus={event => event.currentTarget.select()}
            aria-label={t('devices.pskOfferCode')}
          />
          <div className="devices-page__psk-row">
            <small>{t('devices.pskId', { id: offer.pskId })}</small>
            <MobileButton size="sm" onClick={copyCode}>{t('devices.pskCopy')}</MobileButton>
          </div>
        </div>
      )}

      <div className="devices-page__psk-complete" data-testid="psk-complete">
        <p className="devices-page__psk-hint">{t('devices.pskCompleteHint')}</p>
        <textarea
          className="devices-page__psk-code"
          value={code}
          onChange={event => setCode(event.target.value)}
          placeholder={t('devices.pskCodePlaceholder')}
        />
        <div className="devices-page__psk-row">
          <span />
          <MobileButton
            size="sm"
            appearance="primary"
            disabled={!code.trim()}
            loading={completing}
            onClick={() => void submitCode()}
          >
            {t('devices.pskComplete')}
          </MobileButton>
        </div>
      </div>

      {pairings.length > 0 && (
        <div className="devices-page__psk-list" data-testid="psk-list">
          <h4>{t('devices.pskList')}</h4>
          {pairings.map(entry => (
            <div key={entry.peerDeviceId} className="devices-page__psk-entry">
              <span className="devices-page__psk-entry-name">{entry.peerDeviceId}</span>
              <small>{entry.status === 'paired' ? t('devices.pskPaired') : t('devices.pskOffered')}</small>
              <MobileButton
                size="sm"
                appearance="plain"
                loading={removingId === entry.peerDeviceId}
                onClick={() => void remove(entry.peerDeviceId)}
              >
                {t('devices.pskRemove')}
              </MobileButton>
            </div>
          ))}
        </div>
      )}
    </MobileCard>
  );
};

export default PskPairingPanel;
