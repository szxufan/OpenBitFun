//! Cross-account device grants: one host device shared with specific grantee
//! accounts. Grants are relay-side routing metadata only — message payloads
//! stay pairwise-encrypted and the shared host remains the authoritative
//! policy point for attach, pairing, and scope.

use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::db::{
    self, hash_grant_invite_secret, DeviceGrantRow, DeviceRow, GRANT_INVITE_TTL_SECS,
    GRANT_SCOPE_DRIVE,
};
use crate::routes::api::AppState;
use crate::routes::devices::{is_valid_device_id, validate_user};

/// Upper bound on a grantee account id; they are relay identity strings, not
/// user content.
const MAX_GRANTEE_ID_BYTES: usize = 128;

fn is_valid_user_id(value: &str) -> bool {
    !value.is_empty() && value.len() <= MAX_GRANTEE_ID_BYTES
}

pub fn grants_router() -> Router<AppState> {
    Router::new()
        .route("/api/grants", get(list_grants).post(offer_grant))
        .route(
            "/api/grants/{grant_id}",
            get(grant_detail).delete(revoke_grant),
        )
        .route("/api/grants/{grant_id}/accept", post(accept_grant))
        .route("/api/grants/{grant_id}/peer-key", get(grant_peer_key))
}

/// Grant facts safe for either principal. The invite secret hash is never
/// serialized.
#[derive(Serialize)]
pub struct GrantEntry {
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

impl From<&DeviceGrantRow> for GrantEntry {
    fn from(row: &DeviceGrantRow) -> Self {
        GrantEntry {
            grant_id: row.grant_id.clone(),
            owner_user_id: row.owner_user_id.clone(),
            device_id: row.device_id.clone(),
            grantee_user_id: row.grantee_user_id.clone(),
            scope: row.scope.clone(),
            state: row.state.clone(),
            offered_at: row.offered_at,
            accepted_at: row.accepted_at,
            expires_at: row.expires_at,
            revoked_at: row.revoked_at,
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GrantOfferRequest {
    /// The shared machine itself. The route only accepts an offer whose
    /// device is the calling token's own device, so offers are minted on the
    /// machine that owns the access.
    device_id: String,
    grantee_user_id: String,
    #[serde(default = "default_scope")]
    scope: String,
    expires_at: Option<i64>,
}

fn default_scope() -> String {
    GRANT_SCOPE_DRIVE.to_string()
}

/// `POST /api/grants` — the shared machine offers a pending grant and gets
/// the one-time invite secret back exactly once. The relay stores only the
/// digest; the plaintext secret never appears in any later response.
async fn offer_grant(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Json<GrantOfferRequest>,
) -> Result<Json<GrantOfferResponse>, StatusCode> {
    let auth = validate_user(&state, &headers).await?;
    // Only the machine's own device token may offer: a grant must be decided
    // on the host that owns the access, never from another owner device.
    if !auth.is_device_token() || auth.device_id != body.device_id {
        return Err(StatusCode::FORBIDDEN);
    }
    if !is_valid_device_id(&body.device_id) || !is_valid_user_id(&body.grantee_user_id) {
        return Err(StatusCode::BAD_REQUEST);
    }
    if body.grantee_user_id == auth.user_id {
        return Err(StatusCode::BAD_REQUEST);
    }
    if !db::is_valid_grant_scope(&body.scope) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let now = chrono::Utc::now().timestamp();
    if body.expires_at.is_some_and(|expires_at| expires_at <= now) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let db_pool = state.db.as_ref();
    // The shared device must be a host row under the caller's account.
    let target = DeviceRow::find_by_user_and_device(db_pool, &auth.user_id, &body.device_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    match target.as_ref() {
        Some(row) if db::device_kind_is_host(row.device_kind.as_deref()) => {}
        _ => return Err(StatusCode::NOT_FOUND),
    }
    if db::UserRow::find_by_user_id(db_pool, &body.grantee_user_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .is_none()
    {
        return Err(StatusCode::NOT_FOUND);
    }
    let invite_secret = generate_invite_secret();
    let row = DeviceGrantRow::offer(
        db_pool,
        &auth.user_id,
        &body.device_id,
        &body.grantee_user_id,
        &body.scope,
        &hash_grant_invite_secret(&invite_secret),
        body.expires_at,
    )
    .await
    .map_err(|_| StatusCode::CONFLICT)?;
    Ok(Json(GrantOfferResponse {
        grant: GrantEntry::from(&row),
        invite_secret,
        invite_expires_at: row.offered_at + GRANT_INVITE_TTL_SECS,
    }))
}

#[derive(Serialize)]
pub struct GrantOfferResponse {
    pub grant: GrantEntry,
    pub invite_secret: String,
    pub invite_expires_at: i64,
}

fn generate_invite_secret() -> String {
    let bytes: [u8; 32] = rand::random();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GrantAcceptRequest {
    invite_secret: String,
}

/// `POST /api/grants/{id}/accept` — the grantee redeems the one-time invite.
/// The account binding comes from the grant row, never from the request; the
/// caller must be the named grantee.
async fn accept_grant(
    State(state): State<AppState>,
    Path(grant_id): Path<String>,
    headers: HeaderMap,
    body: Json<GrantAcceptRequest>,
) -> Result<Json<GrantEntry>, StatusCode> {
    let auth = validate_user(&state, &headers).await?;
    let db_pool = state.db.as_ref();
    let row = DeviceGrantRow::find(db_pool, &grant_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    if row.grantee_user_id != auth.user_id {
        // Unknown to this caller: do not reveal that the grant exists.
        return Err(StatusCode::NOT_FOUND);
    }
    let accepted = DeviceGrantRow::accept(
        db_pool,
        &grant_id,
        &auth.user_id,
        &hash_grant_invite_secret(&body.invite_secret),
    )
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    accepted
        .as_ref()
        .map(GrantEntry::from)
        .map(Json)
        .ok_or(StatusCode::FORBIDDEN)
}

#[derive(Serialize)]
pub struct SharedDeviceEntry {
    pub grant: GrantEntry,
    /// Display projection of the owner's shared machine, resolved from the
    /// owner's directory row. The grantee never sees the owner's other
    /// devices.
    pub device_name: String,
    pub device_kind: Option<String>,
    pub client_version: Option<String>,
    pub client_protocol: Option<u32>,
    pub online: bool,
    pub compatible: bool,
}

#[derive(Serialize)]
pub struct GrantListResponse {
    /// Grants the caller's account owns (management view, any state).
    pub owned: Vec<GrantEntry>,
    /// Current grants where the caller is the grantee (shared-with-me roster).
    pub received: Vec<SharedDeviceEntry>,
}

/// `GET /api/grants` — both grant views for the caller in one response.
async fn list_grants(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<GrantListResponse>, StatusCode> {
    let auth = validate_user(&state, &headers).await?;
    let db_pool = state.db.as_ref();
    let owned = DeviceGrantRow::list_for_owner(db_pool, &auth.user_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .iter()
        .map(GrantEntry::from)
        .collect();
    let caller_protocol = db::stored_client_protocol(
        sqlx::query_scalar::<_, Option<i64>>(
            "SELECT client_protocol FROM devices WHERE user_id = ? AND device_id = ?",
        )
        .bind(&auth.user_id)
        .bind(&auth.device_id)
        .fetch_optional(db_pool)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .flatten(),
    );
    let mut received = Vec::new();
    for grant in DeviceGrantRow::list_active_for_grantee(db_pool, &auth.user_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    {
        let row =
            DeviceRow::find_by_user_and_device(db_pool, &grant.owner_user_id, &grant.device_id)
                .await
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let Some(row) = row else {
            // The shared device row vanished (device deleted); the grant
            // cascade removed it too, but stay defensive for the projection.
            continue;
        };
        let target_protocol = row.client_protocol_u32();
        received.push(SharedDeviceEntry {
            grant: GrantEntry::from(&grant),
            device_name: row.device_name.clone().unwrap_or_default(),
            device_kind: row.device_kind.clone(),
            client_version: row.client_version.clone(),
            client_protocol: target_protocol,
            online: state
                .device_manager
                .is_device_online(&grant.owner_user_id, &grant.device_id),
            compatible: db::client_builds_compatible(caller_protocol, target_protocol),
        });
    }
    Ok(Json(GrantListResponse { owned, received }))
}

/// `GET /api/grants/{id}` — grant facts for either principal. A caller that
/// is neither owner nor grantee gets a 404 so grant existence stays private.
async fn grant_detail(
    State(state): State<AppState>,
    Path(grant_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<GrantEntry>, StatusCode> {
    let auth = validate_user(&state, &headers).await?;
    let row = DeviceGrantRow::find(state.db.as_ref(), &grant_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    if auth.user_id != row.owner_user_id && auth.user_id != row.grantee_user_id {
        return Err(StatusCode::NOT_FOUND);
    }
    Ok(Json(GrantEntry::from(&row)))
}

#[derive(Deserialize)]
struct PeerKeyQuery {
    device_id: String,
}

/// `GET /api/grants/{id}/peer-key?device_id=...` — public-key lookup scoped
/// to one grant, for the pairwise E2E handshake and the PSK-pairing MITM
/// check. A grantee may only resolve the shared machine; the owner side may
/// resolve any device under the grantee account. This is deliberately a
/// separate route: the account-scoped `/api/devices/{id}/key` stays untouched.
async fn grant_peer_key(
    State(state): State<AppState>,
    Path(grant_id): Path<String>,
    Query(query): Query<PeerKeyQuery>,
    headers: HeaderMap,
) -> Result<Json<crate::routes::devices::DeviceKeyResponse>, StatusCode> {
    let auth = validate_user(&state, &headers).await?;
    if !is_valid_device_id(&query.device_id) {
        return Err(StatusCode::BAD_REQUEST);
    }
    let db_pool = state.db.as_ref();
    let grant = DeviceGrantRow::find(db_pool, &grant_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    if !grant.is_current() {
        return Err(StatusCode::NOT_FOUND);
    }
    let key_owner_user_id = if auth.user_id == grant.grantee_user_id {
        // Grantee side: only the shared machine is resolvable.
        if query.device_id != grant.device_id {
            return Err(StatusCode::FORBIDDEN);
        }
        grant.owner_user_id.clone()
    } else if auth.user_id == grant.owner_user_id {
        // Owner side: any device under the grantee account.
        grant.grantee_user_id.clone()
    } else {
        return Err(StatusCode::NOT_FOUND);
    };
    let public_key = sqlx::query_scalar::<_, Option<String>>(
        "SELECT public_key FROM devices WHERE user_id = ? AND device_id = ?",
    )
    .bind(&key_owner_user_id)
    .bind(&query.device_id)
    .fetch_optional(db_pool)
    .await
    .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
    .flatten()
    .ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(crate::routes::devices::DeviceKeyResponse {
        device_id: query.device_id,
        public_key,
    }))
}

/// `DELETE /api/grants/{id}` — revoke. Any device token of the owner account
/// may revoke (fastest incident response), and so may the shared machine
/// itself. Revocation propagates to live routing through the grant monitor.
async fn revoke_grant(
    State(state): State<AppState>,
    Path(grant_id): Path<String>,
    headers: HeaderMap,
) -> Result<Json<GrantEntry>, StatusCode> {
    let auth = validate_user(&state, &headers).await?;
    let db_pool = state.db.as_ref();
    let row = DeviceGrantRow::find(db_pool, &grant_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    if auth.user_id != row.owner_user_id {
        return Err(StatusCode::NOT_FOUND);
    }
    DeviceGrantRow::revoke(db_pool, &grant_id, &auth.user_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    let updated = DeviceGrantRow::find(db_pool, &grant_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(Json(GrantEntry::from(&updated)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{build_relay_router, MemoryAssetStore};
    use axum::body::Body;
    use serde_json::{json, Value};
    use std::sync::Arc;
    use tower::ServiceExt;

    async fn post(
        app: &axum::Router,
        path: &str,
        token: &str,
        body: Value,
    ) -> axum::response::Response {
        app.clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("POST")
                    .uri(path)
                    .header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap()
    }
    async fn get(app: &axum::Router, path: &str, token: &str) -> axum::response::Response {
        app.clone()
            .oneshot(
                axum::http::Request::builder()
                    .uri(path)
                    .header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }
    async fn delete(app: &axum::Router, path: &str, token: &str) -> axum::response::Response {
        app.clone()
            .oneshot(
                axum::http::Request::builder()
                    .method("DELETE")
                    .uri(path)
                    .header(axum::http::header::AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap()
    }
    async fn body(response: axum::response::Response) -> Value {
        serde_json::from_slice(
            &axum::body::to_bytes(response.into_body(), 1_000_000)
                .await
                .unwrap(),
        )
        .unwrap()
    }

    struct Fixture {
        app: axum::Router,
        db: Arc<crate::db::DbPool>,
        machine_token: String,
        owner_other_token: String,
        member_token: String,
        outsider_token: String,
        owner_id: String,
        member_id: String,
    }

    async fn fixture() -> Fixture {
        let db = Arc::new(crate::db::connect(":memory:").await.unwrap());
        let app = build_relay_router(
            Arc::new(MemoryAssetStore::new()),
            std::time::Instant::now(),
            db.clone(),
            "test",
        );
        let mut users = Vec::new();
        for phone in ["13900000001", "13900000002", "13900000003"] {
            users.push(
                crate::db::UserRow::create_local_with_phone(&db, phone)
                    .await
                    .unwrap(),
            );
        }
        let mut tokens = Vec::new();
        for (user, device) in [
            (&users[0], "shared-host"),
            (&users[0], "owner-laptop"),
            (&users[1], "member-desk"),
            (&users[2], "outsider-desk"),
        ] {
            crate::db::DeviceRow::upsert(
                &db,
                device,
                &user.user_id,
                device,
                Some("desktop"),
                Some("test-public-key"),
            )
            .await
            .unwrap();
            tokens.push(
                crate::db::AuthToken::create(&db, &user.user_id, device)
                    .await
                    .unwrap()
                    .token,
            );
        }
        Fixture {
            app,
            db,
            machine_token: tokens[0].clone(),
            owner_other_token: tokens[1].clone(),
            member_token: tokens[2].clone(),
            outsider_token: tokens[3].clone(),
            owner_id: users[0].user_id.clone(),
            member_id: users[1].user_id.clone(),
        }
    }

    async fn offer(fx: &Fixture, token: &str, device: &str, grantee: &str) -> Value {
        body(
            post(
                &fx.app,
                "/api/grants",
                token,
                json!({"device_id":device, "grantee_user_id":grantee, "scope":"drive"}),
            )
            .await,
        )
        .await
    }

    #[tokio::test]
    async fn offers_require_the_shared_machines_own_token() {
        let fx = fixture().await;
        // Another owner-account device cannot offer for the machine.
        let response = post(
            &fx.app,
            "/api/grants",
            &fx.owner_other_token,
            json!({"device_id":"shared-host","grantee_user_id":fx.member_id,"scope":"drive"}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        // The grantee cannot offer either.
        let response = post(
            &fx.app,
            "/api/grants",
            &fx.member_token,
            json!({"device_id":"shared-host","grantee_user_id":fx.member_id,"scope":"drive"}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        // Self-grants and unknown grantees are rejected.
        for (grantee, status) in [
            (fx.owner_id.clone(), StatusCode::BAD_REQUEST),
            ("missing-user".to_string(), StatusCode::NOT_FOUND),
        ] {
            let response = post(
                &fx.app,
                "/api/grants",
                &fx.machine_token,
                json!({"device_id":"shared-host","grantee_user_id":grantee,"scope":"drive"}),
            )
            .await;
            assert_eq!(response.status(), status);
        }
        let response = post(
            &fx.app,
            "/api/grants",
            &fx.machine_token,
            json!({"device_id":"shared-host","grantee_user_id":fx.member_id,"scope":"full-admin"}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn accept_is_one_time_secret_and_grantee_bound() {
        let fx = fixture().await;
        let offered = offer(&fx, &fx.machine_token, "shared-host", &fx.member_id).await;
        let grant_id = offered["grant"]["grant_id"].as_str().unwrap().to_string();
        let secret = offered["invite_secret"].as_str().unwrap().to_string();
        // A wrong secret is rejected and does not consume the invite.
        let response = post(
            &fx.app,
            &format!("/api/grants/{grant_id}/accept"),
            &fx.member_token,
            json!({"invite_secret":"0".repeat(64)}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
        // A non-grantee does not even learn the grant exists.
        let response = post(
            &fx.app,
            &format!("/api/grants/{grant_id}/accept"),
            &fx.outsider_token,
            json!({"invite_secret":secret}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        // The named grantee redeems successfully.
        let response = post(
            &fx.app,
            &format!("/api/grants/{grant_id}/accept"),
            &fx.member_token,
            json!({"invite_secret":secret}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body(response).await["state"], "active");
        // Replaying the same secret cannot activate twice.
        let response = post(
            &fx.app,
            &format!("/api/grants/{grant_id}/accept"),
            &fx.member_token,
            json!({"invite_secret":secret}),
        )
        .await;
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn one_active_grant_per_pair() {
        let fx = fixture().await;
        let first = offer(&fx, &fx.machine_token, "shared-host", &fx.member_id).await;
        let second = offer(&fx, &fx.machine_token, "shared-host", &fx.member_id).await;
        for offered in [&first, &second] {
            let grant_id = offered["grant"]["grant_id"].as_str().unwrap();
            let response = post(
                &fx.app,
                &format!("/api/grants/{grant_id}/accept"),
                &fx.member_token,
                json!({"invite_secret":offered["invite_secret"]}),
            )
            .await;
            // Exactly one of the two accepts succeeds; the active-pair
            // unique index fails the second whichever order they land in.
            assert!(
                response.status() == StatusCode::OK || response.status() == StatusCode::FORBIDDEN
            );
        }
        let active = crate::db::DeviceGrantRow::find_active(
            &fx.db,
            &fx.owner_id,
            "shared-host",
            &fx.member_id,
        )
        .await
        .unwrap();
        assert!(active.is_some());
    }

    #[tokio::test]
    async fn list_scopes_owner_and_grantee_views() {
        let fx = fixture().await;
        let offered = offer(&fx, &fx.machine_token, "shared-host", &fx.member_id).await;
        let grant_id = offered["grant"]["grant_id"].as_str().unwrap().to_string();
        post(
            &fx.app,
            &format!("/api/grants/{grant_id}/accept"),
            &fx.member_token,
            json!({"invite_secret":offered["invite_secret"]}),
        )
        .await;
        let member_list = body(get(&fx.app, "/api/grants", &fx.member_token).await).await;
        assert_eq!(member_list["received"].as_array().unwrap().len(), 1);
        let entry = &member_list["received"][0];
        assert_eq!(entry["grant"]["device_id"], "shared-host");
        assert_eq!(entry["grant"]["grantee_user_id"], fx.member_id);
        assert_eq!(entry["device_kind"], "desktop");
        assert_eq!(entry["online"], false);
        // The grantee sees no owned grants; the owner sees the one it offered.
        assert_eq!(member_list["owned"].as_array().unwrap().len(), 0);
        let owner_list = body(get(&fx.app, "/api/grants", &fx.machine_token).await).await;
        assert_eq!(owner_list["owned"].as_array().unwrap().len(), 1);
        assert_eq!(owner_list["received"].as_array().unwrap().len(), 0);
        // A bystander account sees neither view.
        let outsider_list = body(get(&fx.app, "/api/grants", &fx.outsider_token).await).await;
        assert_eq!(outsider_list["owned"].as_array().unwrap().len(), 0);
        assert_eq!(outsider_list["received"].as_array().unwrap().len(), 0);
    }

    #[tokio::test]
    async fn peer_key_is_grant_scoped() {
        let fx = fixture().await;
        let offered = offer(&fx, &fx.machine_token, "shared-host", &fx.member_id).await;
        let grant_id = offered["grant"]["grant_id"].as_str().unwrap().to_string();
        post(
            &fx.app,
            &format!("/api/grants/{grant_id}/accept"),
            &fx.member_token,
            json!({"invite_secret":offered["invite_secret"]}),
        )
        .await;
        // The grantee resolves exactly the shared machine, never another
        // owner device and never through the account-scoped directory.
        assert_eq!(
            get(
                &fx.app,
                &format!("/api/grants/{grant_id}/peer-key?device_id=shared-host"),
                &fx.member_token
            )
            .await
            .status(),
            StatusCode::OK
        );
        assert_eq!(
            get(
                &fx.app,
                &format!("/api/grants/{grant_id}/peer-key?device_id=owner-laptop"),
                &fx.member_token
            )
            .await
            .status(),
            StatusCode::FORBIDDEN
        );
        // The owner resolves the grantee's controller device for reply keys.
        assert_eq!(
            get(
                &fx.app,
                &format!("/api/grants/{grant_id}/peer-key?device_id=member-desk"),
                &fx.machine_token
            )
            .await
            .status(),
            StatusCode::OK
        );
        // Outsiders get nothing.
        assert_eq!(
            get(
                &fx.app,
                &format!("/api/grants/{grant_id}/peer-key?device_id=shared-host"),
                &fx.outsider_token
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            get(&fx.app, "/api/devices/shared-host/key", &fx.member_token)
                .await
                .status(),
            StatusCode::NOT_FOUND,
            "the account-scoped directory stays closed to grantees"
        );
    }

    #[tokio::test]
    async fn revoke_removes_roster_and_hides_detail() {
        let fx = fixture().await;
        let offered = offer(&fx, &fx.machine_token, "shared-host", &fx.member_id).await;
        let grant_id = offered["grant"]["grant_id"].as_str().unwrap().to_string();
        post(
            &fx.app,
            &format!("/api/grants/{grant_id}/accept"),
            &fx.member_token,
            json!({"invite_secret":offered["invite_secret"]}),
        )
        .await;
        // The grantee cannot revoke; the 404 hides the grant's existence.
        assert_eq!(
            delete(
                &fx.app,
                &format!("/api/grants/{grant_id}"),
                &fx.member_token
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
        // Any owner-account device may revoke (incident response).
        let response = delete(
            &fx.app,
            &format!("/api/grants/{grant_id}"),
            &fx.owner_other_token,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(body(response).await["state"], "revoked");
        let member_list = body(get(&fx.app, "/api/grants", &fx.member_token).await).await;
        assert_eq!(member_list["received"].as_array().unwrap().len(), 0);
        assert_eq!(
            get(
                &fx.app,
                &format!("/api/grants/{grant_id}/peer-key?device_id=shared-host"),
                &fx.member_token
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
    }
}
