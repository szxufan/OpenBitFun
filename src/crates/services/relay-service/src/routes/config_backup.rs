//! Token-authenticated manual config backup endpoints.
//!
//! Stores exactly one opaque, client-encrypted config export per account. The
//! relay is zero-knowledge here: it holds AES-256-GCM ciphertext, a nonce, and
//! a non-secret key hint, and never the plaintext or the backup code. Export
//! and import are user-triggered on the owning device; there is no automatic
//! synchronization.

use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, put};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};

use crate::db::account_config_backup;
use crate::routes::api::AppState;
use crate::routes::devices::validate_user;

/// Max request body for a config backup upsert. A full encrypted
/// `ConfigExport` is far below this; the explicit ceiling keeps reverse
/// proxies and operators aligned on a small, auditable limit.
pub const CONFIG_BACKUP_BODY_LIMIT: usize = 2 * 1024 * 1024;

/// Validate the bearer token and require a full device token. Delegated
/// controller tokens (mobile/IM bots) can reach device discovery and RPC but
/// must not read or replace the account's config backup.
async fn validate_device_user(state: &AppState, headers: &HeaderMap) -> Result<String, StatusCode> {
    let auth = validate_user(state, headers).await?;
    if !auth.is_device_token() {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(auth.user_id)
}

pub fn config_backup_router() -> Router<AppState> {
    Router::new()
        .route(
            "/api/account/config-backup",
            get(backup_meta).delete(backup_delete),
        )
        .route(
            "/api/account/config-backup/blob",
            get(backup_blob)
                .put(backup_upsert)
                .layer(DefaultBodyLimit::max(CONFIG_BACKUP_BODY_LIMIT)),
        )
}

#[derive(Serialize, Deserialize)]
pub struct ConfigBackupMeta {
    pub scheme: String,
    pub key_hint: String,
    pub size_bytes: i64,
    pub version: i64,
    pub updated_at: i64,
}

/// `GET /api/account/config-backup` — metadata only, no ciphertext, so the
/// importing UI can show what it would replace before downloading.
async fn backup_meta(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Option<ConfigBackupMeta>>, StatusCode> {
    let user_id = validate_device_user(&state, &headers).await?;
    let meta = account_config_backup::get_meta(state.db.as_ref(), &user_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .map(|m| ConfigBackupMeta {
            scheme: m.scheme,
            key_hint: m.key_hint,
            size_bytes: m.size_bytes,
            version: m.version,
            updated_at: m.updated_at,
        });
    Ok(Json(meta))
}

#[derive(Serialize, Deserialize)]
pub struct ConfigBackupBlob {
    pub scheme: String,
    pub key_hint: String,
    pub encrypted_data: String,
    pub nonce: String,
    pub version: i64,
    pub updated_at: i64,
}

/// `GET /api/account/config-backup/blob` — the full envelope for import.
async fn backup_blob(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<ConfigBackupBlob>, StatusCode> {
    let user_id = validate_device_user(&state, &headers).await?;
    let row = account_config_backup::get(state.db.as_ref(), &user_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::NOT_FOUND)?;
    Ok(Json(ConfigBackupBlob {
        scheme: row.scheme,
        key_hint: row.key_hint,
        encrypted_data: row.encrypted_data,
        nonce: row.nonce,
        version: row.version,
        updated_at: row.updated_at,
    }))
}

#[derive(Deserialize)]
pub struct ConfigBackupUpsertRequest {
    pub scheme: String,
    pub key_hint: String,
    pub encrypted_data: String,
    pub nonce: String,
    pub version: i64,
}

/// `PUT /api/account/config-backup/blob` — replace the account's single
/// backup row. Last manual export wins; metadata-only readers see the new
/// timestamps immediately.
async fn backup_upsert(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ConfigBackupUpsertRequest>,
) -> Result<Json<ConfigBackupMeta>, StatusCode> {
    let user_id = validate_device_user(&state, &headers).await?;
    let row = account_config_backup::upsert(
        state.db.as_ref(),
        &user_id,
        &body.scheme,
        &body.key_hint,
        &body.encrypted_data,
        &body.nonce,
        body.version,
    )
    .await
    .map_err(|error| {
        if error.to_string().contains("ciphertext") {
            StatusCode::PAYLOAD_TOO_LARGE
        } else if error.to_string().contains("invalid") || error.to_string().contains("positive") {
            StatusCode::BAD_REQUEST
        } else {
            StatusCode::INTERNAL_SERVER_ERROR
        }
    })?;
    Ok(Json(ConfigBackupMeta {
        scheme: row.scheme,
        key_hint: row.key_hint,
        size_bytes: row.size_bytes,
        version: row.version,
        updated_at: row.updated_at,
    }))
}

/// `DELETE /api/account/config-backup` — remove the stored backup. Destructive
/// by user intent only; the idempotent 204 also covers an already-empty slot.
async fn backup_delete(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<StatusCode, StatusCode> {
    let user_id = validate_device_user(&state, &headers).await?;
    account_config_backup::delete(state.db.as_ref(), &user_id)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
    Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{connect, AuthToken, DbPool, DeviceRow, UserRow};
    use axum::body::Body;
    use axum::http::{header, Request};
    use std::sync::Arc;
    use tower::ServiceExt;

    struct TestContext {
        app: axum::Router,
        db: Arc<DbPool>,
        owner_token: String,
        delegated_token: String,
        other_token: String,
    }

    async fn setup_app() -> TestContext {
        let db = Arc::new(connect(":memory:").await.unwrap());
        UserRow::create(&db, "owner", "alice").await.unwrap();
        UserRow::create(&db, "other", "bob").await.unwrap();
        DeviceRow::upsert(&db, "owner-device", "owner", "Owner", None, None)
            .await
            .unwrap();
        DeviceRow::upsert(&db, "other-device", "other", "Other", None, None)
            .await
            .unwrap();
        let owner_token = AuthToken::create(&db, "owner", "owner-device")
            .await
            .unwrap()
            .token;
        let delegated_token = AuthToken::create_delegated(&db, "owner", "owner-device")
            .await
            .unwrap()
            .token;
        let other_token = AuthToken::create(&db, "other", "other-device")
            .await
            .unwrap()
            .token;
        let app = crate::build_relay_router(
            Arc::new(crate::MemoryAssetStore::new()),
            std::time::Instant::now(),
            db.clone(),
            "test",
        );
        TestContext {
            app,
            db,
            owner_token,
            delegated_token,
            other_token,
        }
    }

    fn upsert_body(encrypted_data: &str) -> String {
        serde_json::json!({
            "scheme": "backup_code_v1",
            "key_hint": "code_v1:00112233",
            "encrypted_data": encrypted_data,
            "nonce": "aaaaaaaaaaaaaaaa",
            "version": 1_700_000_000_000_i64,
        })
        .to_string()
    }

    async fn read_json<T: serde::de::DeserializeOwned>(response: axum::response::Response) -> T {
        let bytes = axum::body::to_bytes(response.into_body(), 1024 * 1024)
            .await
            .unwrap();
        serde_json::from_slice(bytes.as_ref()).unwrap()
    }

    async fn request(
        ctx: &TestContext,
        method: &str,
        uri: &str,
        token: Option<&str>,
        body: Option<String>,
    ) -> axum::response::Response {
        let mut builder = Request::builder().method(method).uri(uri);
        if let Some(token) = token {
            builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
        }
        if body.is_some() {
            builder = builder.header(header::CONTENT_TYPE, "application/json");
        }
        ctx.app
            .clone()
            .oneshot(builder.body(Body::from(body.unwrap_or_default())).unwrap())
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn roundtrip_upsert_meta_blob_delete() {
        let ctx = setup_app().await;
        // No backup yet.
        let response = request(
            &ctx,
            "GET",
            "/api/account/config-backup",
            Some(&ctx.owner_token),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let meta: Option<ConfigBackupMeta> = read_json(response).await;
        assert!(meta.is_none());

        // Upload.
        let response = request(
            &ctx,
            "PUT",
            "/api/account/config-backup/blob",
            Some(&ctx.owner_token),
            Some(upsert_body("ZGF0YQ==")),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        // Meta reflects it without ciphertext.
        let response = request(
            &ctx,
            "GET",
            "/api/account/config-backup",
            Some(&ctx.owner_token),
            None,
        )
        .await;
        let meta: ConfigBackupMeta = read_json(response).await;
        assert_eq!(meta.scheme, "backup_code_v1");
        assert_eq!(meta.size_bytes, ("ZGF0YQ==".len() + 16) as i64);

        // Blob roundtrip.
        let response = request(
            &ctx,
            "GET",
            "/api/account/config-backup/blob",
            Some(&ctx.owner_token),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let blob: ConfigBackupBlob = read_json(response).await;
        assert_eq!(blob.encrypted_data, "ZGF0YQ==");

        // Overwrite wins.
        let response = request(
            &ctx,
            "PUT",
            "/api/account/config-backup/blob",
            Some(&ctx.owner_token),
            Some(upsert_body("TkdBVEE=")),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let response = request(
            &ctx,
            "GET",
            "/api/account/config-backup/blob",
            Some(&ctx.owner_token),
            None,
        )
        .await;
        let blob: ConfigBackupBlob = read_json(response).await;
        assert_eq!(blob.encrypted_data, "TkdBVEE=");

        // Delete, then blob is 404.
        let response = request(
            &ctx,
            "DELETE",
            "/api/account/config-backup",
            Some(&ctx.owner_token),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        let response = request(
            &ctx,
            "GET",
            "/api/account/config-backup/blob",
            Some(&ctx.owner_token),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn delegated_tokens_and_other_accounts_cannot_access_the_backup() {
        let ctx = setup_app().await;
        let response = request(
            &ctx,
            "PUT",
            "/api/account/config-backup/blob",
            Some(&ctx.owner_token),
            Some(upsert_body("ZGF0YQ==")),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);

        for (token, method, uri) in [
            (
                ctx.delegated_token.as_str(),
                "GET",
                "/api/account/config-backup",
            ),
            (
                ctx.delegated_token.as_str(),
                "GET",
                "/api/account/config-backup/blob",
            ),
            (
                ctx.delegated_token.as_str(),
                "PUT",
                "/api/account/config-backup/blob",
            ),
            (
                ctx.delegated_token.as_str(),
                "DELETE",
                "/api/account/config-backup",
            ),
        ] {
            let response = request(
                &ctx,
                method,
                uri,
                Some(token),
                if method == "PUT" {
                    Some(upsert_body("WFhY"))
                } else {
                    None
                },
            )
            .await;
            assert_eq!(response.status(), StatusCode::FORBIDDEN, "{method} {uri}");
        }

        // Missing token.
        let response = request(&ctx, "GET", "/api/account/config-backup", None, None).await;
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);

        // Another account's device token is scoped to its own (empty) slot:
        // no cross-account data and no existence leak, just its own 404/204.
        let response = request(
            &ctx,
            "GET",
            "/api/account/config-backup/blob",
            Some(&ctx.other_token),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let response = request(
            &ctx,
            "DELETE",
            "/api/account/config-backup",
            Some(&ctx.other_token),
            None,
        )
        .await;
        assert_eq!(response.status(), StatusCode::NO_CONTENT);

        // The owner's row is untouched.
        let meta = account_config_backup::get_meta(&ctx.db, "owner")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(meta.key_hint, "code_v1:00112233");
    }

    #[tokio::test]
    async fn oversized_ciphertext_is_rejected_with_413() {
        let ctx = setup_app().await;
        let body = upsert_body(&"A".repeat(crate::db::MAX_CONFIG_BACKUP_ENCRYPTED_BYTES + 1));
        let response = request(
            &ctx,
            "PUT",
            "/api/account/config-backup/blob",
            Some(&ctx.owner_token),
            Some(body),
        )
        .await;
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        assert!(account_config_backup::get_meta(&ctx.db, "owner")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn invalid_fields_are_rejected_with_400() {
        let ctx = setup_app().await;
        for (scheme, key_hint, nonce, version) in [
            ("", "code_v1:00112233", "aaaaaaaaaaaaaaaa", 1_i64),
            ("backup_code_v1", "", "aaaaaaaaaaaaaaaa", 1),
            ("backup_code_v1", "code_v1:00112233", "", 1),
            ("backup_code_v1", "code_v1:00112233", "aaaaaaaaaaaaaaaa", 0),
        ] {
            let body = serde_json::json!({
                "scheme": scheme,
                "key_hint": key_hint,
                "encrypted_data": "ZGF0YQ==",
                "nonce": nonce,
                "version": version,
            })
            .to_string();
            let response = request(
                &ctx,
                "PUT",
                "/api/account/config-backup/blob",
                Some(&ctx.owner_token),
                Some(body),
            )
            .await;
            assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        }
    }

    #[tokio::test]
    async fn deleting_a_user_removes_their_backup_row() {
        let ctx = setup_app().await;
        let response = request(
            &ctx,
            "PUT",
            "/api/account/config-backup/blob",
            Some(&ctx.owner_token),
            Some(upsert_body("ZGF0YQ==")),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        UserRow::delete(&ctx.db, "owner").await.unwrap();
        assert!(account_config_backup::get_meta(&ctx.db, "owner")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn server_info_advertises_the_config_backup_capability() {
        let ctx = setup_app().await;
        let response = request(&ctx, "GET", "/api/info", None, None).await;
        let info: serde_json::Value = read_json(response).await;
        let capabilities = info["capabilities"].as_array().unwrap();
        assert!(capabilities.iter().any(|c| c == "account_config_backup_v1"));
    }
}
