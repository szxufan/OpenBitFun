//! Local account sign-in (SMS verification codes, optional password) and the
//! keyed device-session exchange for versioned OpenBitFun Relay clients.

use axum::extract::{ConnectInfo, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::{Extension, Json};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use chrono::Utc;
use dashmap::DashMap;
use serde::{Deserialize, Serialize};
use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};

use crate::db::{
    AuthToken, DeviceMetadata, DeviceRow, LocalAuthToken, SmsCode, SmsCodeCheck, SmsSend, UserRow,
};
use crate::routes::api::AppState;
use crate::sms;

/// Max login attempts per IP per minute (across all accounts — stops
/// credential-stuffing where one IP tries many usernames).
const MAX_LOGIN_ATTEMPTS_PER_MIN: usize = 10;
/// Max SMS send requests per IP per minute.
const MAX_SMS_SEND_PER_MIN: usize = 5;
/// Max local refresh rotations per IP per minute.
const MAX_LOCAL_REFRESH_PER_MIN: usize = 30;
/// Max challenge requests per IP per minute (stops bulk salt harvesting).
const MAX_RATE_LIMIT_BUCKETS: usize = 50_000;
const MAX_DEVICE_ID_BYTES: usize = 128;
const MAX_DEVICE_NAME_BYTES: usize = 256;
const MIN_PASSWORD_BYTES: usize = 8;
const MAX_PASSWORD_BYTES: usize = 128;

/// Operator registration gate: `RELAY_LOCAL_REGISTRATION=closed` limits
/// sign-in to phone numbers that already own an account.
pub const ENV_LOCAL_REGISTRATION: &str = "RELAY_LOCAL_REGISTRATION";

/// Everything the local-auth routes need from the operator environment, in
/// one object so tests can inject a fully controlled copy through an
/// `Extension`. Production resolves it once per process.
pub struct LocalAuthEnvironment {
    /// Aliyun delivery backend. `None` disables code sign-in with an explicit
    /// 503; password login keeps working.
    pub transport: Option<Arc<dyn sms::SmsTransport>>,
    /// HMAC key for code digests. `None` together with a transport is a
    /// configuration error and also fails closed.
    pub code_secret: Option<String>,
    pub registration_closed: bool,
}

impl LocalAuthEnvironment {
    fn from_env() -> Self {
        let secret = sms::code_secret().cloned();
        let transport = sms::SmsConfig::from_env()
            .filter(|_| secret.is_some())
            .map(|config| {
                Arc::new(sms::AliyunSmsTransport::new(config)) as Arc<dyn sms::SmsTransport>
            });
        Self {
            transport,
            code_secret: secret,
            registration_closed: std::env::var(ENV_LOCAL_REGISTRATION)
                .map(|value| value.trim().eq_ignore_ascii_case("closed"))
                .unwrap_or(false),
        }
    }
}

fn local_auth(injected: Option<Extension<Arc<LocalAuthEnvironment>>>) -> Arc<LocalAuthEnvironment> {
    static ENV: OnceLock<Arc<LocalAuthEnvironment>> = OnceLock::new();
    injected
        .map(|environment| environment.0)
        .unwrap_or_else(|| {
            ENV.get_or_init(|| Arc::new(LocalAuthEnvironment::from_env()))
                .clone()
        })
}

fn valid_bounded_text(value: &str, max_bytes: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max_bytes && !value.chars().any(char::is_control)
}

fn valid_device_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_DEVICE_ID_BYTES
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn valid_login_request_id(value: &str) -> bool {
    uuid::Uuid::parse_str(value).is_ok()
}

/// Absent is legal — clients that predate the field still log in, and their
/// stored kind is left untouched rather than overwritten with a guess.
fn valid_optional_device_kind(value: Option<&str>) -> bool {
    value.is_none_or(crate::db::is_valid_device_kind)
}

// ── IP rate limiter (sliding window, in-memory) ─────────────────────────

/// Per-IP sliding-window rate limiter. In-memory only; resets on restart,
/// which is acceptable for brute-force throttling (the account lockout in the
/// DB is the durable backstop).
pub struct LoginRateLimiter {
    attempts: DashMap<String, Vec<RateLimitAttempt>>,
}

struct RateLimitAttempt {
    timestamp: i64,
    replay_key: Option<String>,
}

impl LoginRateLimiter {
    pub fn new() -> Self {
        Self {
            attempts: DashMap::new(),
        }
    }

    /// Record an attempt for one rate-limit scope and return `true` if the IP
    /// is still under the per-minute limit. An exact replay key is counted once
    /// so an ambiguous idempotent response cannot consume the full login budget.
    pub(crate) fn check_and_record(
        &self,
        scope: &str,
        ip: &str,
        max_per_min: usize,
        replay_key: Option<&str>,
    ) -> bool {
        let now = Utc::now().timestamp();
        let cutoff = now - 60;
        let bucket_key = format!("{scope}:{ip}");
        if self.attempts.len() >= MAX_RATE_LIMIT_BUCKETS && !self.attempts.contains_key(&bucket_key)
        {
            self.attempts.retain(|_, timestamps| {
                timestamps.retain(|attempt| attempt.timestamp > cutoff);
                !timestamps.is_empty()
            });
            if self.attempts.len() >= MAX_RATE_LIMIT_BUCKETS {
                return false;
            }
        }
        let mut entry = self.attempts.entry(bucket_key).or_default();
        let timestamps = entry.value_mut();
        timestamps.retain(|attempt| attempt.timestamp > cutoff);
        if replay_key.is_some_and(|key| {
            timestamps
                .iter()
                .any(|attempt| attempt.replay_key.as_deref() == Some(key))
        }) {
            return true;
        }
        if timestamps.len() >= max_per_min {
            return false;
        }
        timestamps.push(RateLimitAttempt {
            timestamp: now,
            replay_key: replay_key.map(str::to_string),
        });
        true
    }
}

impl Default for LoginRateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

/// Extract the client IP from `X-Forwarded-For` (first hop) or fall back to a
/// static bucket so all headerless requests share one limiter entry.
pub(crate) fn client_ip(headers: &HeaderMap, peer_addr: Option<SocketAddr>) -> String {
    let Some(peer_addr) = peer_addr else {
        return "unknown".to_string();
    };

    // Forwarded headers are caller-controlled unless the immediate peer is a
    // local reverse proxy. Parse the value as an IP as well, so arbitrary
    // strings cannot create unbounded rate-limit buckets.
    if peer_addr.ip().is_loopback() {
        if let Some(forwarded_ip) = headers
            .get("x-forwarded-for")
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.split(',').next())
            .map(str::trim)
            .and_then(|value| value.parse::<std::net::IpAddr>().ok())
        {
            return forwarded_ip.to_string();
        }
    }

    peer_addr.ip().to_string()
}

// ── Request / response types ────────────────────────────────────────────

#[derive(Serialize)]
pub struct AuthResponse {
    pub token: String,
    pub user_id: String,
}

#[derive(Deserialize)]
pub struct LoginRequest {
    pub access_token: String,
    pub device_id: String,
    pub device_name: String,
    pub device_kind: String,
    pub public_key: String,
    pub request_id: String,
    #[serde(default)]
    pub device_model: Option<String>,
    #[serde(default)]
    pub device_os: Option<String>,
    #[serde(default)]
    pub device_os_version: Option<String>,
    // The client build is reported camelCase, matching the realtime handshake
    // and the client's login body, while the rest of this request stays
    // snake_case for historical compatibility.
    #[serde(default, rename = "clientVersion")]
    pub client_version: Option<String>,
    #[serde(default, rename = "clientProtocol")]
    pub client_protocol: Option<u32>,
}

#[derive(Deserialize)]
pub struct ProvisionDeviceRequest {
    pub public_key: String,
    pub device_id: String,
    pub device_name: String,
    #[serde(default)]
    pub device_kind: Option<String>,
    pub request_id: String,
    #[serde(default)]
    pub device_model: Option<String>,
    #[serde(default)]
    pub device_os: Option<String>,
    #[serde(default)]
    pub device_os_version: Option<String>,
}

#[derive(Serialize, Deserialize)]
pub struct ProvisionDeviceResponse {
    pub token: String,
    pub user_id: String,
    pub device_id: String,
}

#[derive(Serialize)]
pub struct ErrorResponse {
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_secs: Option<i64>,
}

fn err(error: &str, status: StatusCode) -> (StatusCode, Json<ErrorResponse>) {
    (
        status,
        Json(ErrorResponse {
            error: error.to_string(),
            retry_after_secs: None,
        }),
    )
}

fn err_retry(
    error: &str,
    status: StatusCode,
    retry_after_secs: i64,
) -> (StatusCode, Json<ErrorResponse>) {
    (
        status,
        Json(ErrorResponse {
            error: error.to_string(),
            retry_after_secs: Some(retry_after_secs),
        }),
    )
}

// ── Local account: password hashing ─────────────────────────────────────

/// Argon2id PHC string. SMS-code accounts may never set a password; those
/// that do store only this digest. Shared with relay-admin provisioning.
pub(crate) fn hash_password(password: &str) -> Option<String> {
    use argon2::password_hash::{PasswordHasher, SaltString};
    let salt: [u8; 16] = rand::random();
    let salt = SaltString::encode_b64(&salt).ok()?;
    argon2::Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .ok()
        .map(|hash| hash.to_string())
}

fn verify_password(stored_phc: &str, password: &str) -> bool {
    use argon2::password_hash::{PasswordHash, PasswordVerifier};
    PasswordHash::new(stored_phc).ok().is_some_and(|parsed| {
        argon2::Argon2::default()
            .verify_password(password.as_bytes(), &parsed)
            .is_ok()
    })
}

/// Run one Argon2 verification against a throwaway hash so "no such account"
/// and "wrong password" cost the same time. The digest is generated once per
/// process; its plaintext is irrelevant.
fn burn_password_verify(password: &str) {
    static DUMMY: OnceLock<String> = OnceLock::new();
    let dummy = DUMMY.get_or_init(|| hash_password("timing-equalizer").unwrap_or_default());
    if !dummy.is_empty() {
        let _ = verify_password(dummy, password);
    }
}

// ── Local account: request / response types ─────────────────────────────

#[derive(Deserialize)]
pub struct SmsSendRequest {
    pub phone: String,
}

#[derive(Deserialize)]
pub struct LocalLoginRequest {
    pub phone: String,
    #[serde(default)]
    pub code: Option<String>,
    #[serde(default)]
    pub password: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalUserView {
    pub user_id: String,
    pub login: String,
    pub phone_masked: String,
    pub has_password: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalLoginResponse {
    pub access_token: String,
    pub access_expires_at: i64,
    pub refresh_token: String,
    pub refresh_expires_at: i64,
    pub user: LocalUserView,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalRefreshRequest {
    pub refresh_token: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SetPasswordRequest {
    pub new_password: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SmsSendResponse {
    pub status: &'static str,
}

fn local_user_view(user: &UserRow, phone_masked: String) -> LocalUserView {
    LocalUserView {
        user_id: user.user_id.clone(),
        login: user.username.clone(),
        phone_masked,
        has_password: user.password_hash.is_some(),
    }
}

/// `POST /api/auth/local/sms/send` — deliver a verification code to a phone.
///
/// The response never reveals whether the phone owns an account. Rate and
/// cooldown rejections are the only distinguishable outcomes, and they apply
/// identically to known and unknown numbers.
pub(crate) async fn local_sms_send(
    State(state): State<AppState>,
    connect_info: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    environment: Option<Extension<Arc<LocalAuthEnvironment>>>,
    Json(body): Json<SmsSendRequest>,
) -> Result<(StatusCode, Json<SmsSendResponse>), (StatusCode, Json<ErrorResponse>)> {
    let ip = client_ip(
        &headers,
        connect_info.map(|Extension(ConnectInfo(addr))| addr),
    );
    if !state
        .login_rate_limiter
        .check_and_record("sms-send", &ip, MAX_SMS_SEND_PER_MIN, None)
    {
        return Err(err_retry(
            "too many code requests",
            StatusCode::TOO_MANY_REQUESTS,
            60,
        ));
    }
    let Some(phone) = crate::db::normalize_phone(&body.phone) else {
        return Err(err("invalid phone number", StatusCode::BAD_REQUEST));
    };
    let db = state.db.as_ref();
    if SmsSend::sent_within(db, &phone, crate::db::SMS_RESEND_COOLDOWN_SECS)
        .await
        .unwrap_or(true)
    {
        return Err(err_retry(
            "please wait before requesting another code",
            StatusCode::TOO_MANY_REQUESTS,
            crate::db::SMS_RESEND_COOLDOWN_SECS,
        ));
    }
    let day_start = Utc::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .map(|midnight| midnight.and_utc().timestamp())
        .unwrap_or_else(|| Utc::now().timestamp());
    let sent_today = SmsSend::count_since(db, &phone, day_start)
        .await
        .unwrap_or(i64::MAX);
    if sent_today >= crate::db::SMS_DAILY_CAP_PER_PHONE {
        return Err(err_retry(
            "daily code limit reached",
            StatusCode::TOO_MANY_REQUESTS,
            3600,
        ));
    }
    let environment = local_auth(environment);
    let Some(transport) = environment.transport.clone() else {
        return Err(err(
            "sms delivery is not configured",
            StatusCode::SERVICE_UNAVAILABLE,
        ));
    };
    let Some(secret) = environment.code_secret.clone() else {
        tracing::error!("SMS transport resolved without a code secret");
        return Err(err(
            "sms delivery is not configured",
            StatusCode::SERVICE_UNAVAILABLE,
        ));
    };
    let code = sms::generate_code();
    let digest = sms::code_digest(&secret, &phone, &code);
    let expires_at = Utc::now().timestamp() + crate::db::SMS_CODE_TTL_SECS;
    if let Err(error) = SmsCode::replace(db, &phone, &digest, expires_at).await {
        tracing::error!("Failed to store SMS code: {error}");
        return Err(err("internal error", StatusCode::INTERNAL_SERVER_ERROR));
    }
    if let Err(error) = transport.send(&phone, &code).await {
        tracing::warn!(
            "SMS delivery to {} failed: {error}",
            crate::db::mask_phone(&phone)
        );
        return Err(err("code delivery failed", StatusCode::BAD_GATEWAY));
    }
    if let Err(error) = SmsSend::record(db, &phone).await {
        tracing::warn!("Failed to record SMS send: {error}");
    }
    tracing::info!(
        phone = %crate::db::mask_phone(&phone),
        "Verification code sent for local sign-in"
    );
    Ok((
        StatusCode::ACCEPTED,
        Json(SmsSendResponse { status: "sent" }),
    ))
}

/// `POST /api/auth/local/login` — sign in with an SMS code (registering the
/// phone on first use when registration is open) or, for accounts that set
/// one, with the optional password. Exactly one of `code`/`password` must be
/// present. Both paths return the same local token pair shape.
pub(crate) async fn local_login(
    State(state): State<AppState>,
    connect_info: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    environment: Option<Extension<Arc<LocalAuthEnvironment>>>,
    Json(body): Json<LocalLoginRequest>,
) -> Result<Json<LocalLoginResponse>, (StatusCode, Json<ErrorResponse>)> {
    let ip = client_ip(
        &headers,
        connect_info.map(|Extension(ConnectInfo(addr))| addr),
    );
    if !state.login_rate_limiter.check_and_record(
        "local-login",
        &ip,
        MAX_LOGIN_ATTEMPTS_PER_MIN,
        None,
    ) {
        return Err(err(
            "too many login attempts",
            StatusCode::TOO_MANY_REQUESTS,
        ));
    }
    let Some(phone) = crate::db::normalize_phone(&body.phone) else {
        return Err(err("invalid phone number", StatusCode::BAD_REQUEST));
    };
    let environment = local_auth(environment);
    let db = state.db.as_ref();
    let user = match (body.code.as_deref(), body.password.as_deref()) {
        (Some(code), None) => sms_code_login(db, &environment, &phone, code).await?,
        (None, Some(password)) => password_login(db, &phone, password).await?,
        _ => {
            return Err(err(
                "provide exactly one of code or password",
                StatusCode::BAD_REQUEST,
            ))
        }
    };
    // An unknown phone only reaches this point through the code path (which
    // registers it when allowed); the password path resolves an existing row.
    let user = match user {
        Some(user) => user,
        None => {
            if environment.registration_closed {
                return Err(err(
                    "registration is closed on this relay",
                    StatusCode::FORBIDDEN,
                ));
            }
            UserRow::create_local_with_phone(db, &phone)
                .await
                .map_err(|error| {
                    tracing::error!("Local account creation failed: {error}");
                    err("internal error", StatusCode::INTERNAL_SERVER_ERROR)
                })?
        }
    };
    let pair = LocalAuthToken::issue_pair(db, &user.user_id)
        .await
        .map_err(|error| {
            tracing::error!("Local token issue failed: {error}");
            err("internal error", StatusCode::INTERNAL_SERVER_ERROR)
        })?;
    let masked = crate::db::mask_phone(&phone);
    tracing::info!(user_id = %user.user_id, "Local account sign-in");
    Ok(Json(LocalLoginResponse {
        access_token: pair.access_token,
        access_expires_at: pair.access_expires_at,
        refresh_token: pair.refresh_token,
        refresh_expires_at: pair.refresh_expires_at,
        user: local_user_view(&user, masked),
    }))
}

/// Returns `None` when the phone is verified but unregistered — the caller
/// decides whether registration may create the account.
async fn sms_code_login(
    db: &sqlx::SqlitePool,
    environment: &LocalAuthEnvironment,
    phone: &str,
    code: &str,
) -> Result<Option<UserRow>, (StatusCode, Json<ErrorResponse>)> {
    if code.len() != 6 || !code.bytes().all(|b| b.is_ascii_digit()) {
        return Err(err("invalid verification code", StatusCode::UNAUTHORIZED));
    }
    let Some(secret) = environment.code_secret.as_deref() else {
        return Err(err(
            "sms delivery is not configured",
            StatusCode::SERVICE_UNAVAILABLE,
        ));
    };
    let digest = sms::code_digest(secret, phone, code);
    match SmsCode::check_and_consume(db, phone, &digest)
        .await
        .map_err(|error| {
            tracing::error!("SMS code check failed: {error}");
            err("internal error", StatusCode::INTERNAL_SERVER_ERROR)
        })? {
        SmsCodeCheck::Accepted => UserRow::find_by_phone(db, phone).await.map_err(|error| {
            tracing::error!("Phone lookup failed: {error}");
            err("internal error", StatusCode::INTERNAL_SERVER_ERROR)
        }),
        SmsCodeCheck::Rejected { attempts_left } => {
            Err(err("verification code incorrect", StatusCode::UNAUTHORIZED)).map_err(
                |(status, mut body)| {
                    body.retry_after_secs = Some(attempts_left.max(0));
                    (status, body)
                },
            )
        }
        SmsCodeCheck::Denied => Err(err(
            "verification code invalid or expired",
            StatusCode::UNAUTHORIZED,
        )),
    }
}

/// Password login for accounts that opted into one. Unknown phone, missing
/// password, and wrong password share one response so the endpoint cannot be
/// used to enumerate registered numbers.
async fn password_login(
    db: &sqlx::SqlitePool,
    phone: &str,
    password: &str,
) -> Result<Option<UserRow>, (StatusCode, Json<ErrorResponse>)> {
    let generic = || err("invalid phone or password", StatusCode::UNAUTHORIZED);
    let Some(user) = UserRow::find_by_phone(db, phone).await.map_err(|error| {
        tracing::error!("Phone lookup failed: {error}");
        err("internal error", StatusCode::INTERNAL_SERVER_ERROR)
    })?
    else {
        // Burn comparable CPU so timing cannot separate "no account" from
        // "wrong password".
        burn_password_verify(password);
        return Err(generic());
    };
    let now = Utc::now().timestamp();
    if user.locked_until > now {
        return Err(err_retry(
            "account temporarily locked",
            StatusCode::LOCKED,
            user.locked_until - now,
        ));
    }
    let Some(stored) = user.password_hash.as_deref() else {
        burn_password_verify(password);
        return Err(generic());
    };
    if verify_password(stored, password) {
        UserRow::clear_password_failures(db, &user.user_id)
            .await
            .map_err(|_| err("internal error", StatusCode::INTERNAL_SERVER_ERROR))?;
        Ok(Some(user))
    } else {
        let deadline = UserRow::record_password_failure(db, &user.user_id)
            .await
            .unwrap_or(0);
        if deadline > 0 {
            return Err(err_retry(
                "invalid phone or password",
                StatusCode::LOCKED,
                deadline - now,
            ));
        }
        Err(generic())
    }
}

/// `POST /api/auth/local/refresh` — rotate a refresh token into a fresh
/// access/refresh pair. Replaying an already-rotated refresh token revokes
/// its whole family: both the thief and the victim sign in again.
pub(crate) async fn local_refresh(
    State(state): State<AppState>,
    connect_info: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    Json(body): Json<LocalRefreshRequest>,
) -> Result<Json<LocalLoginResponse>, (StatusCode, Json<ErrorResponse>)> {
    let ip = client_ip(
        &headers,
        connect_info.map(|Extension(ConnectInfo(addr))| addr),
    );
    if !state.login_rate_limiter.check_and_record(
        "local-refresh",
        &ip,
        MAX_LOCAL_REFRESH_PER_MIN,
        None,
    ) {
        return Err(err(
            "too many refresh attempts",
            StatusCode::TOO_MANY_REQUESTS,
        ));
    }
    let db = state.db.as_ref();
    let Some(pair) = LocalAuthToken::rotate_refresh(db, &body.refresh_token)
        .await
        .map_err(|error| {
            tracing::error!("Local refresh failed: {error}");
            err("internal error", StatusCode::INTERNAL_SERVER_ERROR)
        })?
    else {
        return Err(err(
            "session expired, sign in again",
            StatusCode::UNAUTHORIZED,
        ));
    };
    let user = UserRow::find_by_user_id(db, &pair.user_id)
        .await
        .map_err(|_| err("internal error", StatusCode::INTERNAL_SERVER_ERROR))?
        .ok_or_else(|| err("session expired, sign in again", StatusCode::UNAUTHORIZED))?;
    let phone = phone_for_user(db, &user.user_id).await;
    Ok(Json(LocalLoginResponse {
        access_token: pair.access_token,
        access_expires_at: pair.access_expires_at,
        refresh_token: pair.refresh_token,
        refresh_expires_at: pair.refresh_expires_at,
        user: local_user_view(&user, phone.unwrap_or_else(|| "***".to_string())),
    }))
}

/// `POST /api/auth/local/password` — set or change the caller's optional
/// password. Authenticated by a local access token, so the caller proved the
/// SMS code first. Other sessions' refresh tokens are revoked; the current
/// family stays signed in.
pub(crate) async fn local_set_password(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<SetPasswordRequest>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    let Some(access_token) = extract_bearer_token(&headers) else {
        return Err(err("Sign in to continue", StatusCode::UNAUTHORIZED));
    };
    let password = body.new_password.trim();
    if password.len() < MIN_PASSWORD_BYTES || password.len() > MAX_PASSWORD_BYTES {
        return Err(err("password must be 8-128 bytes", StatusCode::BAD_REQUEST));
    }
    let Some(hash) = hash_password(password) else {
        return Err(err("internal error", StatusCode::INTERNAL_SERVER_ERROR));
    };
    let db = state.db.as_ref();
    let Some(token) = LocalAuthToken::find_access(db, &access_token)
        .await
        .map_err(|_| err("internal error", StatusCode::INTERNAL_SERVER_ERROR))?
    else {
        return Err(err("Sign in to continue", StatusCode::UNAUTHORIZED));
    };
    UserRow::set_password_hash(db, &token.user_id, &hash)
        .await
        .map_err(|_| err("internal error", StatusCode::INTERNAL_SERVER_ERROR))?;
    LocalAuthToken::revoke_user_refresh_except_family(db, &token.user_id, &token.family_id)
        .await
        .map_err(|_| err("internal error", StatusCode::INTERNAL_SERVER_ERROR))?;
    tracing::info!(user_id = %token.user_id, "Local account password updated");
    Ok(StatusCode::NO_CONTENT)
}

async fn phone_for_user(db: &sqlx::SqlitePool, user_id: &str) -> Option<String> {
    let phone =
        sqlx::query_as::<_, (String,)>("SELECT phone FROM phone_identities WHERE user_id = ?")
            .bind(user_id)
            .fetch_optional(db)
            .await
            .ok()?
            .map(|row| row.0)?;
    Some(crate::db::mask_phone(&phone))
}

pub(crate) async fn verify_identity_credentials(
    state: &AppState,
    peer_addr: Option<SocketAddr>,
    headers: &HeaderMap,
    access_token: &str,
) -> Result<UserRow, (StatusCode, Json<ErrorResponse>)> {
    let db = state.db.as_ref();
    if !state.login_rate_limiter.check_and_record(
        "identity",
        &client_ip(headers, peer_addr),
        MAX_LOGIN_ATTEMPTS_PER_MIN,
        None,
    ) {
        return Err(err(
            "too many login attempts",
            StatusCode::TOO_MANY_REQUESTS,
        ));
    }
    let Some(token) = LocalAuthToken::find_access(db, access_token)
        .await
        .map_err(|_| err("internal error", StatusCode::INTERNAL_SERVER_ERROR))?
    else {
        return Err(err("Sign in to continue", StatusCode::UNAUTHORIZED));
    };
    UserRow::find_by_user_id(db, &token.user_id)
        .await
        .map_err(|error| {
            tracing::error!("Account lookup failed: {error}");
            err("internal error", StatusCode::INTERNAL_SERVER_ERROR)
        })?
        .ok_or_else(|| err("Sign in to continue", StatusCode::UNAUTHORIZED))
}

/// Exchange a local account session for a device-scoped relay token.
pub(crate) async fn login(
    State(state): State<AppState>,
    connect_info: Option<Extension<ConnectInfo<SocketAddr>>>,
    headers: HeaderMap,
    Json(body): Json<LoginRequest>,
) -> Result<Json<AuthResponse>, (StatusCode, Json<ErrorResponse>)> {
    let metadata = DeviceMetadata {
        device_model: body.device_model.clone(),
        device_os: body.device_os.clone(),
        device_os_version: body.device_os_version.clone(),
    };
    if !metadata.is_valid() {
        return Err(err("invalid device metadata", StatusCode::BAD_REQUEST));
    }
    let public_key = BASE64.decode(&body.public_key).ok();
    if !valid_device_id(&body.device_id)
        || !valid_bounded_text(&body.device_name, MAX_DEVICE_NAME_BYTES)
        || !crate::db::is_valid_device_kind(&body.device_kind)
        || !valid_login_request_id(&body.request_id)
        || !public_key
            .as_ref()
            .is_some_and(|key| key.len() == 32 && key.iter().any(|b| *b != 0))
    {
        return Err(err("invalid login parameters", StatusCode::BAD_REQUEST));
    }
    let user = verify_identity_credentials(
        &state,
        connect_info.map(|Extension(ConnectInfo(addr))| addr),
        &headers,
        &body.access_token,
    )
    .await?;
    let db = state.db.as_ref();
    DeviceRow::upsert_with_metadata(
        db,
        &body.device_id,
        &user.user_id,
        &body.device_name,
        Some(&body.device_kind),
        Some(&body.public_key),
        &metadata,
    )
    .await
    .map_err(|error| {
        err(
            "device registration failed",
            registration_error_status(&error),
        )
    })?;
    // Refresh the client build from this login. An older client that omits the
    // fields clears the stored values to NULL rather than leaving a stale build.
    let client_version = crate::db::normalize_client_version(body.client_version.as_deref());
    crate::db::DeviceRow::set_client_build(
        db,
        &user.user_id,
        &body.device_id,
        client_version.as_deref(),
        body.client_protocol,
    )
    .await
    .map_err(|error| {
        err(
            "device registration failed",
            registration_error_status(&error),
        )
    })?;
    let token = AuthToken::create_idempotent(db, &user.user_id, &body.device_id, &body.request_id)
        .await
        .map_err(|error| err("token creation failed", registration_error_status(&error)))?;
    Ok(Json(AuthResponse {
        token: token.token,
        user_id: user.user_id,
    }))
}

/// `POST /api/auth/logout` — revoke the caller's token on the relay.
pub async fn logout(State(state): State<AppState>, headers: HeaderMap) -> StatusCode {
    let db = state.db.as_ref();
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty());
    let Some(token) = token else {
        return StatusCode::UNAUTHORIZED;
    };
    match AuthToken::find(db, &token).await {
        Ok(Some(auth)) => {
            let _presence_projection_guard = state.device_manager.lock_presence_projection().await;
            // The first lookup determines which lifecycle to serialize. Check
            // the exact token again under that lifecycle boundary so a
            // concurrent device deletion cannot leave this handler operating
            // on stale authorization state.
            let current = match AuthToken::find(db, &token).await {
                Ok(Some(current))
                    if current.user_id == auth.user_id
                        && current.device_id == auth.device_id
                        && current.token_kind == auth.token_kind =>
                {
                    current
                }
                _ => return StatusCode::UNAUTHORIZED,
            };
            // Delete the token row
            if let Err(error) = sqlx::query("DELETE FROM auth_tokens WHERE token = ?")
                .bind(&token)
                .execute(db)
                .await
            {
                tracing::error!(%error, "Failed to revoke account token");
                return StatusCode::INTERNAL_SERVER_ERROR;
            }
            // A delegated control token borrows the desktop's device id but
            // does not own its live connection. Logging out that client must
            // never disconnect or mark the desktop offline.
            if current.is_device_token() {
                state.device_manager.disconnect_device_if_token(
                    &auth.user_id,
                    &auth.device_id,
                    &token,
                );
                // A failed token match means another login currently owns the
                // same machine's socket. Keep its durable presence online;
                // otherwise a rejected candidate login could make the still-
                // connected prior account appear offline.
                if !state
                    .device_manager
                    .is_device_online(&auth.user_id, &auth.device_id)
                {
                    let _ =
                        crate::db::DeviceRow::set_online(db, &auth.user_id, &auth.device_id, false)
                            .await;
                }
                drop(_presence_projection_guard);
            } else {
                drop(_presence_projection_guard);
            }
            tracing::info!("Account token revoked for device_id={}", auth.device_id);
            StatusCode::NO_CONTENT
        }
        _ => StatusCode::UNAUTHORIZED,
    }
}

/// `POST /api/auth/delegate` — the caller (an already-authenticated desktop)
/// requests a new token for the same account, to be delegated to a paired
/// mobile-web or IM bot client. Returns `{token, user_id}`.
///
/// The delegated token carries the same `user_id` and references the caller's
/// `device_id` for lifetime tracking, but is limited to device discovery and
/// RPC. It cannot open a device WebSocket, mint more credentials, delete a
/// device, or access account sync/page APIs.
#[derive(Deserialize)]
pub struct DelegateRequest {
    pub public_key: String,
}

pub async fn delegate(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<DelegateRequest>,
) -> Result<Json<AuthResponse>, StatusCode> {
    let db = state.db.as_ref();
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let auth = AuthToken::find(db, &token)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if !auth.is_device_token() {
        return Err(StatusCode::FORBIDDEN);
    }

    let public_key = BASE64.decode(&body.public_key).ok();
    if !public_key
        .as_ref()
        .is_some_and(|key| key.len() == 32 && key.iter().any(|byte| *byte != 0))
    {
        return Err(StatusCode::BAD_REQUEST);
    }
    // Issue a capability-limited token for the same account and bind its
    // lifetime to the delegating device row.
    let new_token =
        AuthToken::create_keyed_delegated(db, &auth.user_id, &auth.device_id, &body.public_key)
            .await
            .map_err(|error| registration_error_status(&error))?;

    tracing::info!(
        "Delegated token for user_id={} device_id={}",
        auth.user_id,
        auth.device_id
    );

    Ok(Json(AuthResponse {
        token: new_token.token,
        user_id: auth.user_id,
    }))
}

/// `POST /api/auth/provision-device` — mint a full device credential for a
/// distinct machine during an authenticated one-click SSH bootstrap.
///
/// Only a full device token can perform this operation. The endpoint never
/// receives the account master key; the controller sends that end-to-end over
/// its already trusted SSH channel after this call succeeds.
pub async fn provision_device(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(body): Json<ProvisionDeviceRequest>,
) -> Result<Json<ProvisionDeviceResponse>, (StatusCode, Json<ErrorResponse>)> {
    let metadata = DeviceMetadata {
        device_model: body.device_model.clone(),
        device_os: body.device_os.clone(),
        device_os_version: body.device_os_version.clone(),
    };
    if !metadata.is_valid() {
        return Err(err("invalid device metadata", StatusCode::BAD_REQUEST));
    }
    let public_key = BASE64.decode(&body.public_key).ok();
    if !public_key
        .as_ref()
        .is_some_and(|key| key.len() == 32 && key.iter().any(|b| *b != 0))
        || body.device_id.len() != 32
        || !body
            .device_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
        || !valid_bounded_text(&body.device_name, MAX_DEVICE_NAME_BYTES)
        || !valid_optional_device_kind(body.device_kind.as_deref())
        || !valid_login_request_id(&body.request_id)
    {
        return Err(err(
            "invalid device provisioning parameters",
            StatusCode::BAD_REQUEST,
        ));
    }

    let db = state.db.as_ref();
    let token = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| err("unauthorized", StatusCode::UNAUTHORIZED))?;
    let auth = AuthToken::find(db, token)
        .await
        .map_err(|error| {
            tracing::error!(%error, "Failed to authenticate device provisioning");
            err("internal error", StatusCode::INTERNAL_SERVER_ERROR)
        })?
        .ok_or_else(|| err("unauthorized", StatusCode::UNAUTHORIZED))?;
    if !auth.is_device_token() {
        return Err(err("forbidden", StatusCode::FORBIDDEN));
    }

    // This route only ever bootstraps a machine over SSH, so an unreported
    // kind is a desktop rather than an unknown.
    let provisioned = AuthToken::provision_new_device(
        db,
        &auth.user_id,
        &body.device_id,
        body.device_name.trim(),
        Some(
            body.device_kind
                .as_deref()
                .unwrap_or(crate::db::DEVICE_KIND_DESKTOP),
        ),
        &body.request_id,
        &body.public_key,
        &metadata,
    )
    .await
    .map_err(|error| {
        tracing::error!(%error, "Failed to provision account device");
        err(
            "device registration unavailable",
            registration_error_status(&error),
        )
    })?
    .ok_or_else(|| {
        err(
            "device is already registered on this account",
            StatusCode::CONFLICT,
        )
    })?;

    tracing::info!(
        user_id = %auth.user_id,
        device_id = %body.device_id,
        "Provisioned account device over authenticated SSH bootstrap"
    );
    Ok(Json(ProvisionDeviceResponse {
        token: provisioned.token,
        user_id: auth.user_id,
        device_id: body.device_id,
    }))
}

/// Validated principal extracted from the bearer token.
pub struct AuthUser {
    pub user_id: String,
    #[allow(dead_code)]
    pub device_id: String,
}

/// Validate the bearer token in `headers`; returns the owning user/device.
pub async fn validate_auth(state: &AppState, headers: &HeaderMap) -> Result<AuthUser, StatusCode> {
    let token = extract_bearer_token(headers).ok_or(StatusCode::UNAUTHORIZED)?;
    validate_token(state, &token).await
}

/// Extract `Bearer` token from the `Authorization` header.
pub fn extract_bearer_token(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.strip_prefix("Bearer "))
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

/// Validate a raw token string against the account database.
pub async fn validate_token(state: &AppState, token: &str) -> Result<AuthUser, StatusCode> {
    let db = state.db.as_ref();
    let auth = AuthToken::find(db, token)
        .await
        .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?
        .ok_or(StatusCode::UNAUTHORIZED)?;
    if !auth.is_device_token() {
        return Err(StatusCode::FORBIDDEN);
    }
    Ok(AuthUser {
        user_id: auth.user_id,
        device_id: auth.device_id,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::{connect, DbPool};
    use crate::MemoryAssetStore;
    use axum::body::{to_bytes, Body};
    use axum::http::{header, Request};
    use std::future::Future;
    use std::pin::Pin;
    use std::sync::Arc;
    use tower::ServiceExt;

    #[test]
    fn forwarded_ip_is_only_trusted_from_a_local_proxy() {
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-for", "198.51.100.25".parse().unwrap());

        assert_eq!(
            client_ip(&headers, Some("203.0.113.10:443".parse().unwrap())),
            "203.0.113.10"
        );
        assert_eq!(
            client_ip(&headers, Some("127.0.0.1:8080".parse().unwrap())),
            "198.51.100.25"
        );
    }

    async fn setup_app() -> (axum::Router, Arc<DbPool>, String) {
        let db = Arc::new(connect(":memory:").await.unwrap());
        UserRow::create(&db, "owner", "alice").await.unwrap();
        DeviceRow::upsert(&db, "owner-device", "owner", "Owner", None, None)
            .await
            .unwrap();
        let token = AuthToken::create(&db, "owner", "owner-device")
            .await
            .unwrap()
            .token;
        let app = crate::build_relay_router(
            Arc::new(MemoryAssetStore::new()),
            std::time::Instant::now(),
            db.clone(),
            "test",
        );
        (app, db, token)
    }

    async fn post(app: &axum::Router, path: &str, token: &str) -> axum::response::Response {
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(if path == "/api/auth/delegate" {
                        serde_json::json!({"public_key": BASE64.encode([9u8; 32])}).to_string()
                    } else {
                        "{}".to_string()
                    }))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    async fn post_json(
        app: &axum::Router,
        path: &str,
        token: &str,
        body: serde_json::Value,
    ) -> axum::response::Response {
        app.clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header(header::AUTHORIZATION, format!("Bearer {token}"))
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap()
    }

    /// A recording transport so tests can read the delivered code.
    struct RecordingTransport(std::sync::Mutex<Vec<(String, String)>>);
    impl RecordingTransport {
        fn new() -> Arc<Self> {
            Arc::new(Self(std::sync::Mutex::new(Vec::new())))
        }
        fn last_code(&self, phone: &str) -> String {
            self.0
                .lock()
                .unwrap()
                .iter()
                .rev()
                .find(|(sent_phone, _)| sent_phone == phone)
                .map(|(_, code)| code.clone())
                .unwrap()
        }
    }
    impl crate::sms::SmsTransport for RecordingTransport {
        fn send<'a>(
            &'a self,
            phone: &'a str,
            code: &'a str,
        ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'a>> {
            Box::pin(async move {
                self.0
                    .lock()
                    .unwrap()
                    .push((phone.to_string(), code.to_string()));
                Ok(())
            })
        }
    }

    fn local_env(
        transport: Arc<RecordingTransport>,
        registration_closed: bool,
    ) -> Arc<LocalAuthEnvironment> {
        Arc::new(LocalAuthEnvironment {
            transport: Some(transport),
            code_secret: Some("test-secret".to_string()),
            registration_closed,
        })
    }

    async fn provision_local_account(db: &DbPool, phone: &str) -> (String, String) {
        let user = UserRow::create_local_with_phone(db, phone).await.unwrap();
        let pair = LocalAuthToken::issue_pair(db, &user.user_id).await.unwrap();
        (pair.access_token, user.user_id)
    }

    #[tokio::test]
    async fn local_login_binds_devices_to_the_access_tokens_account_and_preserves_device_key_on_reconnect(
    ) {
        let (app, db, _) = setup_app().await;
        let (access_token, user_id) = provision_local_account(&db, "13800000001").await;
        let public_key = BASE64.encode([9u8; 32]);
        let request = serde_json::json!({
            "access_token":access_token, "user_id":"attacker-chosen-id",
            "device_id":"new-device", "device_name":"Laptop", "device_kind":"desktop",
            "public_key":public_key, "request_id":uuid::Uuid::new_v4().to_string(),
        });
        let first = post_json(&app, "/api/auth/login", "", request.clone()).await;
        assert_eq!(first.status(), StatusCode::OK);
        let first: serde_json::Value =
            serde_json::from_slice(&to_bytes(first.into_body(), 16384).await.unwrap()).unwrap();
        assert_eq!(first["user_id"], user_id);
        let second = post_json(&app, "/api/auth/login", "", request.clone()).await;
        let second: serde_json::Value =
            serde_json::from_slice(&to_bytes(second.into_body(), 16384).await.unwrap()).unwrap();
        assert_eq!(second["token"], first["token"]);
        let mut metadata_request = request.clone();
        metadata_request["device_model"] = serde_json::json!("Model");
        metadata_request["device_os"] = serde_json::json!("Linux");
        metadata_request["device_os_version"] = serde_json::json!("6");
        assert_eq!(
            post_json(&app, "/api/auth/login", "", metadata_request)
                .await
                .status(),
            StatusCode::OK
        );
        sqlx::query("UPDATE devices SET device_alias='Independent' WHERE user_id = ?")
            .bind(&user_id)
            .execute(&*db)
            .await
            .unwrap();
        assert_eq!(
            post_json(&app, "/api/auth/login", "", request.clone())
                .await
                .status(),
            StatusCode::OK
        );
        let rows = DeviceRow::list_by_user(&db, &user_id).await.unwrap();
        assert_eq!(rows[0].device_alias.as_deref(), Some("Independent"));
        assert_eq!(rows[0].device_model.as_deref(), Some("Model"));
        assert_eq!(rows[0].device_os.as_deref(), Some("Linux"));
        assert_eq!(rows[0].device_os_version.as_deref(), Some("6"));
        DeviceRow::upsert(&db, "new-device", &user_id, "Laptop", None, None)
            .await
            .unwrap();
        let devices = DeviceRow::list_by_user(&db, &user_id).await.unwrap();
        assert_eq!(devices[0].public_key.as_deref(), Some(public_key.as_str()));
        let mut invalid = request;
        invalid["access_token"] = serde_json::json!("expired");
        invalid["device_id"] = serde_json::json!("must-not-register");
        assert_eq!(
            post_json(&app, "/api/auth/login", "", invalid)
                .await
                .status(),
            StatusCode::UNAUTHORIZED
        );
        assert_eq!(
            DeviceRow::list_by_user(&db, &user_id).await.unwrap().len(),
            1
        );
        assert_eq!(
            post_json(
                &app,
                "/api/auth/login/challenge",
                "",
                serde_json::json!({"username":"alice"})
            )
            .await
            .status(),
            StatusCode::NOT_FOUND
        );
        // The GitHub authority routes are gone; old clients get a clear 404.
        assert_eq!(
            post_json(&app, "/api/auth/github/start", "", serde_json::json!({}))
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
        assert_eq!(
            post_json(&app, "/api/auth/github/poll", "", serde_json::json!({}))
                .await
                .status(),
            StatusCode::NOT_FOUND
        );
    }

    #[tokio::test]
    async fn sms_code_signs_in_registers_on_first_use_and_rotates_refresh_tokens() {
        let (app, db, _) = setup_app().await;
        let transport = RecordingTransport::new();
        let app = app.layer(Extension(local_env(transport.clone(), false)));

        let invalid_phone = post_json(
            &app,
            "/api/auth/local/sms/send",
            "",
            serde_json::json!({"phone": "not-a-phone"}),
        )
        .await;
        assert_eq!(invalid_phone.status(), StatusCode::BAD_REQUEST);
        let sent = post_json(
            &app,
            "/api/auth/local/sms/send",
            "",
            serde_json::json!({"phone": "+86 138 0000 0002"}),
        )
        .await;
        assert_eq!(sent.status(), StatusCode::ACCEPTED);

        let wrong = post_json(
            &app,
            "/api/auth/local/login",
            "",
            serde_json::json!({"phone":"13800000002", "code":"000000"}),
        )
        .await;
        assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);

        let code = transport.last_code("13800000002");
        let login = post_json(
            &app,
            "/api/auth/local/login",
            "",
            serde_json::json!({"phone":"13800000002", "code": code}),
        )
        .await;
        assert_eq!(login.status(), StatusCode::OK);
        let login: serde_json::Value =
            serde_json::from_slice(&to_bytes(login.into_body(), 16384).await.unwrap()).unwrap();
        let user_id = login["user"]["userId"].as_str().unwrap().to_string();
        assert!(user_id.starts_with("local-"));
        assert_eq!(login["user"]["phoneMasked"], "138****0002");
        assert_eq!(login["user"]["hasPassword"], false);
        let refresh = login["refreshToken"].as_str().unwrap().to_string();
        let access = login["accessToken"].as_str().unwrap().to_string();

        // Local tokens are stored only as digests, never in plaintext.
        let stored: Vec<String> = sqlx::query_as("SELECT token_hash FROM local_auth_tokens")
            .fetch_all(&*db)
            .await
            .unwrap()
            .into_iter()
            .map(|row: (String,)| row.0)
            .collect();
        assert!(!stored
            .iter()
            .any(|hash| hash == &access || hash == &refresh));

        // The same code cannot be consumed twice.
        let replay = post_json(
            &app,
            "/api/auth/local/login",
            "",
            serde_json::json!({"phone":"13800000002", "code": code}),
        )
        .await;
        assert_eq!(replay.status(), StatusCode::UNAUTHORIZED);

        // Refresh rotation issues a new pair for the same account.
        let rotated = post_json(
            &app,
            "/api/auth/local/refresh",
            "",
            serde_json::json!({"refreshToken": refresh}),
        )
        .await;
        assert_eq!(rotated.status(), StatusCode::OK);
        let rotated: serde_json::Value =
            serde_json::from_slice(&to_bytes(rotated.into_body(), 16384).await.unwrap()).unwrap();
        assert_eq!(rotated["user"]["userId"], user_id);
        let new_refresh = rotated["refreshToken"].as_str().unwrap().to_string();

        // Replaying the rotated token revokes the whole family.
        let replayed = post_json(
            &app,
            "/api/auth/local/refresh",
            "",
            serde_json::json!({"refreshToken": refresh}),
        )
        .await;
        assert_eq!(replayed.status(), StatusCode::UNAUTHORIZED);
        let revoked = post_json(
            &app,
            "/api/auth/local/refresh",
            "",
            serde_json::json!({"refreshToken": new_refresh}),
        )
        .await;
        assert_eq!(revoked.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn closed_registration_signs_in_known_phones_and_refuses_new_ones() {
        let (app, db, _) = setup_app().await;
        let transport = RecordingTransport::new();
        let app = app.layer(Extension(local_env(transport.clone(), true)));
        provision_local_account(&db, "13800000003").await;

        for phone in ["13800000003", "13800000004"] {
            let sent = post_json(
                &app,
                "/api/auth/local/sms/send",
                "",
                serde_json::json!({"phone": phone}),
            )
            .await;
            assert_eq!(sent.status(), StatusCode::ACCEPTED);
        }
        let known = post_json(
            &app,
            "/api/auth/local/login",
            "",
            serde_json::json!({"phone":"13800000003", "code": transport.last_code("13800000003")}),
        )
        .await;
        assert_eq!(known.status(), StatusCode::OK);
        let unknown = post_json(
            &app,
            "/api/auth/local/login",
            "",
            serde_json::json!({"phone":"13800000004", "code": transport.last_code("13800000004")}),
        )
        .await;
        assert_eq!(unknown.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn passwords_are_optional_and_lock_after_repeated_failures() {
        let (app, _db, _) = setup_app().await;
        let transport = RecordingTransport::new();
        let app = app.layer(Extension(local_env(transport.clone(), false)));

        post_json(
            &app,
            "/api/auth/local/sms/send",
            "",
            serde_json::json!({"phone":"13800000005"}),
        )
        .await;
        let login = post_json(
            &app,
            "/api/auth/local/login",
            "",
            serde_json::json!({"phone":"13800000005", "code": transport.last_code("13800000005")}),
        )
        .await;
        assert_eq!(login.status(), StatusCode::OK);
        let login: serde_json::Value =
            serde_json::from_slice(&to_bytes(login.into_body(), 16384).await.unwrap()).unwrap();
        let access = login["accessToken"].as_str().unwrap().to_string();
        assert_eq!(login["user"]["hasPassword"], false);

        // Password login on a passwordless account shares one generic error.
        let no_password = post_json(
            &app,
            "/api/auth/local/login",
            "",
            serde_json::json!({"phone":"13800000005", "password":"whatever1"}),
        )
        .await;
        assert_eq!(no_password.status(), StatusCode::UNAUTHORIZED);

        // Set a password with the access token.
        let too_short = post_json(
            &app,
            "/api/auth/local/password",
            &access,
            serde_json::json!({"newPassword":"short"}),
        )
        .await;
        assert_eq!(too_short.status(), StatusCode::BAD_REQUEST);
        let set = post_json(
            &app,
            "/api/auth/local/password",
            &access,
            serde_json::json!({"newPassword":"correct horse"}),
        )
        .await;
        assert_eq!(set.status(), StatusCode::NO_CONTENT);

        let wrong = post_json(
            &app,
            "/api/auth/local/login",
            "",
            serde_json::json!({"phone":"13800000005", "password":"wrong-password"}),
        )
        .await;
        assert_eq!(wrong.status(), StatusCode::UNAUTHORIZED);
        let right = post_json(
            &app,
            "/api/auth/local/login",
            "",
            serde_json::json!({"phone":"13800000005", "password":"correct horse"}),
        )
        .await;
        assert_eq!(right.status(), StatusCode::OK);
        let right: serde_json::Value =
            serde_json::from_slice(&to_bytes(right.into_body(), 16384).await.unwrap()).unwrap();
        assert_eq!(right["user"]["hasPassword"], true);

        // Repeated failures lock the account with a retry hint.
        let mut locked = None;
        for _ in 0..5 {
            locked = Some(
                post_json(
                    &app,
                    "/api/auth/local/login",
                    "",
                    serde_json::json!({"phone":"13800000005", "password":"wrong-password"}),
                )
                .await,
            );
        }
        let locked = locked.unwrap();
        assert_eq!(locked.status(), StatusCode::LOCKED);
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(locked.into_body(), 16384).await.unwrap()).unwrap();
        assert!(body["retry_after_secs"]
            .as_i64()
            .is_some_and(|secs| secs > 0));
    }

    #[tokio::test]
    async fn sms_send_is_cooldown_and_daily_capped_per_phone() {
        let (app, db, _) = setup_app().await;
        let transport = RecordingTransport::new();
        let app = app.layer(Extension(local_env(transport.clone(), false)));
        let send = |app: &axum::Router| {
            let app = app.clone();
            async move {
                post_json(
                    &app,
                    "/api/auth/local/sms/send",
                    "",
                    serde_json::json!({"phone":"13800000006"}),
                )
                .await
            }
        };
        assert_eq!(send(&app).await.status(), StatusCode::ACCEPTED);
        let cooldown = send(&app).await;
        assert_eq!(cooldown.status(), StatusCode::TOO_MANY_REQUESTS);
        let body: serde_json::Value =
            serde_json::from_slice(&to_bytes(cooldown.into_body(), 16384).await.unwrap()).unwrap();
        assert!(body["retry_after_secs"]
            .as_i64()
            .is_some_and(|secs| secs > 0));

        // Saturate the daily cap with sends back-dated one hour, then clear
        // only the cooldown row so the cap is the sole remaining limit.
        let hour_ago = chrono::Utc::now().timestamp() - 3600;
        for _ in 0..crate::db::SMS_DAILY_CAP_PER_PHONE {
            sqlx::query("INSERT INTO sms_sends (phone, sent_at) VALUES ('13800000006', ?)")
                .bind(hour_ago)
                .execute(&*db)
                .await
                .unwrap();
        }
        sqlx::query("DELETE FROM sms_sends WHERE phone = '13800000006' AND sent_at > ?")
            .bind(hour_ago + 1)
            .execute(&*db)
            .await
            .unwrap();
        assert!(!crate::db::SmsSend::sent_within(
            &db,
            "13800000006",
            crate::db::SMS_RESEND_COOLDOWN_SECS
        )
        .await
        .unwrap());
        let capped = send(&app).await;
        assert_eq!(capped.status(), StatusCode::TOO_MANY_REQUESTS);
    }

    #[tokio::test]
    async fn login_records_and_clears_the_client_build() {
        let (app, db, _) = setup_app().await;
        let (access_token, user_id) = provision_local_account(&db, "13800000007").await;
        let base = serde_json::json!({
            "access_token":access_token,
            "device_id":"new-device", "device_name":"Laptop", "device_kind":"desktop",
            "public_key":BASE64.encode([9u8; 32]),
            "request_id":uuid::Uuid::new_v4().to_string(),
        });

        // An old client that sends no build fields still logs in and stores NULL.
        assert_eq!(
            post_json(&app, "/api/auth/login", "", base.clone())
                .await
                .status(),
            StatusCode::OK
        );
        let row = DeviceRow::list_by_user(&db, &user_id)
            .await
            .unwrap()
            .remove(0);
        assert!(row.client_version.is_none());
        assert!(row.client_protocol.is_none());

        // A newer client reports a build, which is written to the device row.
        let mut reporting = base.clone();
        reporting["clientVersion"] = serde_json::json!("1.4.0");
        reporting["clientProtocol"] = serde_json::json!(7);
        assert_eq!(
            post_json(&app, "/api/auth/login", "", reporting)
                .await
                .status(),
            StatusCode::OK
        );
        let row = DeviceRow::list_by_user(&db, &user_id)
            .await
            .unwrap()
            .remove(0);
        assert_eq!(row.client_version.as_deref(), Some("1.4.0"));
        assert_eq!(row.client_protocol_u32(), Some(7));

        // An older client logging in again clears the recorded build to NULL
        // instead of leaving the newer value behind.
        assert_eq!(
            post_json(&app, "/api/auth/login", "", base.clone())
                .await
                .status(),
            StatusCode::OK
        );
        let row = DeviceRow::list_by_user(&db, &user_id)
            .await
            .unwrap()
            .remove(0);
        assert!(row.client_version.is_none());
        assert!(row.client_protocol.is_none());

        // A malformed build string is treated as unreported, not rejected.
        let mut malformed = base.clone();
        malformed["clientVersion"] = serde_json::json!("bad\nbuild");
        assert_eq!(
            post_json(&app, "/api/auth/login", "", malformed)
                .await
                .status(),
            StatusCode::OK
        );
        let row = DeviceRow::list_by_user(&db, &user_id)
            .await
            .unwrap()
            .remove(0);
        assert!(row.client_version.is_none());
    }

    #[tokio::test]
    async fn device_provisioning_is_full_scope_idempotent_and_device_only() {
        let (app, db, device_token) = setup_app().await;
        let request_id = uuid::Uuid::new_v4().to_string();
        let device_id = "ab".repeat(16);
        let request = serde_json::json!({
            "device_id": device_id,
            "device_name": "SSH Build Host",
            "public_key": BASE64.encode([9u8; 32]),
            "request_id": request_id,
        });

        let mut with_metadata = request.clone();
        with_metadata["device_model"] = serde_json::json!("Build server");
        with_metadata["device_os"] = serde_json::json!("Linux");
        with_metadata["device_os_version"] = serde_json::json!("6");
        let first = post_json(
            &app,
            "/api/auth/provision-device",
            &device_token,
            with_metadata,
        )
        .await;
        assert_eq!(first.status(), StatusCode::OK);
        let first_body = to_bytes(first.into_body(), 16 * 1024).await.unwrap();
        let first: ProvisionDeviceResponse = serde_json::from_slice(&first_body).unwrap();
        assert_eq!(first.user_id, "owner");
        assert_eq!(first.device_id, device_id);
        let issued = AuthToken::find(&db, &first.token).await.unwrap().unwrap();
        assert!(issued.is_device_token());
        assert_eq!(issued.device_id, device_id);

        let replay = post_json(&app, "/api/auth/provision-device", &device_token, request).await;
        assert_eq!(replay.status(), StatusCode::OK);
        let replay_body = to_bytes(replay.into_body(), 16 * 1024).await.unwrap();
        let replay: ProvisionDeviceResponse = serde_json::from_slice(&replay_body).unwrap();
        assert_eq!(replay.token, first.token);
        let row = DeviceRow::list_by_user(&db, "owner")
            .await
            .unwrap()
            .into_iter()
            .find(|row| row.device_id == device_id)
            .unwrap();
        assert_eq!(row.device_model.as_deref(), Some("Build server"));
        assert_eq!(row.device_os.as_deref(), Some("Linux"));
        assert_eq!(row.device_os_version.as_deref(), Some("6"));

        let conflict = post_json(
            &app,
            "/api/auth/provision-device",
            &device_token,
            serde_json::json!({
                "device_id": device_id,
                "device_name": "SSH Build Host",
            "public_key": BASE64.encode([9u8; 32]),
                "request_id": uuid::Uuid::new_v4().to_string(),
            }),
        )
        .await;
        assert_eq!(conflict.status(), StatusCode::CONFLICT);

        let delegated_response = post(&app, "/api/auth/delegate", &device_token).await;
        let delegated_body = to_bytes(delegated_response.into_body(), 16 * 1024)
            .await
            .unwrap();
        let delegated_token = serde_json::from_slice::<serde_json::Value>(&delegated_body).unwrap()
            ["token"]
            .as_str()
            .unwrap()
            .to_string();
        let forbidden = post_json(
            &app,
            "/api/auth/provision-device",
            &delegated_token,
            serde_json::json!({
                "device_id": "cd".repeat(16),
                "device_name": "Another Host",
                "public_key": BASE64.encode([9u8; 32]),
                "request_id": uuid::Uuid::new_v4().to_string(),
            }),
        )
        .await;
        assert_eq!(forbidden.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn delegated_tokens_cannot_chain_or_log_out_the_parent_device() {
        let (app, db, device_token) = setup_app().await;

        let response = post(&app, "/api/auth/delegate", &device_token).await;
        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), 16 * 1024).await.unwrap();
        let delegated_token = serde_json::from_slice::<serde_json::Value>(&body).unwrap()["token"]
            .as_str()
            .unwrap()
            .to_string();
        let delegated = AuthToken::find(&db, &delegated_token)
            .await
            .unwrap()
            .unwrap();
        assert!(!delegated.is_device_token());

        assert_eq!(
            post(&app, "/api/auth/delegate", &delegated_token)
                .await
                .status(),
            StatusCode::FORBIDDEN
        );
        DeviceRow::set_online(&db, "owner", "owner-device", true)
            .await
            .unwrap();
        assert_eq!(
            post(&app, "/api/auth/logout", &delegated_token)
                .await
                .status(),
            StatusCode::NO_CONTENT
        );
        assert!(AuthToken::find(&db, &device_token).await.unwrap().is_some());
        let devices = DeviceRow::list_by_user(&db, "owner").await.unwrap();
        assert_eq!(devices.len(), 1);
        assert_eq!(devices[0].online, 1);
    }

    #[test]
    fn exact_idempotent_login_replays_consume_one_rate_limit_slot() {
        let limiter = LoginRateLimiter::new();
        for _ in 0..20 {
            assert!(limiter.check_and_record("credentials", "127.0.0.1", 2, Some("same")));
        }
        assert!(limiter.check_and_record("credentials", "127.0.0.1", 2, Some("second")));
        assert!(!limiter.check_and_record("credentials", "127.0.0.1", 2, Some("third")));

        // Challenge traffic has its own budget and cannot exhaust credential
        // verification for the same client IP.
        assert!(limiter.check_and_record("challenge", "127.0.0.1", 1, None));
    }
}

fn registration_error_status(error: &anyhow::Error) -> StatusCode {
    if error.chain().any(|cause| {
        let text = cause.to_string();
        text.contains("account device quota exceeded")
            || text.contains("account token quota exceeded")
    }) {
        StatusCode::TOO_MANY_REQUESTS
    } else {
        StatusCode::INTERNAL_SERVER_ERROR
    }
}
