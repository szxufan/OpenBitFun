package com.openbitfun.mobile.core.feature.account

import com.openbitfun.mobile.core.persistence.SecureStore
import com.openbitfun.mobile.core.protocol.CommandStatus
import com.openbitfun.mobile.core.protocol.RemoteCommand
import com.openbitfun.mobile.core.transport.CloudAccountException
import com.openbitfun.mobile.core.transport.CloudAccountFailure
import com.openbitfun.mobile.core.transport.CloudAccountLocalAuthException
import com.openbitfun.mobile.core.transport.LocalAccountUser
import com.openbitfun.mobile.core.transport.LocalTokenPair
import com.openbitfun.mobile.core.transport.RemoteCommandTransport
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.test.advanceUntilIdle
import kotlinx.coroutines.test.runTest
import kotlinx.serialization.DeserializationStrategy
import kotlinx.coroutines.test.advanceTimeBy
import kotlin.test.Test
import kotlin.test.assertContentEquals
import kotlin.test.assertEquals
import kotlin.test.assertFalse
import kotlin.test.assertIs
import kotlin.test.assertNull
import kotlin.test.assertTrue

@OptIn(ExperimentalCoroutinesApi::class)
class AccountStoreTest {
    /** The one sign-in call every test starts from: a phone plus its SMS code. */
    private fun AccountStore.signIn(relayUrl: String = AccountDefaults.CLOUD_RELAY_URL) {
        dispatch(AccountIntent.LocalLogin(relayUrl, "13800138000", "123456", null))
    }

    @Test fun cancelledLoginDirectoryCannotReviveAccountOrPersistSelectedDevice() = runTest {
        for (failure in listOf<Throwable?>(null, CloudAccountException(CloudAccountFailure.NETWORK),
            CloudAccountException(CloudAccountFailure.AUTHENTICATION), IllegalStateException("Late failure"))) {
            val secure = MemorySecureStore(); val backend = FakeAccountBackend()
            var pending: kotlin.coroutines.Continuation<Unit>? = null
            backend.beforeList = { kotlin.coroutines.suspendCoroutine<Unit> { pending = it } }
            val store = AccountStore.create(this, backend, secure, "phone-1", "Android")
            store.signIn(); advanceUntilIdle()
            store.dispatch(AccountIntent.Logout)
            pending!!.resumeWith(if (failure == null) Result.success(Unit) else Result.failure(failure))
            advanceUntilIdle()
            assertIs<AccountUiState.SignedOut>(store.state.value)
            assertNull(secure.read("github_device_session_v1"))
            assertNull(store.createSessionStore(this))
            store.stop()
        }
    }

    @Test fun cancelledRestoreCannotReplaceSignedOutState() = runTest {
        for (failure in listOf<Throwable?>(null, CloudAccountException(CloudAccountFailure.NETWORK),
            CloudAccountException(CloudAccountFailure.AUTHENTICATION))) {
            val secure = MemorySecureStore(); val backend = FakeAccountBackend()
            val first = AccountStore.create(this, backend, secure, "phone-1", "Android")
            first.signIn(); advanceUntilIdle(); first.stop()
            var pending: kotlin.coroutines.Continuation<Unit>? = null
            backend.beforeList = { kotlin.coroutines.suspendCoroutine<Unit> { pending = it } }
            val restored = AccountStore.create(this, backend, secure, "phone-1", "Android")
            restored.dispatch(AccountIntent.Restore); advanceUntilIdle()
            restored.dispatch(AccountIntent.Logout)
            pending!!.resumeWith(if (failure == null) Result.success(Unit) else Result.failure(failure))
            advanceUntilIdle()
            assertIs<AccountUiState.SignedOut>(restored.state.value)
            restored.stop()
        }
    }

    @Test fun cancelledRefreshFailureCannotRestoreSignedOutAccount() = runTest {
        for (failure in listOf(CloudAccountException(CloudAccountFailure.NETWORK),
            CloudAccountException(CloudAccountFailure.AUTHENTICATION), IllegalStateException("Late failure"))) {
            val backend = FakeAccountBackend()
            val store = AccountStore.create(this, backend, MemorySecureStore(), "phone-1", "Android")
            store.signIn(); advanceUntilIdle()
            var pending: kotlin.coroutines.Continuation<Unit>? = null
            backend.beforeList = { kotlin.coroutines.suspendCoroutine<Unit> { pending = it } }
            store.dispatch(AccountIntent.RefreshDevices); advanceUntilIdle()
            store.dispatch(AccountIntent.Logout)
            pending!!.resumeWith(Result.failure(failure)); advanceUntilIdle()
            assertIs<AccountUiState.SignedOut>(store.state.value)
            store.stop()
        }
    }

    @Test fun cancelledAuthenticationFailureCannotExpireReplacementLoginWithSameToken() = runTest {
        val backend = FakeAccountBackend()
        val store = AccountStore.create(this, backend, MemorySecureStore(), "phone-1", "Android")
        store.signIn(); advanceUntilIdle()
        var pending: kotlin.coroutines.Continuation<Unit>? = null
        backend.beforeList = { kotlin.coroutines.suspendCoroutine<Unit> { pending = it } }
        store.dispatch(AccountIntent.RefreshDevices); advanceUntilIdle()
        store.dispatch(AccountIntent.Logout)
        backend.beforeList = null
        backend.userId = "replacement-user"
        store.signIn(); advanceUntilIdle()
        val replacement = assertIs<AccountUiState.Ready>(store.state.value)
        pending!!.resumeWith(Result.failure(CloudAccountException(CloudAccountFailure.AUTHENTICATION)))
        advanceUntilIdle()
        assertEquals(replacement, store.state.value)
        store.stop()
    }

    @Test fun directoryNotificationsRefreshMembershipWithoutAPeriodicPoll() = runTest {
        val changes = kotlinx.coroutines.flow.MutableSharedFlow<Unit>(extraBufferCapacity = 4)
        val backend = FakeAccountBackend().apply { directoryEvents = changes }
        val store = AccountStore.create(this, backend, MemorySecureStore(), "phone-1", "Android")
        store.signIn(); advanceUntilIdle()
        val before = backend.listRequests
        backend.desktop2Online = true; changes.emit(Unit); advanceUntilIdle()
        assertEquals(before + 1, backend.listRequests)
        assertTrue(assertIs<AccountUiState.Ready>(store.state.value).devices.single { it.id == "desktop-2" }.online)
        advanceTimeBy(120_000); advanceUntilIdle()
        assertEquals(before + 1, backend.listRequests)
        store.stop(); changes.emit(Unit); advanceUntilIdle()
        assertEquals(before + 1, backend.listRequests)
    }

    /**
     * A session issued by one relay is never sent to another. The record is
     * kept encrypted where it is, the configured relay stays as configured,
     * and only a fresh sign-in brings the account back.
     */
    @Test fun sessionFromAnotherRelayIsRetainedButNeverSentToTheConfiguredRelay() = runTest {
        val secure = MemorySecureStore(); val backend = FakeAccountBackend()
        val first = AccountStore.create(this, backend, secure, "phone-1", "Android")
        first.signIn(); advanceUntilIdle(); first.stop()
        // A record the user's configured relay did not issue (a version bump
        // of the official relay, or a relay switch on another surface).
        val old = secure.read("github_device_session_v1")!!.decodeToString()
            .replace("\"relayUrl\":\"${AccountDefaults.CLOUD_RELAY_URL}\"", "\"relayUrl\":\"https://remote.openbitfun.com/v/retired\"")
        secure.write("github_device_session_v1", old.encodeToByteArray())
        val requests = backend.listRequests
        val restored = AccountStore.create(this, backend, secure, "phone-1", "Android")
        restored.dispatch(AccountIntent.Restore); advanceUntilIdle()
        val state = assertIs<AccountUiState.Failed>(restored.state.value)
        assertEquals(AccountFailureReason.AUTHENTICATION, state.reason)
        assertEquals(requests, backend.listRequests)
        assertEquals(old, secure.read("github_device_session_v1")!!.decodeToString())
        restored.signIn(); advanceUntilIdle()
        assertEquals(AccountDefaults.CLOUD_RELAY_URL, backend.lastLoginRelay)
        assertIs<AccountUiState.Ready>(restored.state.value)
    }

    @Test
    fun relaySwitchClearsTheSessionPersistsTheSettingAndRequiresFreshSignIn() = runTest {
        val secure = MemorySecureStore(); val backend = FakeAccountBackend()
        val store = AccountStore.create(this, backend, secure, "phone-1", "Android")
        store.signIn(); advanceUntilIdle()
        assertTrue(assertIs<AccountUiState.Ready>(store.state.value).devices.isNotEmpty())

        store.dispatch(AccountIntent.SelectRelay("https://custom.example.com"))
        assertIs<AccountUiState.SignedOut>(store.state.value)
        assertNull(store.createSessionStore(this))
        assertEquals("https://custom.example.com", store.relayUrl.value)
        assertEquals("https://custom.example.com", secure.read("account_relay_url_v1")!!.decodeToString())

        // The retained record belongs to the previous relay and stays unusable.
        val requests = backend.listRequests
        val restored = AccountStore.create(this, backend, secure, "phone-1", "Android")
        assertEquals("https://custom.example.com", restored.relayUrl.value)
        restored.dispatch(AccountIntent.Restore); advanceUntilIdle()
        assertEquals(AccountFailureReason.AUTHENTICATION, assertIs<AccountUiState.Failed>(restored.state.value).reason)
        assertEquals(requests, backend.listRequests)

        restored.signIn("https://custom.example.com"); advanceUntilIdle()
        assertEquals("https://custom.example.com", backend.lastLoginRelay)
        assertIs<AccountUiState.Ready>(restored.state.value)
        assertEquals("https://custom.example.com", restored.relayUrl.value)
    }

    @Test
    fun configuredRelayIsPersistedThroughSignInAndUnreadableSettingsKeepTheirBytes() = runTest {
        val secure = MemorySecureStore(); val backend = FakeAccountBackend()
        // The relay the user actually signs in against becomes the setting,
        // so the record this login persists is the one a later restore expects.
        val first = AccountStore.create(this, backend, secure, "phone-1", "Android")
        first.signIn("https://custom.example.com"); advanceUntilIdle(); first.stop()
        assertEquals("https://custom.example.com", secure.read("account_relay_url_v1")!!.decodeToString())

        val restored = AccountStore.create(this, backend, secure, "phone-1", "Android")
        restored.dispatch(AccountIntent.Restore); advanceUntilIdle()
        assertEquals("https://custom.example.com", assertIs<AccountUiState.Ready>(restored.state.value).relayUrl)
        assertEquals("https://custom.example.com", restored.relayUrl.value)

        // An unparsable stored setting keeps the default without deleting the
        // bytes; a newer build may still accept them.
        secure.write("account_relay_url_v1", "not a url".encodeToByteArray())
        val fallback = AccountStore.create(this, backend, secure, "phone-1", "Android")
        assertEquals(AccountDefaults.CLOUD_RELAY_URL, fallback.relayUrl.value)
        assertEquals("not a url", secure.read("account_relay_url_v1")!!.decodeToString())
    }

    /**
     * An expired device token is not a sign-out when a refresh token is on
     * record: the pair rotates, the SAME stored X25519 key re-registers, and
     * the directory read the user asked for still arrives.
     */
    @Test fun expiredDeviceTokenIsSilentlyRenewedWithTheSameDeviceKey() = runTest {
        val secure = MemorySecureStore(); val backend = FakeAccountBackend()
        val store = AccountStore.create(this, backend, secure, "phone-1", "Android")
        store.signIn(); advanceUntilIdle()
        val firstSecret = backend.loginSecrets.single()
        val firstRefreshToken = backend.localPair.refreshToken

        backend.rejectNextList = CloudAccountFailure.AUTHENTICATION
        store.dispatch(AccountIntent.RefreshDevices); advanceUntilIdle()

        val ready = assertIs<AccountUiState.Ready>(store.state.value)
        assertEquals("desktop-1", ready.selectedDeviceId)
        assertEquals(1, backend.refreshCalls)
        assertEquals(firstRefreshToken, backend.lastRefreshToken)
        assertEquals(2, backend.loginSecrets.size)
        assertContentEquals(firstSecret, backend.loginSecrets[1])
        // The renewed pair's access token — not the retired one — drove the exchange.
        assertEquals("local-access-2", backend.loginAccessTokens.last())
        // The renewed pair is what a later launch restores.
        assertTrue(secure.read("github_device_session_v1")!!.decodeToString().contains("\"token-2\""))
        assertTrue(secure.read("github_device_session_v1")!!.decodeToString().contains("\"refreshToken\":\"refresh-2\""))
        store.stop()
    }

    @Test fun failedRenewalExpiresTheSessionAndPreservesTheStoredRecord() = runTest {
        val secure = MemorySecureStore(); val backend = FakeAccountBackend()
        val store = AccountStore.create(this, backend, secure, "phone-1", "Android")
        store.signIn(); advanceUntilIdle()
        assertTrue(assertIs<AccountUiState.Ready>(store.state.value).devices.isNotEmpty())

        val stored = secure.read("github_device_session_v1")!!.toList()
        // A spent (expired or replayed) refresh token: renewal is impossible.
        backend.rejectNextList = CloudAccountFailure.AUTHENTICATION
        backend.refreshFailure = CloudAccountLocalAuthException(CloudAccountFailure.INVALID_CREDENTIALS, 401, "expired")
        store.dispatch(AccountIntent.RefreshDevices); advanceUntilIdle()

        val failed = assertIs<AccountUiState.Failed>(store.state.value)
        assertEquals(AccountFailureReason.AUTHENTICATION, failed.reason)
        assertEquals(1, backend.refreshCalls)
        assertEquals(stored, secure.read("github_device_session_v1")?.toList())
        assertEquals(0, secure.deleteCount)
        assertNull(store.createSessionStore(this))
        store.stop()
    }

    @Test
    fun relayLocalRejectionSurfacesTheRelaySentenceVerbatim() = runTest {
        val backend = FakeAccountBackend().apply {
            localFailure = CloudAccountLocalAuthException(
                CloudAccountFailure.AUTHENTICATION, 403, "registration is closed on this relay",
            )
        }
        val store = AccountStore.create(this, backend, MemorySecureStore(), "phone-1", "Android")
        store.signIn(); advanceUntilIdle()

        val failed = assertIs<AccountUiState.Failed>(store.state.value)
        assertEquals(AccountFailureReason.AUTHENTICATION, failed.reason)
        assertEquals("registration is closed on this relay", failed.detail)
        assertEquals(0, backend.exchangeCalls)
    }

    @Test
    fun enrichesExistingAccountAndRetainsIdentityAndCacheOffline() = runTest {
        val secure = MemorySecureStore()
        val backend = FakeAccountBackend().apply {
            userId = "42"
            profileResult = com.openbitfun.mobile.core.transport.GitHubProfile(42, "octocat", "https://avatars.githubusercontent.com/u/42")
        }
        val store = AccountStore.create(this, backend, secure, "phone-1", "Android")
        store.signIn()
        advanceUntilIdle()
        val ready = assertIs<AccountUiState.Ready>(store.state.value)
        assertEquals("42", ready.userId)
        assertEquals("octocat", ready.username)
        assertEquals("https://avatars.githubusercontent.com/u/42", ready.avatarUrl)
        val credentials = secure.read("github_device_session_v1")!!
        backend.profileResult = null
        val restored = AccountStore.create(this, backend, secure, "phone-1", "iOS")
        restored.dispatch(AccountIntent.Restore)
        advanceUntilIdle()
        assertEquals("octocat", assertIs<AccountUiState.Ready>(restored.state.value).username)
        assertContentEquals(credentials, secure.read("github_device_session_v1"))
        assertEquals(1, backend.profileLoads)
    }

    @Test
    fun metadataForAnotherAccountCannotReplaceDisplayOrAuthority() = runTest {
        val backend = FakeAccountBackend().apply {
            userId = "42"
            profileResult = com.openbitfun.mobile.core.transport.GitHubProfile(99, "other", null)
        }
        val store = AccountStore.create(this, backend, MemorySecureStore(), "phone-1", "Android")
        store.signIn()
        advanceUntilIdle()
        val ready = assertIs<AccountUiState.Ready>(store.state.value)
        assertEquals("42", ready.userId)
        assertEquals("user", ready.username)
        assertNull(ready.avatarUrl)
    }

    @Test
    fun loginSelectsOnlineDesktopAndPersistsRestorableSession() = runTest {
        val secure = MemorySecureStore()
        val backend = FakeAccountBackend()
        val store = AccountStore.create(this, backend, secure, "phone-1", "Android")

        store.signIn()
        advanceUntilIdle()

        val ready = assertIs<AccountUiState.Ready>(store.state.value)
        assertEquals("user-id", ready.userId)
        assertEquals("desktop-1", ready.selectedDeviceId)
        assertTrue(secure.read("github_device_session_v1")?.isNotEmpty() == true)
        // The relay-local credentials the form collected never leak through
        // the intent that carried them.
        assertFalse(
            AccountIntent.LocalLogin(AccountDefaults.CLOUD_RELAY_URL, "13800138000", null, "top-secret-value")
                .toString()
                .contains("top-secret-value"),
        )
        val restored = AccountStore.create(this, backend, secure, "phone-1", "Android")
        restored.dispatch(AccountIntent.Restore)
        advanceUntilIdle()
        assertEquals("desktop-1", assertIs<AccountUiState.Ready>(restored.state.value).selectedDeviceId)
    }

    @Test
    fun onlyControllableDevicesReachTheList() = runTest {
        val store = AccountStore.create(this, FakeAccountBackend(), MemorySecureStore(), "phone-1", "Android")
        store.signIn()
        advanceUntilIdle()

        val ready = assertIs<AccountUiState.Ready>(store.state.value)
        // This device and the user's other phone are gone; the offline desktop
        // stays, because "known but not running" is worth showing.
        assertEquals(listOf("desktop-1", "desktop-2"), ready.devices.map { it.id })
    }

    @Test
    fun deviceSelectionAndLogoutUpdateSecureState() = runTest {
        val secure = MemorySecureStore()
        val store = AccountStore.create(this, FakeAccountBackend(), secure, "phone-1", "Android")
        store.signIn()
        advanceUntilIdle()

        // Neither this device nor an offline one can become the control target.
        store.dispatch(AccountIntent.SelectDevice("phone-1"))
        store.dispatch(AccountIntent.SelectDevice("desktop-2"))
        assertEquals("desktop-1", assertIs<AccountUiState.Ready>(store.state.value).selectedDeviceId)
        store.dispatch(AccountIntent.Logout)

        assertIs<AccountUiState.SignedOut>(store.state.value)
        assertEquals(1, secure.deleteCount)
        assertNull(secure.read("github_device_session_v1"))
    }

    @Test
    fun deviceSelectionWriteFailureRestoresBytesAndFailsClosed() = runTest {
        val secure = MemorySecureStore()
        val backend = FakeAccountBackend().also { it.desktop2Online = true }
        val store = AccountStore.create(this, backend, secure, "phone-1", "Android")
        store.signIn()
        advanceUntilIdle()
        assertEquals("desktop-1", assertIs<AccountUiState.Ready>(store.state.value).selectedDeviceId)
        val stored = secure.read("github_device_session_v1")!!.toList()

        secure.failWrites = true
        secure.mutateBeforeWriteFailure = true
        store.dispatch(AccountIntent.SelectDevice("desktop-2"))

        val failed = assertIs<AccountUiState.Failed>(store.state.value)
        assertEquals(AccountFailureReason.SECURE_STORAGE, failed.reason)
        assertEquals(AccountFailureStage.SECURE_STORAGE, failed.stage)
        assertEquals(stored, secure.read("github_device_session_v1")?.toList())
        assertNull(store.createSessionStore(this))
        assertNull(store.createSessionStore(this, "desktop-1"))
        assertNull(store.createWorkspaceStore(this))
        assertNull(store.createWorkspaceStore(this, "desktop-1"))
        assertTrue(backend.transportTargets.isEmpty())
        assertEquals(0, secure.deleteCount)
    }

    @Test
    fun refreshPicksUpADesktopThatCameOnlineAndSurvivesFailing() = runTest {
        val backend = FakeAccountBackend()
        val store = AccountStore.create(this, backend, MemorySecureStore(), "phone-1", "Android")
        store.signIn()
        advanceUntilIdle()
        assertFalse(assertIs<AccountUiState.Ready>(store.state.value).devices.single { it.id == "desktop-2" }.online)

        backend.desktop2Online = true
        store.dispatch(AccountIntent.RefreshDevices)
        advanceUntilIdle()
        val refreshed = assertIs<AccountUiState.Ready>(store.state.value)
        assertTrue(refreshed.devices.single { it.id == "desktop-2" }.online)
        assertFalse(refreshed.refreshing)
        assertNull(refreshed.refreshFailure)

        // A failed refresh reports why and leaves the list it could not replace.
        backend.listFailure = CloudAccountFailure.NETWORK
        store.dispatch(AccountIntent.RefreshDevices)
        advanceUntilIdle()
        val failed = assertIs<AccountUiState.Ready>(store.state.value)
        assertEquals(AccountFailureReason.NETWORK, failed.refreshFailure)
        assertFalse(failed.refreshing)
        assertEquals(refreshed.devices, failed.devices)
        assertEquals("desktop-1", failed.selectedDeviceId)
    }

    @Test fun coldRestorePublishesIdentityBeforeDirectoryWithoutGrantingTargetAuthority() = runTest {
        val secure = MemorySecureStore(); val backend = FakeAccountBackend()
        val first = AccountStore.create(this, backend, secure, "phone-1", "Android")
        first.signIn(); advanceUntilIdle(); first.stop()
        val saved = secure.read("github_device_session_v1")!!
        var pending: kotlin.coroutines.Continuation<Unit>? = null
        backend.beforeList = { kotlin.coroutines.suspendCoroutine<Unit> { pending = it } }
        val restored = AccountStore.create(this, backend, secure, "phone-1", "Android")
        restored.dispatch(AccountIntent.Restore); advanceUntilIdle()
        val loading = assertIs<AccountUiState.Ready>(restored.state.value)
        assertEquals("user-id", loading.userId)
        assertTrue(loading.refreshing)
        assertTrue(loading.devices.isEmpty())
        assertNull(loading.selectedDeviceId)
        assertNull(restored.createSessionStore(this))
        assertNull(restored.createWorkspaceStore(this, "desktop-1"))
        pending!!.resumeWith(Result.success(Unit)); advanceUntilIdle()
        val ready = assertIs<AccountUiState.Ready>(restored.state.value)
        assertFalse(ready.refreshing)
        assertEquals("desktop-1", ready.selectedDeviceId)
        assertContentEquals(saved, secure.read("github_device_session_v1"))
        restored.stop()
    }

    @Test fun failedRefreshDoesNotRollBackDeviceSelectedWhileRequestWasPending() = runTest {
        for (failure in listOf(CloudAccountException(CloudAccountFailure.TIMEOUT), IllegalStateException("Network unavailable"))) {
            val backend = FakeAccountBackend().apply { desktop2Online = true }
            val secure = MemorySecureStore()
            val store = AccountStore.create(this, backend, secure, "phone-1", "Android")
            store.signIn(); advanceUntilIdle()
            var pending: kotlin.coroutines.Continuation<Unit>? = null
            backend.beforeList = { kotlin.coroutines.suspendCoroutine<Unit> { pending = it } }
            store.dispatch(AccountIntent.RefreshDevices); advanceUntilIdle()
            store.dispatch(AccountIntent.SelectDevice("desktop-2"))
            val selected = assertIs<AccountUiState.Ready>(store.state.value)
            assertEquals("desktop-2", selected.selectedDeviceId)
            val saved = secure.read("github_device_session_v1")!!
            pending!!.resumeWith(Result.failure(failure)); advanceUntilIdle()
            val failed = assertIs<AccountUiState.Ready>(store.state.value)
            assertEquals(selected.selectedDeviceId, failed.selectedDeviceId)
            assertEquals(selected.selectedDeviceName, failed.selectedDeviceName)
            assertEquals(selected.devices, failed.devices)
            assertFalse(failed.refreshing)
            assertEquals(if (failure is CloudAccountException) AccountFailureReason.TIMEOUT else AccountFailureReason.NETWORK, failed.refreshFailure)
            assertContentEquals(saved, secure.read("github_device_session_v1"))
            store.createSessionStore(this)
            assertEquals("desktop-2", backend.transportTargets.last())
            store.stop()
        }
    }

    @Test
    fun explicitDeviceStoresCoexistWithSelectedTargetStore() = runTest {
        val backend = FakeAccountBackend()
        val store = AccountStore.create(this, backend, MemorySecureStore(), "phone-1", "Android")
        store.signIn()
        advanceUntilIdle()

        // The old single-target entry points still resolve the selected device.
        assertTrue(store.createSessionStore(this) != null)
        assertTrue(store.createWorkspaceStore(this) != null)

        // The new explicit-device entry points address a device without changing selection.
        assertTrue(store.createSessionStore(this, "desktop-2") != null)
        assertTrue(store.createWorkspaceStore(this, "desktop-2") != null)

        assertEquals(
            listOf("desktop-1", "desktop-1", "desktop-2", "desktop-2"),
            backend.transportTargets,
        )
    }

    @Test
    fun explicitDeviceStoresRequireAuthenticatedRegisteredControlTargets() = runTest {
        val backend = FakeAccountBackend()
        val signedOut = AccountStore.create(this, backend, MemorySecureStore(), "phone-1", "Android")
        assertNull(signedOut.createSessionStore(this, "desktop-1"))
        assertNull(signedOut.createWorkspaceStore(this, "desktop-1"))
        assertTrue(backend.transportTargets.isEmpty())

        signedOut.signIn()
        advanceUntilIdle()
        assertNull(signedOut.createSessionStore(this, ""))
        assertNull(signedOut.createSessionStore(this, "phone-1"))
        assertNull(signedOut.createSessionStore(this, "phone-2"))
        assertNull(signedOut.createSessionStore(this, "unknown"))
        // Registered offline targets remain authorized for cache-backed directory rows.
        assertTrue(signedOut.createSessionStore(this, "desktop-2") != null)
        assertTrue(signedOut.createWorkspaceStore(this, "desktop-2") != null)
        assertEquals(listOf("desktop-2", "desktop-2"), backend.transportTargets)
    }

    @Test
    fun invalidRestoreKeepsOpaqueRecordAndCanBeRetried() = runTest {
        val secure = MemorySecureStore()
        val raw = "not-a-session-record".encodeToByteArray()
        secure.write("github_device_session_v1", raw)
        val store = AccountStore.create(this, FakeAccountBackend(), secure, "phone-1", "Android")

        store.dispatch(AccountIntent.Restore)
        advanceUntilIdle()
        assertEquals(AccountFailureReason.SECURE_STORAGE, assertIs<AccountUiState.Failed>(store.state.value).reason)
        assertTrue(assertIs<AccountUiState.Failed>(store.state.value).canRetry)
        assertEquals(0, secure.deleteCount)
        assertEquals(raw.toList(), secure.read("github_device_session_v1")?.toList())

        store.dispatch(AccountIntent.Restore)
        advanceUntilIdle()
        assertEquals(AccountFailureReason.SECURE_STORAGE, assertIs<AccountUiState.Failed>(store.state.value).reason)
        assertEquals(0, secure.deleteCount)
        assertEquals(raw.toList(), secure.read("github_device_session_v1")?.toList())
    }

    @Test
    fun secureReadFailureFailsClosedWithoutDeletingStoredBytes() = runTest {
        val secure = MemorySecureStore()
        val raw = "opaque-existing-session".encodeToByteArray()
        secure.write("github_device_session_v1", raw)
        secure.failReads = true
        val store = AccountStore.create(this, FakeAccountBackend(), secure, "phone-1", "Android")

        store.dispatch(AccountIntent.Restore)
        advanceUntilIdle()

        val failed = assertIs<AccountUiState.Failed>(store.state.value)
        assertEquals(AccountFailureReason.SECURE_STORAGE, failed.reason)
        assertEquals(AccountFailureStage.RESTORE, failed.stage)
        assertEquals(0, secure.deleteCount)
        secure.failReads = false
        assertEquals(raw.toList(), secure.read("github_device_session_v1")?.toList())
    }

    @Test
    fun legacyRestoreKeepsOpaqueRecord() = runTest {
        val secure = MemorySecureStore()
        val raw = "{\"token\":\"legacy-token\"}".encodeToByteArray()
        secure.write("github_device_session_v1", raw)
        val store = AccountStore.create(this, FakeAccountBackend(), secure, "phone-1", "Android")

        store.dispatch(AccountIntent.Restore)
        advanceUntilIdle()

        assertEquals(AccountFailureReason.SECURE_STORAGE, assertIs<AccountUiState.Failed>(store.state.value).reason)
        assertEquals(0, secure.deleteCount)
        assertEquals(raw.toList(), secure.read("github_device_session_v1")?.toList())
    }

    @Test
    fun legacyRecordWithoutRefreshTokenStillRestores() = runTest {
        // A record written before relay-local accounts: no refresh token, so
        // an expiry can only be answered with the sign-in form — but the
        // session itself is as valid as it ever was.
        val secure = MemorySecureStore(); val backend = FakeAccountBackend()
        val first = AccountStore.create(this, backend, secure, "phone-1", "Android")
        first.signIn(); advanceUntilIdle(); first.stop()
        val legacy = secure.read("github_device_session_v1")!!.decodeToString()
            .replace(",\"refreshToken\":\"refresh-1\"", "")
        secure.write("github_device_session_v1", legacy.encodeToByteArray())

        val restored = AccountStore.create(this, backend, secure, "phone-1", "Android")
        restored.dispatch(AccountIntent.Restore); advanceUntilIdle()
        assertEquals("desktop-1", assertIs<AccountUiState.Ready>(restored.state.value).selectedDeviceId)

        // Without a refresh token, an expired session expires; the record survives.
        val stored = secure.read("github_device_session_v1")!!.toList()
        backend.rejectNextList = CloudAccountFailure.AUTHENTICATION
        restored.dispatch(AccountIntent.RefreshDevices); advanceUntilIdle()
        assertEquals(0, backend.refreshCalls)
        assertEquals(AccountFailureReason.AUTHENTICATION, assertIs<AccountUiState.Failed>(restored.state.value).reason)
        assertEquals(stored, secure.read("github_device_session_v1")?.toList())
    }

    @Test
    fun validRestoreDoesNotDeletePersistedSession() = runTest {
        val secure = MemorySecureStore()
        val backend = FakeAccountBackend()
        val first = AccountStore.create(this, backend, secure, "phone-1", "Android")
        first.signIn()
        advanceUntilIdle()
        val raw = secure.read("github_device_session_v1")

        val restored = AccountStore.create(this, backend, secure, "phone-1", "Android")
        restored.dispatch(AccountIntent.Restore)
        advanceUntilIdle()

        assertIs<AccountUiState.Ready>(restored.state.value)
        assertEquals(0, secure.deleteCount)
        assertEquals(raw?.toList(), secure.read("github_device_session_v1")?.toList())
    }

    @Test
    fun expiredSessionWithoutRenewalRevokesCapabilitiesAndPreservesTheStoredRecord() = runTest {
        val secure = MemorySecureStore()
        val backend = FakeAccountBackend()
        val store = AccountStore.create(this, backend, secure, "phone-1", "Android")
        store.signIn()
        advanceUntilIdle()
        assertTrue(assertIs<AccountUiState.Ready>(store.state.value).devices.isNotEmpty())

        val stored = secure.read("github_device_session_v1")!!.toList()
        backend.listFailure = CloudAccountFailure.AUTHENTICATION
        backend.refreshFailure = CloudAccountException(CloudAccountFailure.NETWORK)
        store.dispatch(AccountIntent.RefreshDevices)
        advanceUntilIdle()

        val failed = assertIs<AccountUiState.Failed>(store.state.value)
        assertEquals(AccountFailureReason.AUTHENTICATION, failed.reason)
        assertEquals(stored, secure.read("github_device_session_v1")?.toList())
        assertEquals(0, secure.deleteCount)
        assertNull(store.createSessionStore(this))
    }

    @Test
    fun signInWithNothingOnlinePicksNoTarget() = runTest {
        val backend = FakeAccountBackend().also { it.desktop1Online = false }
        val store = AccountStore.create(this, backend, MemorySecureStore(), "phone-1", "Android")

        store.signIn()
        advanceUntilIdle()

        // Not this device as a consolation prize: the live account has ten
        // registered devices and no desktop running, and that is what it says.
        assertNull(assertIs<AccountUiState.Ready>(store.state.value).selectedDeviceId)
    }

    @Test
    fun backendFailuresStayTyped() = runTest {
        val backend = FakeAccountBackend().also { it.failure = CloudAccountFailure.AUTHENTICATION }
        val store = AccountStore.create(this, backend, MemorySecureStore(), "phone-1", "Android")

        store.signIn()
        advanceUntilIdle()

        val failed = assertIs<AccountUiState.Failed>(store.state.value)
        assertEquals(AccountFailureReason.AUTHENTICATION, failed.reason)
        assertEquals(AccountFailureStage.AUTHENTICATION, failed.stage)
    }

    @Test
    fun unknownAuthenticationFailureIsProtocolFailureNotSecureStorage() = runTest {
        val backend = FakeAccountBackend().also { it.loginThrowable = IllegalStateException("crypto failed") }
        val store = AccountStore.create(this, backend, MemorySecureStore(), "phone-1", "Android")

        store.signIn()
        advanceUntilIdle()

        val failed = assertIs<AccountUiState.Failed>(store.state.value)
        assertEquals(AccountFailureReason.MALFORMED_RESPONSE, failed.reason)
        assertEquals(AccountFailureStage.AUTHENTICATION, failed.stage)
    }

    @Test
    fun secureStoreWriteFailureIsTheOnlyUntypedSecureStorageFailure() = runTest {
        val secure = MemorySecureStore(failWrites = true)
        val store = AccountStore.create(this, FakeAccountBackend(), secure, "phone-1", "Android")

        store.signIn()
        advanceUntilIdle()

        val failed = assertIs<AccountUiState.Failed>(store.state.value)
        assertEquals(AccountFailureReason.SECURE_STORAGE, failed.reason)
        assertEquals(AccountFailureStage.SECURE_STORAGE, failed.stage)
    }

    @Test
    fun deviceListFailurePreservesAuthenticatedAccountAndPersistedData() = runTest {
        val secure = MemorySecureStore()
        val backend = FakeAccountBackend().also { it.listThrowable = IllegalStateException("transport failed") }
        val store = AccountStore.create(this, backend, secure, "phone-1", "Android")

        store.signIn()
        advanceUntilIdle()

        val ready = assertIs<AccountUiState.Ready>(store.state.value)
        assertEquals(AccountFailureReason.NETWORK, ready.refreshFailure)
        assertNull(ready.selectedDeviceId)
        assertTrue(secure.read("github_device_session_v1")?.isNotEmpty() == true)
        assertEquals(0, secure.deleteCount)
    }

    @Test
    fun deviceListFailuresKeepTheirTypedReasons() = runTest {
        CloudAccountFailure.entries.forEach { transportReason ->
            val backend = FakeAccountBackend().also { it.listFailure = transportReason }
            val store = AccountStore.create(this, backend, MemorySecureStore(), "phone-1", "Android")

            store.signIn()
            advanceUntilIdle()

            if (transportReason == CloudAccountFailure.AUTHENTICATION) {
                // The fresh pair's refresh token is rotated once and the retry
                // is refused again; the session then expires.
                val failed = assertIs<AccountUiState.Failed>(store.state.value)
                assertEquals(AccountFailureReason.AUTHENTICATION, failed.reason)
                assertFalse(failed.canRetry)
            } else {
                val ready = assertIs<AccountUiState.Ready>(store.state.value)
                assertEquals(transportReason.toExpectedReason(), ready.refreshFailure)
                assertEquals("user-id", ready.userId)
                assertNull(store.createSessionStore(this))
            }
            store.stop()
        }
    }

    @Test
    fun deviceListRetryReusesSessionWithoutTouchingStoredBytes() = runTest {
        val secure = MemorySecureStore()
        val backend = FakeAccountBackend().also { it.listFailure = CloudAccountFailure.TIMEOUT }
        val store = AccountStore.create(this, backend, secure, "phone-1", "Android")

        store.signIn()
        advanceUntilIdle()
        val readyBeforeRetry = assertIs<AccountUiState.Ready>(store.state.value)
        assertEquals(AccountFailureReason.TIMEOUT, readyBeforeRetry.refreshFailure)
        val stored = secure.read("github_device_session_v1")!!.toList()
        val writes = secure.writeCount
        val deletes = secure.deleteCount

        backend.listFailure = null
        store.dispatch(AccountIntent.Retry)
        advanceUntilIdle()

        val ready = assertIs<AccountUiState.Ready>(store.state.value)
        assertEquals("desktop-1", ready.selectedDeviceId)
        assertEquals(stored, secure.read("github_device_session_v1")?.toList())
        assertEquals(writes, secure.writeCount)
        assertEquals(deletes, secure.deleteCount)
    }

    @Test
    fun writeFailureFailsClosedAndRestoresExistingBytes() = runTest {
        val secure = MemorySecureStore()
        val existing = "existing-session-bytes".encodeToByteArray()
        secure.write("github_device_session_v1", existing)
        secure.failWrites = true
        secure.mutateBeforeWriteFailure = true
        val store = AccountStore.create(this, FakeAccountBackend(), secure, "phone-1", "Android")

        store.signIn()
        advanceUntilIdle()

        val failed = assertIs<AccountUiState.Failed>(store.state.value)
        assertEquals(AccountFailureReason.SECURE_STORAGE, failed.reason)
        assertNull(store.createSessionStore(this))
        assertEquals(existing.toList(), secure.read("github_device_session_v1")?.toList())
        assertEquals(0, secure.deleteCount)
    }

    @Test
    fun deleteFailureLogsOutInMemoryWithoutDestroyingStoredBytes() = runTest {
        val secure = MemorySecureStore()
        val store = AccountStore.create(this, FakeAccountBackend(), secure, "phone-1", "Android")
        store.signIn()
        advanceUntilIdle()
        val stored = secure.read("github_device_session_v1")!!.toList()
        secure.failDeletes = true

        store.dispatch(AccountIntent.Logout)

        val failed = assertIs<AccountUiState.Failed>(store.state.value)
        assertEquals(AccountFailureReason.SECURE_STORAGE, failed.reason)
        assertNull(store.createSessionStore(this))
        assertEquals(stored, secure.read("github_device_session_v1")?.toList())
    }

    @Test
    fun offlineRestoreKeepsEncryptedSessionForRetry() = runTest {
        val secure = MemorySecureStore()
        val backend = FakeAccountBackend()
        val first = AccountStore.create(this, backend, secure, "phone-1", "Android")
        first.signIn()
        advanceUntilIdle()
        backend.listFailure = CloudAccountFailure.NETWORK

        val restored = AccountStore.create(this, backend, secure, "phone-1", "Android")
        restored.dispatch(AccountIntent.Restore)
        advanceUntilIdle()

        val offline = assertIs<AccountUiState.Ready>(restored.state.value)
        assertEquals(AccountFailureReason.NETWORK, offline.refreshFailure)
        assertEquals("user-id", offline.userId)
        assertTrue(offline.devices.isEmpty())
        assertNull(offline.selectedDeviceId)
        assertNull(restored.createSessionStore(this))
        val saved = secure.read("github_device_session_v1")!!.toList()
        backend.listFailure = null
        restored.dispatch(AccountIntent.RefreshDevices); advanceUntilIdle()
        val recovered = assertIs<AccountUiState.Ready>(restored.state.value)
        assertNull(recovered.refreshFailure)
        assertEquals("desktop-1", recovered.selectedDeviceId)
        assertEquals(saved, secure.read("github_device_session_v1")!!.toList())
        first.stop(); restored.stop()
    }
}

private class MemorySecureStore(
    var failWrites: Boolean = false,
    var failReads: Boolean = false,
    var failDeletes: Boolean = false,
    var mutateBeforeWriteFailure: Boolean = false,
) : SecureStore {
    private val values = mutableMapOf<String, ByteArray>()
    var writeCount: Int = 0
        private set
    var deleteCount: Int = 0
        private set

    override fun read(key: String): ByteArray? {
        if (failReads) error("secure store read failed")
        return values[key]?.copyOf()
    }

    override fun write(key: String, value: ByteArray) {
        writeCount += 1
        if (failWrites) {
            if (mutateBeforeWriteFailure) {
                values[key] = value.copyOf()
                // Let AccountStore prove that it restores the previous bytes.
                failWrites = false
            }
            error("secure store write failed")
        }
        values[key] = value.copyOf()
    }

    override fun delete(key: String) {
        deleteCount += 1
        if (failDeletes) error("secure store delete failed")
        values.remove(key)
    }
}

private fun CloudAccountFailure.toExpectedReason(): AccountFailureReason = when (this) {
    CloudAccountFailure.INVALID_CREDENTIALS -> AccountFailureReason.INVALID_CREDENTIALS
    CloudAccountFailure.AUTHENTICATION -> AccountFailureReason.AUTHENTICATION
    CloudAccountFailure.RATE_LIMITED -> AccountFailureReason.RATE_LIMITED
    CloudAccountFailure.RELAY_UNAVAILABLE -> AccountFailureReason.RELAY_UNAVAILABLE
    CloudAccountFailure.NETWORK -> AccountFailureReason.NETWORK
    CloudAccountFailure.TIMEOUT -> AccountFailureReason.TIMEOUT
    CloudAccountFailure.MALFORMED_RESPONSE -> AccountFailureReason.MALFORMED_RESPONSE
}

private class FakeAccountBackend : AccountBackend {
    var directoryEvents: kotlinx.coroutines.flow.Flow<Unit> = kotlinx.coroutines.flow.emptyFlow()
    override fun directoryChanges(session: AccountSessionData) = directoryEvents

    var userId = "user-id"
    var listRequests = 0
    var beforeList: (suspend () -> Unit)? = null
    var lastLoginRelay = ""
    var profileResult: com.openbitfun.mobile.core.transport.GitHubProfile? = null
    var profileLoads = 0
    override suspend fun profile(userId: String): com.openbitfun.mobile.core.transport.GitHubProfile? {
        profileLoads++
        return profileResult
    }

    /** The relay-local pair a sign-in is granted; refresh rotation returns [refreshPair]. */
    var localPair = LocalTokenPair(
        accessToken = "local-access", refreshToken = "refresh-1",
        accessExpiresAt = 1_000, refreshExpiresAt = 2_000,
        user = LocalAccountUser(userId = "user-id", login = "user", phoneMasked = "138****0000", hasPassword = false),
    )
    var refreshPair = LocalTokenPair(
        accessToken = "local-access-2", refreshToken = "refresh-2",
        accessExpiresAt = 3_000, refreshExpiresAt = 4_000,
        user = LocalAccountUser(userId = "user-id", login = "user", phoneMasked = "138****0000", hasPassword = false),
    )
    var refreshCalls = 0
    var lastRefreshToken: String? = null
    var refreshFailure: Throwable? = null
    var localFailure: CloudAccountLocalAuthException? = null

    override suspend fun relaySupportsLocalAuth(relayUrl: String): Boolean = true
    override suspend fun smsSend(relayUrl: String, phone: String) {}

    override suspend fun localLogin(relayUrl: String, phone: String, code: String?, password: String?): LocalTokenPair {
        localFailure?.let { throw it }
        return localPair
    }

    override suspend fun localRefresh(relayUrl: String, refreshToken: String): LocalTokenPair {
        refreshCalls++
        lastRefreshToken = refreshToken
        refreshFailure?.let { throw it }
        return refreshPair
    }

    var failure: CloudAccountFailure? = null
    var loginThrowable: Throwable? = null
    var listFailure: CloudAccountFailure? = null
    /** Rejects exactly the next device list with this failure, then clears. */
    var rejectNextList: CloudAccountFailure? = null
    var listThrowable: Throwable? = null
    var desktop1Online: Boolean = true
    var desktop2Online: Boolean = false
    var settings: String? = null
    val transportTargets = mutableListOf<String>()
    val loginSecrets = mutableListOf<ByteArray>()
    val loginAccessTokens = mutableListOf<String>()
    var exchangeCalls = 0
    override suspend fun login(
        relayUrl: String,
        accessToken: String,
        deviceId: String,
        deviceName: String,
        deviceSecret: ByteArray,
    ): AccountSessionData {
        lastLoginRelay = relayUrl
        exchangeCalls++
        loginSecrets += deviceSecret.copyOf()
        loginAccessTokens += accessToken
        loginThrowable?.let { throw it }
        failure?.let { throw CloudAccountException(it) }
        return AccountSessionData(
            relayUrl,
            "user",
            if (accessToken == refreshPair.accessToken) "token-2" else "token",
            userId,
            ByteArray(32) { it.toByte() },
            null,
            null,
        )
    }

    override suspend fun listDevices(session: AccountSessionData, selfDeviceId: String): List<AccountDeviceUi> {
        listRequests++
        beforeList?.invoke()
        rejectNextList?.let { rejection ->
            rejectNextList = null
            throw CloudAccountException(rejection)
        }
        listThrowable?.let { throw it }
        listFailure?.let { throw CloudAccountException(it) }
        // The shape the live account returns: this device, one of the user's
        // other phones, and the desktops that are the only real targets.
        return listOf(
            AccountDeviceUi("phone-1", "Android", true, null),
            AccountDeviceUi("phone-2", "HarmonyOS Phone", true, 1),
            AccountDeviceUi("desktop-1", "Desktop", desktop1Online, 1),
            AccountDeviceUi("desktop-2", "DESKTOP-KM3L4UI", desktop2Online, 1),
        )
    }


    override fun transport(session: AccountSessionData, targetDeviceId: String): RemoteCommandTransport {
        transportTargets += targetDeviceId
        return object : RemoteCommandTransport {
            override suspend fun <T : CommandStatus> send(
                deserializer: DeserializationStrategy<T>,
                command: RemoteCommand,
                timeoutMs: Long,
            ): T = error("unused")
        }
    }
}
