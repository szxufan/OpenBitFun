import React, { useEffect, useRef, useState } from 'react';
import { MobileStatus } from '@openbitfun/ui/mobile';
import PairingForm, { type LocalAuthCapability, type LocalLoginMode } from '../components/PairingForm';
import { accountDeviceIdFromHash, currentRelayUrl, pskPairingCodeFromHash } from '../services/pairingLink';
import { useI18n } from '../i18n';
import {
  CloudAccountClient, CloudAccountRequestError, type CloudAccountSession,
} from '../services/CloudAccountClient';
import {
  BrowserAccountChangedError, BrowserAccountStorageError, getBrowserAccountStore, releaseBrowserAccount,
  type BrowserAccountSnapshot, type BrowserAccountStore,
} from '../services/BrowserAccountStore';
import { RelayHttpClient } from '../services/RelayHttpClient';
import { PeerPskStore } from '../services/PeerPskStore';
import { RemoteSessionManager } from '../services/RemoteSessionManager';
import { loadMobileNavigation, type PairedNavigation } from '../services/MobileNavigationStore';
import { useMobileStore } from '../services/store';

export interface BrowserAccountBinding { store: BrowserAccountStore; token: string; }
interface PairingPageProps {
  onPaired: (client: RelayHttpClient, sessionMgr: RemoteSessionManager,
    preferredDeviceId?: string, navigation?: PairedNavigation, account?: BrowserAccountBinding) => void;
}

function routeKey(): string { return `${window.location.pathname}${window.location.hash}`; }

/** Mirrors the relay-enforced cooldown between two SMS sends. */
const SMS_RESEND_COOLDOWN_SECS = 60;

const PairingPageContent: React.FC<PairingPageProps> = ({ onPaired }) => {
  const { t } = useI18n();
  const relayUrl = currentRelayUrl();
  const accountStore = getBrowserAccountStore(relayUrl);
  const [restoring, setRestoring] = useState(true);
  const [busy, setBusy] = useState(false);
  const [sending, setSending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [capability, setCapability] = useState<LocalAuthCapability>('probing');
  const [mode, setMode] = useState<LocalLoginMode>('code');
  const [cooldown, setCooldown] = useState(0);
  const generation = useRef(0);
  const connected = useRef(false);
  const onPairedRef = useRef(onPaired);
  onPairedRef.current = onPaired;
  const targetDeviceId = accountDeviceIdFromHash(window.location.hash) || undefined;

  const friendlyError = (cause: unknown) => cause instanceof BrowserAccountStorageError
    ? t(cause.reason === 'invalid' ? 'pairing.browserStorageInvalid' : 'pairing.browserStorageUnavailable')
    : cause instanceof BrowserAccountChangedError ? t('pairing.signInChanged')
      : cause instanceof Error ? cause.message : t('pairing.loginFailed');

  const connect = (saved: BrowserAccountSnapshot, restore: boolean) => {
    const session = saved.session;
    if (!session || connected.current) return;
    connected.current = true;
    generation.current += 1;
    const controllerDeviceId = saved.controllerDeviceId;
    const client = new RelayHttpClient(relayUrl, { ...session, deviceId: controllerDeviceId });
    const store = useMobileStore.getState();
    store.resetForDeviceSwitch();
    store.setAuthenticatedUserId(session.userId);
    store.setAuthenticatedUserLabel(session.userId);
    store.setControlTarget(null);
    store.setConnectionStatus('paired');
    const scope = { accountId: session.userId, controllerDeviceId, relayUrl, routeKey: routeKey() };
    const navigation = restore ? loadMobileNavigation(scope) : null;
    onPairedRef.current(client, new RemoteSessionManager(client),
      navigation?.deviceId || targetDeviceId || undefined, { scope, restored: navigation },
      { store: accountStore, token: session.token });
    // A PSK pairing invitation carries the out-of-band code in the fragment.
    // Completing it verifies the offerer's directory key, stores the pairing,
    // and confirms over the PSK-sealed channel; traffic already handed off
    // switches derivations by the envelope rules, so this may run after.
    const pairingCode = pskPairingCodeFromHash(window.location.hash);
    if (pairingCode) {
      void client.completePskPairing(pairingCode)
        .then(result => console.info('[PairingPage] device pairing completed', result.peerDeviceId))
        .catch(error => console.warn('[PairingPage] device pairing failed', error));
    }
  };

  useEffect(() => {
    let disposed = false;
    let readGeneration = 0;
    const restore = async () => {
      const request = ++readGeneration;
      try {
        // Existing pairings must be in memory before any pairwise RPC picks
        // its derivation; failures degrade to the legacy path with a warning.
        await PeerPskStore.load(window);
        if (disposed || request !== readGeneration) return;
        const saved = await accountStore.read();
        try {
          if (disposed || request !== readGeneration || connected.current) return;
          if (saved.session) {
            connect(saved, true);
          } else if (saved.lastChange === 'expired') {
            setError(t('pairing.accountSessionExpired'));
          }
        } finally { releaseBrowserAccount(saved); }
      } catch (cause) {
        if (!disposed && request === readGeneration) setError(friendlyError(cause));
      } finally {
        if (!disposed && request === readGeneration) setRestoring(false);
      }
    };
    const unsubscribe = accountStore.subscribe(() => { void restore(); });
    void restore();
    return () => {
      disposed = true;
      unsubscribe();
      generation.current += 1;
    };
    // Each route owns its restore/subscription lifecycle. StrictMode's first
    // async restore is fenced by disposal before it can hand off a connection.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [accountStore]);

  // The local form follows the Relay's own capability contract: an older Relay
  // must answer with an explicit unsupported state, not a form that fails late.
  useEffect(() => {
    let disposed = false;
    new CloudAccountClient(relayUrl).supportsLocalAuth()
      .then(supported => { if (!disposed) setCapability(supported ? 'ready' : 'unsupported'); })
      .catch(() => { if (!disposed) setCapability('unreachable'); });
    return () => { disposed = true; };
  }, [relayUrl]);

  // One-second resend countdown, started by a successful send or a 429 answer.
  useEffect(() => {
    if (cooldown <= 0) return;
    const timer = window.setTimeout(() => setCooldown(value => value - 1), 1000);
    return () => window.clearTimeout(timer);
  }, [cooldown]);

  const smsErrorCopy = (cause: unknown) => (cause instanceof CloudAccountRequestError && cause.status === 503
    ? t('pairing.smsUnavailable')
    : friendlyError(cause));

  const loginErrorCopy = (cause: unknown, usingPassword: boolean) => {
    if (cause instanceof CloudAccountRequestError) {
      if (cause.status === 401) return usingPassword ? t('pairing.credentialsRejected') : t('pairing.codeRejected');
      if (cause.status === 403) return t('pairing.registrationClosed');
    }
    return friendlyError(cause);
  };

  const sendCode = async (phone: string) => {
    if (sending || busy || connected.current || restoring) return;
    const attempt = ++generation.current;
    setSending(true); setError(null);
    try {
      await new CloudAccountClient(relayUrl).smsSend(phone);
      if (generation.current === attempt) setCooldown(SMS_RESEND_COOLDOWN_SECS);
    } catch (cause) {
      if (generation.current !== attempt) return;
      if (cause instanceof CloudAccountRequestError && cause.status === 429) {
        const seconds = Math.max(1, Math.min(cause.retryAfterSeconds ?? SMS_RESEND_COOLDOWN_SECS, 3600));
        setCooldown(seconds);
        setError(t('pairing.tooManyAttempts', { seconds }));
        return;
      }
      setError(smsErrorCopy(cause));
    } finally {
      if (generation.current === attempt) setSending(false);
    }
  };

  const signIn = async (phone: string, credentials: { code: string } | { password: string }) => {
    if (busy || connected.current || restoring) return;
    const attempt = ++generation.current;
    const isCurrent = () => generation.current === attempt && !connected.current;
    setBusy(true); setError(null);
    const account = new CloudAccountClient(relayUrl);
    let browser: BrowserAccountSnapshot | null = null;
    let candidate: CloudAccountSession | null = null;
    let committed = false;
    try {
      browser = await accountStore.read();
      if (!isCurrent()) return;
      if (browser.session) { connect(browser, true); return; }
      // The relay-local pair authorizes the browser controller; the device
      // exchange then registers the SAME stored X25519 key as before.
      const pair = await account.localLogin(phone, credentials);
      if (!isCurrent()) return;
      candidate = await account.login(pair.accessToken, browser.controllerDeviceId, browser.privateKey);
      if (!isCurrent()) return;
      const saved = await accountStore.saveSession(browser, candidate, isCurrent, pair.refreshToken);
      committed = true;
      try { if (isCurrent()) connect(saved, false); }
      finally { releaseBrowserAccount(saved); }
    } catch (cause) {
      if (isCurrent()) setError(loginErrorCopy(cause, 'password' in credentials));
    } finally {
      if (candidate) {
        if (!committed) {
          void account.logout(candidate.token).catch(() => {
            console.warn('Could not revoke an unused browser sign-in token');
          });
        }
        candidate.masterKey.fill(0);
      }
      if (browser) releaseBrowserAccount(browser);
      if (generation.current === attempt) setBusy(false);
    }
  };

  return <div className="pairing-page"><div className="pairing-page__shell">
    <div className="pairing-page__brand">
      <img src={`${import.meta.env.BASE_URL}brand/openbitfun-app-icon.png`} alt="" width="40" height="40" />
      <span>OpenBitFun</span>
    </div>
    <section className="pairing-page__panel">
      {restoring ? <MobileStatus loading title={t('pairing.restoringAccount')} />
        : <PairingForm mode={mode} capability={capability} busy={busy} sending={sending}
          cooldown={cooldown} error={error} onSwitchMode={setMode}
          onSendCode={phone => void sendCode(phone)}
          onSubmit={(phone, credentials) => void signIn(phone, credentials)} />}
    </section>
  </div></div>;
};

const PairingPage: React.FC<PairingPageProps> = (props) => {
  const [route, setRoute] = useState(routeKey);
  useEffect(() => {
    const change = () => setRoute(routeKey());
    window.addEventListener('hashchange', change);
    return () => window.removeEventListener('hashchange', change);
  }, []);
  return <PairingPageContent key={route} {...props} />;
};
export default PairingPage;
