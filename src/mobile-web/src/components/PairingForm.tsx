import React, { useState } from 'react';
import { MobileBanner, MobileButton, MobileTextField } from '@openbitfun/ui/mobile';
import { KeyRound, LogIn, MonitorSmartphone } from 'lucide-react';
import { useI18n } from '../i18n';
import { normalizeAccountPhone } from '../services/CloudAccountClient';

export type LocalLoginMode = 'code' | 'password';
export type LocalAuthCapability = 'probing' | 'ready' | 'unsupported' | 'unreachable';

interface PairingFormProps {
  mode: LocalLoginMode;
  capability: LocalAuthCapability;
  busy: boolean;
  sending: boolean;
  cooldown: number;
  error: string | null;
  onSwitchMode: (mode: LocalLoginMode) => void;
  onSendCode: (phone: string) => void;
  onSubmit: (phone: string, credentials: { code: string } | { password: string }) => void;
}

/** Presentation for relay-local phone account sign-in. */
const PairingForm: React.FC<PairingFormProps> = ({
  mode, capability, busy, sending, cooldown, error, onSwitchMode, onSendCode, onSubmit,
}) => {
  const { t } = useI18n();
  const [phone, setPhone] = useState('');
  const [code, setCode] = useState('');
  const [password, setPassword] = useState('');
  // The form stays visible while the Relay contract is probed, but stays inert
  // until the local account contract is confirmed.
  const locked = capability !== 'ready';
  const normalizedPhone = normalizeAccountPhone(phone);
  const canSubmit = !locked && !busy && !sending && !!normalizedPhone
    && (mode === 'code' ? /^\d{6}$/.test(code) : password.length > 0);
  const canSend = !locked && !busy && !sending && !!normalizedPhone && cooldown <= 0;
  const submit = () => {
    if (!canSubmit || !normalizedPhone) return;
    onSubmit(normalizedPhone, mode === 'code' ? { code } : { password });
  };
  return (
    <form className="pairing-page__form" onSubmit={(event) => { event.preventDefault(); submit(); }}>
      <div className="pairing-page__scroll">
        <div className="pairing-page__form-content">
          <span className="pairing-page__symbol"><MonitorSmartphone size={24} aria-hidden="true" /></span>
          <h1 className="pairing-page__title">{t('pairing.loginTitle')}</h1>
          <p className="pairing-page__intro">{t('pairing.githubDescription')}</p>
          {error && <MobileBanner className="pairing-page__error" tone="danger">{error}</MobileBanner>}
          {capability === 'unsupported' && <MobileBanner className="pairing-page__error" tone="danger">
            {t('pairing.localAuthUnsupported')}
          </MobileBanner>}
          <label className="pairing-page__field">
            <span className="pairing-page__field-label">{t('pairing.phoneLabel')}</span>
            <MobileTextField
              appearance="surface"
              type="tel"
              inputMode="tel"
              autoComplete="tel"
              disabled={locked || busy}
              value={phone}
              onChange={(event) => setPhone(event.target.value)}
              placeholder={t('pairing.phonePlaceholder')}
            />
          </label>
          {mode === 'code' ? (
            <label className="pairing-page__field">
              <span className="pairing-page__field-label">{t('pairing.codeLabel')}</span>
              <MobileTextField
                appearance="surface"
                inputMode="numeric"
                autoComplete="one-time-code"
                maxLength={6}
                disabled={locked || busy}
                value={code}
                onChange={(event) => setCode(event.target.value.replace(/\D/g, '').slice(0, 6))}
                placeholder={t('pairing.codePlaceholder')}
              />
            </label>
          ) : (
            <label className="pairing-page__field">
              <span className="pairing-page__field-label">{t('pairing.passwordLabel')}</span>
              <MobileTextField
                appearance="surface"
                type="password"
                autoComplete="current-password"
                disabled={locked || busy}
                value={password}
                onChange={(event) => setPassword(event.target.value)}
                placeholder={t('pairing.passwordPlaceholder')}
              />
            </label>
          )}
        </div>
      </div>
      <div className="pairing-page__action">
        {mode === 'code' && (
          <MobileButton className="pairing-page__send-code" appearance="plain" block type="button"
            disabled={!canSend} loading={sending}
            onClick={() => { if (normalizedPhone) onSendCode(normalizedPhone); }}
            leading={<KeyRound size={18} aria-hidden="true" />}>
            {cooldown > 0 ? t('pairing.resendCodeIn', { seconds: cooldown }) : t('pairing.sendCode')}
          </MobileButton>
        )}
        <MobileButton className="pairing-page__submit" appearance="primary" block type="submit"
          disabled={!canSubmit} loading={busy}
          leading={<LogIn size={18} aria-hidden="true" />}>
          {t('pairing.githubSignIn')}
        </MobileButton>
        <MobileButton className="pairing-page__mode-toggle" appearance="plain" type="button"
          disabled={locked || busy}
          onClick={() => onSwitchMode(mode === 'code' ? 'password' : 'code')}>
          {t(mode === 'code' ? 'pairing.usePasswordLogin' : 'pairing.useCodeLogin')}
        </MobileButton>
      </div>
    </form>
  );
};
export default PairingForm;
