package com.openbitfun.mobile.core.transport

import kotlinx.coroutines.CoroutineDispatcher
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.filter
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.drop
import kotlinx.coroutines.flow.onCompletion
import kotlinx.coroutines.flow.onEach
import kotlinx.coroutines.flow.merge
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.mapNotNull
import kotlinx.coroutines.flow.update
import kotlinx.serialization.json.jsonObject
import com.openbitfun.mobile.core.protocol.CommandStatusResponse

import com.openbitfun.mobile.core.crypto.CloudAccountCipher
import com.openbitfun.mobile.core.crypto.DeviceIdentity
import com.openbitfun.mobile.core.protocol.CommandStatus
import com.openbitfun.mobile.core.protocol.EncryptedPayload
import com.openbitfun.mobile.core.protocol.RelayJson
import com.openbitfun.mobile.core.protocol.RemoteCommand
import com.openbitfun.mobile.core.protocol.isError
import io.ktor.client.HttpClient
import io.ktor.client.plugins.HttpRequestTimeoutException
import io.ktor.client.request.accept
import io.ktor.client.request.bearerAuth
import io.ktor.client.request.request
import io.ktor.client.request.setBody
import io.ktor.client.statement.bodyAsText
import io.ktor.http.ContentType
import io.ktor.http.HttpMethod
import io.ktor.http.contentType
import io.ktor.client.plugins.timeout
import kotlinx.serialization.DeserializationStrategy
import kotlinx.serialization.ExperimentalSerializationApi
import kotlinx.serialization.MissingFieldException
import kotlinx.serialization.SerialName
import kotlinx.serialization.Serializable
import kotlinx.serialization.SerializationStrategy
import kotlinx.serialization.builtins.ListSerializer
import kotlinx.serialization.json.jsonPrimitive
import kotlinx.serialization.json.contentOrNull
import kotlinx.serialization.json.JsonObject
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.serialization.json.decodeFromJsonElement
import kotlinx.serialization.json.encodeToJsonElement
import kotlinx.coroutines.CancellationException
import kotlin.io.encoding.Base64
import kotlin.uuid.Uuid
import kotlin.uuid.ExperimentalUuidApi
import kotlin.time.TimeSource

public const val DEFAULT_CLOUD_RELAY_URL: String = "https://remote.openbitfun.com/v/1.0.2"

/** A relayed stream page slower than this is worth a breadcrumb; faster ones are not. */
internal const val SLOW_STREAM_PAGE_MS: Long = 300L

/** Device kinds the relay accepts; mirrors `relay-service/src/db.rs::DEVICE_KINDS`. */
private const val DEVICE_KIND_DESKTOP = "desktop"

/**
 * A headless host: the CLI and TUI delivery profiles belong to this kind.
 */
private const val DEVICE_KIND_CLI = "cli"

/**
 * What this client registers itself as. Constant rather than a parameter: the
 * shared transport only ships inside the Android and iOS apps, and a desktop
 * never reaches the relay through it.
 */
private const val DEVICE_KIND_MOBILE = "mobile"

/**
 * Names used by our pre-`device_kind` mobile clients.
 *
 * This belongs to the shared transport rather than an Android/iOS adapter: both
 * native clients can receive the same legacy account rows, and both must hide
 * them from the desktop target picker.
 */
private val KNOWN_NON_DESKTOP_DEVICE_NAMES = setOf(
    "HarmonyOS Phone",
    "HarmonyOS Watch",
)

/**
 * Whether a relay device row is a host, and so controllable from a phone.
 *
 * A row that reports its kind is taken at its word: a desktop and a CLI host run
 * the same control plane, so both are targets, while a phone or a watch is only
 * ever a controller. A row without one predates the relay learning about kinds,
 * and is judged by two weaker signals: this phone's own row is never a host, and
 * neither is one carrying a name our own builds register under. Anything else
 * stays visible — hiding a real host would strand the user, while a stale phone
 * row disappears the next time that phone logs in against a relay that stores
 * kinds.
 */
private fun AccountDeviceWire.isHost(
    selfDeviceId: String,
    isLegacyMobileDeviceName: (String) -> Boolean,
): Boolean {
    val kind = deviceKind?.trim().orEmpty()
    if (kind.isNotEmpty()) return kind == DEVICE_KIND_DESKTOP || kind == DEVICE_KIND_CLI
    if (selfDeviceId.isNotEmpty() && deviceId == selfDeviceId) return false
    return !isLegacyMobileDeviceName(deviceName)
}

public enum class CloudAccountFailure {
    INVALID_CREDENTIALS,
    AUTHENTICATION,
    RATE_LIMITED,
    RELAY_UNAVAILABLE,
    NETWORK,

    /**
     * The request left, and nothing came back before [timeoutMs] ran out.
     *
     * Separate from [NETWORK] because the two ask for different things: a
     * network error means try again, a timeout on a device RPC usually means the
     * desktop is working on something big and the wait was too short.
     */
    TIMEOUT,
    MALFORMED_RESPONSE,
}

public class CloudAccountException public constructor(
    public val failure: CloudAccountFailure,
    public val statusCode: Int?,
    cause: Throwable?,
    /** The relay's own wording, when it gave one; for the log, never for the UI. */
    public val detail: String? = null,
) : IllegalStateException("Cloud account request failed: $failure" + (detail?.let { " ($it)" } ?: ""), cause) {
    public constructor(failure: CloudAccountFailure, statusCode: Int?) : this(failure, statusCode, null)

    public constructor(failure: CloudAccountFailure) : this(failure, null, null)
}

public data class CloudAccountSession public constructor(
    public val token: String,
    public val userId: String,
    public val masterKey: ByteArray,
) {
    override fun toString(): String = "CloudAccountSession(token=<redacted>, userId=$userId, masterKey=<redacted>)"
}

public data class CloudAccountDevice public constructor(
    public val deviceId: String,
    public val deviceName: String,
    public val online: Boolean,
    public val lastSeenAt: Long?,
    /** `desktop`, or null for a row the relay stored before kinds existed. */
    public val deviceKind: String? = null,
    /**
     * Relay-computed mutual-control compatibility of this device with this one.
     *
     * `false` means confirmed incompatible: either a client build/protocol
     * mismatch or a peer that reported no version information (an older client).
     * Null only on an older Relay that does not gate at all, which must be
     * treated as "unknown but usable", never as incompatible.
     */
    public val compatible: Boolean? = null,
) {
    /** The single gate every control entry point reuses; see [compatible]. */
    public val controllable: Boolean get() = compatible != false
}

public class CloudAccountClient internal constructor(
    private val client: HttpClient,
    private val log: TransportLog = TransportLog.None,
    legacyMobileDeviceNames: Set<String> = emptySet(),
    private val processingDispatcher: CoroutineDispatcher = Dispatchers.Default,
    private val realtimeFactory: (HttpClient, String, String) -> AccountRpcConnection = { client, url, token -> AccountRealtime(client, url, token, log) },
) {
    private class Connection(val url: String, val token: String, val socket: AccountRpcConnection)
    private val realtime = MutableStateFlow<Connection?>(null)

    private fun connection(relayUrl: String, token: String): AccountRpcConnection {
        val url = requireNotNull(normalizeRelayEndpointUrl(relayUrl))
        while (true) {
            val current = realtime.value
            if (current != null && current.url == url && current.token == token) return current.socket
            val next = Connection(url, token, realtimeFactory(client, url, token))
            if (realtime.compareAndSet(current, next)) {
                current?.socket?.close()
                return next.socket
            }
            next.socket.close()
        }
    }

    /** Retain the closed binding so stale transports cannot reopen a signed-out account. */
    private val historyReaders = mutableMapOf<String, Channel<CompletableDeferred<Unit>>>()
    /**
     * Asks the subscribed stream for one older page and waits for its answer.
     *
     * The stream answers every request it is handed, including by failing the
     * ones it cannot serve before it ends, so a session that is still subscribed
     * never leaves this waiting. A session without a live subscription is an
     * error rather than a silently dropped tap.
     */
    public suspend fun loadOlderSession(targetDeviceId: String, sessionId: String) {
        val channel = historyReaders[targetDeviceId + ":" + sessionId]
            ?: error("Session is not subscribed")
        val request = CompletableDeferred<Unit>()
        log.info("history request started session=${sessionId.take(24)}")
        try {
            channel.send(request)
            request.await()
            log.info("history request answered session=${sessionId.take(24)}")
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (error: Throwable) {
            // Ids and failure kinds only, like the stream reader's own reports.
            log.warn("history request failed session=${sessionId.take(24)} type=${error::class.simpleName} message=${error.message}")
            throw error
        }
    }
    private val foregroundResumes = MutableSharedFlow<Long>(extraBufferCapacity = 1)
    public fun resumeSessionStreams() { foregroundResumes.tryEmit(0L) }

    /** Directory changes are invalidation hints, never a substitute for authenticated membership. */
    public fun deviceDirectoryChanges(relayUrl: String, session: CloudAccountSession): kotlinx.coroutines.flow.Flow<Unit> {
        val socket = connection(relayUrl, session.token)
        return merge(socket.notifications.filter { it["type"]?.jsonPrimitive?.contentOrNull == "device-presence" }.map { Unit },
            socket.connections.drop(1).map { Unit })
    }

    public fun closeAccount() { forgetPeerKeys(); realtime.value?.socket?.close() }

    private val normalizedLegacyMobileDeviceNames =
        (KNOWN_NON_DESKTOP_DEVICE_NAMES + legacyMobileDeviceNames).mapTo(mutableSetOf()) {
            it.trim().lowercase()
        }

    /**
     * Whether this relay serves the relay-local phone-account contract.
     *
     * A 2xx answer is decided by its `capabilities` list; any 4xx is an older
     * relay that does not serve the contract, which reads as a plain `false`
     * rather than an error. A 5xx answer or a network failure throws: the two
     * ask for different UI ("temporarily unavailable" and "relay unreachable")
     * and neither may be reported as "unsupported", which would tell the user
     * to give up on a relay that merely needs a retry or a connection.
     */
    public suspend fun relaySupportsLocalAuth(relayUrl: String): Boolean {
        val (status, text) = perform(relayUrl, "/api/info", HttpMethod.Get, null, "", RELAY_DEFAULT_TIMEOUT_MS)
        if (status in 400..499) return false
        if (status !in 200..299) throw statusFailure(status)
        val capabilities = try {
            withContext(processingDispatcher) {
                RelayJson.parseToJsonElement(text).jsonObject["capabilities"]
            }
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (_: Throwable) {
            throw CloudAccountException(CloudAccountFailure.MALFORMED_RESPONSE)
        }
        return capabilities is kotlinx.serialization.json.JsonArray &&
            "local_auth_v1" in capabilities.mapNotNull { (it as? kotlinx.serialization.json.JsonPrimitive)?.content }
    }

    /**
     * Ask the relay to text a verification code to [phone].
     *
     * The relay remains the authority on phone shape and cooldown; a 429
     * throws [CloudAccountLocalAuthException] carrying the relay's own sentence
     * and [CloudAccountLocalAuthException.retryAfterSeconds], which the form
     * must honor for its resend countdown.
     */
    public suspend fun smsSend(relayUrl: String, phone: String) {
        localAuthRequest(
            relayUrl, "/api/auth/local/sms/send",
            SmsSendRequest.serializer(), SmsSendRequest(phone),
        )
    }

    /**
     * Sign in with an SMS code (registering the account on first use) or, for
     * an account that set one, its password. Exactly one of [code] and
     * [password] is sent.
     */
    public suspend fun localLogin(relayUrl: String, phone: String, code: String?, password: String?): LocalTokenPair {
        require(code.isNullOrBlank() != password.isNullOrBlank()) { "Exactly one of code or password is required." }
        return decodeLocalPair(
            localAuthRequest(
                relayUrl, "/api/auth/local/login",
                LocalLoginRequest.serializer(), LocalLoginRequest(phone, code?.takeIf(String::isNotBlank), password?.takeIf(String::isNotBlank)),
            ),
        )
    }

    /**
     * Rotate a refresh token into a fresh pair. A 401 (expired or replayed
     * token) throws, and the caller must treat that as "sign in again" rather
     * than retry.
     */
    public suspend fun localRefresh(relayUrl: String, refreshToken: String): LocalTokenPair = decodeLocalPair(
        localAuthRequest(
            relayUrl, "/api/auth/local/refresh",
            LocalRefreshRequest.serializer(), LocalRefreshRequest(refreshToken),
        ),
    )

    @OptIn(ExperimentalUuidApi::class)
    public suspend fun login(relayUrl: String, accessToken: String, deviceId: String, deviceName: String, deviceSecret: ByteArray): CloudAccountSession {
        require(accessToken.isNotBlank())
        require(deviceSecret.size == 32) { "Invalid device key." }
        val secret = deviceSecret.copyOf()
        try {
            val auth = request(
                relayUrl, "/api/auth/login", HttpMethod.Post,
                LoginRequest.serializer(), LoginRequest(accessToken, deviceId, deviceName, DEVICE_KIND_MOBILE,
                    Base64.Default.encode(DeviceIdentity.publicKey(secret)), Uuid.random().toString(),
                    CLIENT_VERSION, CLIENT_PROTOCOL_VERSION),
                AccountAuthResponse.serializer(), "", RELAY_DEFAULT_TIMEOUT_MS,
            )
            if (auth.token.isBlank() || auth.userId.isBlank()) throw CloudAccountException(CloudAccountFailure.MALFORMED_RESPONSE)
            return CloudAccountSession(auth.token, auth.userId, secret)
        } catch (cause: Throwable) { secret.fill(0); throw cause }
    }

    /**
     * The account's controllable devices — desktops only.
     *
     * A relay that stores device kinds already filters this list; the client
     * repeats the judgement so a phone stops listing itself and its peers
     * before that relay is deployed. [selfDeviceId] is this install's own id.
     */
    public suspend fun listDevices(
        relayUrl: String,
        session: CloudAccountSession,
        selfDeviceId: String = "",
    ): List<CloudAccountDevice> =
        requestWithoutBody(
            relayUrl,
            "/api/devices",
            HttpMethod.Get,
            ListSerializer(AccountDeviceWire.serializer()),
            session.token,
            RELAY_DEFAULT_TIMEOUT_MS,
        ).filter { device ->
            device.isHost(selfDeviceId) { name ->
                name.trim().lowercase() in normalizedLegacyMobileDeviceNames
            }
        }.map { device ->
            CloudAccountDevice(
                device.deviceId,
                device.deviceName.ifEmpty { device.deviceId },
                device.online,
                device.lastSeenAt,
                device.deviceKind,
                device.compatible,
            )
        }

    /**
     * Opens one host-owned stream on [targetDeviceId], read on demand over
     * encrypted device RPC. The relay forwards ciphertext only, and nothing is
     * written on this device: the transcript lives exactly as long as the flow.
     */
    public suspend fun subscribeSession(
        relayUrl: String, session: CloudAccountSession, targetDeviceId: String, sessionId: String,
        onError: (Throwable) -> Unit, onCaughtUp: () -> Unit,
    ): kotlinx.coroutines.flow.Flow<JsonObject> {
        val socket = connection(relayUrl, session.token)
        val target = targetDeviceId.trim()
        val historyKey = target + ":" + sessionId
        val historyRequests = Channel<CompletableDeferred<Unit>>(Channel.RENDEZVOUS)
        historyReaders[historyKey] = historyRequests
        val hints = socket.notifications.mapNotNull { notice -> decodeStreamHint(relayUrl, session, target, notice) }
        val reads = object : HostStreamReads {
            override suspend fun read(after: Long?, before: Long?, epoch: Long?): StreamPageWire {
                check(realtime.value?.socket === socket) { "Account changed" }
                val startedAt = TimeSource.Monotonic.markNow()
                val page = deviceRpc(relayUrl, session, target,
                    RemoteCommand(cmd = "read_stream", streamId = sessionId, after = after, before = before, epoch = epoch, subscribe = true),
                    StreamPageWire.serializer(), RELAY_DEFAULT_TIMEOUT_MS)
                val elapsedMs = startedAt.elapsedNow().inWholeMilliseconds
                // The opening page (`after == null`) is the one the user waits for,
                // and a slow page is worth naming wherever it happens; a page per
                // hint during a streaming turn is not, so it stays quiet.
                if (after == null || elapsedMs >= SLOW_STREAM_PAGE_MS) {
                    log.info("stream page stream=${sessionId.take(24)} after=${after ?: -1} before=${before ?: -1} events=${page.events.size} has_more=${page.hasMore} ms=$elapsedMs")
                }
                return page
            }
            override suspend fun unsubscribe() {
                if (realtime.value?.socket !== socket) return
                deviceRpc(relayUrl, session, target, RemoteCommand(cmd = "unsubscribe_stream", streamId = sessionId),
                    CommandStatusResponse.serializer(), RELAY_DEFAULT_TIMEOUT_MS)
            }
        }
        // Time from "the user opened this session" to "the host's rows are on
        // screen, ready to render": every page read plus the reading side's own
        // reduction of those pages. The store renders only after `onCaughtUp`, so
        // the gap between the two counts is what the receiving device spends.
        val openedAt = TimeSource.Monotonic.markNow()
        var recordsSeen = 0
        return hostStream(sessionId, target, hints, merge(socket.connections.drop(1), foregroundResumes), reads,
            olderRequests = historyRequests, onError = { error ->
                log.warn("host stream read failed stream=${sessionId.take(24)} type=${error::class.simpleName} failure=${(error as? CloudAccountException)?.failure} message=${error.message}")
                onError(error)
            }, onCaughtUp = {
                log.info("host stream caught up stream=${sessionId.take(24)} events=$recordsSeen elapsed_ms=${openedAt.elapsedNow().inWholeMilliseconds}")
                onCaughtUp()
            }).onEach { recordsSeen++ }.onCompletion { cause ->
                log.info("host stream ended stream=${sessionId.take(24)} events=$recordsSeen cause=${cause?.let { it::class.simpleName } ?: "none"}")
                if (historyReaders[historyKey] === historyRequests) historyReaders.remove(historyKey)
                historyRequests.close()
            }
    }

    /**
     * Decrypts a relayed `device-event` from [target]; null for presence
     * notices, events from other devices, and anything that is not a stream
     * hint. A hint that cannot be decrypted is dropped: the next keepalive or
     * reconnect re-reads the host anyway.
     */
    private suspend fun decodeStreamHint(relayUrl: String, session: CloudAccountSession, target: String, notice: JsonObject): StreamHint? {
        if (notice["type"]?.jsonPrimitive?.contentOrNull != "device-event") return null
        val source = notice["sourceDeviceId"]?.jsonPrimitive?.contentOrNull ?: return null
        if (source != target) return null
        val params = notice["params"] as? JsonObject ?: return null
        val envelope = try { RelayJson.decodeFromJsonElement(EncryptedPayload.serializer(), params) } catch (_: Throwable) { return null }
        return try {
            val key = peerMessageKey(relayUrl, session, target)
            val plain = CloudAccountCipher.decrypt(decode(envelope.encryptedData), key, decode(envelope.nonce)).decodeToString()
            parseStreamHint(source, RelayJson.parseToJsonElement(plain).jsonObject)
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (error: Throwable) {
            log.warn("device event ignored source=${source.take(12)} reason=${error::class.simpleName}")
            null
        }
    }

    /**
     * Pairwise message keys, cached per relay and peer. Stream reads happen on
     * every hint; fetching the peer's public key each time would double their
     * relay traffic. A decrypt failure evicts the entry so a re-paired desktop
     * is picked up on the next call.
     */
    private val peerKeys = MutableStateFlow<Map<String, ByteArray>>(emptyMap())
    private suspend fun peerMessageKey(relayUrl: String, session: CloudAccountSession, target: String): ByteArray {
        val cacheId = requireNotNull(normalizeRelayEndpointUrl(relayUrl)) + "|" + session.userId + "|" + target
        peerKeys.value[cacheId]?.let { return it }
        val peer = requestWithoutBody(relayUrl,
            "/api/devices/" + encodePathSegment(target) + "/key", HttpMethod.Get,
            DeviceKeyWire.serializer(), session.token, RELAY_DEFAULT_TIMEOUT_MS)
        val key = DeviceIdentity.messageKey(session.masterKey, decode(peer.publicKey))
        peerKeys.update { it + (cacheId to key) }
        return key
    }
    private fun forgetPeerKeys() { peerKeys.value = emptyMap() }

    public suspend fun <T : CommandStatus> deviceRpc(
        relayUrl: String,
        session: CloudAccountSession,
        targetDeviceId: String,
        command: RemoteCommand,
        deserializer: DeserializationStrategy<T>,
        timeoutMs: Long,
    ): T {
        val target = targetDeviceId.trim()
        if (target.isEmpty()) throw CloudAccountException(CloudAccountFailure.MALFORMED_RESPONSE)
        val socket = connection(relayUrl, session.token)
        val messageKey = peerMessageKey(relayUrl, session, target)
        val payload = withContext(processingDispatcher) {
            val nonce = DeviceIdentity.randomBytes(12)
            val plain = RelayJson.encodeToString(RemoteCommand.serializer(), command).encodeToByteArray()
            val encrypted = CloudAccountCipher.encrypt(plain, messageKey, nonce)
            EncryptedPayload(Base64.Default.encode(encrypted), Base64.Default.encode(nonce))
        }
        val response = RelayJson.decodeFromJsonElement(EncryptedPayload.serializer(),
            socket.call(target,
                RelayJson.encodeToJsonElement(EncryptedPayload.serializer(), payload), timeoutMs))
        val decoded = try {
            withContext(processingDispatcher) {
                CloudAccountCipher.decrypt(
                    decode(response.encryptedData), messageKey, decode(response.nonce),
                ).decodeToString()
            }
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (error: CloudAccountException) {
            throw error
        } catch (cause: Throwable) {
            forgetPeerKeys()
            log.error("device rpc undecryptable cmd=${command.cmd} reason=${cause::class.simpleName}")
            throw CloudAccountException(CloudAccountFailure.MALFORMED_RESPONSE, null, cause)
        }
        return try {
            withContext(processingDispatcher) { RelayJson.decodeFromString(deserializer, decoded) }
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (cause: Throwable) {
            log.error("device rpc undecodable cmd=${command.cmd} bytes=${decoded.length} ${decodeDetail(cause)}")
            throw CloudAccountException(CloudAccountFailure.MALFORMED_RESPONSE, null, cause)
        }
    }

    private suspend fun <Request, Response> request(
        relayUrl: String,
        path: String,
        method: HttpMethod,
        serializer: SerializationStrategy<Request>,
        body: Request,
        deserializer: DeserializationStrategy<Response>,
        token: String,
        timeoutMs: Long,
    ): Response = execute(relayUrl, path, method,
        withContext(processingDispatcher) { RelayJson.encodeToString(serializer, body) }, deserializer, token, timeoutMs)

    private suspend fun <Response> requestWithoutBody(
        relayUrl: String,
        path: String,
        method: HttpMethod,
        deserializer: DeserializationStrategy<Response>,
        token: String,
        timeoutMs: Long,
    ): Response = execute(relayUrl, path, method, null, deserializer, token, timeoutMs)

    private suspend fun <Response> execute(
        relayUrl: String,
        path: String,
        method: HttpMethod,
        body: String?,
        deserializer: DeserializationStrategy<Response>,
        token: String,
        timeoutMs: Long,
    ): Response {
        val (status, text) = perform(relayUrl, path, method, body, token, timeoutMs)
        if (status !in 200..299) {
            log.warn("account request rejected path=$path status=$status")
            throw statusFailure(status)
        }
        return try {
            withContext(processingDispatcher) { RelayJson.decodeFromString(deserializer, text) }.also {
                if (accountBreadcrumb(path)) log.info("account response decoded path=$path")
            }
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (cause: Throwable) {
            // The body itself is never logged: it carries whatever the desktop
            // was asked for, and on this path that is the user's own sessions.
            log.error("account response undecodable path=$path bytes=${text.length} ${decodeDetail(cause)}")
            throw CloudAccountException(CloudAccountFailure.MALFORMED_RESPONSE, null, cause)
        }
    }

    /**
     * One relay-local auth round trip. Answers carry the relay's own sentences
     * for the form, so a non-2xx status is turned into
     * [CloudAccountLocalAuthException] with that wording (and the cooldown on a
     * 429) instead of the generic status mapping [execute] applies.
     */
    private suspend fun <Request> localAuthRequest(
        relayUrl: String,
        path: String,
        serializer: SerializationStrategy<Request>,
        body: Request,
    ): JsonObject {
        val (status, text) = perform(
            relayUrl, path, HttpMethod.Post,
            withContext(processingDispatcher) { RelayJson.encodeToString(serializer, body) },
            "", RELAY_DEFAULT_TIMEOUT_MS,
        )
        if (status !in 200..299) throw localAuthFailure(status, text)
        return try {
            withContext(processingDispatcher) { RelayJson.parseToJsonElement(text).jsonObject }
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (_: Throwable) {
            log.error("account response undecodable path=$path bytes=${text.length}")
            throw CloudAccountException(CloudAccountFailure.MALFORMED_RESPONSE)
        }
    }

    /**
     * Sends one request and returns its status and body text. Transport-level
     * failures (offline, timeout) are normalized to [CloudAccountException]
     * here so every caller reports them the same way.
     */
    private suspend fun perform(
        relayUrl: String,
        path: String,
        method: HttpMethod,
        body: String?,
        token: String,
        timeoutMs: Long,
    ): Pair<Int, String> {
        val breadcrumb = accountBreadcrumb(path)
        if (breadcrumb) log.info("account request started method=${method.value} path=$path")
        val response = try {
            client.request(requireNotNull(normalizeRelayEndpointUrl(relayUrl)) + path) {
                this.method = method
                contentType(ContentType.Application.Json)
                accept(ContentType.Application.Json)
                if (token.isNotEmpty()) bearerAuth(token)
                if (body != null) setBody(body)
                timeout { requestTimeoutMillis = timeoutMs }
            }
        } catch (cancelled: CancellationException) {
            throw cancelled
        } catch (timedOut: HttpRequestTimeoutException) {
            log.warn("account request timed out path=$path after=${timeoutMs}ms")
            throw CloudAccountException(CloudAccountFailure.TIMEOUT, null, timedOut)
        } catch (cause: Throwable) {
            log.error("account request failed path=$path reason=${cause::class.simpleName}")
            throw CloudAccountException(CloudAccountFailure.NETWORK, null, cause)
        }
        val text = response.bodyAsText()
        if (breadcrumb) log.info("account response received path=$path status=${response.status.value} bytes=${text.length}")
        return response.status.value to text
    }

    private fun accountBreadcrumb(path: String): Boolean =
        path.startsWith("/api/auth/") || path == "/api/info"

    /** GitHub public metadata only: never attach the relay token or master key. */
    public suspend fun githubProfile(userId: String): GitHubProfile? {
        if (userId.isEmpty() || userId.first() == '0' || !userId.all { it in '0'..'9' }) return null
        val response = client.request("https://api.github.com/user/$userId") {
            method = HttpMethod.Get
            accept(ContentType.Application.Json)
            headers.append("User-Agent", "OpenBitFun-Mobile")
        }
        if (response.status.value != 200) return null
        val profile = RelayJson.decodeFromString<GitHubProfile>(response.bodyAsText())
        return profile.takeIf { it.userId == userId && Regex("[A-Za-z0-9][A-Za-z0-9-]{0,38}").matches(it.username) }
            ?.let { it.copy(avatarUrl = it.avatarUrl?.takeIf { url -> url.startsWith("https://avatars.githubusercontent.com/") }) }
    }

    public companion object {
        public fun generateDeviceSecret(): ByteArray = DeviceIdentity.generateSecret()

        public fun create(): CloudAccountClient = CloudAccountClient(relayHttpClient(), TransportLog.None)

        public fun create(log: TransportLog): CloudAccountClient = CloudAccountClient(relayHttpClient(), log)

        public fun create(
            log: TransportLog,
            legacyMobileDeviceNames: Set<String>,
        ): CloudAccountClient = CloudAccountClient(relayHttpClient(), log, legacyMobileDeviceNames)
    }
}

/** Device-to-device commands encrypted using authenticated X25519 public keys. */
public class AccountDeviceCommandTransport public constructor(
    private val client: CloudAccountClient,
    private val relayUrl: String,
    private val session: CloudAccountSession,
    private val targetDeviceId: String,
    private val log: TransportLog,
) : RemoteCommandTransport, RemoteSessionStreamTransport {
    override fun wakeSessionStreams() { client.resumeSessionStreams() }
    override suspend fun loadOlder(sessionId: String) { client.loadOlderSession(targetDeviceId, sessionId) }
    override suspend fun subscribe(sessionId: String, onError: (Throwable) -> Unit, onCaughtUp: () -> Unit): kotlinx.coroutines.flow.Flow<JsonObject> =
        client.subscribeSession(relayUrl, session, targetDeviceId, sessionId, onError, onCaughtUp)

    public constructor(
        client: CloudAccountClient,
        relayUrl: String,
        session: CloudAccountSession,
        targetDeviceId: String,
    ) : this(client, relayUrl, session, targetDeviceId, TransportLog.None)

    override suspend fun <T : CommandStatus> send(
        deserializer: DeserializationStrategy<T>,
        command: RemoteCommand,
        timeoutMs: Long,
    ): T {
        val label = "cmd=${command.cmd} request=${command.requestId.orEmpty().take(12)} " +
            "device=${targetDeviceId.take(12)}"
        log.info("command start $label")

        val response = try {
            client.deviceRpc(relayUrl, session, targetDeviceId, command, deserializer, timeoutMs)
        } catch (error: CloudAccountException) {
            log.warn("command failed $label failure=${error.failure}")
            throw RelayTransportException(error.failure.asRelayFailure(), error)
        }

        if (response.isError) {
            // The desktop's own sentence, already localized there. Echoed to the
            // user, never matched on — same rule as the paired transport.
            log.warn("command rejected $label")
            throw RelayTransportException(RelayFailure.RemoteRejected(response.message))
        }
        log.info("command done $label resp=${response.resp ?: "unknown"}")
        return response
    }
}

private fun CloudAccountFailure.asRelayFailure(): RelayFailure = when (this) {
    CloudAccountFailure.INVALID_CREDENTIALS, CloudAccountFailure.AUTHENTICATION -> RelayFailure.AuthenticationRequired
    CloudAccountFailure.RATE_LIMITED -> RelayFailure.RateLimited
    CloudAccountFailure.RELAY_UNAVAILABLE -> RelayFailure.RelayUnavailable(HTTP_SERVER_ERROR)
    CloudAccountFailure.NETWORK -> RelayFailure.NetworkUnreachable
    CloudAccountFailure.TIMEOUT -> RelayFailure.Timeout
    CloudAccountFailure.MALFORMED_RESPONSE -> RelayFailure.MalformedResponse
}

/**
 * [CloudAccountFailure] keeps the status on the exception rather than in the
 * value, and [RelayFailure.RelayUnavailable] wants one — this stands in for the
 * whole 5xx band, which is all that value is ever switched on.
 */
private const val HTTP_SERVER_ERROR = 500

/** The account profile a relay-local sign-in returns with every token pair. */
@Serializable
public data class LocalAccountUser public constructor(
    public val userId: String,
    public val login: String,
    public val phoneMasked: String,
    public val hasPassword: Boolean,
)

/**
 * One relay-local access/refresh pair. The access token feeds the unchanged
 * device exchange ([CloudAccountClient.login]); the refresh token renews it
 * without a fresh SMS sign-in.
 */
@Serializable
public data class LocalTokenPair public constructor(
    public val accessToken: String,
    public val refreshToken: String,
    public val accessExpiresAt: Long,
    public val refreshExpiresAt: Long,
    public val user: LocalAccountUser,
) {
    override fun toString(): String = "LocalTokenPair(<redacted>)"
}

/**
 * A relay-local auth answer the form must quote rather than map onto a reason
 * code: a cooldown, a closed registration, an unconfigured SMS provider.
 *
 * Unlike [CloudAccountException.detail] — which is log-only — [relayMessage]
 * is the relay's own user-facing sentence and is meant for the UI. It is
 * echoed verbatim, never matched on, the same rule the paired transport
 * applies to a desktop's rejection wording.
 */
public class CloudAccountLocalAuthException public constructor(
    /** The closest [CloudAccountFailure] kind; selects fallback copy, never overrides [relayMessage]. */
    public val failure: CloudAccountFailure,
    public val statusCode: Int,
    public val relayMessage: String,
    /** Present on a 429: the relay's own cooldown in seconds. */
    public val retryAfterSeconds: Long? = null,
) : IllegalStateException(
    "Relay-local auth request failed: HTTP $statusCode" + (relayMessage.takeIf(String::isNotEmpty)?.let { " ($it)" } ?: ""),
) {
    override fun toString(): String = "CloudAccountLocalAuthException(failure=$failure status=$statusCode retryAfterSeconds=$retryAfterSeconds)"
}

/**
 * Normalizes a mainland-China mobile number for the relay-local account API.
 *
 * Mirrors the relay's own `normalize_phone` so a form can gate the send-code
 * action locally; the relay re-validates and remains the authority.
 */
public fun normalizeAccountPhone(raw: String): String? {
    val trimmed = raw.replace(Regex("[\\s-]+"), "")
    val digits = when {
        trimmed.startsWith("+86") -> trimmed.substring(3)
        trimmed.startsWith("86") && trimmed.length == 13 -> trimmed.substring(2)
        else -> trimmed
    }
    return digits.takeIf { Regex("^1[3-9]\\d{9}$").matches(it) }
}

@Serializable
private data class SmsSendRequest(val phone: String)

/** Exactly one of [code] / [password] is non-null; `explicitNulls = false` drops the other. */
@Serializable
private data class LocalLoginRequest(val phone: String, val code: String? = null, val password: String? = null)

@Serializable
private data class LocalRefreshRequest(val refreshToken: String)

@Serializable
private data class LocalAccountUserWire(
    val userId: String? = null,
    val login: String? = null,
    val phoneMasked: String? = null,
    val hasPassword: Boolean = false,
)

@Serializable
private data class LocalTokenPairWire(
    val accessToken: String? = null,
    val refreshToken: String? = null,
    val accessExpiresAt: Long? = null,
    val refreshExpiresAt: Long? = null,
    val user: LocalAccountUserWire? = null,
)

private fun decodeLocalPair(value: JsonObject): LocalTokenPair {
    val wire = try {
        RelayJson.decodeFromJsonElement(LocalTokenPairWire.serializer(), value)
    } catch (cancelled: CancellationException) {
        throw cancelled
    } catch (_: Throwable) {
        throw CloudAccountException(CloudAccountFailure.MALFORMED_RESPONSE)
    }
    val account = wire.user
    val userId = account?.userId
    if (wire.accessToken.isNullOrBlank() || wire.refreshToken.isNullOrBlank() ||
        wire.accessExpiresAt == null || wire.refreshExpiresAt == null || userId.isNullOrBlank()
    ) {
        throw CloudAccountException(CloudAccountFailure.MALFORMED_RESPONSE)
    }
    return LocalTokenPair(
        accessToken = wire.accessToken,
        refreshToken = wire.refreshToken,
        accessExpiresAt = wire.accessExpiresAt,
        refreshExpiresAt = wire.refreshExpiresAt,
        user = LocalAccountUser(
            userId = userId,
            login = account.login?.takeIf(String::isNotBlank) ?: userId,
            phoneMasked = account.phoneMasked?.takeIf(String::isNotBlank) ?: "",
            hasPassword = account.hasPassword,
        ),
    )
}

@Serializable
private data class LocalAuthErrorWire(val error: String? = null, @SerialName("retry_after_secs") val retryAfterSecs: Long? = null)

/**
 * Turns a rejected relay-local auth answer into the exception the form reads.
 * A body that is not JSON, or not the error shape, still yields the status:
 * the relay's wording is preferred, never required.
 */
private fun localAuthFailure(status: Int, text: String): CloudAccountLocalAuthException {
    val wire = try {
        RelayJson.decodeFromString(LocalAuthErrorWire.serializer(), text)
    } catch (_: Throwable) {
        null
    }
    val failure = when (status) {
        400, 401 -> CloudAccountFailure.INVALID_CREDENTIALS
        403 -> CloudAccountFailure.AUTHENTICATION
        429 -> CloudAccountFailure.RATE_LIMITED
        in 500..599 -> CloudAccountFailure.RELAY_UNAVAILABLE
        else -> CloudAccountFailure.MALFORMED_RESPONSE
    }
    return CloudAccountLocalAuthException(failure, status, wire?.error?.takeIf(String::isNotBlank) ?: "", wire?.retryAfterSecs?.takeIf { it > 0 })
}

@Serializable
private data class LoginRequest(
    @SerialName("access_token") val accessToken: String,
    @SerialName("device_id") val deviceId: String,
    @SerialName("device_name") val deviceName: String,
    @SerialName("device_kind") val deviceKind: String,
    @SerialName("public_key") val publicKey: String,
    @SerialName("request_id") val requestId: String,
    /** See [CLIENT_VERSION]; the Relay stores both and gates on the protocol. */
    @SerialName("clientVersion") val clientVersion: String,
    @SerialName("clientProtocol") val clientProtocol: Int,
)
@Serializable
private data class AccountAuthResponse(val token: String, @SerialName("user_id") val userId: String)
@Serializable
private data class DeviceKeyWire(@SerialName("public_key") val publicKey: String)

@Serializable
private data class AccountDeviceWire(
    @SerialName("device_id") val deviceId: String,
    @SerialName("device_name") val deviceName: String,
    val online: Boolean,
    @SerialName("last_seen_at") val lastSeenAt: Long? = null,
    @SerialName("device_kind") val deviceKind: String? = null,
    val compatible: Boolean? = null,
)

private fun decode(value: String): ByteArray = try {
    Base64.Default.withPadding(Base64.PaddingOption.PRESENT_OPTIONAL).decode(value)
} catch (_: Throwable) {
    throw CloudAccountException(CloudAccountFailure.MALFORMED_RESPONSE)
}

/**
 * What to say about a decode failure without quoting the payload.
 *
 * kotlinx puts the useful part — which field, at which position in the document —
 * into a message that also carries a slice of the input, and on this path that
 * input is the user's own sessions. So the message is never printed: the missing
 * field names come from [MissingFieldException]'s own list, and everything else
 * contributes only the `$.a.b[0]` path that kotlinx appends, which names the
 * schema rather than the data.
 */
@OptIn(ExperimentalSerializationApi::class)
internal fun decodeDetail(cause: Throwable): String {
    val kind = "reason=" + (cause::class.simpleName ?: "unknown")
    if (cause is MissingFieldException) {
        return kind + " missing=" + cause.missingFields.joinToString(",")
    }
    val path = cause.message?.let { JSON_PATH.find(it) }?.value
    return if (path == null) kind else "$kind at=$path"
}

private val JSON_PATH = Regex("""\$(\.[A-Za-z_][A-Za-z0-9_]*|\[\d+])+""")

private fun statusFailure(status: Int): CloudAccountException = when (status) {
    401, 403 -> CloudAccountException(CloudAccountFailure.AUTHENTICATION, status)
    429 -> CloudAccountException(CloudAccountFailure.RATE_LIMITED, status)
    in 500..599 -> CloudAccountException(CloudAccountFailure.RELAY_UNAVAILABLE, status)
    else -> CloudAccountException(CloudAccountFailure.MALFORMED_RESPONSE, status)
}

private fun encodePathSegment(value: String): String = value.encodeToByteArray().joinToString("") { byte ->
    val unsigned = byte.toInt() and 0xff
    val character = unsigned.toChar()
    if (character.isLetterOrDigit() || character in "-._~") character.toString()
    else "%" + unsigned.toString(16).uppercase().padStart(2, '0')
}

@Serializable
public data class GitHubProfile(
    @SerialName("id") private val id: Long,
    @SerialName("login") public val username: String,
    @SerialName("avatar_url") public val avatarUrl: String? = null,
) {
    public val userId: String get() = id.toString()
}
