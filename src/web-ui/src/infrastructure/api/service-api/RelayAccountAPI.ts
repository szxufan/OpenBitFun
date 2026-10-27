import { api } from './ApiClient';
import { createTauriCommandError } from '../errors/TauriCommandError';

export interface RelayAccountLoginResult {
  user_id: string;
}

/** Relay-local credential login: exactly one of `code` / `password`. */
export interface RelayAccountCredentials {
  phone: string;
  code?: string;
  password?: string;
}

/**
 * Relay-local account authentication commands. A login is either an SMS
 * verification code (registering the account on first use) or the optional
 * password the account set earlier. Errors carry the relay's message verbatim
 * (cooldown 429, registration closed, SMS not configured) in the command error
 * string.
 */
class RelayAccountAPI {
  async smsSend(phone: string): Promise<void> {
    try {
      await api.invoke<void>('account_sms_send', { request: { phone } });
    } catch (error) {
      throw createTauriCommandError('account_sms_send', error, { phone });
    }
  }

  async login(credentials: RelayAccountCredentials): Promise<RelayAccountLoginResult> {
    try {
      return await api.invoke<RelayAccountLoginResult>('account_login', {
        request: { ...credentials },
      });
    } catch (error) {
      throw createTauriCommandError('account_login', error, credentials);
    }
  }

  /**
   * Renew an expired device token from the persisted refresh token. A failure
   * means the user must sign in again with a new SMS code.
   */
  async refreshRelogin(): Promise<RelayAccountLoginResult> {
    try {
      return await api.invoke<RelayAccountLoginResult>('account_refresh_relogin', {});
    } catch (error) {
      throw createTauriCommandError('account_refresh_relogin', error);
    }
  }

  async setPassword(newPassword: string): Promise<void> {
    try {
      await api.invoke<void>('account_set_password', { request: { newPassword } });
    } catch (error) {
      throw createTauriCommandError('account_set_password', error);
    }
  }

  async logout(): Promise<void> {
    try {
      await api.invoke<void>('account_logout', {});
    } catch (error) {
      throw createTauriCommandError('account_logout', error);
    }
  }
}

export const relayAccountAPI = new RelayAccountAPI();
