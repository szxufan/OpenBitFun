package com.openbitfun.mobile.core.feature.account

import com.openbitfun.mobile.core.feature.relay.HostCatalogNotice
import com.openbitfun.mobile.core.feature.relay.hostCatalogObserver
import com.openbitfun.mobile.core.transport.LocalTokenPair
import com.openbitfun.mobile.core.transport.RemoteSessionStreamTransport
import com.openbitfun.mobile.core.feature.CoreLog
import com.openbitfun.mobile.core.feature.session.RemoteSessionStore
import com.openbitfun.mobile.core.feature.workspace.RemoteWorkspaceStore
import com.openbitfun.mobile.core.persistence.MobilePersistenceStores
import com.openbitfun.mobile.core.persistence.SecureStore
import com.openbitfun.mobile.core.transport.AccountDeviceCommandTransport
import com.openbitfun.mobile.core.transport.CloudAccountClient
import com.openbitfun.mobile.core.transport.CloudAccountDevice
import com.openbitfun.mobile.core.transport.CloudAccountException
import com.openbitfun.mobile.core.transport.CloudAccountFailure
import com.openbitfun.mobile.core.transport.CloudAccountLocalAuthException
import com.openbitfun.mobile.core.transport.CloudAccountSession
import com.openbitfun.mobile.core.transport.RemoteCommandTransport
import com.openbitfun.mobile.core.transport.TransportLog
import com.openbitfun.mobile.core.transport.normalizeConfiguredRelayUrl
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.currentCoroutineContext
import kotlinx.coroutines.ensureActive
import kotlinx.coroutines.isActive
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.emptyFlow
import kotlinx.coroutines.flow.collect
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.serialization.Serializable
import kotlinx.serialization.encodeToString
import kotlinx.serialization.json.Json
import kotlin.io.encoding.Base64

internal data class AccountSessionData(
    val relayUrl: String,
    val username: String,
    val token: String,
    val userId: String,
    val masterKey: ByteArray,
    val targetDeviceId: String?,
    val targetDeviceName: String?,
    /**
     * The relay-local refresh token behind this session, or null on records
     * written before relay-local accounts. Null means an expired device token
     * can only be answered with the sign-in form.
     */
    val refreshToken: String? = null,
)

internal interface AccountBackend {
    /** Whether this relay serves the relay-local phone-account contract. */
    suspend fun relaySupportsLocalAuth(relayUrl: String): Boolean

    /** Ask the relay to text a code; a 429 throws with the relay's cooldown. */
    suspend fun smsSend(relayUrl: String, phone: String)

    /** Sign in with a code or a password (exactly one) and return the pair. */
    suspend fun localLogin(relayUrl: String, phone: String, code: String?, password: String?): LocalTokenPair

    /** Rotate a refresh token; an expired or replayed token throws. */
    suspend fun localRefresh(relayUrl: String, refreshToken: String): LocalTokenPair

    /**
     * The unchanged device exchange: a relay-local [accessToken], this
     * install's stable X25519 key, and the account session it grants. The key
     * must be reused across logins, never regenerated.
     */
    suspend fun login(
        relayUrl: String,
        accessToken: String,
        deviceId: String,
        deviceName: String,
        deviceSecret: ByteArray,
    ): AccountSessionData

    /** [selfDeviceId] lets the transport drop this device's own row. */
    suspend fun listDevices(session: AccountSessionData, selfDeviceId: String): List<AccountDeviceUi>

    suspend fun profile(userId: String): com.openbitfun.mobile.core.transport.GitHubProfile? = null

    fun transport(session: AccountSessionData, targetDeviceId: String): RemoteCommandTransport
    fun closeAccount() {}
    fun resumeSessionStreams() {}
    fun directoryChanges(session: AccountSessionData): Flow<Unit> = emptyFlow()
}

public class AccountStore internal constructor(
    private val scope: CoroutineScope,
    private val backend: AccountBackend,
    private val secureStore: SecureStore,
    private val deviceId: String,
    private val deviceName: String,
    private val persistence: MobilePersistenceStores? = null,
) {
    private val _state = MutableStateFlow<AccountUiState>(AccountUiState.Idle)
    public val state: StateFlow<AccountUiState> = _state.asStateFlow()
    private var session: AccountSessionData? = null

    /**
     * The relay this install signs in against: user-settable, persisted, and
     * the endpoint a restored session must belong to. The default is the
     * official relay; an unparsable or rejected stored value keeps the default
     * rather than dropping the setting's bytes (a newer build may accept it).
     */
    private val _relayUrl = MutableStateFlow(initialConfiguredRelayUrl())
    public val relayUrl: StateFlow<String> = _relayUrl.asStateFlow()
    private var selectedRelayUrl: String get() = _relayUrl.value
        set(value) { _relayUrl.value = value }
    private var catalogScope = CoroutineScope(scope.coroutineContext + kotlinx.coroutines.SupervisorJob())
    private fun closeCatalogs() { catalogScope.coroutineContext[Job]?.cancel(); catalogObservers.clear() }
    private val catalogObservers = mutableMapOf<String, Flow<HostCatalogNotice>>()
    init { scope.coroutineContext[Job]?.invokeOnCompletion { closeCatalogs() } }
    /** [transport] is the store's own transport; the catalog is read through it, on demand from the host. */
    private fun catalogChanges(current: AccountSessionData, target: String, transport: RemoteCommandTransport): Flow<HostCatalogNotice> {
        val source = transport as? RemoteSessionStreamTransport ?: return emptyFlow()
        if (catalogScope.coroutineContext[Job]?.isActive != true) catalogScope = CoroutineScope(scope.coroutineContext + kotlinx.coroutines.SupervisorJob())
        return catalogObservers.getOrPut(current.userId + ":" + current.token + ":" + target) {
            hostCatalogObserver(catalogScope, source)
        }
    }
    private var directoryWork: Job? = null
    private var directoryIdentity: Pair<String, String>? = null
    private var directoryDirty = false
    private var work: Job? = null
    private var profileWork: Job? = null
    private var displayedProfile: AccountProfileRecord? = null
    private var profileAttemptAt: Long = 0
    private var profileAttempt: Pair<String, String>? = null
    /** Latest account membership snapshot, used to authorize explicit device stores. */
    private var controllableDevices: List<AccountDeviceUi> = emptyList()
    private var pendingInitialDeviceSelection = false

    public fun resumeSessionStreams() { backend.resumeSessionStreams() }

    /**
     * Whether [relayUrl] serves the relay-local sign-in contract. Throws on a
     * relay that cannot be reached, so a caller can tell "unsupported" from
     * "unreachable"; see [AccountBackend.relaySupportsLocalAuth].
     */
    public suspend fun relaySupportsLocalAuth(relayUrl: String): Boolean = backend.relaySupportsLocalAuth(relayUrl)

    /**
     * Ask [relayUrl] to text a sign-in code to [phone]. Form-scoped: it never
     * touches account state, and every answer arrives as [SmsSendResult]
     * rather than a transport exception.
     */
    public suspend fun sendSmsCode(relayUrl: String, phone: String): SmsSendResult = try {
        backend.smsSend(relayUrl, phone)
        SmsSendResult.Sent
    } catch (rejected: CloudAccountLocalAuthException) {
        if (rejected.statusCode == 429) {
            SmsSendResult.Cooldown(
                rejected.relayMessage,
                rejected.retryAfterSeconds?.coerceIn(1, 3600)?.toInt() ?: SMS_RESEND_COOLDOWN_SECONDS,
            )
        } else {
            SmsSendResult.Rejected(rejected.failure.toUiReason(), rejected.relayMessage)
        }
    } catch (error: CloudAccountException) {
        SmsSendResult.Rejected(error.failure.toUiReason(), "")
    } catch (cancelled: CancellationException) {
        throw cancelled
    } catch (_: Throwable) {
        SmsSendResult.Rejected(AccountFailureReason.MALFORMED_RESPONSE, "")
    }

    public fun dispatch(intent: AccountIntent) {
        when (intent) {
            AccountIntent.Restore -> restore()
            is AccountIntent.LocalLogin -> login(intent.relayUrl, intent.phone, intent.code, intent.password)
            is AccountIntent.SelectRelay -> {
                val endpoint = normalizeConfiguredRelayUrl(intent.relayUrl) ?: return
                if (session?.relayUrl != endpoint || selectedRelayUrl != endpoint) {
                    work?.cancel()
                    session?.masterKey?.fill(0)
                    session = null
                    controllableDevices = emptyList()
                    selectedRelayUrl = endpoint
                    persistConfiguredRelayUrl(endpoint)
                    _state.value = AccountUiState.SignedOut
                }
            }
            is AccountIntent.SelectDevice -> selectDevice(intent.deviceId)
            AccountIntent.RefreshDevices -> refreshDevices()
            AccountIntent.Retry -> retryFailedStage()
            AccountIntent.Logout -> logout()
            AccountIntent.Stop -> stop()
        }
    }

    public fun createSessionStore(scope: CoroutineScope): RemoteSessionStore? {
        val current = session ?: return null
        val target = current.targetDeviceId?.let(::authorizedDeviceId) ?: return null
        val transport = backend.transport(current, target)
        return RemoteSessionStore.create(
            scope,
            transport,
            deviceKey = target,
            persistence = persistence,
        ).also { it.bindCatalog(catalogChanges(current, target, transport)) }
    }

    public fun createWorkspaceStore(scope: CoroutineScope): RemoteWorkspaceStore? {
        val current = session ?: return null
        val target = current.targetDeviceId?.let(::authorizedDeviceId) ?: return null
        val transport = backend.transport(current, target)
        return RemoteWorkspaceStore.create(
            scope,
            transport,
            kotlinx.coroutines.Dispatchers.Default,
            target,
            persistence?.remoteWorkspaces,
        ).also { it.bindCatalog(catalogChanges(current, target, transport)) }
    }

    /**
     * A session store addressed to one specific device, independent of the
     * currently selected control target. This is how a multi-device directory
     * loads several devices at once while the old single-target methods keep
     * their existing meaning.
     */
    public fun createSessionStore(scope: CoroutineScope, deviceId: String): RemoteSessionStore? {
        val current = session ?: return null
        val target = authorizedDeviceId(deviceId) ?: return null
        val transport = backend.transport(current, target)
        return RemoteSessionStore.create(
            scope,
            transport,
            deviceKey = target,
            persistence = persistence,
        ).also { it.bindCatalog(catalogChanges(current, target, transport)) }
    }

    /** The explicit-device twin of [createWorkspaceStore]. */
    public fun createWorkspaceStore(scope: CoroutineScope, deviceId: String): RemoteWorkspaceStore? {
        val current = session ?: return null
        val target = authorizedDeviceId(deviceId) ?: return null
        val transport = backend.transport(current, target)
        return RemoteWorkspaceStore.create(
            scope,
            transport,
            kotlinx.coroutines.Dispatchers.Default,
            target,
            persistence?.remoteWorkspaces,
        ).also { it.bindCatalog(catalogChanges(current, target, transport)) }
    }

    /**
     * The directory may retain an offline account row, but an explicit store is
     * still only granted to a device in the latest authenticated membership
     * snapshot. Offline is allowed here so cached directory data can be shown;
     * the directory's online guard prevents commands from being sent.
     */
    private fun authorizedDeviceId(deviceId: String): String? {
        val target = deviceId.trim().takeIf(String::isNotBlank) ?: return null
        return controllableDevices.firstOrNull { it.id == target }?.id
    }

    public fun stop() {
        closeCatalogs()
        directoryWork?.cancel(); directoryIdentity = null
        profileWork?.cancel()
        work?.cancel()
        work = null
    }

    private fun restore() {
        pendingInitialDeviceSelection = false
        work?.cancel()
        _state.value = AccountUiState.Restoring
        work = scope.launch {
            val restored = try {
                val stored = secureStore.read(SESSION_KEY)?.decodeToString()
                if (stored.isNullOrEmpty()) {
                    _state.value = AccountUiState.SignedOut
                    return@launch
                }
                decodeRecord(stored)
            } catch (_: Throwable) {
                // A record we cannot decode may belong to a newer or older app.
                // Keep the opaque value in secure storage so a retry or upgraded
                // client can still read it; only clear this store's projection.
                session = null
                controllableDevices = emptyList()
                _state.value = AccountUiState.Failed(
                    AccountFailureReason.SECURE_STORAGE,
                    true,
                    AccountFailureStage.RESTORE,
                )
                return@launch
            }
            if (restored.relayUrl != selectedRelayUrl) {
                // Relay tokens belong to their issuing database. Keep the old
                // encrypted record/cache, but require a fresh login before any
                // byte of it is sent anywhere: the session stays expired until
                // the user signs in against the configured relay.
                expireSession(AccountFailureReason.AUTHENTICATION, AccountFailureStage.AUTHENTICATION)
                return@launch
            }
            session = restored
            // Local credential restoration and remote directory availability are
            // separate facts. Publish identity before any network wait, but grant
            // no target authority until an authenticated membership snapshot arrives.
            publishReady(restored, emptyList(), selectionConfirmed = false)
            refreshDevices()
        }
    }

    /** A directory outage is not an authentication failure or device authority. */
    private fun publishDirectoryFailure(restored: AccountSessionData, reason: AccountFailureReason) {
        publishReady(restored, emptyList())
        val ready = _state.value as AccountUiState.Ready
        _state.value = ready.copy(refreshFailure = reason, selectedDeviceId = null, selectedDeviceName = null)
        // The saved target stays in session and is restored after the relay has
        // supplied its device directory. No new transport is authorized here.
    }

    private fun login(relayUrl: String, phone: String, code: String?, password: String?) {
        pendingInitialDeviceSelection = false
        work?.cancel()
        _state.value = AccountUiState.SigningIn
        work = scope.launch {
            val endpoint = normalizeConfiguredRelayUrl(relayUrl)
            if (endpoint == null) {
                failLogin(AccountFailureReason.MALFORMED_RESPONSE, AccountFailureStage.AUTHENTICATION)
                return@launch
            }
            selectedRelayUrl = endpoint
            val deviceSecret = try {
                secureStore.read(DEVICE_KEY)?.also { require(it.size == 32) }
                    ?: (session?.masterKey?.copyOf() ?: CloudAccountClient.generateDeviceSecret()).also {
                        secureStore.write(DEVICE_KEY, it)
                    }
            } catch (_: Throwable) {
                failLogin(AccountFailureReason.SECURE_STORAGE, AccountFailureStage.SECURE_STORAGE)
                return@launch
            }
            val pair = try {
                backend.localLogin(endpoint, phone, code, password)
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (rejected: CloudAccountLocalAuthException) {
                currentCoroutineContext().ensureActive()
                failLogin(rejected.failure.toUiReason(), AccountFailureStage.AUTHENTICATION, rejected.relayMessage)
                return@launch
            } catch (error: CloudAccountException) {
                currentCoroutineContext().ensureActive()
                failLogin(error.failure.toUiReason(), AccountFailureStage.AUTHENTICATION)
                return@launch
            } catch (_: Throwable) {
                currentCoroutineContext().ensureActive()
                failLogin(AccountFailureReason.MALFORMED_RESPONSE, AccountFailureStage.AUTHENTICATION)
                return@launch
            }
            val loggedIn = try {
                backend.login(endpoint, pair.accessToken, deviceId, deviceName, deviceSecret)
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (error: CloudAccountException) {
                currentCoroutineContext().ensureActive()
                failLogin(error.failure.toUiReason(), AccountFailureStage.AUTHENTICATION)
                return@launch
            } catch (_: Throwable) {
                currentCoroutineContext().ensureActive()
                // Live transport failures are normalized by CloudAccountClient.
                // An untyped failure here is therefore a crypto/protocol failure,
                // never evidence that secure storage was involved.
                failLogin(AccountFailureReason.MALFORMED_RESPONSE, AccountFailureStage.AUTHENTICATION)
                return@launch
            } finally {
                deviceSecret.fill(0)
            }
            val signIn = loggedIn.copy(
                username = pair.user.login,
                refreshToken = pair.refreshToken,
            )

            currentCoroutineContext().ensureActive()
            controllableDevices = emptyList()
            // The relay the user actually signed in against becomes the
            // configured relay, so the record this login persists is the one a
            // later restore compares against.
            persistConfiguredRelayUrl(signIn.relayUrl)
            if (!persistLogin(signIn)) return@launch
            session = signIn
            pendingInitialDeviceSelection = true

            val devices = loadDevices(signIn) ?: return@launch
            // A renewal inside loadDevices may have replaced the session; the
            // selection below must extend whatever served the list, not the
            // token the renewal already retired.
            val active = session ?: return@launch

            // Never this device, even as a fallback: driving the phone from
            // the phone is what `canSelectAccountDevice` forbids, and a
            // target nothing can be asked of is worse than none at all.
            val preferred = AccountDevicePolicy.preferredTarget(devices, deviceId)
            val selected = active.copy(
                targetDeviceId = preferred?.id,
                targetDeviceName = preferred?.name,
            )
            if (!persistLogin(selected)) return@launch
            session = selected
            pendingInitialDeviceSelection = false
            publishReady(selected, devices)
        }
    }

    /**
     * One device-list read, renewing the session first when the relay rejects
     * an expired token and a refresh token is on record. Only one rotation is
     * attempted: a second rejection means the refresh token itself is spent,
     * and the original failure propagates for the caller's expiry handling.
     *
     * [DirectoryRead.session] names the session that actually served the list
     * — the same one passed in, or its renewal — because a successful renewal
     * replaces [AccountStore.session] and the persisted record, and callers
     * must extend that session, not the token the renewal retired.
     */
    private suspend fun listDevicesWithRenewal(current: AccountSessionData): DirectoryRead {
        try {
            return DirectoryRead(current, backend.listDevices(current, deviceId).also { currentCoroutineContext().ensureActive() })
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (error: CloudAccountException) {
            currentCoroutineContext().ensureActive()
            if (error.failure != CloudAccountFailure.AUTHENTICATION) throw error
            val renewed = current.refreshToken
                ?.takeIf(String::isNotBlank)
                ?.let { renewExpiredSession(current) }
                ?: throw error
            currentCoroutineContext().ensureActive()
            session = renewed
            return DirectoryRead(renewed, backend.listDevices(renewed, deviceId))
        }
    }

    /**
     * Recovers an expired device token through the relay-local refresh token:
     * rotate the pair, redo the device exchange with the SAME stored X25519
     * key (reused, never regenerated), and persist the renewed session. Any
     * failure returns null — the caller then keeps the expired-session
     * handling, and the stored record stays exactly as it was.
     */
    private suspend fun renewExpiredSession(current: AccountSessionData): AccountSessionData? {
        val refreshToken = current.refreshToken?.takeIf(String::isNotBlank) ?: return null
        val pair = try {
            backend.localRefresh(current.relayUrl, refreshToken)
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (_: Throwable) {
            return null
        }
        val deviceSecret = try {
            secureStore.read(DEVICE_KEY)?.also { require(it.size == 32) } ?: return null
        } catch (_: Throwable) {
            return null
        }
        val renewed = try {
            backend.login(current.relayUrl, pair.accessToken, deviceId, deviceName, deviceSecret)
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (_: Throwable) {
            return null
        } finally {
            deviceSecret.fill(0)
        }
        val updated = renewed.copy(
            username = pair.user.login,
            refreshToken = pair.refreshToken,
            targetDeviceId = current.targetDeviceId,
            targetDeviceName = current.targetDeviceName,
        )
        // persistLogin fails closed on a storage fault and publishes the
        // secure-storage failure itself; a renewal that cannot be saved must
        // not leave a live in-memory session the next launch cannot restore.
        if (!persistLogin(updated)) return null
        return updated
    }

    private suspend fun loadDevices(current: AccountSessionData): List<AccountDeviceUi>? = try {
        listDevicesWithRenewal(current).devices.also { currentCoroutineContext().ensureActive() }
    } catch (cancelled: CancellationException) {
        throw cancelled
    } catch (error: CloudAccountException) {
        currentCoroutineContext().ensureActive()
        if (error.failure == CloudAccountFailure.AUTHENTICATION) {
            expireSession(error.failure.toUiReason(), AccountFailureStage.DEVICE_LIST)
        } else {
            publishDirectoryFailure(current, error.failure.toUiReason())
        }
        null
    } catch (_: Throwable) {
        currentCoroutineContext().ensureActive()
        publishDirectoryFailure(current, AccountFailureReason.NETWORK)
        null
    }

    private fun retryFailedStage() {
        if (_state.value is AccountUiState.Ready) { refreshDevices(); return }
        val failed = _state.value as? AccountUiState.Failed ?: return
        val current = session ?: return
        if (!failed.canRetry || failed.stage != AccountFailureStage.DEVICE_LIST) return
        work?.cancel()
        _state.value = AccountUiState.SigningIn
        work = scope.launch {
            val devices = loadDevices(current) ?: return@launch
            // A renewal inside loadDevices may have replaced the session; the
            // retry extends whatever served the list.
            val active = session ?: return@launch
            val preferred = active.targetDeviceId
                ?.let { selectedId -> devices.firstOrNull { it.id == selectedId } }
                ?.takeIf { AccountDevicePolicy.canSelect(it, deviceId) }
                ?: AccountDevicePolicy.preferredTarget(devices, deviceId)
            val selected = active.copy(
                targetDeviceId = preferred?.id,
                targetDeviceName = preferred?.name,
            )
            // This retry only refreshes the volatile device-list projection. The
            // authenticated bytes saved before the failed list request stay exact.
            session = selected
            pendingInitialDeviceSelection = false
            publishReady(selected, devices)
        }
    }

    private fun persistLogin(value: AccountSessionData): Boolean {
        val previous = try {
            secureStore.read(SESSION_KEY)
        } catch (_: Throwable) {
            failLogin(AccountFailureReason.SECURE_STORAGE, AccountFailureStage.SECURE_STORAGE)
            return false
        }
        return try {
            secureStore.write(SESSION_KEY, encodeRecord(value).encodeToByteArray())
            true
        } catch (_: Throwable) {
            // Platform secure stores are expected to update atomically. Restore a
            // fake or adapter that mutated before reporting failure as an extra
            // compatibility guard, without ever deleting pre-existing bytes.
            try {
                if (previous != null) secureStore.write(SESSION_KEY, previous)
                else secureStore.delete(SESSION_KEY)
            } catch (_: Throwable) {
                // The observable session still fails closed below. The original
                // write contract must preserve its previous value on failure.
            }
            failLogin(AccountFailureReason.SECURE_STORAGE, AccountFailureStage.SECURE_STORAGE)
            false
        }
    }

    private fun failLogin(reason: AccountFailureReason, stage: AccountFailureStage, detail: String? = null) {
        session = null
        controllableDevices = emptyList()
        _state.value = AccountUiState.Failed(reason, true, stage, detail)
    }

    private fun selectDevice(targetId: String) {
        val current = session ?: return
        val ready = _state.value as? AccountUiState.Ready ?: return
        // `ready.devices` is already filtered, so the id alone would nearly do —
        // but presence is not, and an offline row is a row the user can see.
        val selected = ready.devices.firstOrNull { it.id == targetId }
            ?.takeIf { AccountDevicePolicy.canSelect(it, deviceId) }
            ?: return
        val updated = current.copy(targetDeviceId = selected.id, targetDeviceName = selected.name)
        if (!persistLogin(updated)) return
        session = updated
        _state.value = ready.copy(selectedDeviceId = selected.id, selectedDeviceName = selected.name)
    }

    /**
     * Re-ask the relay who is online.
     *
     * The old list stays on screen while this runs, and survives a failure: the
     * user asked whether anything changed, and "I could not find out" has to
     * leave them no worse off than not asking.
     */
    private fun refreshDevices() {
        val current = session ?: return
        val ready = _state.value as? AccountUiState.Ready ?: return
        if (ready.refreshing) { directoryDirty = true; return }
        work?.cancel()
        _state.value = ready.copy(refreshing = true, refreshFailure = null)
        work = scope.launch {
            // A renewal inside the request swaps the token; `served` names the
            // session this refresh stands for, renewed or not, so the guards
            // below compare against what actually served the list.
            var served = current
            try {
                val read = listDevicesWithRenewal(current)
                val devices = read.devices
                served = read.session
                currentCoroutineContext().ensureActive()
                if (session?.token == served.token && session?.relayUrl == served.relayUrl) {
                    val active = session!!
                    val selected = if (pendingInitialDeviceSelection) {
                        val preferred = AccountDevicePolicy.preferredTarget(devices, deviceId)
                        active.copy(targetDeviceId = preferred?.id, targetDeviceName = preferred?.name)
                    } else active
                    pendingInitialDeviceSelection = false
                    session = selected
                    publishReady(selected, devices)
                }
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (error: CloudAccountException) {
                currentCoroutineContext().ensureActive()
                if (error.failure == CloudAccountFailure.AUTHENTICATION) {
                    expireSession(error.failure.toUiReason(), AccountFailureStage.DEVICE_LIST)
                } else {
                    publishRefreshFailure(served, error.failure.toUiReason())
                }
            } catch (_: Throwable) {
                currentCoroutineContext().ensureActive()
                publishRefreshFailure(served, AccountFailureReason.NETWORK)
            } finally {
                if (currentCoroutineContext().isActive && directoryDirty && session?.token == served.token && session?.relayUrl == served.relayUrl) { directoryDirty = false; refreshDevices() }
            }
        }
    }

    private fun publishRefreshFailure(requestSession: AccountSessionData, reason: AccountFailureReason) {
        val active = session ?: return
        if (active.token != requestSession.token || active.relayUrl != requestSession.relayUrl) return
        val latest = _state.value as? AccountUiState.Ready ?: return
        // Selection/profile changes made during the request belong to the current
        // UI. A failed directory read has no authority to restore an old snapshot.
        _state.value = latest.copy(refreshing = false, refreshFailure = reason)
    }

    private fun logout() {
        closeCatalogs()
        directoryWork?.cancel(); directoryIdentity = null
        backend.closeAccount()
        profileWork?.cancel()
        displayedProfile = null
        profileAttempt = null
        work?.cancel()
        work = null
        // Logout is immediately observable even when Keychain cannot remove the
        // durable record. A stale persisted record must never keep capabilities
        // active in this process.
        session = null
        controllableDevices = emptyList()
        _state.value = AccountUiState.SignedOut
        try {
            secureStore.delete(SESSION_KEY)
        } catch (_: Throwable) {
            _state.value = AccountUiState.Failed(
                AccountFailureReason.SECURE_STORAGE,
                true,
                AccountFailureStage.SECURE_STORAGE,
            )
        }
    }

    private fun expireSession(reason: AccountFailureReason, stage: AccountFailureStage) {
        closeCatalogs()
        directoryWork?.cancel(); directoryIdentity = null
        backend.closeAccount()
        session = null
        controllableDevices = emptyList()
        _state.value = AccountUiState.Failed(reason, false, stage)
    }

    /**
     * The one place the relay's list becomes the list a screen renders, so the
     * filter cannot be forgotten by a caller — or applied twice with two
     * different answers on two platforms.
     */
    private fun publishReady(current: AccountSessionData, devices: List<AccountDeviceUi>, selectionConfirmed: Boolean = true) {
        val directoryKey = current.relayUrl to current.token
        if (directoryIdentity != directoryKey) {
            directoryWork?.cancel(); directoryIdentity = directoryKey
            directoryWork = scope.launch {
                try { backend.directoryChanges(current).collect { if (session?.token == current.token) refreshDevices() } }
                catch (cancelled: CancellationException) { throw cancelled }
                catch (_: Throwable) { if (session?.token == current.token) refreshDevices() }
            }
        }
        controllableDevices = AccountDevicePolicy.controlTargets(devices, deviceId)
        _state.value = AccountUiState.Ready(
            userId = current.userId,
            relayUrl = current.relayUrl,
            username = displayedProfile?.takeIf { it.userId == current.userId }?.username ?: current.username,
            devices = controllableDevices,
            selectedDeviceId = current.targetDeviceId.takeIf { selectionConfirmed },
            selectedDeviceName = current.targetDeviceName.takeIf { selectionConfirmed },
        ).copy(avatarUrl = displayedProfile?.takeIf { it.userId == current.userId }?.avatarUrl)
        enrichProfile(current)
    }


    private fun enrichProfile(current: AccountSessionData) {
        val identity = current.userId to current.token
        val attemptedAt = kotlin.time.Clock.System.now().epochSeconds
        if (profileAttempt == identity && attemptedAt - profileAttemptAt in 0 until 86400) return
        profileAttemptAt = attemptedAt
        profileAttempt = identity
        profileWork?.cancel()
        profileWork = scope.launch {
            fun isCurrent(): Boolean = session?.userId == current.userId && session?.token == current.token
            fun project(profile: AccountProfileRecord) {
                if (!isCurrent() || profile.userId != current.userId) return
                displayedProfile = profile
                val ready = _state.value as? AccountUiState.Ready ?: return
                if (ready.userId == profile.userId) _state.value = ready.copy(username = profile.username, avatarUrl = profile.avatarUrl)
            }
            try {
                val cached = secureStore.read("github_display_profile_v1")?.decodeToString()?.let {
                    runCatching { JSON.decodeFromString<AccountProfileRecord>(it) }.getOrNull()
                }?.takeIf { it.userId == current.userId }
                if (cached != null) project(cached)
                val now = kotlin.time.Clock.System.now().epochSeconds
                if (cached != null && now - cached.fetchedAt in 0 until 86400) return@launch
                val profile = backend.profile(current.userId) ?: return@launch
                if (!isCurrent() || profile.userId != current.userId) return@launch
                val record = AccountProfileRecord(profile.userId, profile.username, profile.avatarUrl, now)
                project(record)
                secureStore.write("github_display_profile_v1", JSON.encodeToString(record).encodeToByteArray())
            } catch (cancelled: CancellationException) {
                throw cancelled
            } catch (_: Throwable) {
                // Public metadata failures retain the session and cached display.
            }
        }
    }

    public companion object {
        internal fun create(
            scope: CoroutineScope,
            backend: AccountBackend,
            secureStore: SecureStore,
            deviceId: String,
            deviceName: String,
            persistence: MobilePersistenceStores? = null,
        ): AccountStore = AccountStore(scope, backend, secureStore, deviceId, deviceName, persistence)

        internal fun backend(log: CoreLog, legacyMobileDeviceNames: Set<String>): AccountBackend {
            val transportLog = log.asTransportLog()
            return CloudBackend(
                CloudAccountClient.create(transportLog, legacyMobileDeviceNames),
                transportLog,
            )
        }

        private const val DEVICE_KEY = "relay_device_private_key_v1"
        private const val SESSION_KEY = "github_device_session_v1"
        private const val CONFIGURED_RELAY_KEY = "account_relay_url_v1"

        /** The relay's own cooldown window when a 429 carries no explicit one. */
        internal const val SMS_RESEND_COOLDOWN_SECONDS = 60

        /** The relay a fresh install signs in against; see [AccountDefaults]. */
        private val DEFAULT_RELAY_URL = com.openbitfun.mobile.core.transport.DEFAULT_CLOUD_RELAY_URL
        private val JSON = Json { ignoreUnknownKeys = true }

        private fun encodeRecord(session: AccountSessionData): String = JSON.encodeToString(
            AccountSessionRecord(
                relayUrl = session.relayUrl,
                username = session.username,
                token = session.token,
                userId = session.userId,
                masterKey = Base64.Default.encode(session.masterKey),
                targetDeviceId = session.targetDeviceId,
                targetDeviceName = session.targetDeviceName,
                refreshToken = session.refreshToken,
            ),
        )

        private fun decodeRecord(value: String): AccountSessionData {
            val record = JSON.decodeFromString<AccountSessionRecord>(value)
            return AccountSessionData(
                record.relayUrl,
                record.username,
                record.token,
                record.userId,
                Base64.Default.decode(record.masterKey),
                record.targetDeviceId,
                record.targetDeviceName,
                // Records written before relay-local accounts carry no refresh
                // token; such a session simply renews by signing in again.
                record.refreshToken?.takeIf(String::isNotBlank),
            )
        }
    }

    /**
     * Reads the persisted configured relay, keeping the official default on a
     * missing, unreadable, or (for this build) unacceptable value. The stored
     * bytes are left alone either way: an unparsable value may belong to a
     * newer build, and deleting it would be data loss for a setting.
     */
    private fun initialConfiguredRelayUrl(): String = try {
        secureStore.read(CONFIGURED_RELAY_KEY)?.decodeToString()?.let { stored -> normalizeConfiguredRelayUrl(stored) }
    } catch (_: Throwable) {
        null
    } ?: DEFAULT_RELAY_URL

    /**
     * Best-effort persistence of the configured relay. The in-memory selection
     * always moves; a store that cannot take the write only loses the setting
     * across restarts, which never justifies failing an unrelated sign-in.
     */
    private fun persistConfiguredRelayUrl(endpoint: String) {
        try {
            secureStore.write(CONFIGURED_RELAY_KEY, endpoint.encodeToByteArray())
        } catch (_: Throwable) {
            // Intentionally silent; see the KDoc above.
        }
    }
}

/** The session that produced a directory read, and the read itself. */
private class DirectoryRead(val session: AccountSessionData, val devices: List<AccountDeviceUi>)

private class CloudBackend(
    private val client: CloudAccountClient,
    private val log: TransportLog,
) : AccountBackend {
    override suspend fun relaySupportsLocalAuth(relayUrl: String): Boolean = client.relaySupportsLocalAuth(relayUrl)

    override suspend fun smsSend(relayUrl: String, phone: String) = client.smsSend(relayUrl, phone)

    override suspend fun localLogin(relayUrl: String, phone: String, code: String?, password: String?): LocalTokenPair {
        log.info("local sign-in started")
        val pair = client.localLogin(relayUrl, phone, code, password)
        log.info("local sign-in accepted")
        return pair
    }

    override suspend fun localRefresh(relayUrl: String, refreshToken: String): LocalTokenPair {
        val pair = client.localRefresh(relayUrl, refreshToken)
        log.info("local session renewed")
        return pair
    }

    override suspend fun login(
        relayUrl: String,
        accessToken: String,
        deviceId: String,
        deviceName: String,
        deviceSecret: ByteArray,
    ): AccountSessionData {
        val session = client.login(relayUrl, accessToken, deviceId, deviceName, deviceSecret)
        log.info("account login response accepted")
        return AccountSessionData(
            relayUrl = relayUrl,
            username = session.userId,
            token = session.token,
            userId = session.userId,
            masterKey = session.masterKey,
            targetDeviceId = null,
            targetDeviceName = null,
        )
    }

    override fun directoryChanges(session: AccountSessionData): Flow<Unit> = client.deviceDirectoryChanges(session.relayUrl, session.toTransportSession())
    override fun resumeSessionStreams() { client.resumeSessionStreams() }
    override fun closeAccount() { client.closeAccount() }

    override suspend fun profile(userId: String): com.openbitfun.mobile.core.transport.GitHubProfile? = client.githubProfile(userId)

    override suspend fun listDevices(session: AccountSessionData, selfDeviceId: String): List<AccountDeviceUi> =
        client.listDevices(session.relayUrl, session.toTransportSession(), selfDeviceId).map { it.toUi() }

    override fun transport(session: AccountSessionData, targetDeviceId: String): RemoteCommandTransport =
        AccountDeviceCommandTransport(client, session.relayUrl, session.toTransportSession(), targetDeviceId, log)

    private fun AccountSessionData.toTransportSession(): CloudAccountSession =
        CloudAccountSession(token, userId, masterKey)

    private fun CloudAccountDevice.toUi(): AccountDeviceUi = AccountDeviceUi(deviceId, deviceName, online, lastSeenAt, compatible)
}

@Serializable
private data class AccountSessionRecord(
    val relayUrl: String,
    val username: String,
    val token: String,
    val userId: String,
    val masterKey: String,
    val targetDeviceId: String?,
    val targetDeviceName: String?,
    /**
     * Absent on records written before relay-local accounts; tolerant both
     * ways so an older build can still read a newer record.
     */
    val refreshToken: String? = null,
)

private fun CloudAccountFailure.toUiReason(): AccountFailureReason = when (this) {
    CloudAccountFailure.INVALID_CREDENTIALS -> AccountFailureReason.INVALID_CREDENTIALS
    CloudAccountFailure.AUTHENTICATION -> AccountFailureReason.AUTHENTICATION
    CloudAccountFailure.RATE_LIMITED -> AccountFailureReason.RATE_LIMITED
    CloudAccountFailure.RELAY_UNAVAILABLE -> AccountFailureReason.RELAY_UNAVAILABLE
    CloudAccountFailure.NETWORK -> AccountFailureReason.NETWORK
    CloudAccountFailure.TIMEOUT -> AccountFailureReason.TIMEOUT
    CloudAccountFailure.MALFORMED_RESPONSE -> AccountFailureReason.MALFORMED_RESPONSE
}

@Serializable
private data class AccountProfileRecord(
    val userId: String,
    val username: String,
    val avatarUrl: String?,
    val fetchedAt: Long,
)
