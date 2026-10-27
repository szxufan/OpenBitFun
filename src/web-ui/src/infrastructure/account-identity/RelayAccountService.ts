import { api } from '@/infrastructure/api/service-api/ApiClient';
import { relayAccountAPI, type RelayAccountLoginResult } from '@/infrastructure/api/service-api/RelayAccountAPI';
import { remoteConnectAPI } from '@/infrastructure/api/service-api/RemoteConnectAPI';

/**
 * Relay-local account identity: an SMS-code (or optional password) account on
 * the configured relay. This service observes the host's account session and
 * owns the credential sign-in actions; it replaces the retired GitHub OAuth
 * identity.
 */

export type RelayAccountStatus = 'loading' | 'signed-out' | 'signed-in';

export interface RelayAccountUser {
  /** Stable relay account id; equals the relay session `user_id`. */
  id: string;
  /** Display name: the masked phone from the credential hint, else the id. */
  login: string;
  /** Relay-local accounts have no avatar; consumers fall back to a letter. */
  avatarUrl: '';
}

export interface RelayAccountMe {
  /** Kept for snapshot-shape compatibility; relay accounts have no email. */
  email?: undefined;
  user: RelayAccountUser;
}

export interface RelayAccountSnapshot {
  resolved: boolean;
  status: RelayAccountStatus;
  me: RelayAccountMe | null;
  lastError?: Error;
}

export interface RelayAccountStatusView {
  logged_in: boolean;
  user_id?: string | null;
}

export interface RelayAccountCredentialHint {
  username: string;
  relay_url: string;
}

/** Commands and host events the service is built on. */
export interface RelayAccountApi {
  status(): Promise<RelayAccountStatusView>;
  credentialHint(): Promise<RelayAccountCredentialHint | null>;
  smsSend(phone: string): Promise<void>;
  loginWithCode(phone: string, code: string): Promise<RelayAccountLoginResult>;
  loginWithPassword(phone: string, password: string): Promise<RelayAccountLoginResult>;
  refreshRelogin(): Promise<RelayAccountLoginResult>;
  setPassword(newPassword: string): Promise<void>;
  logout(): Promise<void>;
  onLoginState?(handler: (payload: { logged_in: boolean }) => void): () => void;
}

export interface RelayAccountChangedEvent {
  kind: 'identity-changed';
  eventId: string;
  sourceId: string;
}

export interface RelayAccountSyncPort {
  publish(event: RelayAccountChangedEvent): void | Promise<void>;
  subscribe(listener: (event: RelayAccountChangedEvent) => void): () => void;
  dispose?(): void;
}

const noopSyncPort: RelayAccountSyncPort = {
  publish: () => undefined,
  subscribe: () => () => undefined,
};

function isRelayAccountChangedEvent(value: unknown): value is RelayAccountChangedEvent {
  if (!value || typeof value !== 'object' || Array.isArray(value)) return false;
  const event = value as Record<string, unknown>;
  return event.kind === 'identity-changed'
    && typeof event.eventId === 'string'
    && typeof event.sourceId === 'string';
}

class BroadcastRelayAccountSyncPort implements RelayAccountSyncPort {
  private readonly listeners = new Set<(event: RelayAccountChangedEvent) => void>();

  constructor(private readonly channel: BroadcastChannel) {
    channel.addEventListener('message', this.handleMessage);
  }

  publish(event: RelayAccountChangedEvent): void {
    this.channel.postMessage(event);
  }

  subscribe(listener: (event: RelayAccountChangedEvent) => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  dispose(): void {
    this.channel.removeEventListener('message', this.handleMessage);
    this.channel.close();
    this.listeners.clear();
  }

  private readonly handleMessage = (message: MessageEvent<unknown>): void => {
    const event = message.data;
    if (!isRelayAccountChangedEvent(event)) return;
    this.listeners.forEach(listener => listener(event));
  };
}

export function createRelayAccountSyncPort(): RelayAccountSyncPort {
  if (typeof BroadcastChannel === 'undefined') return noopSyncPort;
  try {
    // Same channel as the retired GitHub identity so a mixed-version upgrade
    // still syncs the shared "identity changed" ping across windows.
    return new BroadcastRelayAccountSyncPort(new BroadcastChannel('openbitfun-account-identity'));
  } catch {
    return noopSyncPort;
  }
}

export interface RelayAccountServiceDependencies {
  api: RelayAccountApi;
  syncPort: RelayAccountSyncPort;
  sourceId: string;
}

function createId(): string {
  if (typeof crypto !== 'undefined' && typeof crypto.randomUUID === 'function') {
    return crypto.randomUUID();
  }
  return `${Date.now()}-${Math.random().toString(36).slice(2)}`;
}

const defaultApi = (): RelayAccountApi => ({
  status: () => remoteConnectAPI.accountStatus(),
  credentialHint: () => remoteConnectAPI.accountGetCredentialHint(),
  smsSend: phone => relayAccountAPI.smsSend(phone),
  loginWithCode: (phone, code) => relayAccountAPI.login({ phone, code }),
  loginWithPassword: (phone, password) => relayAccountAPI.login({ phone, password }),
  refreshRelogin: () => relayAccountAPI.refreshRelogin(),
  setPassword: newPassword => relayAccountAPI.setPassword(newPassword),
  logout: () => relayAccountAPI.logout(),
  onLoginState: handler => api.listen<{ logged_in: boolean }>('account://login-state', payload => {
    if (payload && typeof payload.logged_in === 'boolean') handler(payload);
  }),
});

const defaultDependencies = (): RelayAccountServiceDependencies => ({
  api: defaultApi(),
  syncPort: createRelayAccountSyncPort(),
  sourceId: createId(),
});

export class RelayAccountService {
  private snapshot: RelayAccountSnapshot = Object.freeze({
    resolved: false,
    status: 'loading',
    me: null,
  });
  private readonly listeners = new Set<(snapshot: RelayAccountSnapshot) => void>();
  private initializePromise: Promise<void> | null = null;
  private statusPromise: Promise<RelayAccountStatusView> | null = null;
  /** Fences status responses that started before a login/logout transition. */
  private generation = 0;
  /** Cached masked phone for the currently signed-in account, if any. */
  private loginHint: { userId: string; username: string } | null = null;
  private stopSync: (() => void) | null = null;
  private stopLoginStateEvents: (() => void) | null = null;

  constructor(private readonly dependencies: RelayAccountServiceDependencies = defaultDependencies()) {}

  getSnapshot(): RelayAccountSnapshot {
    return this.snapshot;
  }

  subscribe(listener: (snapshot: RelayAccountSnapshot) => void): () => void {
    this.listeners.add(listener);
    void this.initialize();
    return () => this.listeners.delete(listener);
  }

  initialize(): Promise<void> {
    if (this.initializePromise) return this.initializePromise;
    this.stopSync = this.dependencies.syncPort.subscribe(event => {
      if (event.sourceId === this.dependencies.sourceId) return;
      void this.refresh(false).catch(() => undefined);
    });
    this.stopLoginStateEvents = this.dependencies.api.onLoginState?.(() => {
      void this.refresh(false).catch(() => undefined);
    }) ?? null;
    const generation = this.generation;
    this.initializePromise = this.refresh()
      .then(() => undefined)
      .catch(error => {
        if (generation !== this.generation) return;
        this.setSnapshot({
          resolved: true,
          status: 'signed-out',
          me: null,
          lastError: error instanceof Error ? error : new Error(String(error)),
        });
      });
    return this.initializePromise;
  }

  /** Re-read the host account session and reconcile the snapshot with it. */
  refresh(broadcastChange = true): Promise<RelayAccountStatusView> {
    if (this.statusPromise) return this.statusPromise;
    const previous = this.snapshot;
    const generation = this.generation;
    const operation = this.dependencies.api.status()
      .then(status => {
        if (generation !== this.generation) return status;
        const me = meFromStatus(status, this.loginHint);
        this.setSnapshot({
          resolved: true,
          status: me ? 'signed-in' : 'signed-out',
          me,
          ...(me ? {} : { lastError: undefined }),
        });
        if (me && this.loginHint?.userId !== me.user.id) {
          this.loadLoginHint(me.user.id);
        }
        if (broadcastChange
          && previous.resolved
          && identityKey(previous.me) !== identityKey(me)) {
          void this.publishIdentityChanged();
        }
        return status;
      })
      .finally(() => {
        if (this.statusPromise === operation) this.statusPromise = null;
      });
    this.statusPromise = operation;
    return operation;
  }

  /** Request an SMS verification code for `phone`; relay errors propagate. */
  sendSmsCode(phone: string): Promise<void> {
    return this.dependencies.api.smsSend(phone);
  }

  /** Sign in (registering on first use) with an SMS verification code. */
  async signInWithCode(phone: string, code: string): Promise<RelayAccountLoginResult> {
    const result = await this.dependencies.api.loginWithCode(phone, code);
    await this.adoptLogin(result);
    return result;
  }

  /** Sign in with the account's optional password. */
  async signInWithPassword(phone: string, password: string): Promise<RelayAccountLoginResult> {
    const result = await this.dependencies.api.loginWithPassword(phone, password);
    await this.adoptLogin(result);
    return result;
  }

  /**
   * Renew an expired device token from the persisted refresh token. Rejects
   * when no refresh token survives; the caller must ask for a new SMS code.
   */
  async relogin(): Promise<RelayAccountLoginResult> {
    const result = await this.dependencies.api.refreshRelogin();
    await this.adoptLogin(result);
    return result;
  }

  /** Set or change the optional password of the signed-in account. */
  setPassword(newPassword: string): Promise<void> {
    return this.dependencies.api.setPassword(newPassword);
  }

  async logout(): Promise<void> {
    this.invalidateStatus();
    await this.dependencies.api.logout();
    this.invalidateStatus();
    this.loginHint = null;
    this.setSnapshot({ resolved: true, status: 'signed-out', me: null });
    await this.publishIdentityChanged();
  }

  dispose(): void {
    this.invalidateStatus();
    this.stopSync?.();
    this.stopSync = null;
    this.stopLoginStateEvents?.();
    this.stopLoginStateEvents = null;
    this.dependencies.syncPort.dispose?.();
    this.listeners.clear();
  }

  private invalidateStatus(): void {
    this.generation += 1;
    this.statusPromise = null;
  }

  /** Commit a fresh login result and refresh the display hint in the background. */
  private async adoptLogin(result: RelayAccountLoginResult): Promise<void> {
    this.invalidateStatus();
    this.loginHint = null;
    this.setSnapshot({
      resolved: true,
      status: 'signed-in',
      me: {
        user: {
          id: result.user_id,
          login: result.user_id,
          avatarUrl: '',
        },
      },
    });
    this.loadLoginHint(result.user_id);
    await this.publishIdentityChanged();
  }

  /** Replace the raw user id with the masked phone once the hint answers. */
  private loadLoginHint(userId: string): void {
    void this.dependencies.api.credentialHint()
      .then(hint => {
        const username = hint?.username?.trim();
        if (!username) return;
        if (this.snapshot.me?.user.id !== userId) return;
        if (this.snapshot.me.user.login === username) return;
        this.loginHint = { userId, username };
        this.setSnapshot({
          ...this.snapshot,
          me: {
            user: {
              id: userId,
              login: username,
              avatarUrl: '',
            },
          },
        });
      })
      .catch(() => undefined);
  }

  private async publishIdentityChanged(): Promise<void> {
    await this.dependencies.syncPort.publish({
      kind: 'identity-changed',
      eventId: createId(),
      sourceId: this.dependencies.sourceId,
    });
  }

  private setSnapshot(snapshot: RelayAccountSnapshot): void {
    this.snapshot = Object.freeze(snapshot);
    this.listeners.forEach(listener => listener(this.snapshot));
  }
}

function meFromStatus(
  status: RelayAccountStatusView,
  hint: { userId: string; username: string } | null,
): RelayAccountMe | null {
  if (!status.logged_in) return null;
  const id = status.user_id?.trim();
  if (!id) return null;
  return {
    user: {
      id,
      login: hint?.userId === id ? hint.username : id,
      avatarUrl: '',
    },
  };
}

function identityKey(me: RelayAccountMe | null): string {
  return me ? me.user.id : '';
}

export const relayAccountService = new RelayAccountService();
