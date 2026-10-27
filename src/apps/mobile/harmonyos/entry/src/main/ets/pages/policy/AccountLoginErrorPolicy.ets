import { CloudAccountRequestError, RelayLocalAuthUnsupportedError } from '../../services/CloudAccountClient';

interface AccountLoginFailure { code?: number; statusCode?: number; }

/** Present structured failures without exposing SDK messages or response bodies. */
export class AccountLoginErrorPolicy {
  /**
   * Refusal text the product or the relay authored for the user: relay error
   * bodies ("invalid verification code", "registration is closed on this
   * relay") and the local unsupported-relay state, already localized where
   * they were thrown. Empty for local transport failures, which map to
   * actionable copy instead of surfacing SDK messages.
   */
  static relayMessage(error: Object): string {
    if (error instanceof CloudAccountRequestError) return error.message;
    if (error instanceof RelayLocalAuthUnsupportedError) return error.message;
    return '';
  }

  /** Cooldown the relay attached to a 429, or 0 when it did not send one. */
  static retryAfterSecs(error: Object): number {
    if (error instanceof CloudAccountRequestError) {
      const secs = error.retryAfterSecs;
      if (typeof secs === 'number' && secs > 0) return Math.ceil(secs);
    }
    return 0;
  }

  static messageKey(error: Object): string {
    const failure = error as AccountLoginFailure;
    if (failure.code === 2300028) return 'account.errorTimeout';
    if (failure.code === 2300060 || failure.code === 2300035) return 'account.errorSecureConnection';
    if (failure.code !== undefined) return 'account.errorNetwork';
    if (failure.statusCode === 429) return 'account.errorRateLimited';
    return 'remote.settings.accountLoginFailed';
  }
}
