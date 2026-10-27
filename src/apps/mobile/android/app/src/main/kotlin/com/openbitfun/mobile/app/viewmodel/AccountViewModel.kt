package com.openbitfun.mobile.app.viewmodel

import android.app.Application
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.ViewModel
import androidx.lifecycle.ViewModelProvider
import androidx.lifecycle.viewModelScope
import com.openbitfun.mobile.app.platform.LogcatCoreLog
import com.openbitfun.mobile.app.platform.LEGACY_MOBILE_DEVICE_NAMES
import com.openbitfun.mobile.app.platform.deviceIdentity
import com.openbitfun.mobile.core.feature.account.AccountDefaults
import com.openbitfun.mobile.core.feature.account.AccountIntent
import com.openbitfun.mobile.core.feature.account.AccountStore
import com.openbitfun.mobile.core.feature.account.AccountUiState
import com.openbitfun.mobile.core.feature.account.SmsSendResult
import com.openbitfun.mobile.core.feature.account.create
import com.openbitfun.mobile.core.feature.connection.ConnectionPhase
import com.openbitfun.mobile.core.feature.session.RemoteSessionIntent
import com.openbitfun.mobile.core.feature.session.RemoteSessionStore
import com.openbitfun.mobile.core.feature.session.RemoteSessionUiState
import com.openbitfun.mobile.core.feature.session.WorkspaceSessionDirectoryUiState
import com.openbitfun.mobile.core.feature.workspace.RemoteWorkspaceIntent
import com.openbitfun.mobile.core.feature.workspace.RemoteWorkspaceStore
import com.openbitfun.mobile.core.feature.workspace.RemoteWorkspaceUiState
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch

/** What the sign-in form knows about the relay the address field names. */
internal enum class LocalRelayReadiness { PROBING, READY, UNSUPPORTED, UNREACHABLE }

/** An error the sign-in form can state without a screen of its own. */
internal sealed interface LocalSignInError {
    /** The relay's own sentence (cooldown, registration closed, SMS not configured). */
    data class RelayMessage(val text: String) : LocalSignInError

    /** A transport fact with established local copy. */
    data class Reason(val reason: com.openbitfun.mobile.core.feature.account.AccountFailureReason) : LocalSignInError
}

/** The relay-local sign-in form; account-level facts stay in [AccountUiState]. */
internal data class LocalSignInForm(
    val relayUrl: String = "",
    val phone: String = "",
    val code: String = "",
    val password: String = "",
    val usePassword: Boolean = false,
    val readiness: LocalRelayReadiness = LocalRelayReadiness.PROBING,
    val sending: Boolean = false,
    val cooldownSeconds: Int = 0,
    val error: LocalSignInError? = null,
) {
    val normalizedPhone: String? get() = AccountDefaults.normalizePhone(phone)
    val relayAddressValid: Boolean get() = AccountDefaults.normalizeRelayUrl(relayUrl) != null
    val canSendCode: Boolean
        get() = readiness == LocalRelayReadiness.READY && !sending && cooldownSeconds == 0 &&
            normalizedPhone != null && !usePassword
    val canSignIn: Boolean
        get() = readiness == LocalRelayReadiness.READY && relayAddressValid &&
            normalizedPhone != null && (if (usePassword) password.isNotBlank() else code.length == 6)
}

internal class AccountViewModel(application: Application) : AndroidViewModel(application) {
    private val completionNotifier = com.openbitfun.mobile.app.platform.TaskCompletionNotifier(application)
    private var foreground = true
    fun setBackground(value: Boolean) {
        LogcatCoreLog.info("account host visibility foreground=${!value}")
        if (!value) store.resumeSessionStreams()
        foreground = !value
        completionNotifier.setBackground(value)
        remoteStore?.dispatch(RemoteSessionIntent.SetForeground(foreground))
    }
    private val identity = application.deviceIdentity()
    private val store = AccountStore.create(
        viewModelScope,
        application,
        identity.installId,
        identity.displayName,
        LogcatCoreLog,
        LEGACY_MOBILE_DEVICE_NAMES,
    )
    val state: StateFlow<AccountUiState> = store.state
    val relayUrl: StateFlow<String> = store.relayUrl

    /** The local sign-in form's editable state and its relay probe. */
    private val _signInForm = MutableStateFlow(LocalSignInForm())
    val signInForm: StateFlow<LocalSignInForm> = _signInForm.asStateFlow()
    private var probeJob: Job? = null
    private var cooldownJob: Job? = null
    private var probedRelayUrl: String? = null

    init {
        _signInForm.value = LocalSignInForm(relayUrl = store.relayUrl.value)
        probeRelay(store.relayUrl.value)
    }

    fun updateSignInRelay(value: String) {
        val current = _signInForm.value
        _signInForm.value = current.copy(relayUrl = value, error = null)
        val endpoint = AccountDefaults.normalizeRelayUrl(value) ?: return
        if (endpoint != probedRelayUrl) {
            _signInForm.value = _signInForm.value.copy(readiness = LocalRelayReadiness.PROBING)
            probeJob?.cancel()
            probeJob = viewModelScope.launch {
                delay(RELAY_PROBE_SETTLE_MS)
                probeRelay(endpoint)
            }
        }
    }

    fun updateSignInPhone(value: String) {
        _signInForm.value = _signInForm.value.copy(phone = value.filter(Char::isDigit).take(13), error = null)
    }

    fun updateSignInCode(value: String) {
        _signInForm.value = _signInForm.value.copy(code = value.filter(Char::isDigit).take(6), error = null)
    }

    fun updateSignInPassword(value: String) {
        _signInForm.value = _signInForm.value.copy(password = value.take(128), error = null)
    }

    fun toggleSignInUsePassword() {
        _signInForm.value = _signInForm.value.copy(usePassword = !_signInForm.value.usePassword, error = null)
    }

    /** Re-probes the current address; the UNREACHABLE state's way forward. */
    fun retryRelayProbe() {
        probeRelay(_signInForm.value.relayUrl)
    }

    private fun probeRelay(address: String) {
        val endpoint = AccountDefaults.normalizeRelayUrl(address) ?: run {
            _signInForm.value = _signInForm.value.copy(readiness = LocalRelayReadiness.UNSUPPORTED)
            return
        }
        probedRelayUrl = endpoint
        _signInForm.value = _signInForm.value.copy(readiness = LocalRelayReadiness.PROBING)
        probeJob?.cancel()
        probeJob = viewModelScope.launch {
            val readiness = try {
                if (store.relaySupportsLocalAuth(endpoint)) LocalRelayReadiness.READY else LocalRelayReadiness.UNSUPPORTED
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (_: Throwable) {
                LocalRelayReadiness.UNREACHABLE
            }
            if (probedRelayUrl == endpoint) {
                _signInForm.value = _signInForm.value.copy(readiness = readiness)
            }
        }
    }

    fun sendSignInCode() {
        val form = _signInForm.value
        val phone = form.normalizedPhone ?: return
        val endpoint = AccountDefaults.normalizeRelayUrl(form.relayUrl) ?: return
        if (!form.canSendCode) return
        _signInForm.value = form.copy(sending = true, error = null)
        viewModelScope.launch {
            when (val result = store.sendSmsCode(endpoint, phone)) {
                SmsSendResult.Sent -> {
                    _signInForm.value = _signInForm.value.copy(sending = false)
                    startResendCountdown(SMS_RESEND_COOLDOWN_SECONDS)
                }
                is SmsSendResult.Cooldown -> {
                    // The countdown honors the relay's own window; the message
                    // is its sentence, quoted verbatim.
                    _signInForm.value = _signInForm.value.copy(
                        sending = false,
                        cooldownSeconds = 0,
                        error = LocalSignInError.RelayMessage(result.message),
                    )
                    startResendCountdown(result.retryAfterSeconds)
                }
                is SmsSendResult.Rejected -> _signInForm.value = _signInForm.value.copy(
                    sending = false,
                    error = if (result.message.isNotBlank()) LocalSignInError.RelayMessage(result.message)
                    else LocalSignInError.Reason(result.reason),
                )
            }
        }
    }

    private fun startResendCountdown(seconds: Int) {
        cooldownJob?.cancel()
        _signInForm.value = _signInForm.value.copy(cooldownSeconds = seconds)
        cooldownJob = viewModelScope.launch {
            var remaining = seconds
            while (remaining > 0) {
                delay(1_000)
                remaining -= 1
                _signInForm.value = _signInForm.value.copy(cooldownSeconds = remaining)
            }
        }
    }

    fun submitSignIn() {
        val form = _signInForm.value
        val phone = form.normalizedPhone ?: return
        val endpoint = AccountDefaults.normalizeRelayUrl(form.relayUrl) ?: return
        if (!form.canSignIn) return
        store.dispatch(
            AccountIntent.LocalLogin(
                relayUrl = endpoint,
                phone = phone,
                code = form.code.takeIf { !form.usePassword && it.length == 6 },
                password = form.password.takeIf { form.usePassword && it.isNotBlank() },
            ),
        )
    }

    private val _remoteState = MutableStateFlow<RemoteSessionUiState>(RemoteSessionUiState.Idle)
    val remoteState: StateFlow<RemoteSessionUiState> = _remoteState.asStateFlow()
    private val _connectionPhase = MutableStateFlow(ConnectionPhase.IDLE)
    val connectionPhase: StateFlow<ConnectionPhase> = _connectionPhase.asStateFlow()
    private val _workspaceState = MutableStateFlow<RemoteWorkspaceUiState>(RemoteWorkspaceUiState.Idle)
    val workspaceState: StateFlow<RemoteWorkspaceUiState> = _workspaceState.asStateFlow()
    private val _workspaceDirectory = MutableStateFlow(WorkspaceSessionDirectoryUiState(emptyList()))

    /** Per-workspace session lists, filled in as the sidebar discloses each branch. */
    val workspaceDirectory: StateFlow<WorkspaceSessionDirectoryUiState> = _workspaceDirectory.asStateFlow()
    private var remoteStore: RemoteSessionStore? = null
    private var workspaceStore: RemoteWorkspaceStore? = null
    private var remoteJob: Job? = null
    private var connectionJob: Job? = null
    private var directoryJob: Job? = null
    private var workspaceJob: Job? = null
    private var activeTarget: String? = null
    // The selected device the user disconnected from. The selection stays so the
    // card can offer it again, but account churn must not bind it back.
    private var releasedTarget: String? = null

    init {
        viewModelScope.launch {
            store.state.collect { current ->
                val target = (current as? AccountUiState.Ready)?.selectedDeviceId
                if (target != releasedTarget) releasedTarget = null
                if (target != activeTarget && target != releasedTarget) bindTarget(target)
            }
        }
        store.dispatch(AccountIntent.Restore)
    }

    fun dispatch(intent: AccountIntent) {
        store.dispatch(intent)
    }

    /** The handle General Chat reads the account's synced models through. */

    fun dispatchSession(intent: RemoteSessionIntent) {
        remoteStore?.dispatch(intent)
    }

    fun dispatchWorkspace(intent: RemoteWorkspaceIntent) {
        workspaceStore?.dispatch(intent)
    }

    /**
     * Point the session and workspace stores at [deviceId], rebuilding them even
     * when it is already the selected device.
     *
     * The collector above only rebinds when `selectedDeviceId` changes, which is
     * right for ordinary account churn but makes the row inert exactly when it
     * matters: selecting the device that is already selected produces an
     * identical state, so nothing rebinds and a failed load has no way back.
     * `canSelectAccountDevice` in `ConnectAccountDevicePage.ets` has no such
     * guard, and a row that reads "Controlling" while nothing loaded has to be
     * a way back.
     */
    fun selectDevice(deviceId: String) {
        if (deviceId == activeTarget || deviceId == releasedTarget) {
            releasedTarget = null
            bindTarget(deviceId)
        } else {
            // Clear the outgoing projection before publishing the new selection.
            // The account collector binds the new target's shared stores.
            val ready = state.value as? AccountUiState.Ready ?: return
            if (ready.devices.none { it.id == deviceId && it.online }) return
            remoteJob?.cancel()
            workspaceJob?.cancel()
            connectionJob?.cancel()
            directoryJob?.cancel()
            _workspaceDirectory.value = WorkspaceSessionDirectoryUiState(emptyList())
            _remoteState.value = RemoteSessionUiState.Loading
            _workspaceState.value = RemoteWorkspaceUiState.Loading
            _connectionPhase.value = ConnectionPhase.IDLE
            store.dispatch(AccountIntent.SelectDevice(deviceId))
        }
    }

    fun disconnectDevice() {
        releasedTarget = activeTarget
        bindTarget(null)
    }

    private fun bindTarget(target: String?) {
        remoteJob?.cancel()
        connectionJob?.cancel()
        workspaceJob?.cancel()
        directoryJob?.cancel()
        remoteStore?.dispatch(RemoteSessionIntent.Stop)
        workspaceStore?.dispatch(RemoteWorkspaceIntent.Stop)
        remoteStore = null
        workspaceStore = null
        _remoteState.value = RemoteSessionUiState.Idle
        _connectionPhase.value = ConnectionPhase.IDLE
        _workspaceState.value = RemoteWorkspaceUiState.Idle
        _workspaceDirectory.value = WorkspaceSessionDirectoryUiState(emptyList())
        completionNotifier.reset()
        activeTarget = target
        if (target == null) return

        remoteStore = store.createSessionStore(viewModelScope)?.also { created ->
            remoteJob = viewModelScope.launch { created.state.collect {
                _remoteState.value = it
                completionNotifier.observe(it, target)
            } }
            connectionJob = viewModelScope.launch {
                created.connectionPhase.collect { _connectionPhase.value = it }
            }
            directoryJob = viewModelScope.launch {
                created.workspaceDirectory.collect { _workspaceDirectory.value = it }
            }
            created.dispatch(RemoteSessionIntent.Load)
            created.dispatch(RemoteSessionIntent.SetForeground(foreground))
        }
        workspaceStore = store.createWorkspaceStore(viewModelScope)?.also { created ->
            workspaceJob = viewModelScope.launch { created.state.collect { _workspaceState.value = it } }
            created.dispatch(RemoteWorkspaceIntent.Load)
        }
    }

    override fun onCleared() {
        bindTarget(null)
        probeJob?.cancel()
        cooldownJob?.cancel()
        store.stop()
        super.onCleared()
    }

    companion object {
        /** Typing settles before a probe leaves, so one address is one request. */
        private const val RELAY_PROBE_SETTLE_MS = 600L

        /** The relay's own cooldown window for a resend; a 429 can extend it. */
        private const val SMS_RESEND_COOLDOWN_SECONDS = 60

        val Factory: ViewModelProvider.Factory = object : ViewModelProvider.Factory {
            @Suppress("UNCHECKED_CAST")
            override fun <T : ViewModel> create(
                modelClass: Class<T>,
                extras: androidx.lifecycle.viewmodel.CreationExtras,
            ): T = AccountViewModel(
                extras[ViewModelProvider.AndroidViewModelFactory.APPLICATION_KEY]!!,
            ) as T
        }
    }
}
