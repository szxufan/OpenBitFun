//! Independent desktop Relay connections under the shared signed-in identity.
//!
//! Official routing owns the persisted account session and device directory.
//! LAN routing owns only an in-memory credential and the embedded server. A
//! connection replacement, cancellation or presence update is local to its route.

use super::*;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RelayRoute {
    #[serde(rename = "openbitfun_server")]
    Official,
    Lan,
}

struct RouteRuntime {
    service: Arc<RwLock<Option<RemoteConnectService>>>,
    state: std::sync::Mutex<DeviceRoutingState>,
    lifecycle: tokio::sync::RwLock<()>,
    cancellation: tokio::sync::watch::Sender<u64>,
    // The official route retains its existing account transition owner. These
    // fields belong only to the transient LAN session, never to shared identity.
    context: RwLock<Option<AccountContextState>>,
    generation: AtomicU64,
    operation: tokio::sync::Mutex<()>,
}

impl Default for RouteRuntime {
    fn default() -> Self {
        Self {
            service: Arc::new(RwLock::new(None)),
            state: std::sync::Mutex::new(DeviceRoutingState::default()),
            lifecycle: tokio::sync::RwLock::new(()),
            cancellation: tokio::sync::watch::channel(0).0,
            context: RwLock::new(None),
            generation: AtomicU64::new(1),
            operation: tokio::sync::Mutex::new(()),
        }
    }
}

impl RelayRoute {
    pub(super) const ALL: [Self; 2] = [Self::Official, Self::Lan];

    fn runtime(self) -> &'static RouteRuntime {
        static OFFICIAL: OnceLock<RouteRuntime> = OnceLock::new();
        static LAN: OnceLock<RouteRuntime> = OnceLock::new();
        match self {
            Self::Official => &OFFICIAL,
            Self::Lan => &LAN,
        }
        .get_or_init(RouteRuntime::default)
    }

    pub(super) fn service(self) -> &'static Arc<RwLock<Option<RemoteConnectService>>> {
        &self.runtime().service
    }

    pub(super) fn state(self) -> &'static std::sync::Mutex<DeviceRoutingState> {
        &self.runtime().state
    }

    pub(super) fn lifecycle(self) -> &'static tokio::sync::RwLock<()> {
        &self.runtime().lifecycle
    }

    pub(super) fn cancellation(self) -> &'static tokio::sync::watch::Sender<u64> {
        &self.runtime().cancellation
    }

    pub(super) fn cancel(self) {
        self.cancellation()
            .send_modify(|epoch| *epoch = epoch.wrapping_add(1));
    }

    pub(super) fn generation(self) -> u64 {
        match self {
            Self::Official => account_context_generation(),
            Self::Lan => self.runtime().generation.load(Ordering::Acquire),
        }
    }

    pub(super) fn is_current(self, generation: u64) -> bool {
        match self {
            Self::Official => account_context_is_current(generation),
            Self::Lan => self.generation() == generation,
        }
    }

    pub(super) async fn lock_operation(
        self,
        generation: u64,
    ) -> Result<tokio::sync::MutexGuard<'static, ()>, String> {
        let guard = match self {
            Self::Official => ACCOUNT_OPERATION_LOCK.lock().await,
            Self::Lan => self.runtime().operation.lock().await,
        };
        if !self.is_current(generation) {
            return Err("account context changed".into());
        }
        Ok(guard)
    }

    pub(super) async fn context(self, generation: u64) -> Result<(AccountSession, String), String> {
        if self == Self::Official {
            return read_account_context_for_generation(generation).await;
        }
        let context = self.runtime().context.read().await.clone();
        if !self.is_current(generation) {
            return Err("account context changed".into());
        }
        context
            .map(|context| (context.session, context.relay_url))
            .ok_or_else(|| "LAN Relay is not connected".into())
    }

    pub(super) async fn matches(self, generation: u64, token: &str) -> bool {
        self.context(generation)
            .await
            .is_ok_and(|(session, _)| session.token == token)
            && self.is_current(generation)
    }

    /// Caller holds the account transition boundary and LAN start/stop owner.
    pub(super) async fn replace_lan_context(
        context: Option<AccountContextState>,
    ) -> Option<AccountContextState> {
        let runtime = Self::Lan.runtime();
        runtime.generation.fetch_add(1, Ordering::AcqRel);
        Self::Lan.cancel();
        let _operation = runtime.operation.lock().await;
        std::mem::replace(&mut *runtime.context.write().await, context)
    }
}

/// The caller serializes start/stop. Shared identity is verified for this Relay
/// without replacing or persisting the official account context.
pub(super) async fn start_lan(method: ConnectionMethod) -> Result<ConnectionResult, String> {
    let _login = ACCOUNT_LOGIN_LOCK.lock().await;
    let generation = account_context_generation();
    if !account_context_is_current(generation) {
        return Err("account context changed".into());
    }
    let route = RelayRoute::Lan;
    {
        let mut holder = route.service().write().await;
        if holder.is_none() {
            *holder = Some(
                new_remote_connect_service(RemoteConnectConfig {
                    mobile_web_dir: detect_mobile_web_dir(),
                    ..RemoteConnectConfig::default()
                })
                .map_err(|error| error.to_string())?,
            );
        }
    }
    // Replacing a LAN invitation retires only the previous LAN credential.
    // The boundary also prevents a concurrent logout from reopening LAN.
    let _boundary = ACCOUNT_TRANSITION_BOUNDARY_LOCK.lock().await;
    if !account_context_is_current(generation) {
        return Err("account context changed".into());
    }
    // Showing an existing invitation must not reconnect phones already using
    // this interface. Reuse only the current route owner and live transport.
    {
        let holder = route.service().read().await;
        if let Some(service) = holder.as_ref() {
            let owner = with_route_state(route, |state| state.owner.clone());
            if service.active_method().await.as_ref() == Some(&method)
                && service.is_device_connected().await
                && match owner {
                    Some(owner) => device_routing_owner_is_current(&owner).await,
                    None => false,
                }
            {
                return service
                    .start(method)
                    .await
                    .map_err(|error| error.to_string());
            }
        }
    }
    stop_lan().await;
    let result = async {
        let holder = route.service().read().await;
        let service = holder.as_ref().ok_or("LAN service not initialized")?;
        let relay_url = service
            .prepare_relay(&method)
            .await
            .map_err(|error| error.to_string())?;
        let identity = current_device_identity()?;
        // Local accounts have no cross-relay credential. The LAN Relay is
        // process-local and owned by this app, so the owner session is
        // provisioned directly for the account signed in on the configured
        // relay; controllers on the LAN Relay must still match that account.
        let owner_user_id = get_account_context()
            .read()
            .await
            .as_ref()
            .map(|context| context.session.user_id.clone())
            .ok_or("Sign in on the configured relay before starting LAN mode")?;
        let owner_username = session_store::load_credential_hint()
            .map(|hint| hint.username)
            .unwrap_or_else(|| owner_user_id.clone());
        let owner = service
            .provision_embedded_owner(&relay_url, &owner_user_id, &owner_username, &identity)
            .await
            .map_err(|error| error.to_string())?;
        let device_secret =
            session_store::device_secret(&relay_url, &owner_user_id, &identity.device_id)
                .map_err(|error| error.to_string())?;
        let session = AccountSession::new(owner.token, owner_user_id, device_secret);
        RelayRoute::replace_lan_context(Some(AccountContextState { session, relay_url })).await;
        drop(holder);
        connect_route_devices(route).await?;
        route
            .service()
            .read()
            .await
            .as_ref()
            .ok_or("LAN service not initialized")?
            .start(method)
            .await
            .map_err(|error| error.to_string())
    }
    .await;
    if result.is_err() {
        stop_lan().await;
    }
    result
}

/// Caller holds the account transition boundary. Revoke while the embedded
/// server is still alive; socket/hub cancellation is confined to the LAN owner.
pub(super) async fn stop_lan() {
    let previous = RelayRoute::replace_lan_context(None).await;
    stop_route_device_routing(RelayRoute::Lan, "LAN Relay stopped").await;
    revoke_replaced_account(&AccountClient::new(), previous).await;
    if let Some(service) = RelayRoute::Lan.service().read().await.as_ref() {
        service.stop_relay().await;
    }
}
