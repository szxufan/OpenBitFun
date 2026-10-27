package com.openbitfun.mobile.core.feature.account

public enum class AccountFailureReason {
    INVALID_CREDENTIALS,
    AUTHENTICATION,
    RATE_LIMITED,
    RELAY_UNAVAILABLE,
    NETWORK,
    TIMEOUT,
    MALFORMED_RESPONSE,
    SECURE_STORAGE,
}

public enum class AccountFailureStage {
    RESTORE,
    AUTHENTICATION,
    DEVICE_LIST,
    SECURE_STORAGE,
}

public data class AccountDeviceUi public constructor(
    public val id: String,
    public val name: String,
    public val online: Boolean,
    public val lastSeenAt: Long?,
    /**
     * Relay-computed: whether this desktop and this client run matching
     * builds. `false` is confirmed incompatible and is never a control target;
     * null is an older Relay that does not gate, "unknown but usable".
     */
    public val compatible: Boolean? = null,
) {
    public constructor(id: String, name: String, online: Boolean, lastSeenAt: Long?) : this(id, name, online, lastSeenAt, null)

    /** The single gate every control entry point reuses; see [compatible]. */
    public val controllable: Boolean get() = compatible != false
}

public sealed interface AccountUiState {
    public data object Idle : AccountUiState
    public data object Restoring : AccountUiState
    public data object SignedOut : AccountUiState
    public data object SigningIn : AccountUiState
    public data class Ready public constructor(
        public val userId: String,
        public val relayUrl: String,
        /**
         * The name this session signed in under.
         *
         * What a settings page names the account by: the id behind it is a
         * machine identifier, and a screen that shows it in place of a name has
         * told the user nothing and published an identifier for nothing.
         */
        public val username: String,
        /**
         * The devices this one can drive, already filtered by
         * [AccountDevicePolicy] — a screen that renders these rows has no
         * further rule to remember.
         */
        public val devices: List<AccountDeviceUi>,
        public val selectedDeviceId: String?,
        public val selectedDeviceName: String?,
        /** A device-list reload in flight, with the previous list still shown. */
        public val refreshing: Boolean,
        /**
         * Why the last reload failed, or null. [devices] is still the last list
         * that arrived, so a failed refresh costs the user nothing they had.
         */
        public val refreshFailure: AccountFailureReason?,
        public val avatarUrl: String?,
    ) : AccountUiState {
        /** Preserve callers created before explicit Relay selection was added. */
        public constructor(
            userId: String,
            username: String,
            devices: List<AccountDeviceUi>,
            selectedDeviceId: String?,
            selectedDeviceName: String?,
        ) : this(userId, AccountDefaults.CLOUD_RELAY_URL, username, devices,
            selectedDeviceId, selectedDeviceName, false, null, null)

        public constructor(
            userId: String, relayUrl: String, username: String, devices: List<AccountDeviceUi>,
            selectedDeviceId: String?, selectedDeviceName: String?, refreshing: Boolean,
            refreshFailure: AccountFailureReason?,
        ) : this(userId, relayUrl, username, devices, selectedDeviceId, selectedDeviceName, refreshing, refreshFailure, null)

        public constructor(
            userId: String,
            username: String,
            relayUrl: String,
            devices: List<AccountDeviceUi>,
            selectedDeviceId: String?,
            selectedDeviceName: String?,
        ) : this(userId, relayUrl, username, devices, selectedDeviceId, selectedDeviceName, false, null)

    }
    public data class Failed public constructor(
        public val reason: AccountFailureReason,
        public val canRetry: Boolean,
        public val stage: AccountFailureStage,
        /**
         * The relay's own sentence for a rejected relay-local sign-in, or null
         * when [reason]'s usual copy already says everything.
         *
         * Echoed verbatim to the user, never matched on: a cooldown length, a
         * closed registration, and an unconfigured SMS provider are the relay's
         * facts to state, and this field is how they reach the form without a
         * reason code per relay sentence.
         */
        public val detail: String?,
    ) : AccountUiState {
        /** Preserve callers created before relay-local sign-in added the detail. */
        public constructor(reason: AccountFailureReason, canRetry: Boolean, stage: AccountFailureStage) :
            this(reason, canRetry, stage, null)
    }
}

/**
 * One answer to a send-code request, already free of transport types so the
 * form layer never needs a module below this seam.
 */
public sealed interface SmsSendResult {
    /** The relay accepted the request and texted the code. */
    public data object Sent : SmsSendResult

    /**
     * The relay refused with a cooldown. [message] is its own sentence, shown
     * verbatim; [retryAfterSeconds] is the cooldown the resend control must
     * honor.
     */
    public data class Cooldown(public val message: String, public val retryAfterSeconds: Int) : SmsSendResult

    /**
     * Any other refusal. [reason] selects local copy; [message] is the relay's
     * own sentence when it gave one, and takes precedence over that copy.
     */
    public data class Rejected(public val reason: AccountFailureReason, public val message: String) : SmsSendResult
}

public sealed interface AccountIntent {
    public data object Restore : AccountIntent

    /**
     * Sign in (registering on first use) against one relay: [phone] plus
     * exactly one of an SMS [code] or a [password]. A successful sign-in also
     * adopts [relayUrl] as the configured relay, so the session that lands is
     * the one a later restore expects.
     */
    public data class LocalLogin public constructor(
        public val relayUrl: String,
        public val phone: String,
        public val code: String?,
        public val password: String?,
    ) : AccountIntent {
        /** A credential travels in this intent; it never travels in a log. */
        override fun toString(): String = "LocalLogin(relayUrl=$relayUrl, phone=$phone, code=${if (code == null) "null" else "<redacted>"}, password=${if (password == null) "null" else "<redacted>"})"
    }

    public data class SelectRelay(public val relayUrl: String) : AccountIntent
    public data class SelectDevice public constructor(public val deviceId: String) : AccountIntent

    /**
     * Ask the relay for the device list again.
     *
     * The list is a presence snapshot, so it is stale the moment it arrives —
     * a desktop that starts up after sign-in is invisible until something asks
     * again, and this is that something.
     */
    public data object RefreshDevices : AccountIntent

    /** Retry the failed device-list stage with the authenticated session already in memory. */
    public data object Retry : AccountIntent
    public data object Logout : AccountIntent
    public data object Stop : AccountIntent
}
