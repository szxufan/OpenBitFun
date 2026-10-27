package com.openbitfun.mobile.app

import androidx.compose.foundation.layout.requiredWidth
import androidx.compose.runtime.mutableStateOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.test.*
import androidx.compose.ui.test.junit4.v2.createComposeRule
import androidx.compose.ui.unit.dp
import androidx.test.platform.app.InstrumentationRegistry
import com.openbitfun.mobile.app.ui.account.AccountLoginPage
import com.openbitfun.mobile.app.ui.account.AccountLoginPageActions
import com.openbitfun.mobile.app.ui.theme.OpenBitFunTheme
import com.openbitfun.mobile.app.viewmodel.LocalRelayReadiness
import com.openbitfun.mobile.app.viewmodel.LocalSignInForm
import com.openbitfun.mobile.core.feature.account.AccountUiState
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test

class AccountLoginPageTest {
    @get:Rule val rule = createComposeRule()
    private fun text(id: Int, vararg args: Any): String =
        InstrumentationRegistry.getInstrumentation().targetContext.getString(id, *args)

    private class RecordingActions : AccountLoginPageActions() {
        val sentCodes = mutableListOf<Unit>()
        val signIn = mutableListOf<Unit>()
        val probes = mutableListOf<Unit>()
        override fun onSendCode() { sentCodes += Unit }
        override fun onSignIn() { signIn += Unit }
        override fun onRetryProbe() { probes += Unit }
    }

    private fun form(
        readiness: LocalRelayReadiness = LocalRelayReadiness.READY,
        phone: String = "13800138000",
        code: String = "123456",
        password: String = "",
        usePassword: Boolean = false,
        cooldownSeconds: Int = 0,
    ) = LocalSignInForm(
        relayUrl = "https://remote.openbitfun.com/v/1.0.2",
        phone = phone,
        code = code,
        password = password,
        usePassword = usePassword,
        readiness = readiness,
        cooldownSeconds = cooldownSeconds,
    )

    /**
     * An older relay answers with an explicit unsupported state: one sentence,
     * no fields, no sign-in button that would fail late against /api/auth/local.
     */
    @Test fun relayWithoutLocalAuthShowsAnExplicitStateInsteadOfTheForm() {
        rule.setContent {
            OpenBitFunTheme(dark = false) {
                AccountLoginPage(
                    AccountUiState.SignedOut,
                    form(readiness = LocalRelayReadiness.UNSUPPORTED),
                    {},
                    Modifier.requiredWidth(386.dp).testTag("login-panel"),
                )
            }
        }
        rule.onNodeWithText(text(R.string.account_local_auth_unsupported)).assertIsDisplayed()
        rule.onNodeWithText(text(R.string.account_phone_label)).assertDoesNotExist()
        rule.onNodeWithText(text(R.string.account_send_code)).assertDoesNotExist()
        rule.onNodeWithText(text(R.string.account_sign_in)).assertDoesNotExist()
    }

    @Test fun unreachableRelayOffersRetryAndNeverReportsItAsUnsupported() {
        val actions = RecordingActions()
        rule.setContent {
            OpenBitFunTheme(dark = false) {
                AccountLoginPage(
                    AccountUiState.SignedOut,
                    form(readiness = LocalRelayReadiness.UNREACHABLE),
                    {},
                    Modifier.requiredWidth(386.dp),
                    actions,
                )
            }
        }
        rule.onNodeWithText(text(R.string.account_relay_unreachable)).assertIsDisplayed()
        rule.onNodeWithText(text(R.string.account_local_auth_unsupported)).assertDoesNotExist()
        rule.onNodeWithText(text(R.string.sessions_retry)).assertIsEnabled().performClick()
        assertEquals(1, actions.probes.size)
    }

    /** The send-code action is gated by the phone, and the cooldown by the relay. */
    @Test fun sendCodeNeedsAValidPhoneAndTheCountdownDisablesIt() {
        val actions = RecordingActions()
        val current = mutableStateOf(form(phone = "138"))
        rule.setContent {
            OpenBitFunTheme(dark = false) {
                AccountLoginPage(AccountUiState.SignedOut, current.value, {}, Modifier.requiredWidth(386.dp), actions)
            }
        }
        rule.onNodeWithText(text(R.string.account_send_code)).assertIsNotEnabled()

        rule.runOnIdle { current.value = form(phone = "13800138000") }
        rule.onNodeWithText(text(R.string.account_send_code)).assertIsEnabled().performClick()
        assertEquals(1, actions.sentCodes.size)

        rule.runOnIdle { current.value = form(cooldownSeconds = 45) }
        rule.onNodeWithText(text(R.string.account_resend_code_in, 45)).assertIsNotEnabled()
    }

    @Test fun signInRequiresValidFieldsAndTogglesBetweenCodeAndPassword() {
        val actions = RecordingActions()
        val current = mutableStateOf(form(code = "12345"))
        rule.setContent {
            OpenBitFunTheme(dark = false) {
                AccountLoginPage(AccountUiState.SignedOut, current.value, {}, Modifier.requiredWidth(386.dp), actions)
            }
        }
        // A short code is not a code the relay will accept.
        rule.onNodeWithText(text(R.string.account_sign_in)).assertIsNotEnabled()

        rule.runOnIdle { current.value = form(code = "123456") }
        rule.onNodeWithText(text(R.string.account_sign_in)).assertIsEnabled().performClick()
        assertEquals(1, actions.signIn.size)

        // Password mode replaces the code field entirely.
        rule.runOnIdle { current.value = form(usePassword = true, password = "") }
        rule.onNodeWithText(text(R.string.account_code_label)).assertDoesNotExist()
        rule.onNodeWithText(text(R.string.account_local_password_label)).assertIsDisplayed()
        rule.onNodeWithText(text(R.string.account_sign_in)).assertIsNotEnabled()
        rule.onNodeWithText(text(R.string.account_use_code_login)).assertIsDisplayed()

        rule.runOnIdle { current.value = form(usePassword = true, password = "hunter2") }
        rule.onNodeWithText(text(R.string.account_sign_in)).assertIsEnabled()
    }

    /** The relay's own sentence for a rejected sign-in is quoted, not decoded. */
    @Test fun rejectedSignInSurfacesTheRelaySentenceVerbatim() {
        rule.setContent {
            OpenBitFunTheme(dark = false) {
                AccountLoginPage(
                    AccountUiState.Failed(
                        com.openbitfun.mobile.core.feature.account.AccountFailureReason.AUTHENTICATION,
                        true,
                        com.openbitfun.mobile.core.feature.account.AccountFailureStage.AUTHENTICATION,
                        "registration is closed on this relay",
                    ),
                    form(),
                    {},
                    Modifier.requiredWidth(386.dp),
                )
            }
        }
        rule.onNodeWithText("registration is closed on this relay").assertIsDisplayed()
    }
}
