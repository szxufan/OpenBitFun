//! Relay-local account credentials and pairwise encrypted relay messages.

use super::device_crypto;
use anyhow::{anyhow, Result};
use serde::{Deserialize, Serialize};
use std::{collections::HashMap, sync::Arc};
use tokio::sync::{Mutex, OnceCell};

use crate::remote_connect::device::DeviceIdentity;
use crate::remote_connect::relay_http::{
    relay_http_client, send_with_retry, BufferedRelayResponse, RelayHttpRetry,
};

pub const MASTER_KEY_LEN: usize = 32;

/// Decryption failed under the PSK-paired derivation the sender declared. A
/// paired device must never silently fall back to the legacy derivation: the
/// fallback is exactly what a key-swapping relay needs. Mirrors the shared
/// TypeScript constant in `src/shared/relay-transport/HostStream.ts`.
pub const PAIRING_KEY_MISMATCH_MESSAGE: &str = "Peer pairing key mismatch: the paired PSK cannot decrypt this message. Complete pairing on both devices or remove the pairing.";

/// A sealed pairwise message. `key_id` is `Some` exactly when the PSK-paired
/// derivation sealed the payload; the envelope carries it so the receiver
/// knows which derivation to use without trying both.
#[derive(Debug, Clone)]
pub struct PeerSealedMessage {
    pub data: String,
    pub nonce: String,
    pub key_id: Option<String>,
}

/// The derivation a sender selected for one peer.
#[derive(Debug, Clone)]
struct PeerCipherKey {
    key: [u8; 32],
    key_id: Option<String>,
}

/// A retired official deployment has its own credential database. Authenticate
/// with the new deployment instead of replaying its token or deleting the record.
///
/// Every official release lives under `https://remote.openbitfun.com/v/<version>`;
/// any such endpoint other than the one this build targets is retired.
pub fn is_retired_official_relay(value: &str) -> bool {
    let current = reqwest::Url::parse(openbitfun_product_domains::account::DEFAULT_RELAY_URL)
        .expect("official relay endpoint is a valid URL");
    reqwest::Url::parse(value).is_ok_and(|url| {
        url.scheme() == "https"
            && url.host_str() == current.host_str()
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none()
            && url.query().is_none()
            && url.fragment().is_none()
            && is_retired_official_version_path(url.path(), current.path())
    })
}

fn is_retired_official_version_path(path: &str, current: &str) -> bool {
    let path = path.trim_end_matches('/');
    let current = current.trim_end_matches('/');
    path != current
        && path
            .strip_prefix("/v/")
            .is_some_and(|version| !version.is_empty() && !version.contains('/'))
}

/// A host announced that one of its streams changed. Hints are lossy wake-ups;
/// the subscriber always reads the authoritative page from the host.
#[derive(Clone, Debug, PartialEq)]
pub struct StreamHint {
    pub source_device_id: String,
    pub stream_id: String,
    pub epoch: u64,
    pub cursor: u64,
}

/// Device-scoped relay credentials and a locally owned X25519 private key.
#[derive(Clone)]
pub struct AccountSession {
    pub token: String,
    hints: tokio::sync::broadcast::Sender<Option<StreamHint>>,
    pub user_id: String,
    pub master_key: [u8; MASTER_KEY_LEN],
    pub grants: Arc<super::device_grants::HostGrantGate>,
    device_id: Arc<std::sync::RwLock<Option<String>>>,
    peer_keys: Arc<Mutex<HashMap<String, Arc<OnceCell<[u8; 32]>>>>>,
    transports: Arc<Mutex<HashMap<String, Arc<OnceCell<Arc<super::relay_client::RelayClient>>>>>>,
}

impl std::fmt::Debug for AccountSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccountSession")
            .field("user_id", &self.user_id)
            .finish_non_exhaustive()
    }
}

impl AccountSession {
    pub fn new(token: String, user_id: String, device_secret: [u8; 32]) -> Self {
        Self {
            token,
            hints: tokio::sync::broadcast::channel(64).0,
            user_id,
            master_key: device_secret,
            grants: Arc::new(super::device_grants::HostGrantGate::new()),
            device_id: Arc::new(std::sync::RwLock::new(None)),
            peer_keys: Arc::new(Mutex::new(HashMap::new())),
            transports: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn adopt_device_id(&self, device_id: &str) {
        if let Ok(mut current) = self.device_id.write() {
            *current = Some(device_id.to_owned());
        }
    }

    pub fn local_device_id(&self) -> Result<String> {
        if let Some(id) = self.device_id.read().ok().and_then(|id| id.clone()) {
            return Ok(id);
        }
        Ok(super::DeviceIdentity::from_current_machine()?.device_id)
    }

    pub async fn incoming_peer(
        &self,
        relay: &str,
        source_account: Option<&str>,
        device: &str,
    ) -> Result<super::device_grants::PeerTarget> {
        self.grants
            .incoming_peer(
                &AccountClient::new(),
                relay,
                self,
                &self.local_device_id()?,
                source_account,
                device,
            )
            .await
    }

    pub async fn authorize_peer_command(
        &self,
        relay: &str,
        peer: &super::device_grants::PeerTarget,
        key_id: Option<&str>,
        command: &super::RemoteCommand,
    ) -> Result<Option<super::device_grants::GrantScope>> {
        self.grants
            .authorize(
                &AccountClient::new(),
                relay,
                self,
                &self.local_device_id()?,
                peer,
                key_id,
                command,
            )
            .await
            .map_err(anyhow::Error::msg)
    }

    pub async fn authorize_peer_delivery(&self, relay: &str, peer: &str) -> Result<()> {
        let peer = super::device_grants::PeerTarget::parse(peer)?;
        self.grants
            .authorize_delivery(
                &AccountClient::new(),
                relay,
                self,
                &self.local_device_id()?,
                &peer,
            )
            .await
            .map_err(anyhow::Error::msg)
    }

    /// grantee 完成配对：依 grant 将主机设备解析为复合对端（owner 账号取本地
    /// roster），经授权范围 peer-key 路由验证主机公钥并把配对存入该对端。
    pub async fn complete_grant_pairing(
        &self,
        relay: &str,
        grant_id: &str,
        host_device_id: &str,
        psk: &[u8; 32],
        offerer_public_key: [u8; 32],
    ) -> Result<super::psk_store::PeerPskEntry> {
        let target = self
            .grants
            .resolve_pairing_target(&AccountClient::new(), relay, self, grant_id, host_device_id)
            .await?;
        let directory_key = self.peer_public_key(relay, &target.key()).await?;
        anyhow::ensure!(directory_key == offerer_public_key,
            "Pairing refused: the host's grant-scoped directory key does not match the pairing code.");
        super::psk_store::store_entry(
            relay,
            &self.user_id,
            &target.key(),
            psk,
            super::psk_store::PeerPskStatus::Paired,
            None,
        )
    }

    /// 共享机为某个 grant 铸配对邀请：先按 grant id 暂存待提升的 PSK，grantee
    /// 完成后的首条 PSK 封包消息再把提升为设备级配对。
    pub async fn offer_grant_pairing(
        &self,
        relay: &str,
        grant_id: &str,
    ) -> Result<(super::psk_store::PeerPskEntry, [u8; 32], String)> {
        let my_device = self.local_device_id()?;
        let (_, list) = self
            .grants
            .roster(&AccountClient::new(), relay, self)
            .await
            .map_err(anyhow::Error::msg)?;
        let grant = list
            .owned
            .iter()
            .find(|g| {
                g.grant_id == grant_id
                    && g.owner_user_id == self.user_id
                    && g.device_id == my_device
                    && g.is_current()
            })
            .ok_or_else(|| anyhow!("{}", super::device_grants::PEER_GRANT_REQUIRED))?;
        let psk = super::pairing::generate_psk();
        let entry = super::psk_store::store_grant_offer(
            relay,
            &self.user_id,
            grant_id,
            &psk,
            Some(&grant.grantee_user_id),
        )?;
        Ok((entry, psk, grant.grantee_user_id.clone()))
    }

    /// Stream change hints from hosts. `None` marks a (re)connect; receiver lag
    /// means catch up every subscribed stream.
    pub fn stream_hints(&self) -> tokio::sync::broadcast::Receiver<Option<StreamHint>> {
        self.hints.subscribe()
    }

    /// Route an already-decrypted `DeviceEvent` from `source_device_id` to the
    /// local stream subscribers. Returns true when the event was a stream hint.
    pub fn deliver_device_event(
        &self,
        source_device_id: &str,
        event: &str,
        payload: &serde_json::Value,
    ) -> bool {
        if event != super::host_stream::HOST_STREAM_CHANGED_EVENT {
            return false;
        }
        let (Some(stream_id), Some(epoch), Some(cursor)) = (
            payload["stream_id"].as_str(),
            payload["epoch"].as_u64(),
            payload["cursor"].as_u64(),
        ) else {
            return true;
        };
        let _ = self.hints.send(Some(StreamHint {
            source_device_id: source_device_id.to_owned(),
            stream_id: stream_id.to_owned(),
            epoch,
            cursor,
        }));
        true
    }

    pub async fn clear_peer_keys(&self) {
        self.peer_keys.lock().await.clear();
    }

    /// The peer's authenticated X25519 public key from the relay directory.
    /// Cached per endpoint; invalidation removes the cell so an older lookup
    /// cannot refill it.
    pub async fn peer_public_key(&self, relay_url: &str, device_id: &str) -> Result<[u8; 32]> {
        let target = super::device_grants::PeerTarget::parse(device_id)?;
        if target.is_shared() {
            self.grants
                .peer_grant(
                    &AccountClient::new(),
                    relay_url,
                    self,
                    &self.local_device_id()?,
                    &target,
                )
                .await
                .map_err(anyhow::Error::msg)?;
        }
        let endpoint = Self::peer_key_endpoint(relay_url, &target)?;
        let cache_id = endpoint.to_string();
        let cell = {
            let mut keys = self.peer_keys.lock().await;
            keys.entry(cache_id).or_default().clone()
        };
        // Only callers for this peer share initialization. No account-wide
        // lock survives network IO. Failed/cancelled lookups can be retried.
        cell.get_or_try_init(|| async {
            let response = relay_http_client()
                .get(endpoint)
                .bearer_auth(&self.token)
                .send()
                .await?;
            if !response.status().is_success() {
                return Err(AccountClient::into_error(response).await);
            }
            #[derive(Deserialize)]
            struct PeerKey {
                device_id: String,
                public_key: String,
            }
            let peer: PeerKey = response.json().await?;
            if peer.device_id != target.device_id {
                return Err(anyhow!("relay returned a different device identity"));
            }
            super::encryption::parse_public_key(&peer.public_key)
        })
        .await
        .copied()
    }

    fn peer_key_endpoint(
        relay: &str,
        peer: &super::device_grants::PeerTarget,
    ) -> Result<reqwest::Url> {
        let path = match &peer.grant_id {
            Some(grant) => format!(
                "/api/grants/{}/peer-key?device_id={}",
                urlencoding::encode(grant),
                urlencoding::encode(&peer.device_id)
            ),
            None => format!("/api/devices/{}/key", urlencoding::encode(&peer.device_id)),
        };
        AccountClient::endpoint(relay, &path)
    }

    async fn invalidate_peer_public_key(&self, relay_url: &str, device_id: &str) -> Result<()> {
        let endpoint = Self::peer_key_endpoint(
            relay_url,
            &super::device_grants::PeerTarget::parse(device_id)?,
        )?;
        self.peer_keys.lock().await.remove(endpoint.as_str());
        Ok(())
    }

    /// The key this device uses to seal unsolicited traffic to one peer: the
    /// PSK derivation once pairing is confirmed on both sides, the legacy
    /// derivation otherwise (including while an offer awaits confirmation).
    async fn sender_key(&self, relay_url: &str, device_id: &str) -> Result<PeerCipherKey> {
        let public = self.peer_public_key(relay_url, device_id).await?;
        if let Some(entry) = super::psk_store::lookup_entry(relay_url, &self.user_id, device_id)? {
            if entry.status == super::psk_store::PeerPskStatus::Paired {
                return Ok(PeerCipherKey {
                    key: device_crypto::derive_message_key_psk(
                        &self.master_key,
                        &public,
                        &entry.psk,
                    )?,
                    key_id: Some(format!(
                        "{}{}",
                        device_crypto::PSK_KEY_ID_PREFIX,
                        entry.psk_id
                    )),
                });
            }
        }
        Ok(PeerCipherKey {
            key: device_crypto::derive_message_key(&self.master_key, &public)?,
            key_id: None,
        })
    }

    /// The key for a message one peer sealed. The envelope's `key_id` is the
    /// only authority for which derivation was used: absent means legacy even
    /// when this device already holds a pairing, so a peer that removed its
    /// pairing keeps a working legacy path. A `key_id` without a matching
    /// stored pairing is a loud error, never a legacy fallback.
    async fn receiver_key(
        &self,
        relay_url: &str,
        device_id: &str,
        key_id: Option<&str>,
    ) -> Result<[u8; 32]> {
        let public = self.peer_public_key(relay_url, device_id).await?;
        let Some(id) = key_id else {
            return device_crypto::derive_message_key(&self.master_key, &public);
        };
        let Some(psk_id) = id.strip_prefix(device_crypto::PSK_KEY_ID_PREFIX) else {
            return Err(anyhow!("unsupported device message key id: {id}"));
        };
        if let Some(entry) = super::psk_store::lookup_entry(relay_url, &self.user_id, device_id)? {
            if entry.psk_id != psk_id {
                return Err(anyhow!("{PAIRING_KEY_MISMATCH_MESSAGE}"));
            }
            return device_crypto::derive_message_key_psk(&self.master_key, &public, &entry.psk);
        }
        // A shared target with no device-scoped pairing yet: the shared host
        // minted a grant-keyed offer before it learned this controller device.
        // Match the confirming PSK id, then promote to a device-scoped entry so
        // later sealing reuses the paired derivation.
        let target = super::device_grants::PeerTarget::parse(device_id)?;
        if let (Some(grant), Some(_)) = (target.grant_id.as_ref(), target.account_id.as_ref()) {
            if let Some(offer) =
                super::psk_store::lookup_grant_offer(relay_url, &self.user_id, grant)?
            {
                if offer.psk_id == psk_id {
                    let _ = super::psk_store::promote_grant_offer(
                        relay_url,
                        &self.user_id,
                        grant,
                        device_id,
                        psk_id,
                    );
                    return device_crypto::derive_message_key_psk(
                        &self.master_key,
                        &public,
                        &offer.psk,
                    );
                }
            }
        }
        Err(anyhow!("{PAIRING_KEY_MISMATCH_MESSAGE}"))
    }

    pub async fn encrypt_for_peer(
        &self,
        relay_url: &str,
        device_id: &str,
        plaintext: &str,
    ) -> Result<PeerSealedMessage> {
        let selected = self.sender_key(relay_url, device_id).await?;
        if super::device_grants::PeerTarget::parse(device_id)?.is_shared()
            && selected.key_id.is_none()
        {
            let command = serde_json::from_str::<super::RemoteCommand>(plaintext)?;
            anyhow::ensure!(
                matches!(command, super::RemoteCommand::HostInvoke { command, .. } if command == "peer_mode_ping"),
                "{}",
                super::device_grants::PEER_GRANT_PAIRING_REQUIRED
            );
        }
        let (data, nonce) = super::encryption::encrypt_to_base64(&selected.key, plaintext)?;
        Ok(PeerSealedMessage {
            data,
            nonce,
            key_id: selected.key_id,
        })
    }

    /// The symmetric key for one peer under a specific envelope variant.
    /// Public for host paths that seal structured payloads outside
    /// [`AccountSession::encrypt_for_peer`].
    pub async fn peer_envelope_key(
        &self,
        relay_url: &str,
        device_id: &str,
        key_id: Option<&str>,
    ) -> Result<[u8; 32]> {
        self.receiver_key(relay_url, device_id, key_id).await
    }

    /// Seal a reply with the same derivation that decrypted the request. Host
    /// responses mirror the request's `key_id` (absent stays legacy) so the
    /// pairing transition and one-sided removal never break a live exchange.
    pub async fn encrypt_for_peer_variant(
        &self,
        relay_url: &str,
        device_id: &str,
        mirror_key_id: Option<&str>,
        plaintext: &str,
    ) -> Result<PeerSealedMessage> {
        let (key, key_id) = match mirror_key_id {
            Some(id) => (
                self.receiver_key(relay_url, device_id, Some(id)).await?,
                Some(id.to_string()),
            ),
            None => {
                let public = self.peer_public_key(relay_url, device_id).await?;
                (
                    device_crypto::derive_message_key(&self.master_key, &public)?,
                    None,
                )
            }
        };
        let (data, nonce) = super::encryption::encrypt_to_base64(&key, plaintext)?;
        Ok(PeerSealedMessage {
            data,
            nonce,
            key_id,
        })
    }

    pub async fn decrypt_from_peer(
        &self,
        relay_url: &str,
        device_id: &str,
        data: &str,
        nonce: &str,
        key_id: Option<&str>,
    ) -> Result<String> {
        if key_id.is_some() {
            // A paired exchange that fails to decrypt reports the pairing
            // mismatch loudly; the legacy derivation is never a fallback.
            let key = self.receiver_key(relay_url, device_id, key_id).await?;
            if let Ok(plaintext) = super::encryption::decrypt_from_base64(&key, data, nonce) {
                // A successful PSK decrypt from the peer is itself proof both
                // sides hold the secret: promote a pending offer, whatever
                // command travels inside the envelope.
                if let Some(id) =
                    key_id.and_then(|id| id.strip_prefix(device_crypto::PSK_KEY_ID_PREFIX))
                {
                    let _ = super::psk_store::mark_paired(relay_url, &self.user_id, device_id, id);
                }
                return Ok(plaintext);
            }
            self.invalidate_peer_public_key(relay_url, device_id)
                .await?;
            let key = self.receiver_key(relay_url, device_id, key_id).await?;
            return super::encryption::decrypt_from_base64(&key, data, nonce)
                .map_err(|_| anyhow!("{PAIRING_KEY_MISMATCH_MESSAGE}"));
        }
        let key = self.receiver_key(relay_url, device_id, None).await?;
        match super::encryption::decrypt_from_base64(&key, data, nonce) {
            Ok(plaintext) => Ok(plaintext),
            Err(_) => {
                // A device can rotate its private key while a controller is
                // disconnected. Refresh its authenticated key once on failure.
                self.invalidate_peer_public_key(relay_url, device_id)
                    .await?;
                let key = self.receiver_key(relay_url, device_id, None).await?;
                super::encryption::decrypt_from_base64(&key, data, nonce)
            }
        }
    }
}

/// A delegated token for a paired client (mobile-web / IM bot).
/// The desktop requests this from the relay and transmits it along
/// with a fresh controller private key over the authenticated E2E channel.
#[derive(Clone)]
pub struct DelegateToken {
    pub token: String,
    pub user_id: String,
    pub device_secret: [u8; 32],
}

/// Full device credential minted for a distinct SSH host. Unlike
/// [`DelegateToken`], this token may authenticate a device WebSocket and is
/// therefore only issued by the narrow authenticated provisioning endpoint.
#[derive(Clone, Serialize, Deserialize)]
pub struct ProvisionedDeviceToken {
    pub token: String,
    pub user_id: String,
    pub device_id: String,
}

// ── Relay HTTP client ───────────────────────────────────────────────────

#[derive(Deserialize)]
struct AuthResponse {
    token: String,
    user_id: String,
}

#[derive(Deserialize)]
struct ErrorBody {
    error: String,
    #[serde(default)]
    retry_after_secs: Option<i64>,
}

/// The relay-local account profile returned with a sign-in token pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalAccountUser {
    pub user_id: String,
    pub login: String,
    pub phone_masked: String,
    pub has_password: bool,
}

/// A relay-local sign-in token pair (camelCase wire, matching the relay).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalTokenPair {
    pub access_token: String,
    pub access_expires_at: i64,
    pub refresh_token: String,
    pub refresh_expires_at: i64,
    pub user: LocalAccountUser,
}

/// Environment override for the relay base URL. Self-hosted deployments point
/// desktop and CLI hosts at their own relay instead of the official endpoint.
pub const ENV_RELAY_URL_OVERRIDE: &str = "OPENBITFUN_RELAY_URL";

/// Resolve the relay base URL, applying the [`ENV_RELAY_URL_OVERRIDE`]. The
/// override must use https, or plain http only on a loopback or private
/// address, so account tokens cannot silently ship over the open internet.
pub fn configured_relay_url() -> Result<String> {
    let configured = std::env::var(ENV_RELAY_URL_OVERRIDE)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let Some(configured) = configured else {
        return Ok(openbitfun_product_domains::account::DEFAULT_RELAY_URL.to_string());
    };
    let url = validate_relay_base_url(&configured)?;
    let private_http = url.scheme() == "http"
        && url
            .host_str()
            .and_then(|host| host.parse::<std::net::Ipv4Addr>().ok())
            .is_some_and(|ip| ip.is_private() || ip.is_loopback() || ip.is_link_local());
    if url.scheme() != "https" && !private_http {
        return Err(anyhow!(
            "{ENV_RELAY_URL_OVERRIDE} must use https (plain http is only allowed on loopback or private addresses)"
        ));
    }
    Ok(url.to_string())
}

/// HTTP client for the relay's account endpoints.
pub struct AccountClient {
    http: reqwest::Client,
}

/// Older Desktop builds persisted the temporary embedded LAN credential into
/// the shared account slot. Re-establish the official route from shared identity
/// instead of dialing a stale DHCP address on startup. Other custom endpoints
/// retain their legacy behavior.
pub fn persisted_relay_requires_migration(relay_url: &str) -> bool {
    if is_retired_official_relay(relay_url) {
        return true;
    }
    let Ok(url) = validate_relay_base_url(relay_url) else {
        return false;
    };
    url.scheme() == "http"
        && url.port() == Some(9700)
        && url.path() == "/"
        && url
            .host_str()
            .and_then(|host| host.parse::<std::net::Ipv4Addr>().ok())
            .is_some_and(|ip| ip.is_private() || ip.is_loopback() || ip.is_link_local())
}

/// Check whether an account/relay error message indicates an invalid or
/// expired account token (relay auth failure). Shared by Desktop, CLI, and
/// account surfaces so they all react to the same relay wording.
pub fn error_indicates_expired_token(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("http 401")
        || lower.contains("unauthorized")
        || lower.contains("invalid or expired token")
        || lower.contains("relay auth error")
}

/// Parse a user/config supplied relay base URL at the shared transport
/// boundary. Paths are allowed for reverse-proxy prefixes; credentials,
/// query strings, fragments, and non-HTTP schemes are not.
pub fn validate_relay_base_url(relay_url: &str) -> Result<reqwest::Url> {
    let url = reqwest::Url::parse(relay_url.trim())
        .map_err(|error| anyhow!("invalid relay URL: {error}"))?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(anyhow!(
            "relay URL must be an http(s) server address without credentials, query, or fragment"
        ));
    }
    Ok(url)
}

impl Default for AccountClient {
    fn default() -> Self {
        Self::new()
    }
}

/// Device kinds a host may report. Mirrors
/// `relay-service/src/db.rs::DEVICE_KINDS`, which is what validates them.
pub const DEVICE_KIND_DESKTOP: &str = "desktop";
/// A headless host: the CLI and TUI delivery profiles.
pub const DEVICE_KIND_CLI: &str = "cli";

/// Relay capability advertising that a host may report [`DEVICE_KIND_CLI`].
pub const RELAY_CAPABILITY_DEVICE_KIND_CLI: &str = "device_kind_cli_v1";

/// Relay capability advertising relay-local accounts: SMS-code sign-in and
/// registration plus the optional-password and refresh endpoints under
/// `/api/auth/local/*`.
pub const RELAY_CAPABILITY_LOCAL_AUTH: &str = "local_auth_v1";

/// Relay capability advertising cross-account device grants under
/// `/api/grants*`. Clients must probe before using the routes; older relays
/// answer 404 and the feature must surface an unsupported state, not a
/// silent failure.
pub const RELAY_CAPABILITY_DEVICE_GRANTS: &str = "device_grants_v1";

/// Relay capability advertising manual, client-encrypted config backup
/// storage under `/api/account/config-backup*`.
pub const RELAY_CAPABILITY_CONFIG_BACKUP: &str = "account_config_backup_v1";

/// The GitHub/relay login request body.
///
/// `clientVersion`/`clientProtocol` are optional from the Relay's point of view,
/// but a current build always reports both so the Relay can gate control
/// compatibility instead of treating this device as legacy.
///
/// `device_kind` is validated by the Relay against its own kind list, so callers
/// must pass a value that Relay accepts — see
/// [`AccountClient::reported_device_kind`].
fn login_request_body(
    access_token: &str,
    device: &DeviceIdentity,
    device_kind: &str,
    public_key: String,
    request_id: String,
) -> serde_json::Value {
    serde_json::json!({
        "access_token": access_token,
        "device_id": device.device_id,
        "device_name": device.device_name,
        "device_kind": device_kind,
        "public_key": public_key,
        "request_id": request_id,
        "clientVersion": openbitfun_product_domains::account::client_version(),
        "clientProtocol": openbitfun_product_domains::account::CLIENT_PROTOCOL_VERSION,
    })
}

impl AccountClient {
    /// The device kind to report to this Relay for a host that is not a desktop.
    ///
    /// A CLI host may only report [`DEVICE_KIND_CLI`] to a Relay that advertises
    /// [`RELAY_CAPABILITY_DEVICE_KIND_CLI`]: an older Relay validates the kind
    /// against a shorter list and rejects the whole login, which would strand the
    /// user on a host that used to work. Such a Relay gets [`DEVICE_KIND_DESKTOP`]
    /// instead, which every Relay accepts, so the device still registers — only
    /// its artwork falls back. An unreadable capability list counts as
    /// unsupported, never as a guess.
    pub async fn reported_device_kind(&self, relay_url: &str, host_is_cli: bool) -> &'static str {
        if !host_is_cli {
            return DEVICE_KIND_DESKTOP;
        }
        match self.relay_capabilities(relay_url).await {
            Ok(capabilities) => {
                if capabilities
                    .iter()
                    .any(|name| name == RELAY_CAPABILITY_DEVICE_KIND_CLI)
                {
                    DEVICE_KIND_CLI
                } else {
                    DEVICE_KIND_DESKTOP
                }
            }
            Err(error) => {
                log::warn!(
                    "Failed to read relay capabilities ({error}); reporting a desktop device"
                );
                DEVICE_KIND_DESKTOP
            }
        }
    }

    /// Sign in (registering on first use) with an SMS verification code, or
    /// with the account's optional password, then exchange the returned
    /// access token for a keyed device session. `code` and `password` are
    /// mutually exclusive; pass exactly one.
    pub async fn login_with_local_credentials(
        &self,
        relay_url: &str,
        phone: &str,
        code: Option<&str>,
        password: Option<&str>,
        device: &DeviceIdentity,
        device_kind: &str,
    ) -> Result<(AccountSession, LocalTokenPair)> {
        let pair = self.local_sign_in(relay_url, phone, code, password).await?;
        self.exchange_local_access_token(relay_url, &pair, device, device_kind)
            .await
    }

    /// Sign in (registering on first use) with an SMS verification code or
    /// the account's optional password. Exactly one of `code`/`password`.
    pub async fn local_sign_in(
        &self,
        relay_url: &str,
        phone: &str,
        code: Option<&str>,
        password: Option<&str>,
    ) -> Result<LocalTokenPair> {
        self.require_local_auth(relay_url).await?;
        let body = match (code, password) {
            (Some(code), None) => serde_json::json!({ "phone": phone, "code": code }),
            (None, Some(password)) => serde_json::json!({ "phone": phone, "password": password }),
            _ => return Err(anyhow!("provide exactly one of code or password")),
        };
        // Not idempotent: every call mints a token pair, so never blind-retry.
        let response = send_with_retry(
            "local relay sign-in",
            self.http
                .post(Self::endpoint(relay_url, "/api/auth/local/login")?)
                .json(&body),
            RelayHttpRetry::SingleAttempt,
        )
        .await?;
        if !response.status().is_success() {
            return Err(Self::into_buffered_error(response));
        }
        Ok(response.json().await?)
    }

    /// Exchange a local access token for a keyed device session. Works for a
    /// fresh sign-in pair and for a refreshed pair alike.
    pub async fn exchange_local_access_token(
        &self,
        relay_url: &str,
        pair: &LocalTokenPair,
        device: &DeviceIdentity,
        device_kind: &str,
    ) -> Result<(AccountSession, LocalTokenPair)> {
        let account_id = pair.user.user_id.clone();
        let device_secret =
            super::session_store::device_secret(relay_url, &account_id, &device.device_id)?;
        let exchange_body = login_request_body(
            &pair.access_token,
            device,
            device_kind,
            device_crypto::public_key_base64(&device_secret),
            uuid::Uuid::new_v4().to_string(),
        );
        let response = send_with_retry(
            "local relay login",
            self.http
                .post(Self::endpoint(relay_url, "/api/auth/login")?)
                .json(&exchange_body),
            RelayHttpRetry::IdempotentWrite,
        )
        .await?;
        if !response.status().is_success() {
            return Err(Self::into_buffered_error(response));
        }
        let auth: AuthResponse = response.json().await?;
        if auth.user_id != account_id {
            return Err(anyhow!("relay returned a different account identity"));
        }
        let session = AccountSession::new(auth.token, auth.user_id, device_secret);
        if let Err(error) = self
            .report_local_metadata(relay_url, &session, &device.device_id)
            .await
        {
            log::warn!("Failed to report device metadata after login: {error}");
        }
        Ok((session, pair.clone()))
    }

    /// Request a verification code for `phone`. Rate-limit and cooldown
    /// rejections surface the relay's message verbatim.
    pub async fn local_sms_send(&self, relay_url: &str, phone: &str) -> Result<()> {
        self.require_local_auth(relay_url).await?;
        let response = send_with_retry(
            "local relay code request",
            self.http
                .post(Self::endpoint(relay_url, "/api/auth/local/sms/send")?)
                .json(&serde_json::json!({ "phone": phone })),
            RelayHttpRetry::SingleAttempt,
        )
        .await?;
        if !response.status().is_success() {
            return Err(Self::into_buffered_error(response));
        }
        Ok(())
    }

    /// Rotate a refresh token into a fresh access/refresh pair. Callers
    /// persist the returned pair before re-exchanging the device token.
    pub async fn refresh_local_pair(
        &self,
        relay_url: &str,
        refresh_token: &str,
    ) -> Result<LocalTokenPair> {
        self.require_local_auth(relay_url).await?;
        let response = send_with_retry(
            "local relay refresh",
            self.http
                .post(Self::endpoint(relay_url, "/api/auth/local/refresh")?)
                .json(&serde_json::json!({ "refreshToken": refresh_token })),
            RelayHttpRetry::SingleAttempt,
        )
        .await?;
        if !response.status().is_success() {
            return Err(Self::into_buffered_error(response));
        }
        Ok(response.json().await?)
    }

    /// Set or change the signed-in account's optional password using a fresh
    /// access token. Other sessions' refresh tokens are revoked by the relay.
    pub async fn local_set_password(
        &self,
        relay_url: &str,
        access_token: &str,
        new_password: &str,
    ) -> Result<()> {
        self.require_local_auth(relay_url).await?;
        let response = send_with_retry(
            "local relay password update",
            self.http
                .post(Self::endpoint(relay_url, "/api/auth/local/password")?)
                .bearer_auth(access_token)
                .json(&serde_json::json!({ "newPassword": new_password })),
            RelayHttpRetry::SingleAttempt,
        )
        .await?;
        if !response.status().is_success() {
            return Err(Self::into_buffered_error(response));
        }
        Ok(())
    }

    async fn require_local_auth(&self, relay_url: &str) -> Result<()> {
        let capabilities = self.relay_capabilities(relay_url).await?;
        if capabilities
            .iter()
            .any(|name| name == RELAY_CAPABILITY_LOCAL_AUTH)
        {
            return Ok(());
        }
        Err(anyhow!(
            "this relay does not support local account sign-in (missing {RELAY_CAPABILITY_LOCAL_AUTH})"
        ))
    }

    pub fn new() -> Self {
        Self {
            http: relay_http_client(),
        }
    }

    fn endpoint(relay_url: &str, path: &str) -> Result<reqwest::Url> {
        let mut url = validate_relay_base_url(relay_url)?;
        let base_path = url.path().trim_end_matches('/').to_string();
        url.set_path(&format!("{base_path}{path}"));
        Ok(url)
    }

    /// Map a non-2xx relay response into a human-readable error.
    async fn into_error(resp: reqwest::Response) -> anyhow::Error {
        let status = resp.status();
        let body = resp.bytes().await.unwrap_or_default();
        Self::error_from_response_parts(status, &body)
    }

    fn into_buffered_error(resp: BufferedRelayResponse) -> anyhow::Error {
        let (status, body) = resp.into_parts();
        Self::error_from_response_parts(status, &body)
    }

    fn error_from_response_parts(status: reqwest::StatusCode, body: &[u8]) -> anyhow::Error {
        if status == reqwest::StatusCode::PAYLOAD_TOO_LARGE {
            return anyhow!(
                "relay returned HTTP 413 Payload Too Large \
                 (encrypted blob exceeds the relay body limit; raise the route's \
                  Axum DefaultBodyLimit and any reverse-proxy client_max_body_size)"
            );
        }
        if status == reqwest::StatusCode::INSUFFICIENT_STORAGE {
            return anyhow!(
                "relay returned HTTP 507 Insufficient Storage (the configured account or asset quota is full)"
            );
        }
        match serde_json::from_slice::<ErrorBody>(body) {
            Ok(body) => {
                let msg = body.error;
                if let Some(retry) = body.retry_after_secs {
                    anyhow!("{msg} (HTTP {status}, retry in {retry}s)")
                } else {
                    anyhow!("{msg} (HTTP {status})")
                }
            }
            Err(_) => anyhow!("relay returned HTTP {status}"),
        }
    }

    /// Fetch the login challenge and unwrap the master key locally.
    /// Does not call `/api/auth/login` and does not mint a token.
    fn auth_header(session: &AccountSession) -> String {
        format!("Bearer {}", session.token)
    }

    /// Issue an account token for a paired controller.
    pub async fn delegate_token(
        &self,
        relay_url: &str,
        session: &AccountSession,
    ) -> Result<DelegateToken> {
        let device_secret = super::device_crypto::generate_secret();
        let resp = send_with_retry(
            "delegate account token",
            self.http
                .post(Self::endpoint(relay_url, "/api/auth/delegate")?)
                .header("Authorization", Self::auth_header(session))
                .json(&serde_json::json!({ "public_key": super::device_crypto::public_key_base64(&device_secret) })),
            RelayHttpRetry::SingleAttempt,
        )
        .await?;
        if !resp.status().is_success() {
            return Err(Self::into_buffered_error(resp));
        }
        let auth: AuthResponse = resp.json().await?;
        Ok(DelegateToken {
            token: auth.token,
            user_id: auth.user_id,
            device_secret,
        })
    }

    /// Register a new account device and mint its full routing token.
    ///
    /// `device_kind` is what keeps the minted row out of the wrong lists: this
    /// route serves both an SSH host being bootstrapped (`"desktop"`) and a
    /// keyboard-less peer that cannot type a password (`"watch"`), and only the
    /// caller knows which one it is holding.
    /// `request_id` makes an ambiguous HTTP response safe to replay.
    pub async fn provision_device_token(
        &self,
        relay_url: &str,
        session: &AccountSession,
        device_id: &str,
        device_name: &str,
        device_kind: &str,
        request_id: uuid::Uuid,
        device_secret: &[u8; 32],
    ) -> Result<ProvisionedDeviceToken> {
        let body = serde_json::json!({
            "device_id": device_id,
            "device_name": device_name,
            "device_kind": device_kind,
            "public_key": device_crypto::public_key_base64(device_secret),
            "request_id": request_id.to_string(),
        });
        let resp = send_with_retry(
            "provision account device",
            self.http
                .post(Self::endpoint(relay_url, "/api/auth/provision-device")?)
                .header("Authorization", Self::auth_header(session))
                .json(&body),
            RelayHttpRetry::IdempotentWrite,
        )
        .await?;
        if !resp.status().is_success() {
            return Err(Self::into_buffered_error(resp));
        }
        let provisioned: ProvisionedDeviceToken = resp.json().await?;
        if provisioned.user_id != session.user_id || provisioned.device_id != device_id {
            return Err(anyhow!(
                "relay returned a mismatched provisioned device identity"
            ));
        }
        Ok(provisioned)
    }

    /// Revoke the account token on the relay (server-side logout).
    pub async fn revoke_token(&self, relay_url: &str, session: &AccountSession) -> Result<()> {
        let resp = send_with_retry(
            "revoke account token",
            self.http
                .post(Self::endpoint(relay_url, "/api/auth/logout")?)
                .header("Authorization", Self::auth_header(session)),
            RelayHttpRetry::IdempotentWrite,
        )
        .await?;
        if !resp.status().is_success() {
            // Non-fatal — best-effort revocation
            log::warn!("revoke_token: relay returned {}", resp.status());
        }
        Ok(())
    }

    /// List all devices in the account (online + offline). Returns
    /// `(device_id, device_name, online, last_seen_at)`.
    pub async fn list_devices(
        &self,
        relay_url: &str,
        session: &AccountSession,
    ) -> Result<Vec<DeviceInfo>> {
        let resp = send_with_retry(
            "list devices",
            self.http
                .get(Self::endpoint(relay_url, "/api/devices")?)
                .header("Authorization", Self::auth_header(session)),
            RelayHttpRetry::SafeRead,
        )
        .await?;
        if !resp.status().is_success() {
            return Err(Self::into_buffered_error(resp));
        }
        let entries: Vec<DeviceListEntry> = resp.json().await?;
        Ok(entries.into_iter().map(project_device_list_entry).collect())
    }

    /// Missing capabilities (including an old info endpoint) mean unsupported.
    pub async fn relay_capabilities(&self, relay_url: &str) -> Result<Vec<String>> {
        let response = self
            .http
            .get(Self::endpoint(relay_url, "/api/info")?)
            .send()
            .await?;
        if matches!(response.status().as_u16(), 404 | 405) {
            return Ok(Vec::new());
        }
        if !response.status().is_success() {
            return Err(Self::into_error(response).await);
        }
        #[derive(Deserialize)]
        struct Info {
            #[serde(default)]
            capabilities: Vec<String>,
        }
        Ok(response.json::<Info>().await?.capabilities)
    }

    pub async fn update_device_alias(
        &self,
        relay_url: &str,
        session: &AccountSession,
        device_id: &str,
        device_alias: Option<&str>,
    ) -> Result<()> {
        if !self
            .relay_capabilities(relay_url)
            .await?
            .iter()
            .any(|c| c == "device_alias_v1")
        {
            return Err(anyhow!("Relay does not support device_alias_v1"));
        }
        self.patch_device(
            relay_url,
            session,
            device_id,
            &serde_json::json!({"device_alias": device_alias}),
        )
        .await
    }

    async fn patch_device(
        &self,
        relay_url: &str,
        session: &AccountSession,
        device_id: &str,
        body: &serde_json::Value,
    ) -> Result<()> {
        let response = self
            .http
            .patch(Self::endpoint(
                relay_url,
                &format!("/api/devices/{}", urlencoding::encode(device_id)),
            )?)
            .bearer_auth(&session.token)
            .json(body)
            .send()
            .await?;
        if matches!(response.status().as_u16(), 404 | 405) {
            return Err(anyhow!("Device update unavailable: Relay does not support PATCH or device no longer exists (HTTP {})", response.status()));
        }
        if !response.status().is_success() {
            return Err(Self::into_error(response).await);
        }
        Ok(())
    }

    /// Report only metadata collected on this authenticated host. Provisioning
    /// another device must never call this on its controller.
    pub async fn report_local_metadata(
        &self,
        relay_url: &str,
        session: &AccountSession,
        device_id: &str,
    ) -> Result<()> {
        if !self
            .relay_capabilities(relay_url)
            .await?
            .iter()
            .any(|c| c == "device_metadata_v1")
        {
            return Ok(());
        }
        let metadata = super::device::local_device_metadata().await;
        self.patch_device(relay_url, session, device_id, &metadata)
            .await
    }

    /// Whether the relay advertises manual config backup storage. Missing
    /// capabilities (including an old info endpoint) mean unsupported.
    pub async fn config_backup_supported(&self, relay_url: &str) -> Result<bool> {
        Ok(self
            .relay_capabilities(relay_url)
            .await?
            .iter()
            .any(|name| name == RELAY_CAPABILITY_CONFIG_BACKUP))
    }

    async fn require_config_backup_support(&self, relay_url: &str) -> Result<()> {
        if !self.config_backup_supported(relay_url).await? {
            return Err(anyhow!(
                "this relay does not support config backups (missing {RELAY_CAPABILITY_CONFIG_BACKUP})"
            ));
        }
        Ok(())
    }

    /// Replace the account's single stored config backup (last export wins).
    pub async fn upload_config_backup(
        &self,
        relay_url: &str,
        session: &AccountSession,
        upload: &super::config_backup::ConfigBackupUpload,
    ) -> Result<super::config_backup::ConfigBackupMeta> {
        self.require_config_backup_support(relay_url).await?;
        let resp = send_with_retry(
            "upload config backup",
            self.http
                .put(Self::endpoint(
                    relay_url,
                    "/api/account/config-backup/blob",
                )?)
                .header("Authorization", Self::auth_header(session))
                .json(upload),
            RelayHttpRetry::IdempotentWrite,
        )
        .await?;
        if !resp.status().is_success() {
            return Err(Self::into_buffered_error(resp));
        }
        resp.json().await
    }

    /// Stored backup metadata without the ciphertext, for import previews.
    pub async fn get_config_backup_meta(
        &self,
        relay_url: &str,
        session: &AccountSession,
    ) -> Result<Option<super::config_backup::ConfigBackupMeta>> {
        self.require_config_backup_support(relay_url).await?;
        let resp = send_with_retry(
            "get config backup meta",
            self.http
                .get(Self::endpoint(relay_url, "/api/account/config-backup")?)
                .header("Authorization", Self::auth_header(session)),
            RelayHttpRetry::SafeRead,
        )
        .await?;
        if !resp.status().is_success() {
            return Err(Self::into_buffered_error(resp));
        }
        resp.json().await
    }

    /// Download the full encrypted envelope; `None` when no backup exists.
    pub async fn download_config_backup(
        &self,
        relay_url: &str,
        session: &AccountSession,
    ) -> Result<Option<super::config_backup::ConfigBackupEnvelope>> {
        self.require_config_backup_support(relay_url).await?;
        let resp = send_with_retry(
            "download config backup",
            self.http
                .get(Self::endpoint(
                    relay_url,
                    "/api/account/config-backup/blob",
                )?)
                .header("Authorization", Self::auth_header(session)),
            RelayHttpRetry::SafeRead,
        )
        .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            return Err(Self::into_buffered_error(resp));
        }
        resp.json().await.map(Some)
    }

    /// Remove the stored backup. Idempotent: an already-empty slot is success.
    pub async fn delete_config_backup(
        &self,
        relay_url: &str,
        session: &AccountSession,
    ) -> Result<()> {
        self.require_config_backup_support(relay_url).await?;
        let resp = send_with_retry(
            "delete config backup",
            self.http
                .delete(Self::endpoint(relay_url, "/api/account/config-backup")?)
                .header("Authorization", Self::auth_header(session)),
            RelayHttpRetry::IdempotentWrite,
        )
        .await?;
        if resp.status().is_success() || resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(());
        }
        Err(Self::into_buffered_error(resp))
    }

    async fn require_device_grants_support(&self, relay_url: &str) -> Result<()> {
        let capabilities = self.relay_capabilities(relay_url).await?;
        if !capabilities
            .iter()
            .any(|capability| capability == RELAY_CAPABILITY_DEVICE_GRANTS)
        {
            return Err(anyhow!(
                "this relay does not support device grants (missing {RELAY_CAPABILITY_DEVICE_GRANTS})"
            ));
        }
        Ok(())
    }

    /// Both grant views for the caller's account in one request
    /// (GET /api/grants): grants the account owns (management) and current
    /// grants where it is the grantee (shared-with-me roster).
    pub async fn list_device_grants(
        &self,
        relay_url: &str,
        session: &AccountSession,
    ) -> Result<super::device_grants::DeviceGrantList> {
        self.require_device_grants_support(relay_url).await?;
        let resp = send_with_retry(
            "list device grants",
            self.http
                .get(Self::endpoint(relay_url, "/api/grants")?)
                .header("Authorization", Self::auth_header(session)),
            RelayHttpRetry::SafeRead,
        )
        .await?;
        if !resp.status().is_success() {
            return Err(Self::into_buffered_error(resp));
        }
        resp.json().await
    }

    /// Offer a pending grant from this host device. Only the shared machine's
    /// own device token is accepted by the relay, so callers must pass the
    /// session of the device being shared.
    #[allow(clippy::too_many_arguments)]
    pub async fn offer_device_grant(
        &self,
        relay_url: &str,
        session: &AccountSession,
        device_id: &str,
        grantee_user_id: &str,
        scope: &str,
        expires_at: Option<i64>,
    ) -> Result<super::device_grants::DeviceGrantOffer> {
        self.require_device_grants_support(relay_url).await?;
        let resp = send_with_retry(
            "offer device grant",
            self.http
                .post(Self::endpoint(relay_url, "/api/grants")?)
                .header("Authorization", Self::auth_header(session))
                .json(&serde_json::json!({
                    "device_id": device_id,
                    "grantee_user_id": grantee_user_id,
                    "scope": scope,
                    "expires_at": expires_at,
                })),
            RelayHttpRetry::SingleAttempt,
        )
        .await?;
        if !resp.status().is_success() {
            return Err(Self::into_buffered_error(resp));
        }
        resp.json().await
    }

    /// Redeem a one-time grant invite as the named grantee account.
    pub async fn accept_device_grant(
        &self,
        relay_url: &str,
        session: &AccountSession,
        grant_id: &str,
        invite_secret: &str,
    ) -> Result<super::device_grants::DeviceGrantInfo> {
        self.require_device_grants_support(relay_url).await?;
        let resp = send_with_retry(
            "accept device grant",
            self.http
                .post(Self::endpoint(
                    relay_url,
                    &format!("/api/grants/{}/accept", urlencoding::encode(grant_id)),
                )?)
                .header("Authorization", Self::auth_header(session))
                .json(&serde_json::json!({ "invite_secret": invite_secret })),
            RelayHttpRetry::SingleAttempt,
        )
        .await?;
        if !resp.status().is_success() {
            return Err(Self::into_buffered_error(resp));
        }
        resp.json().await
    }

    /// Revoke a grant. Any device token of the owning account may call this.
    pub async fn revoke_device_grant(
        &self,
        relay_url: &str,
        session: &AccountSession,
        grant_id: &str,
    ) -> Result<super::device_grants::DeviceGrantInfo> {
        self.require_device_grants_support(relay_url).await?;
        let resp = send_with_retry(
            "revoke device grant",
            self.http
                .delete(Self::endpoint(
                    relay_url,
                    &format!("/api/grants/{}", urlencoding::encode(grant_id)),
                )?)
                .header("Authorization", Self::auth_header(session)),
            RelayHttpRetry::IdempotentWrite,
        )
        .await?;
        if !resp.status().is_success() {
            return Err(Self::into_buffered_error(resp));
        }
        resp.json().await
    }

    /// Remove a device from the account (DELETE /api/devices/:id).
    pub async fn delete_device(
        &self,
        relay_url: &str,
        session: &AccountSession,
        target_device_id: &str,
    ) -> Result<()> {
        let resp = self
            .http
            .delete(Self::endpoint(
                relay_url,
                &format!("/api/devices/{}", urlencoding::encode(target_device_id)),
            )?)
            .header("Authorization", Self::auth_header(session))
            .send()
            .await?;
        if !resp.status().is_success() {
            return Err(Self::into_error(resp).await);
        }
        Ok(())
    }

    /// Send an encrypted RemoteCommand over the shared account connection.
    /// The relay routes opaque ciphertext and returns the encrypted acknowledgement.
    /// Pairwise device keys protect both directions; the shared account
    /// Socket.IO connection owns acknowledgement and reconnect lifetimes.
    pub async fn device_rpc(
        &self,
        relay_url: &str,
        session: &AccountSession,
        target_device_id: &str,
        plaintext_command: &str,
    ) -> Result<String> {
        // Encrypt for the authenticated target device
        let sealed = session
            .encrypt_for_peer(relay_url, target_device_id, plaintext_command)
            .await?;
        let cell = {
            let mut transports = session.transports.lock().await;
            transports.entry(relay_url.to_string()).or_default().clone()
        };
        let transport = cell
            .get_or_try_init(|| async {
                let (transport, mut events) = super::relay_client::RelayClient::new_controller();
                transport.connect(relay_url).await?;
                transport
                    .connect_authenticated(&session.token, "Controller")
                    .await?;
                let hints = session.hints.clone();
                let peer_keys = session.peer_keys.clone();
                let event_session = session.clone();
                let event_relay = relay_url.to_string();
                tokio::spawn(async move {
                    while let Some(event) = events.recv().await {
                        use super::relay_client::RelayEvent;
                        match event {
                            RelayEvent::DeviceMessageReceived {
                                source_device_id,
                                source_account_id,
                                correlation_id,
                                encrypted_data,
                                nonce,
                                key_id,
                            } if correlation_id.is_empty() => {
                                // Stream hints arrive as encrypted DeviceEvents on
                                // the controller transport; RPC requests to this
                                // device are answered by its routing owner instead.
                                let Ok(peer) = event_session
                                    .incoming_peer(
                                        &event_relay,
                                        source_account_id.as_deref(),
                                        &source_device_id,
                                    )
                                    .await
                                else {
                                    continue;
                                };
                                let source_device_id = peer.key();
                                let Ok(plaintext) = event_session
                                    .decrypt_from_peer(
                                        &event_relay,
                                        &source_device_id,
                                        &encrypted_data,
                                        &nonce,
                                        key_id.as_deref(),
                                    )
                                    .await
                                else {
                                    continue;
                                };
                                let Ok(command) =
                                    serde_json::from_str::<super::RemoteCommand>(&plaintext)
                                else {
                                    continue;
                                };
                                if event_session
                                    .authorize_peer_command(
                                        &event_relay,
                                        &peer,
                                        key_id.as_deref(),
                                        &command,
                                    )
                                    .await
                                    .is_err()
                                {
                                    continue;
                                }
                                let Ok(value) =
                                    serde_json::from_str::<serde_json::Value>(&plaintext)
                                else {
                                    continue;
                                };
                                if value["cmd"] == "device_event" {
                                    if let Some(event) = value["event"].as_str() {
                                        event_session.deliver_device_event(
                                            &source_device_id,
                                            event,
                                            &value["payload"],
                                        );
                                    }
                                }
                            }
                            RelayEvent::AuthOk { device_id, .. } => {
                                event_session.adopt_device_id(&device_id);
                                peer_keys.lock().await.clear();
                                let _ = hints.send(None);
                            }
                            RelayEvent::Connected | RelayEvent::Reconnected => {
                                peer_keys.lock().await.clear();
                                let _ = hints.send(None);
                            }
                            RelayEvent::DevicePresence { .. } => {
                                peer_keys.lock().await.clear();
                            }
                            _ => {}
                        }
                    }
                });
                Ok::<_, anyhow::Error>(Arc::new(transport))
            })
            .await?;
        let (encrypted_data, nonce, key_id) = transport
            .request_device(
                target_device_id,
                &sealed.data,
                &sealed.nonce,
                sealed.key_id.as_deref(),
            )
            .await?;
        // Decrypt using the authenticated target device key
        session
            .decrypt_from_peer(
                relay_url,
                target_device_id,
                &encrypted_data,
                &nonce,
                key_id.as_deref(),
            )
            .await
    }
}

/// A device in the account (online or offline).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub device_id: String,
    pub device_name: String,
    /// Kind the device reported (`desktop`, `cli`, `mobile`, `watch`). Absent
    /// for a device that never reported one and for Relays that predate the
    /// field: absent stays "unknown", which is not the same as "not a host".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_kind: Option<String>,
    #[serde(default)]
    pub device_alias: Option<String>,
    #[serde(default)]
    pub device_model: Option<String>,
    #[serde(default)]
    pub device_os: Option<String>,
    #[serde(default)]
    pub device_os_version: Option<String>,
    /// Build string the device last reported to the Relay. Absent for legacy
    /// devices and for Relays that predate the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_client_version: Option<String>,
    /// Control-contract protocol number the device last reported. Absent for
    /// legacy devices and for Relays that predate the field.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_client_protocol: Option<u32>,
    /// Relay-computed compatibility with this client's control contract.
    /// `None` means unknown (an older Relay without the field) and must be
    /// treated as compatible rather than incompatible.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub compatible: Option<bool>,
    pub online: bool,
    pub last_seen_at: Option<i64>,
}

impl DeviceInfo {
    /// Whether the Relay considers this device compatible with our control
    /// contract.
    ///
    /// `Some(false)` is the Relay's verdict that the pair must not be
    /// remote-controlled (a device that never reported a protocol number, or
    /// one whose number differs). `None` means an older Relay that predates the
    /// gating field and cannot judge, which is unknown and treated as
    /// compatible. These are different situations; do not fold the
    /// missing-version case into the fallback.
    pub fn is_compatible(&self) -> bool {
        self.compatible.unwrap_or(true)
    }
}

#[derive(Deserialize)]
struct DeviceListEntry {
    device_id: String,
    device_name: String,
    /// Relay field `device_kind`; absent on legacy Relays.
    #[serde(default)]
    device_kind: Option<String>,
    #[serde(default)]
    device_alias: Option<String>,
    #[serde(default)]
    device_model: Option<String>,
    #[serde(default)]
    device_os: Option<String>,
    #[serde(default)]
    device_os_version: Option<String>,
    /// Relay field `client_version`; absent on legacy Relays.
    #[serde(default, rename = "client_version", alias = "clientVersion")]
    device_client_version: Option<String>,
    /// Relay field `client_protocol`; absent on legacy Relays.
    #[serde(default, rename = "client_protocol", alias = "clientProtocol")]
    device_client_protocol: Option<u32>,
    /// Relay-computed; absent on legacy Relays.
    #[serde(default)]
    compatible: Option<bool>,
    /// Absent on legacy relays that only listed in-memory online devices.
    /// `None` means "legacy online list" → treat as online.
    #[serde(default)]
    online: Option<bool>,
    #[serde(default)]
    last_seen_at: Option<i64>,
}

fn project_device_list_entry(entry: DeviceListEntry) -> DeviceInfo {
    DeviceInfo {
        device_id: entry.device_id,
        device_name: entry.device_name,
        device_kind: entry.device_kind,
        device_alias: entry.device_alias,
        device_model: entry.device_model,
        device_os: entry.device_os,
        device_os_version: entry.device_os_version,
        device_client_version: entry.device_client_version,
        device_client_protocol: entry.device_client_protocol,
        compatible: entry.compatible,
        // Legacy relays omit `online` and only return currently-online
        // devices; treat a missing field as online.
        online: entry.online.unwrap_or(true),
        last_seen_at: entry.last_seen_at,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_embedded_session_migrates_without_capturing_other_private_relays() {
        for url in [
            "http://192.168.1.2:9700",
            "http://10.1.2.3:9700/",
            "http://127.0.0.1:9700",
        ] {
            assert!(persisted_relay_requires_migration(url), "{url}");
        }
        for url in [
            "https://remote.openbitfun.com/v/1.0.2",
            "http://192.168.1.2:8080",
            "http://192.168.1.2:9700/custom",
            "http://relay.example:9700",
            "http://8.8.8.8:9700",
            "http://user:secret@192.168.1.2:9700",
            "http://192.168.1.2:9700?other=1",
        ] {
            assert!(!persisted_relay_requires_migration(url), "{url}");
        }
    }

    #[test]
    fn device_directory_accepts_legacy_nullable_and_extended_payloads() {
        let old = serde_json::json!({"device_id":"id", "device_name":"technical", "online":true, "last_seen_at":null});
        let device: DeviceInfo = serde_json::from_value(old).unwrap();
        assert!(device.device_alias.is_none());
        let round_trip: DeviceInfo =
            serde_json::from_value(serde_json::to_value(&device).unwrap()).unwrap();
        assert_eq!(round_trip.device_name, "technical");
        let entry: DeviceListEntry = serde_json::from_value(serde_json::json!({
            "device_id":"id", "device_name":"technical", "device_alias":"alias",
            "device_model":"model", "device_os":"linux", "device_os_version":null,
            "future_field":true
        }))
        .unwrap();
        assert_eq!(entry.device_alias.as_deref(), Some("alias"));
        assert_eq!(entry.device_name, "technical");
        assert_eq!(entry.online, None);
        assert_eq!(entry.device_os_version, None);
    }

    #[test]
    fn login_request_body_always_reports_client_protocol_and_version() {
        let device = DeviceIdentity {
            device_id: "0123456789abcdef0123456789abcdef".to_string(),
            device_name: "Laptop".to_string(),
            mac_address: "aa:bb:cc:dd:ee:ff".to_string(),
        };
        let body = login_request_body(
            "token-1",
            &device,
            DEVICE_KIND_CLI,
            "public-key".to_string(),
            "req-1".into(),
        );
        assert_eq!(
            body["clientProtocol"].as_u64(),
            Some(openbitfun_product_domains::account::CLIENT_PROTOCOL_VERSION as u64)
        );
        assert_eq!(
            body["clientVersion"].as_str(),
            Some(openbitfun_product_domains::account::client_version())
        );
        assert_eq!(body["device_id"], device.device_id);
        assert_eq!(body["request_id"], "req-1");
        // Pins the wire value the Relay validates against its own kind list.
        assert_eq!(body["device_kind"], "cli");
    }

    #[test]
    fn device_directory_projects_client_compatibility_for_legacy_and_extended_relays() {
        // Legacy Relay rows have neither the new build fields nor `compatible`.
        let legacy: DeviceListEntry = serde_json::from_value(serde_json::json!({
            "device_id": "id", "device_name": "technical", "online": true
        }))
        .unwrap();
        let projected = project_device_list_entry(legacy);
        assert!(projected.device_client_version.is_none());
        assert!(projected.device_client_protocol.is_none());
        assert!(projected.compatible.is_none());
        assert!(projected.is_compatible());
        // A legacy row carries no kind, which must stay unknown rather than be
        // read as "not a host".
        assert!(projected.device_kind.is_none());

        // A current Relay reports the build fields and the computed flag.
        let extended: DeviceListEntry = serde_json::from_value(serde_json::json!({
            "device_id": "id", "device_name": "technical", "online": true,
            "client_version": "1.0.1", "client_protocol": 2, "compatible": false,
            "device_kind": "cli"
        }))
        .unwrap();
        let projected = project_device_list_entry(extended);
        assert_eq!(projected.device_client_version.as_deref(), Some("1.0.1"));
        assert_eq!(projected.device_client_protocol, Some(2));
        assert_eq!(projected.device_kind.as_deref(), Some("cli"));
        assert!(!projected.is_compatible());

        // A device that never reported a version is judged incompatible by the
        // Relay: no build fields, present `compatible: false`.
        let unversioned: DeviceListEntry = serde_json::from_value(serde_json::json!({
            "device_id": "id", "device_name": "technical", "online": true, "compatible": false
        }))
        .unwrap();
        let projected = project_device_list_entry(unversioned);
        assert!(projected.device_client_protocol.is_none());
        assert!(!projected.is_compatible());

        // The camelCase aliases are accepted for forward tolerance.
        let aliased: DeviceListEntry = serde_json::from_value(serde_json::json!({
            "device_id": "id", "device_name": "technical",
            "clientVersion": "1.0.1", "clientProtocol": 2, "compatible": true
        }))
        .unwrap();
        assert_eq!(aliased.device_client_version.as_deref(), Some("1.0.1"));
        assert_eq!(aliased.device_client_protocol, Some(2));
        assert_eq!(aliased.compatible, Some(true));
    }

    #[tokio::test]
    async fn alias_patch_negotiates_and_sends_explicit_null() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/prefix", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            for index in 0..2 {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                loop {
                    let mut chunk = [0; 4096];
                    let n = socket.read(&mut chunk).await.unwrap();
                    assert!(n > 0);
                    request.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&request);
                    if let Some((headers, body)) = text.split_once("\r\n\r\n") {
                        let length: usize = headers
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length: ")
                                    .and_then(|n| n.parse().ok())
                            })
                            .unwrap_or(0);
                        if body.len() >= length {
                            break;
                        }
                    }
                }
                let request = String::from_utf8(request).unwrap();
                let body = if index == 0 {
                    assert!(request.starts_with("GET /prefix/api/info "));
                    r#"{"capabilities":["device_alias_v1"]}"#
                } else {
                    assert!(request.starts_with("PATCH /prefix/api/devices/id "));
                    assert!(request
                        .to_ascii_lowercase()
                        .contains("authorization: bearer fixture"));
                    assert!(request.ends_with(r#"{"device_alias":null}"#));
                    "{}"
                };
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body).as_bytes()).await.unwrap();
            }
        });
        let session = AccountSession::new("fixture".into(), "user".into(), [7; 32]);
        AccountClient::new()
            .update_device_alias(&url, &session, "id", None)
            .await
            .unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn cli_kind_is_reported_only_to_a_relay_that_advertises_it() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            for reply in [
                // A Relay that learned the kind.
                Some(r#"{"capabilities":["device_alias_v1","device_kind_cli_v1"]}"#),
                // An older Relay validates kinds against a shorter list and
                // would reject the whole login, so the CLI must stay a desktop.
                Some(r#"{"capabilities":["device_alias_v1"]}"#),
                // No capability list at all: unknown, so unsupported.
                None,
            ] {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut buffer = [0; 4096];
                assert!(socket.read(&mut buffer).await.unwrap() > 0);
                let response = match reply {
                    Some(body) => format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                        body.len(),
                        body
                    ),
                    None => "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
                };
                socket.write_all(response.as_bytes()).await.unwrap();
            }
        });

        let client = AccountClient::new();
        assert_eq!(
            client.reported_device_kind(&url, true).await,
            DEVICE_KIND_CLI
        );
        assert_eq!(
            client.reported_device_kind(&url, true).await,
            DEVICE_KIND_DESKTOP
        );
        assert_eq!(
            client.reported_device_kind(&url, true).await,
            DEVICE_KIND_DESKTOP
        );
        // A Desktop host issues no probe at all: it already reports the value
        // every Relay accepts.
        assert_eq!(
            client.reported_device_kind(&url, false).await,
            DEVICE_KIND_DESKTOP
        );
        server.await.unwrap();
    }

    #[tokio::test]
    async fn slow_peer_lookup_does_not_block_another_device_or_invalidation() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        use tokio::time::{timeout, Duration};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (started_tx, started_rx) = tokio::sync::oneshot::channel();
        let (release_tx, release_rx) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut slow, _) = listener.accept().await.unwrap();
            let mut request = [0; 4096];
            // The fixture only needs each request to have arrived before it
            // answers; the reply does not depend on the bytes received.
            assert!(slow.read(&mut request).await.unwrap() > 0);
            started_tx.send(()).unwrap();
            let (mut fast, _) = listener.accept().await.unwrap();
            assert!(fast.read(&mut request).await.unwrap() > 0);
            let reply = |device: &str| {
                let body = serde_json::json!({
                    "device_id": device,
                    "public_key": super::super::encryption::KeyPair::generate().public_key_base64()
                })
                .to_string();
                format!("HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", body.len(), body)
            };
            fast.write_all(reply("fast").as_bytes()).await.unwrap();
            release_rx.await.unwrap();
            slow.write_all(reply("slow").as_bytes()).await.unwrap();
        });
        let session = AccountSession::new("fixture".into(), "fixture".into(), [7; 32]);
        let slow_session = session.clone();
        let slow_url = url.clone();
        let slow =
            tokio::spawn(async move { slow_session.peer_public_key(&slow_url, "slow").await });
        timeout(Duration::from_secs(5), started_rx)
            .await
            .unwrap()
            .unwrap();
        timeout(
            Duration::from_secs(5),
            session.peer_public_key(&url, "fast"),
        )
        .await
        .unwrap()
        .unwrap();
        timeout(Duration::from_secs(5), session.clear_peer_keys())
            .await
            .unwrap();
        release_tx.send(()).unwrap();
        slow.await.unwrap().unwrap();
        server.await.unwrap();
        assert!(
            session.peer_keys.lock().await.is_empty(),
            "invalidation must survive an older lookup completing"
        );
    }

    #[test]
    fn relay_endpoint_accepts_http_servers_and_rejects_ambiguous_urls() {
        let endpoint =
            AccountClient::endpoint("https://relay.example.com/prefix/", "/api/devices").unwrap();
        assert_eq!(
            endpoint.as_str(),
            "https://relay.example.com/prefix/api/devices"
        );

        for invalid in [
            "file:///tmp/relay",
            "https://user:pass@relay.example.com",
            "https://relay.example.com?target=other",
            "https://relay.example.com/#fragment",
        ] {
            assert!(AccountClient::endpoint(invalid, "/api/devices").is_err());
        }
    }

    #[test]
    fn retired_official_endpoint_does_not_capture_custom_relays() {
        let current = openbitfun_product_domains::account::DEFAULT_RELAY_URL;
        assert_eq!(current, "https://remote.openbitfun.com/v/1.0.2");
        for old in [
            ["https://remote.openbitfun.com", "/v/1.0.0"].concat(),
            ["https://remote.openbitfun.com", "/v/1.0.1"].concat(),
        ] {
            assert!(is_retired_official_relay(&old), "{old}");
            assert!(is_retired_official_relay(&format!("{old}/")), "{old}/");
            assert!(!is_retired_official_relay(&format!("{old}?other=1")));
            assert!(!is_retired_official_relay(&format!("{old}#pair")));
        }
        for endpoint in [
            current,
            &format!("{current}/"),
            "https://custom.example/v/1.0.0",
            "http://127.0.0.1:9700",
            "https://remote.openbitfun.com/relay",
            "https://remote.openbitfun.com/v/",
            "https://remote.openbitfun.com/v/1.0.0/p/alice/demo",
            "https://user@remote.openbitfun.com/v/1.0.0",
            "https://remote.openbitfun.com:444/v/1.0.0",
            "http://remote.openbitfun.com/v/1.0.1",
        ] {
            assert!(!is_retired_official_relay(endpoint), "{endpoint}");
        }
    }

    /// The envelope's `key_id` is the only authority for the derivation:
    /// absent means legacy even when a pairing exists, present means the PSK
    /// derivation, and a present id without a matching stored pairing is a
    /// loud error that never falls back to the legacy key.
    #[tokio::test]
    async fn psk_envelope_dispatch_follows_key_id() {
        let dir = tempfile::tempdir().unwrap();
        let _store_guard = crate::remote_connect::session_store::test_store_guard()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        crate::remote_connect::session_store::set_session_store_directory_for_test(
            dir.path().to_path_buf(),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let relay_url = format!("http://{}", listener.local_addr().unwrap());
        let body = serde_json::json!({
            "device_id": "host-1",
            "public_key": device_crypto::public_key_base64(&[11u8; 32]),
        })
        .to_string();
        let reply = format!(
            "HTTP/1.1 200 OK
Content-Type: application/json
Content-Length: {}
Connection: close

{}",
            body.len(),
            body
        );
        let server = tokio::spawn(async move {
            use tokio::io::{AsyncReadExt, AsyncWriteExt};
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let mut request = [0u8; 4096];
                if socket.read(&mut request).await.unwrap() == 0 {
                    continue;
                }
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        let session = AccountSession::new("fixture".into(), "fixture".into(), [7u8; 32]);

        // Unpaired traffic keeps the byte-identical legacy envelope.
        let legacy = session
            .encrypt_for_peer(&relay_url, "host-1", "legacy hello")
            .await
            .unwrap();
        assert!(legacy.key_id.is_none());
        assert_eq!(
            session
                .decrypt_from_peer(&relay_url, "host-1", &legacy.data, &legacy.nonce, None)
                .await
                .unwrap(),
            "legacy hello"
        );

        // Confirming the pairing switches the sender to the PSK derivation.
        let psk = [91u8; 32];
        crate::remote_connect::psk_store::store_entry(
            &relay_url,
            "fixture",
            "host-1",
            &psk,
            crate::remote_connect::psk_store::PeerPskStatus::Paired,
            None,
        )
        .unwrap();
        let paired = session
            .encrypt_for_peer(&relay_url, "host-1", "paired hello")
            .await
            .unwrap();
        assert_eq!(paired.key_id.as_deref(), Some("psk_v1:b95439ddd3dec598"));
        assert_eq!(
            session
                .decrypt_from_peer(
                    &relay_url,
                    "host-1",
                    &paired.data,
                    &paired.nonce,
                    paired.key_id.as_deref(),
                )
                .await
                .unwrap(),
            "paired hello"
        );

        // A mirrored legacy response stays decryptable while paired.
        let mirrored = session
            .encrypt_for_peer_variant(&relay_url, "host-1", None, "legacy response")
            .await
            .unwrap();
        assert!(mirrored.key_id.is_none());
        assert_eq!(
            session
                .decrypt_from_peer(&relay_url, "host-1", &mirrored.data, &mirrored.nonce, None)
                .await
                .unwrap(),
            "legacy response"
        );

        // key_id is authoritative in both directions: a PSK payload cannot be
        // opened as legacy, and a foreign id is a loud pairing mismatch.
        assert!(session
            .decrypt_from_peer(&relay_url, "host-1", &paired.data, &paired.nonce, None)
            .await
            .is_err());
        let mismatch = session
            .decrypt_from_peer(
                &relay_url,
                "host-1",
                &paired.data,
                &paired.nonce,
                Some("psk_v1:0000000000000000"),
            )
            .await
            .unwrap_err();
        assert!(
            mismatch.to_string().contains("Peer pairing key mismatch"),
            "unexpected error: {mismatch}"
        );
        server.abort();
    }

    /// A raw-TCP relay double for the local sign-in flow. Each incoming
    /// request gets the next scripted body; the request line is recorded so
    /// the test can pin the exact call order.
    struct LocalAuthRelayDouble {
        url: String,
        handle: tokio::task::JoinHandle<Vec<String>>,
    }

    fn local_pair_body(access: &str, refresh: &str) -> String {
        serde_json::json!({
            "accessToken": access,
            "accessExpiresAt": 946684800,
            "refreshToken": refresh,
            "refreshExpiresAt": 946684900,
            "user": {
                "userId": "local-abc123",
                "login": "u-ab12cd",
                "phoneMasked": "138****0001",
                "hasPassword": false,
            },
        })
        .to_string()
    }

    async fn spawn_local_auth_relay(script: Vec<String>) -> LocalAuthRelayDouble {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let handle = tokio::spawn(async move {
            let mut seen = Vec::new();
            for body in script {
                let (mut socket, _) = listener.accept().await.unwrap();
                use tokio::io::{AsyncReadExt, AsyncWriteExt};
                let mut buffer = [0; 8192];
                let read = socket.read(&mut buffer).await.unwrap();
                let request = String::from_utf8_lossy(&buffer[..read]).to_string();
                let request_line = request.split("\r\n").next().unwrap_or_default().to_string();
                seen.push(request_line);
                let response = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                socket.write_all(response.as_bytes()).await.unwrap();
            }
            seen
        });
        LocalAuthRelayDouble { url, handle }
    }

    fn local_device() -> DeviceIdentity {
        DeviceIdentity {
            device_id: "0123456789abcdef0123456789abcdef".to_string(),
            device_name: "Test Host".to_string(),
            mac_address: "00:11:22:33:44:55".to_string(),
        }
    }

    /// Hold the shared store guard for the whole test: returning it keeps the
    /// lock alive until the end of the test function.
    fn redirect_session_store_for_test() -> (std::path::PathBuf, std::sync::MutexGuard<'static, ()>)
    {
        let guard = crate::remote_connect::session_store::test_store_guard()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let directory = std::env::temp_dir().join(format!(
            "openbitfun-local-auth-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        crate::remote_connect::session_store::set_session_store_directory_for_test(
            directory.clone(),
        );
        (directory, guard)
    }

    #[tokio::test]
    async fn local_credentials_login_requires_the_local_auth_capability() {
        let relay = spawn_local_auth_relay(vec![
            // A relay without local_auth_v1 must be refused before any
            // credential ever leaves the host.
            r#"{"capabilities":["device_alias_v1"]}"#.to_string(),
        ])
        .await;
        let (_store, _store_guard) = redirect_session_store_for_test();
        let client = AccountClient::new();
        let error = client
            .login_with_local_credentials(
                &relay.url,
                "13800000001",
                Some("123456"),
                None,
                &local_device(),
                DEVICE_KIND_DESKTOP,
            )
            .await
            .unwrap_err()
            .to_string();
        assert!(error.contains("local account sign-in"), "got: {error}");
        let seen = relay.handle.await.unwrap();
        assert_eq!(seen.len(), 1, "no credential request may be attempted");
        assert!(seen[0].contains("/api/info"));
    }

    #[tokio::test]
    async fn local_credentials_login_signs_in_exchanges_and_rotates() {
        // The post-login metadata report probes capabilities first; without
        // `device_metadata_v1` in the answer it stops there.
        let relay = spawn_local_auth_relay(vec![
            r#"{"capabilities":["local_auth_v1"]}"#.to_string(),
            local_pair_body("access-1", "refresh-1"),
            r#"{"token":"device-token","user_id":"local-abc123"}"#.to_string(),
            r#"{"capabilities":["local_auth_v1"]}"#.to_string(),
            r#"{"capabilities":["local_auth_v1"]}"#.to_string(),
            local_pair_body("access-2", "refresh-2"),
        ])
        .await;
        let (store, _store_guard) = redirect_session_store_for_test();
        let client = AccountClient::new();
        let (session, pair) = client
            .login_with_local_credentials(
                &relay.url,
                "13800000001",
                Some("123456"),
                None,
                &local_device(),
                DEVICE_KIND_DESKTOP,
            )
            .await
            .expect("local login");
        assert_eq!(session.user_id, "local-abc123");
        assert_eq!(pair.access_token, "access-1");
        assert_eq!(pair.refresh_token, "refresh-1");
        assert_eq!(pair.user.phone_masked, "138****0001");
        assert!(!pair.user.has_password);
        let rotated = client
            .refresh_local_pair(&relay.url, "refresh-1")
            .await
            .expect("refresh");
        assert_eq!(rotated.access_token, "access-2");
        assert_eq!(rotated.refresh_token, "refresh-2");
        let seen = relay.handle.await.unwrap();
        assert_eq!(
            seen[0], "GET /api/info HTTP/1.1",
            "capability probe comes first"
        );
        assert!(seen[1].contains("POST /api/auth/local/login"));
        assert!(seen[2].contains("POST /api/auth/login"));
        assert!(seen[3].contains("/api/info"));
        assert!(seen[4].contains("/api/info"));
        assert!(seen[5].contains("POST /api/auth/local/refresh"));
        let _ = std::fs::remove_dir_all(store);
    }

    #[test]
    fn relay_url_override_validates_scheme_and_private_http() {
        let guard = EnvGuard::new("OPENBITFUN_RELAY_URL", "");
        assert_eq!(
            super::configured_relay_url().unwrap(),
            openbitfun_product_domains::account::DEFAULT_RELAY_URL
        );
        guard.set("https://relay.example.com/v1");
        assert_eq!(
            super::configured_relay_url().unwrap(),
            "https://relay.example.com/v1"
        );
        guard.set("http://192.168.1.5:9700");
        assert!(super::configured_relay_url().is_ok());
        guard.set("http://relay.example.com");
        assert!(super::configured_relay_url().is_err());
    }

    /// Sequential env-var override so parallel tests cannot observe the
    /// mutation half-applied; the guard restores the previous value on drop.
    struct EnvGuard {
        name: &'static str,
        previous: Option<String>,
    }

    impl EnvGuard {
        fn new(name: &'static str, initial: &str) -> Self {
            let previous = std::env::var(name).ok();
            std::env::set_var(name, initial);
            Self { name, previous }
        }
        fn set(&self, value: &str) {
            std::env::set_var(self.name, value);
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match self.previous.take() {
                Some(value) => std::env::set_var(self.name, value),
                None => std::env::remove_var(self.name),
            }
        }
    }
}
