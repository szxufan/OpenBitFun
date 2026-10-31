//! Cross-account device grants: wire projections plus the host-side
//! admission policy for grant-riding controllers.
//!
//! The relay only decides *reachability* (an active grant routes the call);
//! this module is the authoritative policy point on the shared machine. A
//! cross-account controller is admitted only when a current grant names the
//! grantee account and the controller device completed out-of-band PSK
//! pairing with this host, so a compromised or buggy relay never alone
//! yields control.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use super::account::{AccountClient, AccountSession};
use super::psk_store;

/// 跨账号的完整对端地址；旧调用方仍可传入同账号设备 ID。
/// 字符串形式仅用于已有订阅、surface 与 API 的身份键，不能作为授权事实。
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PeerTarget {
    pub device_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant_id: Option<String>,
}

impl PeerTarget {
    pub fn same_account(device_id: &str) -> Self {
        Self {
            device_id: device_id.to_owned(),
            account_id: None,
            grant_id: None,
        }
    }

    pub fn shared(account_id: &str, device_id: &str, grant_id: &str) -> Self {
        Self {
            device_id: device_id.to_owned(),
            account_id: Some(account_id.to_owned()),
            grant_id: Some(grant_id.to_owned()),
        }
    }

    pub fn parse(value: &str) -> anyhow::Result<Self> {
        let Some(value) = value.strip_prefix("grant-v1:") else {
            anyhow::ensure!(!value.trim().is_empty(), "peer device is required");
            return Ok(Self::same_account(value));
        };
        let parts = value.split(':').collect::<Vec<_>>();
        anyhow::ensure!(parts.len() == 3, "invalid shared peer target");
        let parts = parts
            .into_iter()
            .map(|part| {
                let decoded = urlencoding::decode(part)?.into_owned();
                anyhow::ensure!(!decoded.is_empty(), "invalid shared peer target");
                Ok(decoded)
            })
            .collect::<anyhow::Result<Vec<String>>>()?;
        let peer = Self::shared(&parts[0], &parts[1], &parts[2]);
        anyhow::ensure!(
            peer.key() == format!("grant-v1:{value}"),
            "noncanonical shared peer target"
        );
        Ok(peer)
    }

    pub fn key(&self) -> String {
        match (&self.account_id, &self.grant_id) {
            (Some(account), Some(grant)) => format!(
                "grant-v1:{}:{}:{}",
                urlencoding::encode(account),
                urlencoding::encode(&self.device_id),
                urlencoding::encode(grant)
            ),
            _ => self.device_id.clone(),
        }
    }

    pub fn is_shared(&self) -> bool {
        self.account_id.is_some()
    }

    /// 配对信任属于账号和设备，不因同一设备重新获得授权而改变。
    pub fn pairing_identity(&self) -> String {
        match &self.account_id {
            Some(account) => format!(
                "peer-v1:{}:{}",
                urlencoding::encode(account),
                urlencoding::encode(&self.device_id)
            ),
            None => self.device_id.clone(),
        }
    }
}

/// Grant facts as the relay serializes them (`snake_case` wire shape). Safe
/// for either principal: the invite secret hash never travels.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceGrantInfo {
    pub grant_id: String,
    pub owner_user_id: String,
    pub device_id: String,
    pub grantee_user_id: String,
    pub scope: String,
    pub state: String,
    pub offered_at: i64,
    pub accepted_at: Option<i64>,
    pub expires_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

impl DeviceGrantInfo {
    /// Active and not expired; mirrors the relay's own routing predicate.
    pub fn is_current(&self) -> bool {
        if self.state != "active" {
            return false;
        }
        match self.expires_at {
            Some(expires_at) => expires_at > unix_now(),
            None => true,
        }
    }
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs() as i64)
        .unwrap_or(0)
}

/// 由 grant 计算控制端驱动/配对共享机时使用的复合对端标识；编码只在 Rust 产生。
pub fn shared_target_key(grant: &DeviceGrantInfo) -> String {
    PeerTarget::shared(&grant.owner_user_id, &grant.device_id, &grant.grant_id).key()
}

/// A roster entry in the grantee's shared-with-me view, with the owner
/// machine's directory projection. The grantee never sees the owner's other
/// devices.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SharedDeviceGrant {
    pub grant: DeviceGrantInfo,
    pub device_name: String,
    pub device_kind: Option<String>,
    pub client_version: Option<String>,
    pub client_protocol: Option<u32>,
    pub online: bool,
    pub compatible: bool,
    /// 控制端进入该共享机时使用的复合对端标识（`grant-v1:{owner}:{device}:{grant}`）。
    /// Relay 的 wire 不含此字段（`default` 为空），由宿主在返回前按本地 roster 计算，
    /// 使编码只在 Rust 一处产生，前端只透传。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub target_key: String,
}

/// Response of `GET /api/grants`: both views for the caller.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceGrantList {
    pub owned: Vec<DeviceGrantInfo>,
    pub received: Vec<SharedDeviceGrant>,
}

/// Response of `POST /api/grants`: the one-time invite secret is returned
/// exactly once and never persisted by the relay in plaintext.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeviceGrantOffer {
    pub grant: DeviceGrantInfo,
    pub invite_secret: String,
    pub invite_expires_at: i64,
}

pub const GRANT_SCOPE_DRIVE: &str = "drive";
pub const GRANT_SCOPE_OBSERVE: &str = "observe";

/// Byte-stable refusal reasons surfaced to controllers and logs. Hosts must
/// return these verbatim so clients can classify them.
pub const PEER_GRANT_REQUIRED: &str =
    "peer grant required: the controller account has no active grant for this host";
pub const PEER_GRANT_PAIRING_REQUIRED: &str =
    "peer grant pairing required: complete PSK pairing with this host before controlling it";
pub const PEER_GRANT_STALE: &str =
    "peer grant state unavailable: new sessions require a reachable relay";
pub const PEER_GRANT_SCOPE_REFUSED: &str =
    "peer grant scope refused: this command is not allowed for observe grants";

/// How often the host refreshes its grant roster from the relay.
const GRANT_ROSTER_REFRESH: Duration = Duration::from_secs(60);
/// How long an already-admitted controller keeps working when the relay
/// cannot be reached to revalidate. New admissions always fail closed.
pub const GRANT_SESSION_GRACE: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantScope {
    Drive,
    Observe,
}

impl GrantScope {
    pub fn from_wire(scope: &str) -> Option<Self> {
        match scope {
            GRANT_SCOPE_OBSERVE => Some(GrantScope::Observe),
            GRANT_SCOPE_DRIVE => Some(GrantScope::Drive),
            _ => None,
        }
    }

    /// Whether an observe grant may run this HostInvoke verb. Observe keeps
    /// the control-plane surface (ping, attach, keepalive), event delivery,
    /// and read-only session/workspace facts; every mutation-class verb is
    /// refused.
    pub fn observe_allows(command: &str) -> bool {
        openbitfun_product_domains::remote_surface::operation(command)
            .is_some_and(|operation| operation.observe_allowed)
    }
}

/// The admission decision for one incoming controller request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GrantAdmission {
    /// Same-account controller (or an older relay that stamps no account):
    /// peer mode behavior is unchanged.
    SameAccount,
    /// Cross-account controller admitted under a grant.
    Granted {
        grant_id: String,
        grantee_user_id: String,
        scope: GrantScope,
    },
    /// Refused with a byte-stable reason constant.
    Refused(&'static str),
}

#[derive(Clone)]
struct AdmittedSession {
    grant: DeviceGrantInfo,
    last_validated: Instant,
}

#[derive(Default)]
struct RosterState {
    snapshot: Option<(Instant, DeviceGrantList)>,
    attempted_at: Option<Instant>,
}

#[derive(Default)]
struct GateState {
    rosters: HashMap<String, RosterState>,
    sessions: HashMap<(String, String, String), AdmittedSession>,
}

/// 授权缓存由账号会话拥有；锁内不执行网络或 vault IO。
#[derive(Default)]
pub struct HostGrantGate {
    state: Mutex<GateState>,
    refresh: tokio::sync::Mutex<()>,
}

fn context_key(relay: &str, account: &str) -> String {
    serde_json::to_string(&(relay.trim_end_matches('/'), account)).expect("identity tuple")
}

impl HostGrantGate {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn invalidate(&self, relay: &str, account: &str) {
        if let Ok(mut state) = self.state.lock() {
            let key = context_key(relay, account);
            state.rosters.remove(&key);
            state.sessions.retain(|(context, _, _), _| context != &key);
        }
    }

    pub fn drop_peer(&self, relay: &str, account: &str, peer: &str) {
        if let Ok(mut state) = self.state.lock() {
            let key = context_key(relay, account);
            state
                .sessions
                .retain(|(context, _, target), _| context != &key || target != peer);
        }
    }

    pub async fn roster(
        &self,
        client: &AccountClient,
        relay: &str,
        session: &AccountSession,
    ) -> Result<(Instant, DeviceGrantList), &'static str> {
        let key = context_key(relay, &session.user_id);
        let _refresh = self.refresh.lock().await;
        let needs_refresh = {
            let state = self.state.lock().map_err(|_| PEER_GRANT_STALE)?;
            state
                .rosters
                .get(&key)
                .and_then(|row| row.attempted_at)
                .is_none_or(|at| at.elapsed() >= GRANT_ROSTER_REFRESH)
        };
        if needs_refresh {
            let result = client.list_device_grants(relay, session).await;
            let mut state = self.state.lock().map_err(|_| PEER_GRANT_STALE)?;
            let now = Instant::now();
            state.rosters.entry(key.clone()).or_default().attempted_at = Some(now);
            match result {
                Ok(list) => {
                    state.sessions.retain(|(context, _, _), admitted| {
                        context != &key
                            || list
                                .owned
                                .iter()
                                .chain(list.received.iter().map(|r| &r.grant))
                                .any(|grant| grant == &admitted.grant && grant.is_current())
                    });
                    state.rosters.entry(key.clone()).or_default().snapshot = Some((now, list));
                }
                Err(error) => log::warn!("Failed to refresh device grant roster: {error}"),
            }
        }
        self.state
            .lock()
            .map_err(|_| PEER_GRANT_STALE)?
            .rosters
            .get(&key)
            .and_then(|row| row.snapshot.clone())
            .ok_or(PEER_GRANT_STALE)
    }

    /// 解密前解析路由身份；旧目录只解析同账号设备，绝不猜测跨账号。
    pub async fn incoming_peer(
        &self,
        client: &AccountClient,
        relay: &str,
        session: &AccountSession,
        host: &str,
        source_account: Option<&str>,
        device: &str,
    ) -> anyhow::Result<PeerTarget> {
        let Some(account) = source_account.filter(|id| *id != session.user_id) else {
            // 原始设备 ID 不得借用内部复合地址命名空间。
            anyhow::ensure!(
                !device.starts_with("grant-v1:"),
                "invalid wire device identity"
            );
            return Ok(PeerTarget::same_account(device));
        };
        let (_, list) = self
            .roster(client, relay, session)
            .await
            .map_err(anyhow::Error::msg)?;
        let grant = list
            .owned
            .iter()
            .find(|g| {
                g.owner_user_id == session.user_id
                    && g.device_id == host
                    && g.grantee_user_id == account
                    && g.is_current()
            })
            .or_else(|| {
                list.received.iter().map(|r| &r.grant).find(|g| {
                    g.grantee_user_id == session.user_id
                        && g.owner_user_id == account
                        && g.device_id == device
                        && g.is_current()
                })
            })
            .ok_or_else(|| anyhow::anyhow!(PEER_GRANT_REQUIRED))?;
        Ok(PeerTarget::shared(account, device, &grant.grant_id))
    }

    /// 配对时由 grantee 依接受到的 grant 将主机设备解析为复合对端，owner 账号
    /// 取自本地 roster（不由客户端声称），用于经授权范围的 peer-key 路由。
    pub async fn resolve_pairing_target(
        &self,
        client: &AccountClient,
        relay: &str,
        session: &AccountSession,
        grant_id: &str,
        host_device_id: &str,
    ) -> anyhow::Result<PeerTarget> {
        let (_, list) = self
            .roster(client, relay, session)
            .await
            .map_err(anyhow::Error::msg)?;
        let grant = list
            .received
            .iter()
            .map(|r| &r.grant)
            .find(|g| {
                g.grant_id == grant_id
                    && g.grantee_user_id == session.user_id
                    && g.device_id == host_device_id
                    && g.is_current()
            })
            .ok_or_else(|| anyhow::anyhow!(PEER_GRANT_REQUIRED))?;
        Ok(PeerTarget::shared(
            &grant.owner_user_id,
            host_device_id,
            &grant.grant_id,
        ))
    }

    /// 宿主发布者（owner）为配对解析 grantee 设备。owner 侧的 grant 在 owned
    /// 视图，账号与设备绑定共享机自身，防止误路由到其他 owner 设备。
    pub async fn host_target_for_grantee(
        &self,
        client: &AccountClient,
        relay: &str,
        session: &AccountSession,
        host_device_id: &str,
        grantee_account: &str,
        grantee_device: &str,
    ) -> anyhow::Result<PeerTarget> {
        let (_, list) = self
            .roster(client, relay, session)
            .await
            .map_err(anyhow::Error::msg)?;
        let grant = list
            .owned
            .iter()
            .find(|g| {
                g.grantee_user_id == grantee_account
                    && g.device_id == host_device_id
                    && g.owner_user_id == session.user_id
                    && g.is_current()
            })
            .ok_or_else(|| anyhow::anyhow!(PEER_GRANT_REQUIRED))?;
        Ok(PeerTarget::shared(
            grantee_account,
            grantee_device,
            &grant.grant_id,
        ))
    }

    /// 只返回方向、设备与账号均匹配的授权；调用方不能只持有 grant ID。
    pub async fn peer_grant(
        &self,
        client: &AccountClient,
        relay: &str,
        session: &AccountSession,
        host: &str,
        peer: &PeerTarget,
    ) -> Result<(Instant, DeviceGrantInfo), &'static str> {
        let (at, list) = self.roster(client, relay, session).await?;
        let grant = list
            .owned
            .iter()
            .chain(list.received.iter().map(|r| &r.grant))
            .find(|g| {
                Some(&g.grant_id) == peer.grant_id.as_ref()
                    && ((g.owner_user_id == session.user_id
                        && g.device_id == host
                        && Some(&g.grantee_user_id) == peer.account_id.as_ref())
                        || (g.grantee_user_id == session.user_id
                            && g.device_id == peer.device_id
                            && Some(&g.owner_user_id) == peer.account_id.as_ref()))
            })
            .filter(|g| g.is_current())
            .cloned()
            .ok_or(PEER_GRANT_REQUIRED)?;
        Ok((at, grant))
    }

    fn admit_at(
        &self,
        context: &str,
        host: &str,
        peer: &str,
        grant: &DeviceGrantInfo,
        verified_at: Instant,
        now: Instant,
        establish: bool,
    ) -> Result<(), &'static str> {
        let mut state = self.state.lock().map_err(|_| PEER_GRANT_STALE)?;
        let key = (context.to_owned(), host.to_owned(), peer.to_owned());
        if !grant.is_current() {
            state.sessions.remove(&key);
            return Err(PEER_GRANT_REQUIRED);
        }
        if now.saturating_duration_since(verified_at) >= GRANT_ROSTER_REFRESH {
            let existing = state.sessions.get(&key).ok_or(PEER_GRANT_STALE)?;
            if existing.grant != *grant
                || now.saturating_duration_since(existing.last_validated) >= GRANT_SESSION_GRACE
            {
                state.sessions.remove(&key);
                return Err(PEER_GRANT_STALE);
            }
        } else if establish {
            // 使用快照验证时间，绝不使用请求时间续期。
            state.sessions.insert(
                key,
                AdmittedSession {
                    grant: grant.clone(),
                    last_validated: verified_at,
                },
            );
        }
        Ok(())
    }

    /// 仅在本条消息已成功解密后调用；key_id 必须与实际使用的 PSK 一致。
    pub async fn authorize(
        &self,
        client: &AccountClient,
        relay: &str,
        session: &AccountSession,
        host: &str,
        peer: &PeerTarget,
        key_id: Option<&str>,
        command: &super::RemoteCommand,
    ) -> Result<Option<GrantScope>, &'static str> {
        if !peer.is_shared() {
            return Ok(None);
        }
        let peer_key = peer.key();
        let result = async {
            let (at, grant) = self.peer_grant(client, relay, session, host, peer).await?;
            let scope = GrantScope::from_wire(&grant.scope).ok_or(PEER_GRANT_SCOPE_REFUSED)?;
            let event = matches!(command, super::RemoteCommand::DeviceEvent { .. });
            if event != (grant.grantee_user_id == session.user_id) {
                return Err(PEER_GRANT_SCOPE_REFUSED);
            }
            let ping = matches!(command, super::RemoteCommand::HostInvoke { command, .. } if command == "peer_mode_ping");
            let confirm = matches!(command, super::RemoteCommand::HostInvoke { command, .. } if command == "peer_psk_confirm");
            if !ping {
                let entry = psk_store::lookup_entry(relay, &session.user_id, &peer_key)
                    .map_err(|_| PEER_GRANT_PAIRING_REQUIRED)?.ok_or(PEER_GRANT_PAIRING_REQUIRED)?;
                let expected = format!("{}{}", super::device_crypto::PSK_KEY_ID_PREFIX, entry.psk_id);
                if key_id != Some(expected.as_str()) || entry.status != psk_store::PeerPskStatus::Paired {
                    return Err(PEER_GRANT_PAIRING_REQUIRED);
                }
            }
            if !event && !command_allowed(scope, command) { return Err(PEER_GRANT_SCOPE_REFUSED); }
            self.admit_at(&context_key(relay, &session.user_id), host, &peer_key, &grant,
                at, Instant::now(), !ping && !confirm)?;
            if matches!(command, super::RemoteCommand::HostInvoke { command, .. } if command == "peer_control_detach") {
                self.drop_peer(relay, &session.user_id, &peer_key);
            }
            Ok(Some(scope))
        }.await;
        if let Err(reason) = result {
            self.drop_peer(relay, &session.user_id, &peer_key);
            append_grant_audit(
                "attach_refused",
                peer.account_id.as_deref(),
                Some(&peer.device_id),
                peer.grant_id.as_deref(),
                Some(reason),
            );
        }
        result
    }

    /// 宿主发布者也必须有已准入租约，避免撤销/宽限到期后继续泄露事件。
    pub async fn authorize_delivery(
        &self,
        client: &AccountClient,
        relay: &str,
        session: &AccountSession,
        host: &str,
        peer: &PeerTarget,
    ) -> Result<(), &'static str> {
        if !peer.is_shared() {
            return Ok(());
        }
        let (at, grant) = self.peer_grant(client, relay, session, host, peer).await?;
        if grant.owner_user_id != session.user_id {
            return Err(PEER_GRANT_SCOPE_REFUSED);
        }
        let key = peer.key();
        let entry = psk_store::lookup_entry(relay, &session.user_id, &key)
            .map_err(|_| PEER_GRANT_PAIRING_REQUIRED)?
            .ok_or(PEER_GRANT_PAIRING_REQUIRED)?;
        if entry.status != psk_store::PeerPskStatus::Paired {
            return Err(PEER_GRANT_PAIRING_REQUIRED);
        }
        let context = context_key(relay, &session.user_id);
        if !self
            .state
            .lock()
            .map_err(|_| PEER_GRANT_STALE)?
            .sessions
            .contains_key(&(context.clone(), host.to_owned(), key.clone()))
        {
            return Err(PEER_GRANT_REQUIRED);
        }
        self.admit_at(&context, host, &key, &grant, at, Instant::now(), false)
    }
}

/// Append-only audit trail for grant lifecycle and cross-account control
/// sessions, one JSON line per event. English-only records; the file lives
/// under the product home directory and never contains message content.
#[derive(Debug, Serialize)]
pub struct GrantAuditRecord {
    pub at_unix: i64,
    pub event: &'static str,
    pub grantee_account: Option<String>,
    pub controller_device: Option<String>,
    pub grant_id: Option<String>,
    pub detail: Option<String>,
}

/// Append a grant audit event. Failures are logged, never surfaced: the audit
/// trail must not break control-plane execution.
pub fn append_grant_audit(
    event: &'static str,
    grantee_account: Option<&str>,
    controller_device: Option<&str>,
    grant_id: Option<&str>,
    detail: Option<&str>,
) {
    let record = GrantAuditRecord {
        at_unix: unix_now(),
        event,
        grantee_account: grantee_account.map(str::to_string),
        controller_device: controller_device.map(str::to_string),
        grant_id: grant_id.map(str::to_string),
        detail: detail.map(str::to_string),
    };
    let Some(home) = super::product_home_dir() else {
        return;
    };
    let path = home.join("device-grant-audit.jsonl");
    let line = serde_json::to_string(&record).unwrap_or_default();
    let result = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| std::io::Write::write_all(&mut file, format!("{line}\n").as_bytes()));
    if let Err(error) = result {
        log::warn!("Failed to append device grant audit event: {error}");
    }
}

/// 穷尽分类所有远程命令；新增变体必须显式决定授权，不能绕过宿主 gate。
pub fn command_allowed(scope: GrantScope, command: &super::RemoteCommand) -> bool {
    use super::RemoteCommand::*;
    match command {
        HostInvoke { command, .. } => {
            use openbitfun_product_domains::remote_surface::{operation, PeerStance};
            operation(command).is_some_and(|op| {
                matches!(op.peer, PeerStance::Proxied | PeerStance::HostControlPlane)
                    && (scope == GrantScope::Drive
                        || op.observe_allowed
                        || command == "peer_psk_confirm")
            })
        }
        ReadStream { .. }
        | UnsubscribeStream { .. }
        | GetWorkspaceInfo
        | GetWorkspaceIcon { .. }
        | ListRecentWorkspaces
        | ListAssistants
        | ListSessions { .. }
        | GetModelCatalog { .. }
        | GetSessionMessages { .. }
        | GetPermissionMode
        | PollSession { .. }
        | ReadFile { .. }
        | ReadFileChunk { .. }
        | GetFileInfo { .. }
        | Ping { .. }
        | DeviceQueryInfo => true,
        // 授权不能用于复制 owner 凭据；旧历史协议仍保持明确拒绝。
        ProvisionPeerDevice { .. } | GetSessionKey { .. } | DeviceEvent { .. } => false,
        DialogQueue { .. }
        | SetWorkspace { .. }
        | SetWorkspaceIcon { .. }
        | SetAssistant { .. }
        | CreateSession { .. }
        | SetSessionModel { .. }
        | UpdateSessionTitle { .. }
        | SendMessage { .. }
        | BuildPlan { .. }
        | SteerTurn { .. }
        | CancelTask { .. }
        | DeleteSession { .. }
        | RollbackSessionToTurn { .. }
        | CancelTool { .. }
        | ConfirmTool { .. }
        | RejectTool { .. }
        | SetPermissionMode { .. }
        | StartQuestionInteraction { .. }
        | AnswerQuestion { .. }
        | SendSessionToDevice { .. }
        | ExecuteOnDevice { .. }
        | StoreSessionAttachment { .. }
        | CreateWorkspace { .. } => scope == GrantScope::Drive,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant(state: &str, scope: &str, expires_at: Option<i64>) -> DeviceGrantInfo {
        DeviceGrantInfo {
            grant_id: "g1".to_string(),
            owner_user_id: "owner".to_string(),
            device_id: "shared".to_string(),
            grantee_user_id: "member".to_string(),
            scope: scope.to_string(),
            state: state.to_string(),
            offered_at: 1,
            accepted_at: Some(2),
            expires_at,
            revoked_at: None,
        }
    }

    #[test]
    fn is_current_requires_active_and_unexpired() {
        assert!(grant("active", "drive", None).is_current());
        assert!(!grant("revoked", "drive", None).is_current());
        assert!(!grant("pending", "drive", None).is_current());
        assert!(!grant("active", "drive", Some(unix_now() - 1)).is_current());
        assert!(grant("active", "drive", Some(unix_now() + 60)).is_current());
    }

    #[test]
    fn observe_scope_allows_only_read_surface() {
        for command in [
            "peer_mode_ping",
            "peer_control_attach",
            "get_current_workspace",
            "get_session_interaction_mailbox",
        ] {
            assert!(GrantScope::observe_allows(command), "{command}");
        }
        for command in [
            "peer_submit_message",
            "peer_write_file",
            "peer_run_terminal",
            "peer_delete_session",
        ] {
            assert!(!GrantScope::observe_allows(command), "{command}");
        }
    }

    #[test]
    fn stale_roster_never_admits_or_renews_a_controller() {
        let gate = HostGrantGate::new();
        let at = Instant::now();
        let row = grant("active", "drive", None);
        let check = |peer, elapsed, establish| {
            gate.admit_at(
                "relay-owner",
                "host",
                peer,
                &row,
                at,
                at + Duration::from_secs(elapsed),
                establish,
            )
        };
        assert_eq!(check("paired-a", 0, true), Ok(()));
        assert_eq!(check("paired-b", 61, true), Err(PEER_GRANT_STALE));
        for elapsed in [61, 120, 400, 599] {
            assert_eq!(check("paired-a", elapsed, true), Ok(()));
        }
        assert_eq!(check("paired-a", 600, true), Err(PEER_GRANT_STALE));
    }

    #[test]
    fn handshake_does_not_create_grace_and_contexts_are_isolated() {
        let gate = HostGrantGate::new();
        let at = Instant::now();
        let row = grant("active", "drive", None);
        assert!(gate.admit_at("a", "h", "c", &row, at, at, false).is_ok());
        assert_eq!(
            gate.admit_at("a", "h", "c", &row, at, at + GRANT_ROSTER_REFRESH, false),
            Err(PEER_GRANT_STALE)
        );
        assert!(gate.admit_at("a", "h", "c", &row, at, at, true).is_ok());
        for (context, host, device) in [("b", "h", "c"), ("a", "other", "c"), ("a", "h", "other")] {
            assert_eq!(
                gate.admit_at(
                    context,
                    host,
                    device,
                    &row,
                    at,
                    at + GRANT_ROSTER_REFRESH,
                    true
                ),
                Err(PEER_GRANT_STALE)
            );
        }
        assert_eq!(
            gate.admit_at(
                "a",
                "h",
                "c",
                &grant("revoked", "drive", None),
                at,
                at,
                true
            ),
            Err(PEER_GRANT_REQUIRED)
        );
    }

    #[test]
    fn shared_identity_round_trip_and_pairing_scope() {
        let peer = PeerTarget::shared("email-owner", "same-device", "grant:1");
        assert_eq!(PeerTarget::parse(&peer.key()).unwrap(), peer);
        assert_ne!(
            peer.key(),
            PeerTarget::shared("other", "same-device", "grant:1").key()
        );
        assert_eq!(
            peer.pairing_identity(),
            PeerTarget::shared("email-owner", "same-device", "renewed").pairing_identity()
        );
        assert!(PeerTarget::parse("grant-v1::device:grant").is_err());
        assert_eq!(PeerTarget::parse("legacy").unwrap().key(), "legacy");
    }

    #[test]
    fn direct_commands_cannot_bypass_scope_or_provision_credentials() {
        use super::super::{RemoteCommand, RemotePermissionMode};
        for command in [
            RemoteCommand::DeleteSession {
                session_id: "s".into(),
            },
            RemoteCommand::SetPermissionMode {
                mode: RemotePermissionMode::FullAccess,
            },
            RemoteCommand::ExecuteOnDevice {
                session_id: None,
                content: "run".into(),
                agent_type: None,
                workspace_path: None,
            },
        ] {
            assert!(!command_allowed(GrantScope::Observe, &command));
            assert!(command_allowed(GrantScope::Drive, &command));
        }
        let provision = RemoteCommand::ProvisionPeerDevice {
            device_id: "d".into(),
            device_name: "d".into(),
            request_id: "r".into(),
        };
        assert!(!command_allowed(GrantScope::Drive, &provision));
        assert!(GrantScope::from_wire("future-admin").is_none());
        assert!(!command_allowed(
            GrantScope::Drive,
            &RemoteCommand::HostInvoke {
                command: "account_login".into(),
                args: serde_json::json!({})
            }
        ));
    }
}
