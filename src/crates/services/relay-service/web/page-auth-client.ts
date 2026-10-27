interface ErrorBody {
  error?: string;
  retry_after_secs?: number;
}

const form = document.querySelector<HTMLFormElement>('[data-page-login-form]');
const submitButton = document.querySelector<HTMLButtonElement>('[data-page-login-submit]');
const sendCodeButton = document.querySelector<HTMLButtonElement>('[data-page-login-send-code]');
const errorElement = document.querySelector<HTMLElement>('[data-page-login-error]');
const phoneInput = document.querySelector<HTMLInputElement>('[data-page-login-phone]');
const codeInput = document.querySelector<HTMLInputElement>('[data-page-login-code]');
const loginState = form?.dataset.pageLoginState;
const SEND_CODE_COOLDOWN_SECS = 60;

function relayPathPrefix(): string {
  const authRouteIndex = window.location.pathname.indexOf('/api/page-auth/');
  if (authRouteIndex >= 0) {
    return window.location.pathname.slice(0, authRouteIndex);
  }
  const pageRouteIndex = window.location.pathname.indexOf('/p/');
  if (pageRouteIndex >= 0) {
    return window.location.pathname.slice(0, pageRouteIndex);
  }
  return '';
}

function relayApiPath(path: string): string {
  return `${relayPathPrefix()}${path}`;
}

function currentPageReturnPath(): string {
  const pageRouteIndex = window.location.pathname.indexOf('/p/');
  const pagePath = pageRouteIndex >= 0
    ? window.location.pathname.slice(pageRouteIndex)
    : window.location.pathname;
  return `${pagePath}${window.location.search}`;
}

function externalRedirectTarget(target: string): string {
  if (/^https?:\/\//i.test(target)) return target;
  return `${relayPathPrefix()}${target}`;
}

function isChineseLocale(): boolean {
  return navigator.language.toLowerCase().startsWith('zh');
}

function message(zh: string, en: string): string {
  return isChineseLocale() ? zh : en;
}

function showError(value: string): void {
  if (!errorElement) return;
  errorElement.textContent = value;
  errorElement.hidden = value.length === 0;
}

async function readError(response: Response): Promise<string> {
  const fallback = message('登录失败，请重试。', 'Sign-in failed. Try again.');
  try {
    const body = await response.json() as ErrorBody;
    const error = typeof body.error === 'string' ? body.error : '';
    if (error === 'invalid verification code' || error === 'verification code incorrect'
      || error === 'verification code invalid or expired') {
      return message('验证码不正确，请检查后重试。', 'The verification code was not accepted. Try again.');
    }
    if (error === 'invalid phone or password') {
      return message('手机号或密码未通过验证。', 'The phone number or password was not accepted.');
    }
    if (error === 'registration is closed on this relay') {
      return message('该服务器当前未开放新账号注册。', 'New account registration is currently closed on this server.');
    }
    if (error === 'invalid phone number') {
      return message('手机号无效，请检查后重试。', 'The phone number is invalid. Check it and try again.');
    }
    if (error === 'sms delivery is not configured') {
      return message('该服务器未配置短信登录，请联系服务器管理员。', 'SMS sign-in is not configured on this server. Contact the server administrator.');
    }
    if (error === 'code delivery failed') {
      return message('验证码发送失败，请稍后重试。', 'The verification code could not be delivered. Try again shortly.');
    }
    if (error === 'account does not have access to this Page') {
      return message('该账号没有此页面的访问权限。', 'This account cannot access the Page.');
    }
    if (body.retry_after_secs && body.retry_after_secs > 0) {
      return message(
        `尝试次数过多，请在 ${body.retry_after_secs} 秒后重试。`,
        `Too many attempts. Try again in ${body.retry_after_secs} seconds.`,
      );
    }
    return error || fallback;
  } catch {
    return fallback;
  }
}

async function postJson<T>(path: string, body: Record<string, unknown>): Promise<T> {
  const response = await fetch(path, {
    method: 'POST',
    credentials: 'same-origin',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
  });
  if (!response.ok) {
    throw new Error(await readError(response));
  }
  return response.json() as Promise<T>;
}

interface LocalTokenPair {
  accessToken: string;
  refreshToken: string;
}

let cooldownTimer: number | undefined;

function startCooldown(seconds: number): void {
  if (!sendCodeButton) return;
  let remaining = seconds;
  const render = () => {
    if (!sendCodeButton) return;
    if (remaining > 0) {
      sendCodeButton.disabled = true;
      sendCodeButton.textContent = message(`${remaining} 秒后可重发`, `Resend in ${remaining}s`);
    } else {
      sendCodeButton.disabled = false;
      sendCodeButton.textContent = message('发送验证码', 'Send code');
    }
  };
  window.clearInterval(cooldownTimer);
  render();
  cooldownTimer = window.setInterval(() => {
    remaining -= 1;
    render();
    if (remaining <= 0) window.clearInterval(cooldownTimer);
  }, 1000);
}

sendCodeButton?.addEventListener('click', async () => {
  if (!phoneInput || sendCodeButton.disabled) return;
  if (!phoneInput.value.trim()) {
    showError(message('请输入手机号。', 'Enter your phone number first.'));
    return;
  }
  showError('');
  sendCodeButton.disabled = true;
  sendCodeButton.textContent = message('发送中…', 'Sending…');
  try {
    await postJson<{ status: string }>(
      relayApiPath('/api/auth/local/sms/send'),
      { phone: phoneInput.value.trim() },
    );
    startCooldown(SEND_CODE_COOLDOWN_SECS);
  } catch (error) {
    showError(error instanceof Error ? error.message : message('发送失败，请重试。', 'Could not send the code. Try again.'));
    sendCodeButton.disabled = false;
    sendCodeButton.textContent = message('发送验证码', 'Send code');
  }
});

form?.addEventListener('submit', async (event) => {
  event.preventDefault();
  if (!submitButton || submitButton.disabled) return;
  if (!phoneInput?.value.trim() || !codeInput?.value.trim()) {
    showError(message('请输入手机号和验证码。', 'Enter your phone number and the verification code.'));
    return;
  }
  showError('');
  submitButton.disabled = true;
  submitButton.textContent = message('登录中…', 'Signing in…');
  try {
    // Relay-local sign-in: the SMS code exchanges for an account access
    // token, which then authorizes this Page session. Both calls stay
    // same-origin under the Relay path prefix.
    const pair = await postJson<LocalTokenPair>(relayApiPath('/api/auth/local/login'), {
      phone: phoneInput.value.trim(),
      code: codeInput.value.trim(),
    });
    const loginBody: Record<string, unknown> = { access_token: pair.accessToken };
    if (loginState) {
      loginBody.state = loginState;
    } else {
      loginBody.return_to = currentPageReturnPath();
      loginBody.path_prefix = relayPathPrefix();
    }
    const result = await postJson<{ redirect_to: string }>(relayApiPath('/api/page-auth/login'), loginBody);
    window.location.replace(externalRedirectTarget(result.redirect_to));
  } catch (error) {
    showError(error instanceof Error ? error.message : message('登录失败，请重试。', 'Sign-in failed. Try again.'));
    submitButton.disabled = false;
    submitButton.textContent = message('使用手机号登录', 'Sign in with phone number');
  }
});
