package com.openbitfun.mobile.core.feature.account

import com.openbitfun.mobile.core.transport.DEFAULT_CLOUD_RELAY_URL
import com.openbitfun.mobile.core.transport.normalizeAccountPhone
import com.openbitfun.mobile.core.transport.normalizeConfiguredRelayUrl

/** Stable account defaults exposed to native presentation adapters. */
public object AccountDefaults {
    public const val CLOUD_RELAY_URL: String = DEFAULT_CLOUD_RELAY_URL

    /**
     * The user-settable relay address: https anywhere, or plain http only for
     * a loopback / private-LAN development relay. Null rejects the value.
     */
    public fun normalizeRelayUrl(value: String): String? = normalizeConfiguredRelayUrl(value)

    /**
     * Normalizes a mainland-China mobile number; null when [value] cannot be
     * one. The relay re-validates and remains the authority.
     */
    public fun normalizePhone(value: String): String? = normalizeAccountPhone(value)
}
