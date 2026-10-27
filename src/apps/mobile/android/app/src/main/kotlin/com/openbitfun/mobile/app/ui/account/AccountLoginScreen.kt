package com.openbitfun.mobile.app.ui.account

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import com.openbitfun.mobile.app.ui.common.ConnectionSheetHeader
import com.openbitfun.mobile.app.ui.common.ConnectionSheetFooter
import com.openbitfun.mobile.app.ui.common.connectionSheetTextStyle
import com.openbitfun.mobile.app.R
import com.openbitfun.mobile.app.viewmodel.AccountViewModel
import com.openbitfun.mobile.app.viewmodel.LocalRelayReadiness
import com.openbitfun.mobile.app.viewmodel.LocalSignInError
import com.openbitfun.mobile.app.viewmodel.LocalSignInForm
import com.openbitfun.mobile.core.feature.account.AccountFailureReason
import com.openbitfun.mobile.core.feature.account.AccountUiState
import com.openbitfun.mobile.app.ui.theme.generated.MobileDesignGeometry

/**
 * The login step, and only that. A signed-in account is shown inside every
 * settings page (see [AccountSettingsIdentityCard]), so once login lands the
 * shell closes this sheet and puts the user on settings; the spinner covers the
 * moment between the last form tap and that hand-off.
 */
@Composable
internal fun AccountLoginScreen(
    modifier: Modifier,
    onBack: () -> Unit = {},
    viewModel: AccountViewModel = viewModel(factory = AccountViewModel.Factory),
) {
    val state by viewModel.state.collectAsStateWithLifecycle()
    val form by viewModel.signInForm.collectAsStateWithLifecycle()
    when (val current = state) {
        AccountUiState.Idle, AccountUiState.Restoring, is AccountUiState.Ready ->
            Box(modifier.fillMaxWidth().heightIn(min = MobileDesignGeometry.LoginSheetBodyMinHeight), contentAlignment = Alignment.Center) {
                CircularProgressIndicator()
            }
        AccountUiState.SigningIn, AccountUiState.SignedOut, is AccountUiState.Failed -> AccountLoginPage(
            state = current,
            form = form,
            onBack = onBack,
            modifier = modifier,
            actions = object : AccountLoginPageActions() {
                override fun onRelayChange(value: String) = viewModel.updateSignInRelay(value)
                override fun onPhoneChange(value: String) = viewModel.updateSignInPhone(value)
                override fun onCodeChange(value: String) = viewModel.updateSignInCode(value)
                override fun onPasswordChange(value: String) = viewModel.updateSignInPassword(value)
                override fun onToggleUsePassword() = viewModel.toggleSignInUsePassword()
                override fun onSendCode() = viewModel.sendSignInCode()
                override fun onRetryProbe() = viewModel.retryRelayProbe()
                override fun onSignIn() = viewModel.submitSignIn()
            },
        )
    }
}

/** Everything the inline relay-local form needs from whoever hosts it. */
internal open class AccountLoginPageActions {
    open fun onRelayChange(value: String) {}
    open fun onPhoneChange(value: String) {}
    open fun onCodeChange(value: String) {}
    open fun onPasswordChange(value: String) {}
    open fun onToggleUsePassword() {}
    open fun onSendCode() {}
    open fun onRetryProbe() {}
    open fun onSignIn() {}
}

/**
 * The relay-local sign-in form: server address, phone, a code the relay texts
 * (or a password the account set), and one sign-in button.
 *
 * The capability probe runs before the form is offered. A relay without the
 * contract gets a plain sentence instead of fields that would fail late, and a
 * relay that cannot be reached gets a retry rather than an "unsupported" that
 * would send the user looking for an upgrade.
 */
@Composable
internal fun AccountLoginPage(
    state: AccountUiState,
    form: LocalSignInForm,
    onBack: () -> Unit,
    modifier: Modifier,
    actions: AccountLoginPageActions = AccountLoginPageActions(),
) {
    val busy = state is AccountUiState.SigningIn
    Column(modifier.fillMaxWidth()) {
        ConnectionSheetHeader(onBack, uniformGlyph = true)
        Box(Modifier.weight(1f, fill = false).fillMaxWidth(), contentAlignment = Alignment.TopCenter) {
            Column(
                modifier = Modifier.fillMaxWidth().verticalScroll(rememberScrollState())
                    .padding(horizontal = 20.dp).heightIn(min = MobileDesignGeometry.LoginSheetBodyMinHeight),
                horizontalAlignment = Alignment.CenterHorizontally,
            ) {
                Text(stringResource(R.string.account_login_title),
                    style = MaterialTheme.typography.displayMedium.connectionSheetTextStyle(), textAlign = TextAlign.Center)
                Text(stringResource(R.string.account_login_body),
                    style = MaterialTheme.typography.bodyMedium.connectionSheetTextStyle(),
                    color = MaterialTheme.colorScheme.onSurfaceVariant, textAlign = TextAlign.Center,
                    modifier = Modifier.padding(top = 8.dp))
                when (form.readiness) {
                    LocalRelayReadiness.PROBING -> Box(Modifier.padding(top = 24.dp)) { CircularProgressIndicator() }
                    // One sentence, no fields: an older relay must answer with
                    // an explicit unsupported state, never a broken form.
                    LocalRelayReadiness.UNSUPPORTED -> Text(stringResource(R.string.account_local_auth_unsupported),
                        color = MaterialTheme.colorScheme.onSurfaceVariant, textAlign = TextAlign.Center,
                        style = MaterialTheme.typography.bodyMedium.connectionSheetTextStyle(),
                        modifier = Modifier.padding(top = 20.dp))
                    LocalRelayReadiness.UNREACHABLE -> Column(horizontalAlignment = Alignment.CenterHorizontally) {
                        Text(stringResource(R.string.account_relay_unreachable),
                            color = MaterialTheme.colorScheme.error, textAlign = TextAlign.Center,
                            style = MaterialTheme.typography.bodyMedium.connectionSheetTextStyle(),
                            modifier = Modifier.padding(top = 20.dp))
                        TextButton(onClick = actions::onRetryProbe, enabled = !busy) {
                            Text(stringResource(R.string.sessions_retry))
                        }
                    }
                    LocalRelayReadiness.READY -> Column(Modifier.fillMaxWidth().padding(top = 20.dp)) {
                        OutlinedTextField(
                            value = form.relayUrl,
                            onValueChange = actions::onRelayChange,
                            label = { Text(stringResource(R.string.account_login_server)) },
                            placeholder = { Text(stringResource(R.string.account_relay_url_placeholder)) },
                            enabled = !busy,
                            singleLine = true,
                            isError = !form.relayAddressValid,
                            modifier = Modifier.fillMaxWidth(),
                        )
                        OutlinedTextField(
                            value = form.phone,
                            onValueChange = actions::onPhoneChange,
                            label = { Text(stringResource(R.string.account_phone_label)) },
                            enabled = !busy,
                            singleLine = true,
                            keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Phone),
                            modifier = Modifier.fillMaxWidth().padding(top = 12.dp),
                        )
                        if (form.usePassword) {
                            OutlinedTextField(
                                value = form.password,
                                onValueChange = actions::onPasswordChange,
                                label = { Text(stringResource(R.string.account_local_password_label)) },
                                enabled = !busy,
                                singleLine = true,
                                visualTransformation = PasswordVisualTransformation(),
                                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password),
                                modifier = Modifier.fillMaxWidth().padding(top = 12.dp),
                            )
                        } else {
                            OutlinedTextField(
                                value = form.code,
                                onValueChange = actions::onCodeChange,
                                label = { Text(stringResource(R.string.account_code_label)) },
                                placeholder = { Text(stringResource(R.string.account_code_placeholder)) },
                                enabled = !busy,
                                singleLine = true,
                                keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.NumberPassword),
                                modifier = Modifier.fillMaxWidth().padding(top = 12.dp),
                            )
                            TextButton(
                                onClick = actions::onSendCode,
                                enabled = form.canSendCode && !busy,
                                modifier = Modifier.align(Alignment.End),
                            ) {
                                Text(
                                    if (form.cooldownSeconds > 0) stringResource(R.string.account_resend_code_in, form.cooldownSeconds)
                                    else stringResource(R.string.account_send_code),
                                )
                            }
                        }
                        TextButton(onClick = actions::onToggleUsePassword, enabled = !busy, modifier = Modifier.align(Alignment.End)) {
                            Text(stringResource(
                                if (form.usePassword) R.string.account_use_code_login else R.string.account_use_password_login,
                            ))
                        }
                        form.error?.let { error -> Text(error.message(),
                            color = MaterialTheme.colorScheme.error,
                            style = MaterialTheme.typography.bodySmall.connectionSheetTextStyle(),
                            modifier = Modifier.padding(top = 8.dp)) }
                    }
                }
                (state as? AccountUiState.Failed)?.let { failure ->
                    // The relay's own sentence for a rejected sign-in, verbatim;
                    // otherwise the reason's established copy.
                    val detail = failure.detail
                    Text(
                        detail?.takeIf(String::isNotBlank)
                            ?: stringResource(failure.reason.messageRes()),
                        color = MaterialTheme.colorScheme.error,
                        style = MaterialTheme.typography.bodySmall.connectionSheetTextStyle(),
                        modifier = Modifier.padding(top = 12.dp),
                    )
                }
            }
        }
        if (form.readiness != LocalRelayReadiness.UNSUPPORTED) {
            ConnectionSheetFooter(
                label = stringResource(when {
                    busy -> R.string.account_signing_in
                    form.readiness == LocalRelayReadiness.UNREACHABLE -> R.string.sessions_retry
                    else -> R.string.account_sign_in
                }),
                primary = true, elevated = false,
                enabled = !busy && (form.readiness == LocalRelayReadiness.UNREACHABLE || form.canSignIn),
                onClick = { if (form.readiness == LocalRelayReadiness.UNREACHABLE) actions.onRetryProbe() else actions.onSignIn() },
            )
        }
    }
}

@Composable
private fun LocalSignInError.message(): String = when (this) {
    is LocalSignInError.RelayMessage -> text
    is LocalSignInError.Reason -> stringResource(reason.messageRes())
}

internal fun AccountFailureReason.messageRes(): Int = when (this) {
    AccountFailureReason.INVALID_CREDENTIALS -> R.string.account_invalid_credentials
    AccountFailureReason.AUTHENTICATION -> R.string.account_authentication
    AccountFailureReason.RATE_LIMITED -> R.string.account_rate_limited
    AccountFailureReason.RELAY_UNAVAILABLE -> R.string.account_relay_unavailable
    AccountFailureReason.NETWORK -> R.string.account_network
    AccountFailureReason.TIMEOUT -> R.string.account_timeout
    AccountFailureReason.MALFORMED_RESPONSE -> R.string.account_malformed_response
    AccountFailureReason.SECURE_STORAGE -> R.string.account_secure_storage
}
