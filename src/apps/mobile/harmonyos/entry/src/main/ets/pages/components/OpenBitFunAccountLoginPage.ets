import { AccountLoginErrorPolicy } from '../policy/AccountLoginErrorPolicy';
import { MobileDesignGeometry, MobileDesignTypography } from '../../generated/MobileDesignTokens';
import { RemoteI18n } from '../../i18n/RemoteI18n';
import {
  LocalAccountCredential,
  DEFAULT_CLOUD_RELAY_URL
} from '../../services/CloudAccountClient';
import {
  normalizeAccountRelayUrl,
  normalizeManualRelayUrl
} from '../../services/AccountDeviceLink';
import { AppActionButton, AppActionStyle } from './AppActionButton';
import {
  CARD,
  INK,
  LINE,
  MUTED,
  SHADOW_FAINT,
  STATUS_DANGER,
  SUBTLE
} from './Theme';
import { SheetActionFooter } from './SheetActionFooter';
import { SheetCloseHeader } from './SheetCloseHeader';
import {
  SHEET_CONTENT_BOTTOM_PADDING,
  SHEET_CONTENT_MAX_WIDTH,
  SHEET_HORIZONTAL_PADDING
} from './SheetLayout';

/** Resend countdown the form applies when the relay sends no cooldown of its own. */
const SEND_CODE_COUNTDOWN_SECS: number = 60;
const FIELD_HEIGHT: number = 52;
const SEND_CODE_BUTTON_WIDTH: number = 128;

/**
 * Relay capability state for the inline form. Only `supported` enables the
 * phone-number actions; every other value is an explicit, explained state.
 */
type RelaySupportState = 'idle' | 'checking' | 'supported' | 'unsupported' | 'invalid';

@ComponentV2
export struct OpenBitFunAccountLoginPage {
  @Event cloudRelayUrl: () => string = (): string => DEFAULT_CLOUD_RELAY_URL;
  @Event cloudProbeRelay: (relayUrl: string) => Promise<boolean> =
    async (_relayUrl: string): Promise<boolean> => false;
  @Event cloudSmsSend: (relayUrl: string, phone: string) => Promise<void> =
    async (_relayUrl: string, _phone: string): Promise<void> => {};
  @Event cloudLogin: (credential: LocalAccountCredential) => Promise<string> =
    async (_credential: LocalAccountCredential): Promise<string> => '';
  @Event cloudCancelLogin: () => void = () => {};
  @Event onClose: () => void = () => {};
  @Event onLoginSuccess: () => void = () => {};
  @Local relayUrl: string = '';
  @Local relayCheck: RelaySupportState = 'idle';
  @Local phone: string = '';
  @Local code: string = '';
  @Local password: string = '';
  @Local usePassword: boolean = false;
  @Local errorText: string = '';
  @Local isBusy: boolean = false;
  @Local isSendingCode: boolean = false;
  @Local resendInSecs: number = 0;
  private countdownTimer: number = -1;

  aboutToAppear(): void {
    this.relayUrl = this.cloudRelayUrl() || DEFAULT_CLOUD_RELAY_URL;
    void this.probeRelay();
  }

  aboutToDisappear(): void {
    if (this.countdownTimer !== -1) {
      clearTimeout(this.countdownTimer);
      this.countdownTimer = -1;
    }
    this.cloudCancelLogin();
  }

  build() {
    Column({ space: 0 }) {
      SheetCloseHeader({ onClose: this.onClose, uniformGlyph: true })

      Scroll() {
        Column() {
          this.LoginForm()
        }
        .width('100%')
        .padding({
          left: SHEET_HORIZONTAL_PADDING,
          right: SHEET_HORIZONTAL_PADDING,
          top: 0,
          bottom: SHEET_CONTENT_BOTTOM_PADDING
        })
        .alignItems(HorizontalAlign.Center)
      }
      .width('100%')
      .scrollBar(BarState.Off)

      SheetActionFooter({
        label: this.isBusy ? RemoteI18n.t('remote.settings.accountSigningIn') :
          RemoteI18n.t('remote.settings.accountSignIn'),
        primary: true,
        elevated: false,
        isEnabled: this.canSubmit(),
        onAction: (): void => {
          void this.submit();
        }
      })
    }
    .width('100%')

  }

  @Builder
  private LoginForm() {
    Column({ space: 0 }) {
      Text(RemoteI18n.t('remote.settings.accountLoginTitle'))
        .fontSize(MobileDesignTypography.displayMedium.size)
        .lineHeight(MobileDesignTypography.displayMedium.lineHeight)
        .fontWeight(FontWeight.Bold)
        .fontColor(INK)
        .width('100%')
        .textAlign(TextAlign.Center)

      Text(RemoteI18n.t('remote.settings.accountLoginBody'))
        .fontSize(MobileDesignTypography.bodyMedium.size)
        .lineHeight(MobileDesignTypography.bodyMedium.lineHeight)
        .fontColor(MUTED)
        .width('100%')
        .textAlign(TextAlign.Center)
        .margin({ top: 8 })

      this.RelayField()
      this.PhoneField()
      if (this.usePassword) {
        this.PasswordField()
      } else {
        this.CodeField()
      }
      this.CredentialModeToggle()

      if (this.errorText.length > 0) {
        Text(this.errorText)
          .fontSize(MobileDesignTypography.bodySmall.size)
          .lineHeight(MobileDesignTypography.bodySmall.lineHeight)
          .fontColor(STATUS_DANGER)
          .width('100%')
          .margin({ top: 12, left: 4, right: 4 })
      }
    }
    .width('100%')
    .constraintSize({ maxWidth: SHEET_CONTENT_MAX_WIDTH, minHeight: MobileDesignGeometry.loginSheetBodyMinHeight })
    .alignItems(HorizontalAlign.Center)
  }

  @Builder
  private RelayField() {
    Column({ space: 8 }) {
      Text(RemoteI18n.t('remote.settings.loginServer'))
        .fontSize(MobileDesignTypography.bodyMedium.size)
        .fontWeight(FontWeight.Medium)
        .fontColor(INK)
        .width('100%')
      TextInput({
        placeholder: RemoteI18n.t('remote.settings.relayUrlPlaceholder'),
        text: this.relayUrl
      })
        .width('100%')
        .height(FIELD_HEIGHT)
        .fontSize(MobileDesignTypography.titleSmall.size)
        .fontColor(INK)
        .placeholderColor(SUBTLE)
        .backgroundColor(CARD)
        .border({ width: 0.5, color: LINE })
        .borderRadius(12)
        .shadow({ radius: 12, color: SHADOW_FAINT, offsetY: 4 })
        .padding({ left: 14, right: 14 })
        .onChange((value: string) => {
          this.relayUrl = value;
          this.relayCheck = 'idle';
          this.errorText = '';
        })
        .onBlur(() => {
          void this.probeRelay();
        })
        .onSubmit(() => {
          void this.probeRelay();
        })
      this.RelayProbeStatus()
    }
    .width('100%')
    .alignItems(HorizontalAlign.Start)
    .margin({ top: 24 })
  }

  @Builder
  private RelayProbeStatus() {
    if (this.relayCheck === 'checking') {
      Text(RemoteI18n.t('account.loginServerChecking'))
        .fontSize(MobileDesignTypography.bodySmall.size)
        .lineHeight(MobileDesignTypography.bodySmall.lineHeight)
        .fontColor(MUTED)
        .width('100%')
    } else if (this.relayCheck === 'unsupported') {
      Text(RemoteI18n.t('account.loginServerUnsupported'))
        .fontSize(MobileDesignTypography.bodySmall.size)
        .lineHeight(MobileDesignTypography.bodySmall.lineHeight)
        .fontColor(STATUS_DANGER)
        .width('100%')
    } else if (this.relayCheck === 'invalid') {
      Text(RemoteI18n.t('account.errorInvalidRelay'))
        .fontSize(MobileDesignTypography.bodySmall.size)
        .lineHeight(MobileDesignTypography.bodySmall.lineHeight)
        .fontColor(STATUS_DANGER)
        .width('100%')
    }
  }

  @Builder
  private PhoneField() {
    Column({ space: 8 }) {
      Text(RemoteI18n.t('account.loginPhoneLabel'))
        .fontSize(MobileDesignTypography.bodyMedium.size)
        .fontWeight(FontWeight.Medium)
        .fontColor(INK)
        .width('100%')
      TextInput({
        placeholder: RemoteI18n.t('account.loginPhonePlaceholder'),
        text: this.phone
      })
        .width('100%')
        .height(FIELD_HEIGHT)
        .fontSize(MobileDesignTypography.titleSmall.size)
        .fontColor(INK)
        .placeholderColor(SUBTLE)
        .backgroundColor(CARD)
        .border({ width: 0.5, color: LINE })
        .borderRadius(12)
        .shadow({ radius: 12, color: SHADOW_FAINT, offsetY: 4 })
        .padding({ left: 14, right: 14 })
        .maxLength(16)
        .onChange((value: string) => {
          // Keep only the characters a mainland-mobile number can carry; the
          // relay normalizes the prefix, so stray letters never reach it.
          this.phone = value.replace(/[^0-9+\-\s]/g, '');
          this.errorText = '';
        })
    }
    .width('100%')
    .alignItems(HorizontalAlign.Start)
    .margin({ top: 16 })
  }

  @Builder
  private CodeField() {
    Column({ space: 8 }) {
      Text(RemoteI18n.t('account.loginCodeLabel'))
        .fontSize(MobileDesignTypography.bodyMedium.size)
        .fontWeight(FontWeight.Medium)
        .fontColor(INK)
        .width('100%')
      Row({ space: 8 }) {
        TextInput({
          placeholder: RemoteI18n.t('account.loginCodePlaceholder'),
          text: this.code
        })
          .height(FIELD_HEIGHT)
          .fontSize(MobileDesignTypography.titleSmall.size)
          .fontColor(INK)
          .placeholderColor(SUBTLE)
          .backgroundColor(CARD)
          .border({ width: 0.5, color: LINE })
          .borderRadius(12)
          .shadow({ radius: 12, color: SHADOW_FAINT, offsetY: 4 })
          .padding({ left: 14, right: 14 })
          .maxLength(6)
          .onChange((value: string) => {
            this.code = value.replace(/[^0-9]/g, '');
            this.errorText = '';
          })
          .layoutWeight(1)
        Column() {
          AppActionButton({
            label: this.sendCodeLabel(),
            style: AppActionStyle.Secondary,
            actionHeight: FIELD_HEIGHT,
            elevated: false,
            isEnabled: this.canSendCode(),
            onAction: (): void => {
              void this.sendCode();
            }
          })
        }
        .width(SEND_CODE_BUTTON_WIDTH)
      }
      .width('100%')
      .alignItems(VerticalAlign.Center)
    }
    .width('100%')
    .alignItems(HorizontalAlign.Start)
    .margin({ top: 16 })
  }

  @Builder
  private PasswordField() {
    Column({ space: 8 }) {
      Text(RemoteI18n.t('account.loginPasswordLabel'))
        .fontSize(MobileDesignTypography.bodyMedium.size)
        .fontWeight(FontWeight.Medium)
        .fontColor(INK)
        .width('100%')
      TextInput({
        placeholder: RemoteI18n.t('account.loginPasswordPlaceholder'),
        text: this.password
      })
        .width('100%')
        .height(FIELD_HEIGHT)
        .fontSize(MobileDesignTypography.titleSmall.size)
        .fontColor(INK)
        .placeholderColor(SUBTLE)
        .backgroundColor(CARD)
        .border({ width: 0.5, color: LINE })
        .borderRadius(12)
        .shadow({ radius: 12, color: SHADOW_FAINT, offsetY: 4 })
        .padding({ left: 14, right: 10 })
        .type(InputType.Password)
        .showPasswordIcon(true)
        .onChange((value: string) => {
          this.password = value;
          this.errorText = '';
        })
    }
    .width('100%')
    .alignItems(HorizontalAlign.Start)
    .margin({ top: 16 })
  }

  @Builder
  private CredentialModeToggle() {
    Row() {
      Text(RemoteI18n.t(this.usePassword ? 'account.loginUseCode' : 'account.loginUsePassword'))
        .fontSize(MobileDesignTypography.bodySmall.size)
        .fontWeight(FontWeight.Medium)
        .fontColor(this.canToggleCredentialMode() ? INK : SUBTLE)
        .padding({ top: 6, bottom: 6 })
        .onClick(() => {
          if (this.canToggleCredentialMode()) {
            this.usePassword = !this.usePassword;
            this.errorText = '';
          }
        })
    }
    .width('100%')
    .margin({ top: 10 })
  }

  private sendCodeLabel(): string {
    if (this.resendInSecs > 0) {
      return RemoteI18n.f('account.loginResendCode', `${this.resendInSecs}`);
    }
    return this.isSendingCode ? RemoteI18n.t('account.loginSendingCode') : RemoteI18n.t('account.loginSendCode');
  }

  private canToggleCredentialMode(): boolean {
    return !this.isBusy && !this.isSendingCode;
  }

  private canSendCode(): boolean {
    return !this.isBusy && !this.isSendingCode && this.relayCheck === 'supported' &&
      this.resendInSecs === 0 && this.isPhoneValid();
  }

  private canSubmit(): boolean {
    if (this.isBusy || this.isSendingCode || this.relayCheck !== 'supported' || !this.isPhoneValid()) {
      return false;
    }
    return this.usePassword ? this.password.length > 0 : /^\d{6}$/.test(this.code);
  }

  /** Relay-side rule: 11 mainland digits, with an optional +86/86 prefix. */
  private normalizedPhone(): string {
    const trimmed = this.phone.trim().replace(/[\s-]/g, '');
    return trimmed.replace(/^(\+86|86)(?=[0-9]{11}$)/, '');
  }

  private isPhoneValid(): boolean {
    return /^1[3-9][0-9]{9}$/.test(this.normalizedPhone());
  }

  private errorDisplay(err: Object): string {
    const relayText = AccountLoginErrorPolicy.relayMessage(err);
    return relayText.length > 0 ? relayText : RemoteI18n.t(AccountLoginErrorPolicy.messageKey(err));
  }

  private async probeRelay(): Promise<void> {
    const candidate = this.relayUrl.trim();
    if (candidate.length === 0) {
      this.relayCheck = 'idle';
      return;
    }
    if (!normalizeAccountRelayUrl(candidate) && !normalizeManualRelayUrl(candidate)) {
      this.relayCheck = 'invalid';
      return;
    }
    this.relayCheck = 'checking';
    this.errorText = '';
    try {
      const supported = await this.cloudProbeRelay(candidate);
      this.relayCheck = supported ? 'supported' : 'unsupported';
    } catch (err) {
      // An unreachable relay keeps the form blocked; the transport failure is
      // the actionable fact, so it takes the error slot.
      this.relayCheck = 'idle';
      this.errorText = this.errorDisplay(err);
    }
  }

  private async sendCode(): Promise<void> {
    if (!this.canSendCode()) return;
    this.isSendingCode = true;
    this.errorText = '';
    try {
      await this.cloudSmsSend(this.relayUrl.trim(), this.normalizedPhone());
      this.startResendCountdown(SEND_CODE_COUNTDOWN_SECS);
    } catch (err) {
      const cooldown = AccountLoginErrorPolicy.retryAfterSecs(err);
      if (cooldown > 0) {
        this.startResendCountdown(cooldown);
      }
      this.errorText = this.errorDisplay(err);
    } finally {
      this.isSendingCode = false;
    }
  }

  private async submit(): Promise<void> {
    if (!this.canSubmit()) return;
    this.isBusy = true;
    this.errorText = '';
    const credential: LocalAccountCredential = this.usePassword
      ? { relayUrl: this.relayUrl.trim(), phone: this.normalizedPhone(), password: this.password }
      : { relayUrl: this.relayUrl.trim(), phone: this.normalizedPhone(), code: this.code };
    try {
      await this.cloudLogin(credential);
      this.onLoginSuccess();
    } catch (err) {
      this.errorText = this.errorDisplay(err);
    } finally {
      this.isBusy = false;
    }
  }

  private startResendCountdown(secs: number): void {
    if (this.countdownTimer !== -1) clearTimeout(this.countdownTimer);
    this.resendInSecs = Math.max(1, Math.ceil(secs));
    this.scheduleResendTick();
  }

  private scheduleResendTick(): void {
    this.countdownTimer = setTimeout((): void => {
      this.resendInSecs = Math.max(0, this.resendInSecs - 1);
      if (this.resendInSecs > 0) {
        this.scheduleResendTick();
      } else {
        this.countdownTimer = -1;
      }
    }, 1000);
  }
}
