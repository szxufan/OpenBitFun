package com.openbitfun.mobile.core.transport

import io.ktor.http.Url

public data class AccountDeviceLink(public val relayUrl: String, public val deviceId: String)

/**
 * Canonical form of any http(s) relay endpoint: no credentials, query, or
 * fragment, and no trailing slash.
 *
 * This is the shape the transport needs every relay URL handed to it to share,
 * so one relay is one string. It gates nothing by itself — the entries that
 * choose an endpoint ([accountDeviceLink] invitations, the user-settable
 * configured relay via [normalizeConfiguredRelayUrl]) apply their own host
 * rules on top of it.
 */
public fun normalizeRelayEndpointUrl(value: String): String? = runCatching {
    val url = Url(value.trim())
    if (url.protocol.name !in setOf("http", "https") || url.user != null || url.password != null ||
        url.parameters.names().isNotEmpty() || url.fragment.isNotEmpty()) return null
    url.toString().trimEnd('/')
}.getOrNull()

/** Loopback and private-LAN hosts: the only plain-http relays a link or setting may name. */
private fun isLocalRelayHost(host: String): Boolean {
    val name = host.removeSurrounding("[", "]").lowercase()
    val octets = name.split('.').mapNotNull(String::toIntOrNull)
    return name == "localhost" || name == "::1" ||
        Regex("^(f[cd][0-9a-f]{2}|fe[89ab][0-9a-f]):").containsMatchIn(name) ||
        (octets.size == 4 && octets.all { it in 0..255 } && (octets[0] == 10 || octets[0] == 127 ||
            (octets[0] == 192 && octets[1] == 168) || (octets[0] == 172 && octets[1] in 16..31) ||
            (octets[0] == 169 && octets[1] == 254)))
}

public fun normalizeAccountRelayUrl(value: String): String? {
    val normalized = normalizeRelayEndpointUrl(value) ?: return null
    if (normalized == DEFAULT_CLOUD_RELAY_URL) return normalized
    return runCatching {
        val url = Url(normalized)
        normalized.takeIf { isLocalRelayHost(url.host) && url.encodedPath in setOf("", "/") }
    }.getOrNull()
}

/**
 * The user-settable relay address: https on any host, or plain http only for a
 * loopback / private-LAN development relay (served from the root, like the
 * pairing links that already accept those hosts).
 *
 * A public http endpoint is rejected, so the setting cannot quietly move the
 * account's tokens onto an unauthenticated network path.
 */
public fun normalizeConfiguredRelayUrl(value: String): String? {
    val normalized = normalizeRelayEndpointUrl(value) ?: return null
    if (normalized == DEFAULT_CLOUD_RELAY_URL) return normalized
    return runCatching {
        val url = Url(normalized)
        if (url.protocol.name == "https") normalized
        else normalized.takeIf { isLocalRelayHost(url.host) && url.encodedPath in setOf("", "/") }
    }.getOrNull()
}

/** The link chooses an endpoint and target; authenticated membership grants access. */
public fun accountDeviceLink(value: String): AccountDeviceLink? = runCatching {
    if (value.length > 8192) return null
    val url = Url(value.trim())
    if (url.parameters.names().isNotEmpty() || !url.fragment.startsWith("/pair?")) return null
    val endpoint = normalizeAccountRelayUrl(value.trim().substringBefore('#')) ?: return null
    val query = Url("https://localhost/?" + url.fragment.removePrefix("/pair?"))
    if (query.parameters.names() != setOf("did")) return null
    val id = query.parameters.getAll("did")?.singleOrNull()?.takeIf { id ->
        id.isNotEmpty() && id.length <= 128 && id !in setOf(".", "..") &&
            id.all { (it.isLetterOrDigit() && it.code < 128) || it in "-_." }
    } ?: return null
    AccountDeviceLink(endpoint, id)
}.getOrNull()
