//! Administrative listing, provisioning, and explicit deletion of relay-local
//! account records.

use crate::db::{self, DbPool, DeviceGrantRow, LocalAuthToken, UserRow};
use anyhow::{anyhow, Result};

const MIN_PASSWORD_BYTES: usize = 8;
const MAX_PASSWORD_BYTES: usize = 128;

pub struct LocalUserSummary {
    pub username: String,
    pub user_id: String,
    pub phone_masked: Option<String>,
    pub created_at: i64,
}

/// List all accounts with their (masked) phone binding.
pub async fn list_users(pool: &DbPool) -> Result<Vec<LocalUserSummary>> {
    let rows = sqlx::query_as::<_, (String, String, Option<String>, i64)>(
        "SELECT u.username, u.user_id, p.phone, u.created_at \
         FROM users u LEFT JOIN phone_identities p ON p.user_id = u.user_id \
         ORDER BY u.created_at",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| anyhow!("list users: {e}"))?;
    Ok(rows
        .into_iter()
        .map(|(username, user_id, phone, created_at)| LocalUserSummary {
            username,
            user_id,
            phone_masked: phone.as_deref().map(db::mask_phone),
            created_at,
        })
        .collect())
}

/// Delete an account and all associated data.
pub async fn delete_user(pool: &DbPool, username: &str) -> Result<()> {
    let user = UserRow::find_by_username(pool, username)
        .await?
        .ok_or_else(|| anyhow!("username '{username}' not found"))?;
    UserRow::delete(pool, &user.user_id).await?;
    Ok(())
}

/// Delete the account bound to a phone number.
pub async fn delete_user_by_phone(pool: &DbPool, raw_phone: &str) -> Result<()> {
    let phone = db::normalize_phone(raw_phone).ok_or_else(|| anyhow!("invalid phone number"))?;
    let user = UserRow::find_by_phone(pool, &phone)
        .await?
        .ok_or_else(|| anyhow!("no account registered for {}", db::mask_phone(&phone)))?;
    UserRow::delete(pool, &user.user_id).await?;
    Ok(())
}

/// Create a local account with an operator-verified phone. Intended for
/// relays running with registration closed; open relays self-register through
/// the SMS-code flow.
pub async fn add_local_user(
    pool: &DbPool,
    raw_phone: &str,
    password: Option<&str>,
) -> Result<String> {
    let phone = db::normalize_phone(raw_phone).ok_or_else(|| anyhow!("invalid phone number"))?;
    if UserRow::find_by_phone(pool, &phone).await?.is_some() {
        return Err(anyhow!(
            "an account is already registered for {}",
            db::mask_phone(&phone)
        ));
    }
    let user = UserRow::create_local_with_phone(pool, &phone).await?;
    if let Some(password) = password {
        set_account_password(pool, &user.user_id, password).await?;
    }
    Ok(user.user_id)
}

/// Set or replace the optional password of the account bound to a phone.
/// Revokes every local sign-in token for that account.
pub async fn set_password(pool: &DbPool, raw_phone: &str, password: &str) -> Result<()> {
    let phone = db::normalize_phone(raw_phone).ok_or_else(|| anyhow!("invalid phone number"))?;
    let user = UserRow::find_by_phone(pool, &phone)
        .await?
        .ok_or_else(|| anyhow!("no account registered for {}", db::mask_phone(&phone)))?;
    set_account_password(pool, &user.user_id, password).await
}

pub struct DeviceGrantSummary {
    pub grant_id: String,
    pub owner_user_id: String,
    pub device_id: String,
    pub grantee_user_id: String,
    pub scope: String,
    pub state: String,
    pub accepted_at: Option<i64>,
}

/// List every device grant, any state — the operator's incident-response view.
pub async fn list_device_grants(pool: &DbPool) -> Result<Vec<DeviceGrantSummary>> {
    let rows = sqlx::query_as::<_, DeviceGrantRow>(
        "SELECT grant_id, owner_user_id, device_id, grantee_user_id, scope, state, invite_secret_hash, offered_at, accepted_at, expires_at, revoked_at \
         FROM device_grants ORDER BY offered_at DESC",
    )
    .fetch_all(pool)
    .await
    .map_err(|e| anyhow!("list device grants: {e}"))?;
    Ok(rows
        .into_iter()
        .map(|row| DeviceGrantSummary {
            grant_id: row.grant_id,
            owner_user_id: row.owner_user_id,
            device_id: row.device_id,
            grantee_user_id: row.grantee_user_id,
            scope: row.scope,
            state: row.state,
            accepted_at: row.accepted_at,
        })
        .collect())
}

/// Force-revoke a grant by id. Unlike the owner-facing route, this operator
/// action does not require the caller to be the owning account; live routing
/// still tears down through the grant monitor within one tick.
pub async fn revoke_device_grant(pool: &DbPool, grant_id: &str) -> Result<bool> {
    let revoked_at = chrono::Utc::now().timestamp();
    let result = sqlx::query(
        "UPDATE device_grants \
           SET state = 'revoked', revoked_at = ?, invite_secret_hash = NULL \
         WHERE grant_id = ? AND state != 'revoked'",
    )
    .bind(revoked_at)
    .bind(grant_id)
    .execute(pool)
    .await
    .map_err(|e| anyhow!("revoke device grant: {e}"))?;
    Ok(result.rows_affected() > 0)
}

async fn set_account_password(pool: &DbPool, user_id: &str, password: &str) -> Result<()> {
    let password = password.trim();
    if password.len() < MIN_PASSWORD_BYTES || password.len() > MAX_PASSWORD_BYTES {
        return Err(anyhow!(
            "password must be {MIN_PASSWORD_BYTES}-{MAX_PASSWORD_BYTES} bytes"
        ));
    }
    let hash = crate::routes::auth::hash_password(password)
        .ok_or_else(|| anyhow!("password hashing failed"))?;
    UserRow::set_password_hash(pool, user_id, &hash).await?;
    LocalAuthToken::revoke_all_for_user(pool, user_id).await?;
    Ok(())
}
