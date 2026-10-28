//! Account / machine scoped realtime transport, following Happy's Socket.IO
//! ephemeral and acknowledged RPC contracts.
//!
//! Reference: slopus/happy @ 108a337e87a5653b604250a9ac3e3bd873dba551.
//! OpenBitFun keeps GitHub device credentials and host-owned execution. The
//! relay forwards opaque ciphertext between online devices and stores no
//! session content: controllers read session records from the online host on
//! demand, and the host-history HTTP routes of earlier releases answer
//! `410 Gone` (see `retired_session_history`).
mod device_lifecycle;
mod origin;
mod payloads;
pub(crate) mod presence;
pub(crate) mod retired_session_history;

use crate::{db::AuthToken, db::DeviceGrantRow, routes::api::AppState};
use axum::{
    http::StatusCode,
    routing::{any, get, post},
    Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use socketioxide::{
    extract::{AckSender, SocketRef, TryData},
    handler::ConnectHandler,
    SocketIo,
};
use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex, Weak},
    time::Duration,
};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

#[derive(Clone)]
struct Identity {
    account: String,
    device: String,
    token: String,
    scope: Scope,
    /// Client build reported by this connection, normalized at the handshake.
    /// Malformed or absent values are `None` (unreported), never a rejection.
    /// The version text is recorded on the device row; compatibility is decided
    /// from the protocol number below.
    #[allow(dead_code)]
    client_version: Option<String>,
    client_protocol: Option<u32>,
    account_calls: Arc<Semaphore>,
    server_calls: Arc<Semaphore>,
    /// Grant ids this socket routed through. Inserted at grant-authorized
    /// dispatch, revalidated by the revocation monitor, so a revoked or
    /// expired grant tears the socket down like a revoked token.
    grants_riding: Arc<Mutex<HashSet<String>>>,
    _connection: Arc<OwnedSemaphorePermit>,
}
#[derive(Clone, Copy, Deserialize, PartialEq)]
enum Scope {
    #[serde(rename = "user-scoped")]
    User,
    #[serde(rename = "machine-scoped")]
    Machine,
    #[serde(rename = "session-scoped")]
    Session,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Handshake {
    token: String,
    client_type: Scope,
    machine_id: Option<String>,
    /// Optional self-reported client build. Older clients omit both.
    #[serde(default)]
    client_version: Option<String>,
    #[serde(default)]
    client_protocol: Option<u32>,
    /// Sent by session-scoped clients of earlier releases; the handshake is
    /// rejected before it is read.
    #[allow(dead_code)]
    session_id: Option<String>,
}
#[derive(Deserialize)]
struct Method {
    method: String,
}
#[derive(Deserialize)]
struct Call {
    method: String,
    params: Value,
    #[serde(default, rename = "timeoutMs")]
    timeout_ms: Option<u64>,
    /// Owner account of a cross-account RPC target. The relay never routes by
    /// this claim alone: the grant row must confirm an active grant from this
    /// account to the verified caller. Absent for same-account calls, and
    /// ignored by older relays (additive field).
    #[serde(default, rename = "targetAccountId")]
    target_account_id: Option<String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct MachineEvent {
    target_device_id: String,
    /// Grantee account for a cross-account push. The relay verifies an active
    /// grant from this machine to that account before routing. Absent keeps
    /// the same-account room.
    #[serde(default)]
    target_account_id: Option<String>,
    params: Value,
}

fn account_room(account: &str) -> String {
    format!("user:{account}")
}
fn rpc_room(account: &str, method: &str) -> String {
    format!("rpc:{account}:{method}")
}
fn failure(error: &str) -> Value {
    json!({"ok":false,"error":error})
}

async fn authorized(state: &AppState, identity: &Identity) -> bool {
    matches!(AuthToken::find(&state.db,&identity.token).await,
        Ok(Some(auth)) if auth.user_id==identity.account && auth.can_control_devices())
}

/// Record a grant this socket now rides on, for the revocation monitor.
fn ride_grant(identity: &Identity, grant_id: &str) {
    if let Ok(mut grants) = identity.grants_riding.lock() {
        grants.insert(grant_id.to_string());
    }
}

/// Resolve a cross-account RPC routing account. The caller may claim a target
/// account, but the routing room is only derived from a grant row that the
/// relay itself resolved: an active grant from the claimed owner to the
/// verified caller account, bound to the method's device prefix. Device ids
/// are only unique per account, so without this server-side resolution the
/// claim could name the wrong account's room.
async fn routing_account_for_call(
    state: &AppState,
    identity: &Identity,
    data: &Call,
) -> Option<String> {
    let claimed_owner = data.target_account_id.as_deref()?;
    if claimed_owner == identity.account {
        return None;
    }
    let method_device = data.method.split_once(':')?.0;
    let grant =
        DeviceGrantRow::find_active(&state.db, claimed_owner, method_device, &identity.account)
            .await
            .ok()
            .flatten()?;
    ride_grant(identity, &grant.grant_id);
    Some(grant.owner_user_id)
}

fn grant_authorizes_socket(
    row: &DeviceGrantRow,
    account: &str,
    device: &str,
    scope: Scope,
) -> bool {
    row.is_current()
        && (row.grantee_user_id == account
            || (row.owner_user_id == account && row.device_id == device && scope == Scope::Machine))
}

fn can_register(identity: &Identity, method: &str) -> bool {
    if method.len() > 256 {
        return false;
    }
    let Some((owner, name)) = method.split_once(':') else {
        return false;
    };
    if name.is_empty()
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_-:".contains(&b))
    {
        return false;
    }
    match identity.scope {
        Scope::Machine => owner == identity.device,
        Scope::Session => false,
        Scope::User => false,
    }
}

/// Include JSON container/node overhead, strings and task/ack state. This is
/// deliberately conservative and independent from request count.
fn rpc_memory_cost(value: &Value) -> usize {
    fn node(value: &Value) -> usize {
        let nested = match value {
            Value::String(text) => text.len().saturating_mul(4),
            Value::Array(values) => values.iter().map(node).fold(0usize, usize::saturating_add),
            Value::Object(values) => values
                .iter()
                .map(|(key, value)| key.len().saturating_mul(4).saturating_add(node(value)))
                .fold(0usize, usize::saturating_add),
            _ => 0,
        };
        128usize.saturating_add(nested)
    }
    4096usize.saturating_add(node(value))
}

/// Construct once per Relay host. Socket.IO owns heartbeat, framing, rooms and
/// acknowledgement lifetime. Platform clients do not implement competing RPC
/// correlation registries in their UI or feature modules.
pub(crate) fn mount(router: Router<AppState>, state: AppState) -> Router<AppState> {
    let (layer, io) = SocketIo::builder()
        .req_path("/v1/updates")
        .ping_interval(Duration::from_secs(15))
        .ping_timeout(Duration::from_secs(45))
        .connect_timeout(Duration::from_secs(15))
        .ack_timeout(Duration::from_secs(30))
        .max_buffer_size(128)
        .max_payload(256 * 1024)
        .build_layer();
    let connections = Arc::new(Semaphore::new(4096));
    // Admission accounts for retained JSON and per-call state rather than an
    // arbitrary number of requests. Bulk ciphertext travels through HTTP.
    let server_calls = Arc::new(Semaphore::new(64 * 1024 * 1024));
    let account_budgets: Arc<Mutex<HashMap<String, Weak<Semaphore>>>> =
        Arc::new(Mutex::new(HashMap::new()));
    let auth_state = state.clone();
    let handler_state = state.clone();
    let handler_io = io.clone();
    io.ns(
        "/",
        (move |socket: SocketRef| {
            let state = handler_state.clone();
            let io = handler_io.clone();
            async move {
                install(socket, state, io).await;
            }
        })
        .with(
            move |socket: SocketRef, TryData(data): TryData<Handshake>| {
                let state = auth_state.clone();
                let connections = connections.clone();
                let server_calls = server_calls.clone();
                let account_budgets = account_budgets.clone();
                async move {
                    let data = data.map_err(|_| "invalid authentication payload")?;
                    if !origin::is_websocket_origin_allowed(
                        &socket.req_parts().headers,
                        &state.cors_allow_origins,
                    ) {
                        return Err("origin is not allowed");
                    }
                    let permit = connections
                        .try_acquire_owned()
                        .map_err(|_| "connection capacity exceeded")?;
                    let auth = AuthToken::find(&state.db, &data.token)
                        .await
                        .map_err(|_| "authentication unavailable")?
                        .filter(|auth| auth.can_control_devices())
                        .ok_or("invalid or expired token")?;
                    let device = auth
                        .routing_device_id(&state.db)
                        .await
                        .map_err(|_| "device identity unavailable")?;
                    match data.client_type {
                        Scope::Machine
                            if !auth.is_device_token()
                                || data.machine_id.as_deref().is_some_and(|id| id != device) =>
                        {
                            return Err("machine identity mismatch")
                        }
                        // Session-scoped sockets only ever received relay-stored
                        // history updates, which no longer exist.
                        Scope::Session => {
                            return Err(retired_session_history::HANDSHAKE_MESSAGE);
                        }
                        _ => {}
                    }
                    let account_calls = {
                        let mut budgets = account_budgets
                            .lock()
                            .map_err(|_| "account capacity unavailable")?;
                        budgets.retain(|_, budget| budget.strong_count() > 0);
                        if let Some(budget) = budgets.get(&auth.user_id).and_then(Weak::upgrade) {
                            budget
                        } else {
                            let budget = Arc::new(Semaphore::new(16 * 1024 * 1024));
                            budgets.insert(auth.user_id.clone(), Arc::downgrade(&budget));
                            budget
                        }
                    };
                    let client_version =
                        crate::db::normalize_client_version(data.client_version.as_deref());
                    let client_protocol = data.client_protocol;
                    // Refresh the device's recorded build from *this* connection
                    // on every handshake, including reconnects. Unreported values
                    // are written as NULL. A failure here must not tear down an
                    // otherwise valid authenticated session, so it is logged.
                    if let Err(error) = crate::db::DeviceRow::set_client_build(
                        &state.db,
                        &auth.user_id,
                        &device,
                        client_version.as_deref(),
                        client_protocol,
                    )
                    .await
                    {
                        tracing::warn!(%error, "Failed to record device client build at handshake");
                    }
                    socket.extensions.insert(Identity {
                        account: auth.user_id,
                        device,
                        token: data.token,
                        scope: data.client_type,
                        client_version,
                        client_protocol,
                        account_calls,
                        server_calls,
                        grants_riding: Arc::new(Mutex::new(HashSet::new())),
                        _connection: Arc::new(permit),
                    });
                    Ok(())
                }
            },
        ),
    );
    router
        // Own the Engine.IO path explicitly. A host may replace its fallback
        // with ServeDir after construction; Socket.IO must remain a route.
        .route("/v1/updates", any(|| async { StatusCode::NOT_FOUND }))
        .route("/v1/updates/", any(|| async { StatusCode::NOT_FOUND }))
        .route(
            "/v1/rpc/payloads",
            post(payloads::upload).layer(axum::extract::DefaultBodyLimit::disable()),
        )
        .route("/v1/rpc/payloads/{id}", get(payloads::download))
        .layer(axum::Extension(payloads::Payloads::new()))
        // Earlier releases stored encrypted session history here. Admission
        // answers these paths before any body is read; the routes stay owned so
        // a host static fallback can never shadow the retirement answer.
        .route("/v1/sessions", post(retired_session_history::gone))
        .route("/v1/sessions/{id}", get(retired_session_history::gone))
        .route(
            "/v3/sessions/{id}/messages",
            get(retired_session_history::gone).post(retired_session_history::gone),
        )
        .layer(axum::Extension(io))
        .layer(layer)
}

async fn install(socket: SocketRef, state: AppState, io: SocketIo) {
    let Some(identity) = socket.extensions.get::<Identity>() else {
        let _ = socket.disconnect();
        return;
    };
    socket.join(account_room(&identity.account));
    socket.join(format!(
        "user:{}:device:{}",
        identity.account, identity.device
    ));
    if identity.scope == Scope::User {
        socket.join(format!("user:{}:user-scoped", identity.account));
    }
    if identity.scope == Scope::Machine
        && !presence::register(&socket, &state, &identity, &io).await
    {
        let _ = socket.disconnect();
        return;
    }

    let auth_socket = socket.clone();
    let auth_identity = identity.clone();
    let auth_state = state.clone();
    // Also applies to idle sockets. Revoked credentials cannot keep receiving
    // another device's encrypted traffic indefinitely after logout. A socket
    // that routed through a device grant is held to the same standard: a
    // revoked or expired grant disconnects it within one tick.
    tokio::spawn(async move {
        let mut timer = tokio::time::interval(Duration::from_secs(5));
        loop {
            timer.tick().await;
            if !auth_socket.connected() {
                break;
            }
            if !authorized(&auth_state, &auth_identity).await {
                let _ = auth_socket.disconnect();
                break;
            }
            // Snapshot the grant ids before awaiting: a poisoned lock fails
            // closed, and no non-Send guard is held across the await.
            let mut grants_valid = true;
            let grant_ids = match auth_identity.grants_riding.lock() {
                Ok(grants) => grants.iter().cloned().collect::<Vec<_>>(),
                Err(_) => {
                    grants_valid = false;
                    Vec::new()
                }
            };
            for grant_id in grant_ids {
                match DeviceGrantRow::find(&auth_state.db, &grant_id).await {
                    Ok(Some(row))
                        if grant_authorizes_socket(
                            &row,
                            &auth_identity.account,
                            &auth_identity.device,
                            auth_identity.scope,
                        ) => {}
                    _ => {
                        grants_valid = false;
                        break;
                    }
                }
            }
            if !grants_valid {
                let _ = auth_socket.disconnect();
                break;
            }
        }
    });

    let register_state = state.clone();
    socket.on(
        "rpc-register",
        move |socket: SocketRef, TryData(data): TryData<Method>, ack: AckSender| {
            let state = register_state.clone();
            async move {
                let Some(identity) = socket.extensions.get::<Identity>() else {
                    return;
                };
                let Ok(data) = data else {
                    let _ = ack.send(&failure("invalid method"));
                    return;
                };
                if !authorized(&state, &identity).await || !can_register(&identity, &data.method) {
                    let _ = ack.send(&failure("method registration forbidden"));
                    return;
                }
                socket.join(rpc_room(&identity.account, &data.method));
                let _ = socket.emit("rpc-registered", &json!({"method":data.method}));
                let _ = ack.send(&json!({"ok":true}));
            }
        },
    );
    socket.on(
        "rpc-unregister",
        |socket: SocketRef, TryData(data): TryData<Method>, ack: AckSender| async move {
            let Some(identity) = socket.extensions.get::<Identity>() else {
                return;
            };
            if let Ok(data) = data {
                socket.leave(rpc_room(&identity.account, &data.method));
                let _ = ack.send(&json!({"ok":true}));
            } else {
                let _ = ack.send(&failure("invalid method"));
            }
        },
    );
    let rpc_state = state.clone();
    let rpc_io = io.clone();
    socket.on("rpc-call",move |socket:SocketRef,TryData(data):TryData<Call>,ack:AckSender| {
        let state=rpc_state.clone(); let io=rpc_io.clone();
        async move {
            let Some(identity)=socket.extensions.get::<Identity>() else { return; };
            let Ok(data)=data else { let _=ack.send(&failure("invalid RPC")); return; };
            let timeout_ms = data.timeout_ms.unwrap_or(120_000);
            if timeout_ms == 0 || timeout_ms > i32::MAX as u64 { let _=ack.send(&failure("invalid RPC timeout")); return; }
            let call_deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
            let reservation = rpc_memory_cost(&data.params).saturating_add(data.method.len()).min(u32::MAX as usize) as u32;
            let Ok(_account_permit) = identity.account_calls.clone().try_acquire_many_owned(reservation) else { let _=ack.send(&failure("account RPC memory budget busy; request was not submitted")); return; };
            let Ok(_server_permit) = identity.server_calls.clone().try_acquire_many_owned(reservation) else { let _=ack.send(&failure("server RPC memory budget busy; request was not submitted")); return; };
            if data.method.len()>256 || !authorized(&state,&identity).await { let _=ack.send(&failure("RPC forbidden")); return; }
            // Same-account calls keep the caller's room. A cross-account call
            // is only routable through a relay-resolved grant; the claimed
            // owner never selects the room directly.
            let routing_account = match routing_account_for_call(&state,&identity,&data).await {
                Some(owner) => owner,
                None => {
                    if data.target_account_id.as_deref().is_some_and(|claimed| claimed != identity.account) {
                        let _=ack.send(&failure("RPC forbidden: no active device grant")); return;
                    }
                    identity.account.clone()
                }
            };
            let room=rpc_room(&routing_account,&data.method);
            // Happy's reconnect grace belongs to target lookup, before dispatch.
            // After dispatch, a missing acknowledgement is UNKNOWN, never a
            // reason to execute the mutation again on a replacement socket.
            let deadline=(tokio::time::Instant::now()+Duration::from_secs(15)).min(call_deadline);
            let target=loop {
                let targets=io.within(room.clone()).sockets();
                if targets.len()>1 { let _=ack.send(&failure("multiple RPC owners")); return; }
                if let Some(target)=targets.into_iter().next() { break target; }
                if tokio::time::Instant::now()>=deadline || !socket.connected() {
                    let _=ack.send(&failure("RPC target unavailable")); return;
                }
                tokio::time::sleep(Duration::from_millis(200)).await;
            };
            if target.id==socket.id { let _=ack.send(&failure("RPC self-call forbidden")); return; }
            let Some(target_identity)=target.extensions.get::<Identity>() else { let _=ack.send(&failure("RPC target unavailable")); return; };
            if !authorized(&state,&identity).await || !authorized(&state,&target_identity).await {
                let _=ack.send(&failure("RPC authorization expired")); return;
            }
            // Both ends must run a comparable client build before anything is
            // dispatched. The caller may hold a delegated token, so its build is
            // taken from its own connection identity rather than the token's
            // (parent) device row.
            if !crate::db::client_builds_compatible(identity.client_protocol, target_identity.client_protocol) {
                let _=ack.send(&failure("incompatible client build: remote control requires matching client versions")); return;
            }
            if routing_account != identity.account
                && routing_account_for_call(&state, &identity, &data).await.as_deref() != Some(routing_account.as_str()) {
                let _=ack.send(&failure("RPC forbidden: device grant expired before dispatch")); return;
            }
            let remaining = call_deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() { let _=ack.send(&failure("RPC deadline elapsed before dispatch")); return; }
            // `sourceAccountId` is relay-derived from the verified token, like
            // `auth-ok`; the host trusts it to attribute and authorize
            // grant-riding controllers. Older hosts ignore the additive field.
            let request=json!({"method":data.method,"params":data.params,"sourceDeviceId":identity.device,"sourceAccountId":identity.account,"timeoutMs":remaining.as_millis()});
            let response=match target.timeout(remaining).emit_with_ack::<_,Value>("rpc-request",&request) {
                Ok(pending)=>match pending.await {
                    Ok(value)=>match value.get("$relayError").and_then(Value::as_str) {
                        Some(error) => failure(error),
                        None => json!({"ok":true,"result":value}),
                    },
                    Err(_)=>failure("RPC acknowledgement lost; delivery outcome is unknown"),
                },
                Err(_)=>failure("RPC target unavailable before dispatch"),
            };
            let _=ack.send(&response);
        }
    });
    let event_state = state.clone();
    let event_io = io.clone();
    socket.on("machine-event",move |socket:SocketRef,TryData(data):TryData<MachineEvent>,ack:AckSender| {
        let state=event_state.clone(); let io=event_io.clone();
        async move {
            let Some(identity)=socket.extensions.get::<Identity>() else{return;};
            let Ok(data)=data else {let _=ack.send(&failure("invalid event"));return;};
            if identity.scope!=Scope::Machine || !authorized(&state,&identity).await {let _=ack.send(&failure("event forbidden"));return;}
            // A machine may push outside its account only to a grantee of
            // itself, named by a grant row the relay resolves. The room then
            // lives in the grantee account, where the controller socket joined.
            let (room, ridden_grant)=match data.target_account_id.as_deref() {
                Some(target_account) if target_account != identity.account => {
                    match DeviceGrantRow::find_active(&state.db,&identity.account,&identity.device,target_account).await {
                        Ok(Some(grant)) if grant.grantee_user_id == target_account => {
                            ride_grant(&identity,&grant.grant_id);
                            (format!("user:{}:device:{}",target_account,data.target_device_id), Some(grant.grant_id))
                        }
                        _ => {let _=ack.send(&failure("event forbidden: no active device grant"));return;}
                    }
                }
                _ => (format!("user:{}:device:{}",identity.account,data.target_device_id), None),
            };
            let targets=io.within(room).sockets();
            if targets.is_empty(){let _=ack.send(&failure("event target offline"));return;}
            let event=json!({"type":"device-event","sourceDeviceId":identity.device,"sourceAccountId":identity.account,"params":data.params});
            let mut sent=false;
            for target in targets {
                let Some(recipient) = target.extensions.get::<Identity>() else { continue; };
                if !authorized(&state, &recipient).await { let _=target.disconnect(); continue; }
                if let Some(grant_id) = &ridden_grant { ride_grant(&recipient, grant_id); }
                if target.emit("ephemeral",&event).is_ok(){sent=true;}else{let _=target.disconnect();}
            }
            let _=ack.send(&json!({"ok":sent}));
        }
    });
    socket.on(
        "update-metadata",
        |TryData(_data): TryData<Value>, ack: AckSender| async move {
            let _ = ack.send(&failure(retired_session_history::HANDSHAKE_MESSAGE));
        },
    );
    let _ = socket.emit(
        "auth-ok",
        &json!({"userId":identity.account,"deviceId":identity.device}),
    );
    presence::broadcast(&io, &state, &identity.account).await;
}

#[cfg(test)]
mod route_tests {
    use super::*;
    use axum::{
        body::{to_bytes, Body},
        http::Request,
    };
    use tower::ServiceExt;

    #[tokio::test]
    async fn engine_handshake_survives_host_static_fallback() {
        let app = crate::build_relay_router(
            Arc::new(crate::MemoryAssetStore::new()),
            std::time::Instant::now(),
            Arc::new(crate::db::connect(":memory:").await.unwrap()),
            "test",
        )
        .fallback(|| async { "static page" });
        for path in ["/v1/updates", "/v1/updates/"] {
            let response = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri(format!("{path}?EIO=4&transport=polling"))
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            let body = to_bytes(response.into_body(), 4096).await.unwrap();
            assert!(body.starts_with(b"0{"), "Engine.IO open packet expected");
        }
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/index.html")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            to_bytes(response.into_body(), 4096).await.unwrap(),
            "static page"
        );
    }

    fn grant(state: &str, expires_at: Option<i64>) -> DeviceGrantRow {
        DeviceGrantRow {
            grant_id: "g1".into(),
            owner_user_id: "owner".into(),
            device_id: "shared-host".into(),
            grantee_user_id: "member".into(),
            scope: crate::db::GRANT_SCOPE_DRIVE.into(),
            state: state.into(),
            invite_secret_hash: None,
            offered_at: 1,
            accepted_at: Some(1),
            expires_at,
            revoked_at: None,
        }
    }

    // Issue 4: the revocation monitor must keep a legitimate owner machine
    // alive after it pushes a cross-account event, while still tearing down a
    // grantee that lost its grant and never trusting a wrong owner device.
    #[test]
    fn grant_monitor_accepts_grantee_and_bound_owner_machine_only() {
        let row = grant(crate::db::GRANT_STATE_ACTIVE, None);
        // Grantee controller socket is authorized by the grant.
        assert!(grant_authorizes_socket(
            &row,
            "member",
            "member-desk",
            Scope::User
        ));
        // Owner machine that pushed the event is authorized only on the bound device.
        assert!(grant_authorizes_socket(
            &row,
            "owner",
            "shared-host",
            Scope::Machine
        ));
        // Another owner-account device is not the shared host and never rode it.
        assert!(!grant_authorizes_socket(
            &row,
            "owner",
            "owner-laptop",
            Scope::Machine
        ));
        // Owner under a user-scoped (non-machine) socket is refused.
        assert!(!grant_authorizes_socket(
            &row,
            "owner",
            "shared-host",
            Scope::User
        ));
        // A stranger account is refused on either branch.
        assert!(!grant_authorizes_socket(
            &row,
            "outsider",
            "shared-host",
            Scope::Machine
        ));
        // Revoked and expired grants fail both branches.
        let gone = grant(crate::db::GRANT_STATE_REVOKED, None);
        assert!(!grant_authorizes_socket(
            &gone,
            "member",
            "member-desk",
            Scope::User
        ));
        assert!(!grant_authorizes_socket(
            &gone,
            "owner",
            "shared-host",
            Scope::Machine
        ));
        let expired = grant(crate::db::GRANT_STATE_ACTIVE, Some(0));
        assert!(!grant_authorizes_socket(
            &expired,
            "owner",
            "shared-host",
            Scope::Machine
        ));
    }
}
