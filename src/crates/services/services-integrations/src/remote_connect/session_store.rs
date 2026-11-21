//! Machine-bound persistent session store.
//!
//! Saves the device `AccountSession` (token + private key + user_id) and relay
//! URL to disk, encrypted with a key that combines machine identity and a
//! random per-install secret. Secret files are owner-only on Unix and replaced
//! through a private temporary file. This lets Desktop / CLI restart without
//! requiring a fresh GitHub sign-in while keeping copied session ciphertext
//! unusable without the separate install key.
//!
//! File location: `<OPENBITFUN_HOME>/relay-v1.0.0/account_session.enc` when configured,
//! otherwise `~/.openbitfun/relay-v1.0.0/account_session.enc`.
//! Format: base64(nonce || ciphertext) where the plaintext is a JSON
//! payload `{ token, user_id, master_key_b64, relay_url }`.

use anyhow::{anyhow, Result};
use std::path::PathBuf;
use std::sync::{OnceLock, RwLock};

fn session_store_directory_override() -> &'static RwLock<Option<PathBuf>> {
    static OVERRIDE: OnceLock<RwLock<Option<PathBuf>>> = OnceLock::new();
    OVERRIDE.get_or_init(|| RwLock::new(None))
}

/// Redirects session, local-key, and credential-hint files for integration
/// tests. Tests must use this instead of changing HOME or touching real login
/// state shared by Desktop and CLI.
/// Serializes tests that redirect the global store directory. The override
/// is process-global, so parallel tests would otherwise read each other's
/// redirected paths mid-flight.
#[cfg(test)]
pub(crate) fn test_store_guard() -> &'static std::sync::Mutex<()> {
    static GUARD: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    GUARD.get_or_init(|| std::sync::Mutex::new(()))
}

pub fn set_session_store_directory_for_test(path: PathBuf) {
    let mut override_path = session_store_directory_override()
        .write()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    *override_path = Some(path);
}

#[cfg(test)]
thread_local! { static TEST_STORE: std::cell::RefCell<Option<PathBuf>> = const { std::cell::RefCell::new(None) }; }

fn session_store_directory() -> Result<PathBuf> {
    #[cfg(test)]
    if let Some(dir) = TEST_STORE.with(|dir| dir.borrow().clone()) {
        return Ok(dir);
    }
    let override_path = session_store_directory_override()
        .read()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(path) = override_path.as_ref() {
        return Ok(path.join("relay-v1.0.0"));
    }
    drop(override_path);
    super::product_home_dir()
        .map(|path| path.join("relay-v1.0.0"))
        .ok_or_else(|| anyhow!("cannot determine OpenBitFun home directory"))
}

/// Resolve the shared relay data directory (`<home>/relay-v1.0.0`). Sibling
/// stores such as the PSK pairing vault live beside the session file so the
/// test override covers them too.
pub(crate) fn data_directory() -> Result<PathBuf> {
    session_store_directory()
}

/// Resolve the persistent session file path.
fn session_file_path() -> Result<PathBuf> {
    Ok(session_store_directory()?.join("account_session.enc"))
}

// ── Public API ──────────────────────────────────────────────────────────

pub fn device_secret(relay_url: &str, user_id: &str, device_id: &str) -> Result<[u8; 32]> {
    crate::remote_persistence::load_or_create_device_secret(
        &session_store_directory()?,
        relay_url,
        user_id,
        device_id,
    )
}

/// Persist the session (token, master_key, user_id, relay_url) to disk,
/// encrypted with the machine-bound key.
pub fn save_session(
    token: &str,
    user_id: &str,
    master_key: &[u8; 32],
    relay_url: &str,
) -> Result<()> {
    save_session_with_device(token, user_id, master_key, relay_url, None)
}

use super::account::{AccountClient, AccountSession};
pub use crate::remote_persistence::AccountSessionRecord as LoadedSession;

pub const WEEK: std::time::Duration = std::time::Duration::from_secs(7 * 86400);
pub const RETRY_DELAY: std::time::Duration = std::time::Duration::from_secs(3600);
const DEVICE_TOKEN_LIFETIME: i64 = 30 * 86400;

impl LoadedSession {
    pub fn replaces(&self, session: &super::account::AccountSession, relay: &str) -> bool {
        self.token != session.token && self.same_device(session, relay)
    }

    fn same_device(&self, session: &AccountSession, relay: &str) -> bool {
        self.user_id == session.user_id
            && self.master_key == session.master_key
            && openbitfun_product_domains::account::same_relay_deployment(&self.relay_url, relay)
    }
}

fn renewal_due(issued_at: Option<i64>, now: i64) -> bool {
    issued_at
        .is_none_or(|issued| issued > now || now.saturating_sub(issued) > DEVICE_TOKEN_LIFETIME / 2)
}

fn lock_file(name: &str) -> Result<std::fs::File> {
    let directory = session_store_directory()?;
    std::fs::create_dir_all(&directory)?;
    Ok(std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(directory.join(name))?)
}

// Held only for the synchronous compare-and-write, including across Desktop/CLI.
fn state_lock() -> Result<std::fs::File> {
    let file = lock_file("account_session.lock")?;
    fs2::FileExt::lock_exclusive(&file)?;
    Ok(file)
}

fn write_record(session: &LoadedSession) -> Result<()> {
    crate::remote_persistence::write_current_account_session(
        &session_store_directory()?,
        &crate::remote_persistence::MachineBinding::current(),
        session,
    )
}

/// Import a relay credential without inferring when it was issued.
pub fn save_session_with_device(
    token: &str,
    user_id: &str,
    master_key: &[u8; 32],
    relay_url: &str,
    device_id: Option<&str>,
) -> Result<()> {
    save_session_with_device_and_refresh(token, user_id, master_key, relay_url, device_id, None)
}

/// Persist the session together with a relay-local refresh token so the next
/// device-token expiry can renew silently instead of re-prompting.
pub fn save_session_with_device_and_refresh(
    token: &str,
    user_id: &str,
    master_key: &[u8; 32],
    relay_url: &str,
    device_id: Option<&str>,
    refresh_token: Option<&str>,
) -> Result<()> {
    let _guard = state_lock()?;
    write_record(&LoadedSession {
        token: token.to_owned(),
        user_id: user_id.to_owned(),
        master_key: *master_key,
        relay_url: relay_url.to_owned(),
        device_id: device_id
            .map(str::trim)
            .filter(|id| !id.is_empty())
            .map(str::to_string),
        refresh_token: refresh_token
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .map(str::to_string),
        issued_at: None,
        superseded_tokens: Vec::new(),
    })
}

/// An interactive login replaces the previous credential family. The host
/// separately revokes its in-memory token, which may lag behind the disk.
/// The fork's relay-local refresh token is stored alongside so an expired
/// device token can renew silently through the local-auth refresh flow.
pub async fn save_issued_session(
    session: &AccountSession,
    relay: &str,
    device_id: Option<&str>,
    previous_host_token: Option<&str>,
    refresh_token: Option<&str>,
) -> Result<()> {
    let previous = {
        let _guard = state_lock()?;
        let previous = load_session_detailed()?;
        write_record(&LoadedSession {
            token: session.token.clone(),
            user_id: session.user_id.clone(),
            master_key: session.master_key,
            relay_url: relay.to_owned(),
            device_id: device_id.map(str::to_owned),
            refresh_token: refresh_token
                .map(str::trim)
                .filter(|token| !token.is_empty())
                .map(str::to_string),
            issued_at: Some(chrono::Utc::now().timestamp()),
            superseded_tokens: Vec::new(),
        })?;
        previous
    };
    if let Some(previous) = previous {
        revoke_saved_tokens(previous, previous_host_token, Some(&session.token)).await;
    }
    Ok(())
}

async fn revoke_saved_tokens(
    saved: LoadedSession,
    host_token: Option<&str>,
    new_token: Option<&str>,
) {
    let now = chrono::Utc::now().timestamp();
    let tokens = std::iter::once(saved.token).chain(
        saved
            .superseded_tokens
            .into_iter()
            .filter(|prior| prior.expires_at > now)
            .map(|prior| prior.token),
    );
    let client = AccountClient::new();
    for token in tokens {
        if Some(token.as_str()) == host_token || Some(token.as_str()) == new_token {
            continue;
        }
        let credential = AccountSession::new(token, saved.user_id.clone(), saved.master_key);
        if let Err(error) = client.revoke_token(&saved.relay_url, &credential).await {
            log::warn!("Failed to revoke previous device credential: {error}");
        }
    }
}

/// AuthOk can arrive long after issuance and after another host rotated the token.
pub fn record_authenticated_device(token: &str, device_id: &str) -> Result<bool> {
    let _guard = state_lock()?;
    if let Some(mut saved) = load_session_detailed()? {
        if saved.token == token && saved.device_id.as_deref() != Some(device_id) {
            saved.device_id = Some(device_id.to_owned());
            write_record(&saved)?;
            return Ok(renewal_due(saved.issued_at, chrono::Utc::now().timestamp()));
        }
    }
    Ok(false)
}

/// Refresh a persisted device using the host's existing signed-in account.
pub async fn renew_session_if_due(host_is_cli: bool) -> Result<Option<LoadedSession>> {
    use super::account::persisted_relay_requires_migration;
    let Some(saved) = load_session_detailed()? else {
        return Ok(None);
    };
    if persisted_relay_requires_migration(&saved.relay_url)
        || !renewal_due(saved.issued_at, chrono::Utc::now().timestamp())
    {
        return Ok(Some(saved));
    }
    // Fork: local-auth sessions carry a relay-local refresh token and renew
    // through the refresh-token re-login flow on expiry; the identity-based
    // proactive renewal below only applies to GitHub-identity sessions.
    if saved
        .refresh_token
        .as_deref()
        .is_some_and(|token| !token.is_empty())
    {
        return Ok(Some(saved));
    }
    let renewing = lock_file("account_renewal.lock")?;
    fs2::FileExt::try_lock_exclusive(&renewing)?;
    // Another host may have completed renewal since our first read.
    let current = load_session_detailed()?;
    if !current
        .as_ref()
        .is_some_and(|record| record.token == saved.token)
    {
        return Ok(current);
    }
    let device_id = saved
        .device_id
        .as_ref()
        .ok_or_else(|| anyhow!("Waiting for the Relay to confirm the session device ID"))?;
    let client = AccountClient::new();
    let session = AccountSession::new(saved.token.clone(), saved.user_id.clone(), saved.master_key);
    // Never silently re-register a device whose credential has been revoked.
    client.list_devices(&saved.relay_url, &session).await?;
    let mut device = super::DeviceIdentity::from_current_machine()?;
    device.device_id.clone_from(device_id);
    let kind = client
        .reported_device_kind(&saved.relay_url, host_is_cli)
        .await;
    let issued_at = chrono::Utc::now().timestamp();
    let (new_session, _) = client
        .exchange_identity(&saved.relay_url, &device, kind, Some(&saved))
        .await?;
    let committed = replace_renewed_session(
        &saved.token,
        &new_session.token,
        &device.device_id,
        issued_at,
    );
    if !matches!(&committed, Ok(Some(current)) if current.token == new_session.token) {
        if let Err(error) = client.revoke_token(&saved.relay_url, &new_session).await {
            log::warn!("Failed to revoke unused renewal credential: {error}");
        }
    }
    committed
}

fn replace_renewed_session(
    previous: &str,
    token: &str,
    device: &str,
    issued_at: i64,
) -> Result<Option<LoadedSession>> {
    let _guard = state_lock()?;
    let mut current = load_session_detailed()?;
    if let Some(saved) = current.as_mut().filter(|saved| saved.token == previous) {
        // A supported older host may still be using the previous token. Keep
        // its encrypted revocation handle through logout or account switch.
        saved
            .superseded_tokens
            .retain(|prior| prior.expires_at > issued_at);
        let expires_at = saved
            .issued_at
            .unwrap_or(issued_at)
            .saturating_add(DEVICE_TOKEN_LIFETIME);
        if expires_at > issued_at {
            saved
                .superseded_tokens
                .push(crate::remote_persistence::SupersededDeviceToken {
                    token: saved.token.clone(),
                    expires_at,
                });
        }
        saved.token = token.to_owned();
        saved.device_id = Some(device.to_owned());
        saved.issued_at = Some(issued_at);
        write_record(saved)?;
    }
    Ok(current)
}

/// A different process may already have replaced a locally rejected credential.
pub fn has_saved_replacement(session: &super::account::AccountSession, relay: &str) -> bool {
    matches!(load_session_detailed(), Ok(Some(saved)) if saved.replaces(session, relay))
}

/// Start a single maintenance loop for an owning host. A dropped host exits on its next tick.
pub fn schedule_renewal_checks<F, Fut>(
    wake: std::sync::Arc<tokio::sync::Notify>,
    mut retry: bool,
    mut check: F,
) where
    F: FnMut() -> Fut + Send + 'static,
    Fut: std::future::Future<Output = Result<bool>> + Send,
{
    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = tokio::time::sleep(if retry { RETRY_DELAY } else { WEEK }) => {},
                _ = wake.notified() => {},
            }
            match check().await {
                Ok(false) => break,
                Ok(true) => retry = false,
                Err(error) => {
                    log::warn!("Device credential maintenance failed: {error}");
                    retry = true;
                }
            }
        }
    });
}

/// Load and decrypt the session from disk.
/// Returns `Ok(None)` if the file doesn't exist (not an error).
pub fn load_session() -> Result<Option<(String, String, [u8; 32], String)>> {
    Ok(load_session_detailed()?.map(|s| (s.token, s.user_id, s.master_key, s.relay_url)))
}

/// Load and decrypt the session, including optional account-bound `device_id`.
pub fn load_session_detailed() -> Result<Option<LoadedSession>> {
    crate::remote_persistence::read_current_account_session(
        &session_store_directory()?,
        &crate::remote_persistence::MachineBinding::current(),
    )
}

/// Atomically retire the saved credential before network revocation. A racing
/// renewal then either appears in this record or discards its own candidate.
fn take_session_for_logout(
    active: Option<&(AccountSession, String)>,
) -> Result<Option<LoadedSession>> {
    let _guard = state_lock()?;
    let Some(saved) = load_session_detailed()? else {
        return Ok(None);
    };
    if active.is_some_and(|(session, relay)| !saved.same_device(session, relay)) {
        return Ok(None);
    }
    std::fs::remove_file(session_file_path()?)?;
    Ok(Some(saved))
}

pub async fn logout_session(active: Option<(AccountSession, String)>, revoke: bool) {
    let saved = match take_session_for_logout(active.as_ref()) {
        Ok(saved) => saved,
        Err(error) => {
            log::warn!("Failed to clear saved device credential: {error}");
            None
        }
    };
    if !revoke {
        return;
    }
    if let Some(saved) = saved {
        revoke_saved_tokens(
            saved,
            active.as_ref().map(|(session, _)| session.token.as_str()),
            None,
        )
        .await;
    }
    if let Some((session, relay)) = active {
        if let Err(error) = AccountClient::new().revoke_token(&relay, &session).await {
            log::warn!("Failed to revoke active device credential: {error}");
        }
    }
}

pub fn clear_rejected_session(token: &str) -> Result<()> {
    let _guard = state_lock()?;
    if load_session_detailed()?.is_some_and(|saved| saved.token == token) {
        std::fs::remove_file(session_file_path()?)?;
    }
    Ok(())
}

// ── Credential hint (non-secret: username + relay_url) ─────────────────
// Shared by Desktop and CLI so login forms pre-fill the same values.

fn credential_hint_path() -> Result<PathBuf> {
    Ok(session_store_directory()?.join("account_hint.json"))
}

/// Non-secret login pre-fill (never stores password or master key).
pub use crate::remote_persistence::AccountHintRecord as AccountHint;

/// Persist username + relay URL for the next login form.
pub fn save_credential_hint(username: &str, relay_url: &str) {
    let hint = AccountHint {
        username: username.to_string(),
        relay_url: relay_url.to_string(),
    };
    if let Ok(path) = credential_hint_path() {
        let _ = crate::remote_persistence::write_account_hint(&path, &hint);
    }
}

/// Load the persisted credential hint, if any.
pub fn load_credential_hint() -> Option<AccountHint> {
    crate::remote_persistence::read_account_hint(&credential_hint_path().ok()?)
        .ok()
        .flatten()
}

/// Clear the credential hint (called on logout).
pub fn clear_credential_hint() {
    if let Ok(path) = credential_hint_path() {
        let _ = std::fs::remove_file(&path);
    }
}

#[cfg(test)]
mod renewal_tests {
    use super::*;

    #[test]
    fn device_renewal_uses_the_issue_time_across_weekly_checks() {
        let day = 86400;
        assert_eq!(WEEK.as_secs(), 7 * day as u64);
        assert!(RETRY_DELAY < WEEK);
        for age in [0, 7 * day, 14 * day, 15 * day] {
            assert!(!renewal_due(Some(0), age));
        }
        for age in [15 * day + 1, 21 * day, 30 * day] {
            assert!(renewal_due(Some(0), age));
        }
        assert!(renewal_due(None, day));
        assert!(renewal_due(Some(day + 1), day));
    }

    #[tokio::test]
    async fn a_stale_host_can_wake_the_weekly_check_immediately() {
        let wake = std::sync::Arc::new(tokio::sync::Notify::new());
        let completed = std::sync::Arc::new(tokio::sync::Notify::new());
        let signal = completed.clone();
        schedule_renewal_checks(wake.clone(), false, move || {
            signal.notify_one();
            std::future::ready(Ok(false))
        });
        wake.notify_one();
        tokio::time::timeout(std::time::Duration::from_secs(3), completed.notified())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn startup_skips_fresh_credentials_and_preserves_a_legacy_record_on_rejection() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let temp = tempfile::tempdir().unwrap();
        TEST_STORE.with(|dir| *dir.borrow_mut() = Some(temp.path().to_owned()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let relay = format!("http://{}", listener.local_addr().unwrap());
        let id = "0123456789abcdef0123456789abcdef";
        save_issued_session(
            &AccountSession::new("fresh".into(), "me".into(), [2; 32]),
            &relay,
            Some(id),
            None,
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            renew_session_if_due(true).await.unwrap().unwrap().token,
            "fresh"
        );
        save_session_with_device("legacy", "me", &[2; 32], &relay, Some(id)).unwrap();
        let before = load_session_detailed().unwrap();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut stream = BufReader::new(socket);
            let mut request = String::new();
            while !request.ends_with("\r\n\r\n") {
                assert!(stream.read_line(&mut request).await.unwrap() > 0);
            }
            assert!(request.starts_with("GET /api/devices "));
            stream
                .get_mut()
                .write_all(
                    b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
        });
        assert!(renew_session_if_due(false)
            .await
            .unwrap_err()
            .is::<super::super::account::RelayUnauthorized>());
        server.await.unwrap();
        assert_eq!(load_session_detailed().unwrap(), before);
        TEST_STORE.with(|dir| *dir.borrow_mut() = None);
    }

    #[tokio::test]
    async fn legacy_renewal_waits_for_the_authenticated_device_id() {
        let temp = tempfile::tempdir().unwrap();
        TEST_STORE.with(|dir| *dir.borrow_mut() = Some(temp.path().to_owned()));
        save_session("legacy", "me", &[2; 32], "http://127.0.0.1:1").unwrap();
        let error = renew_session_if_due(true).await.unwrap_err();
        assert!(error.to_string().contains("confirm the session device ID"));
        assert!(!record_authenticated_device("stale", "other").unwrap());
        assert!(load_session_detailed()
            .unwrap()
            .unwrap()
            .device_id
            .is_none());
        assert!(record_authenticated_device("legacy", "0123456789abcdef0123456789abcdef").unwrap());
        assert!(
            !record_authenticated_device("legacy", "0123456789abcdef0123456789abcdef").unwrap()
        );
        TEST_STORE.with(|dir| *dir.borrow_mut() = None);
    }

    #[tokio::test]
    async fn logout_from_a_stale_host_revokes_all_live_rotations_once() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let temp = tempfile::tempdir().unwrap();
        TEST_STORE.with(|dir| *dir.borrow_mut() = Some(temp.path().to_owned()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let relay = format!("http://{}", listener.local_addr().unwrap());
        let now = chrono::Utc::now().timestamp();
        save_session_with_device(
            "expired",
            "me",
            &[2; 32],
            &relay,
            Some("0123456789abcdef0123456789abcdef"),
        )
        .unwrap();
        for (old, new, age) in [
            ("expired", "first", 32),
            ("first", "second", 16),
            ("second", "current", 0),
        ] {
            replace_renewed_session(
                old,
                new,
                "0123456789abcdef0123456789abcdef",
                now - age * 86400,
            )
            .unwrap();
        }
        let other = AccountSession::new("different".into(), "me".into(), [3; 32]);
        assert!(take_session_for_logout(Some(&(other, relay.clone())))
            .unwrap()
            .is_none());
        let saved = load_session_detailed().unwrap().unwrap();
        assert_eq!(saved.superseded_tokens.len(), 1);
        assert_eq!(saved.superseded_tokens[0].token, "second");

        let server = tokio::spawn(async move {
            let mut revoked = Vec::new();
            for _ in 0..2 {
                let (socket, _) = listener.accept().await.unwrap();
                let mut stream = BufReader::new(socket);
                let mut request = String::new();
                while !request.ends_with("\r\n\r\n") {
                    assert!(stream.read_line(&mut request).await.unwrap() > 0);
                }
                assert!(request.starts_with("POST /api/auth/logout "));
                let token = request
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("authorization")
                            .then_some(value.trim())
                    })
                    .unwrap();
                revoked.push(token.to_owned());
                stream
                    .get_mut()
                    .write_all(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n")
                    .await
                    .unwrap();
            }
            revoked.sort();
            revoked
        });
        let stale = AccountSession::new("second".into(), "me".into(), [2; 32]);
        tokio::time::timeout(std::time::Duration::from_secs(5), async {
            logout_session(Some((stale, relay)), true).await;
            assert_eq!(server.await.unwrap(), ["Bearer current", "Bearer second"]);
        })
        .await
        .unwrap();
        assert!(load_session_detailed().unwrap().is_none());
        assert!(replace_renewed_session(
            "current",
            "late",
            "0123456789abcdef0123456789abcdef",
            now
        )
        .unwrap()
        .is_none());
        TEST_STORE.with(|dir| *dir.borrow_mut() = None);
    }

    #[test]
    fn auth_ok_and_stale_hosts_cannot_overwrite_a_renewal_or_logout() {
        let temp = tempfile::tempdir().unwrap();
        // Scope this test's store to its own thread, independently of API fixtures.
        TEST_STORE.with(|dir| *dir.borrow_mut() = Some(temp.path().to_owned()));
        let id = "0123456789abcdef0123456789abcdef";
        let relay = openbitfun_product_domains::account::DEFAULT_RELAY_URL;
        save_session("old", "account", &[1; 32], relay).unwrap();
        record_authenticated_device("old", id).unwrap();
        assert_eq!(load_session_detailed().unwrap().unwrap().issued_at, None);
        let renewed = replace_renewed_session("old", "new", id, 1234)
            .unwrap()
            .unwrap();
        assert!(renewed.replaces(
            &super::super::account::AccountSession::new("old".into(), "account".into(), [1; 32]),
            relay.split('?').next().unwrap()
        ));
        record_authenticated_device("old", "wrong-id").unwrap();
        record_authenticated_device("new", id).unwrap();
        clear_rejected_session("old").unwrap();
        assert_eq!(
            replace_renewed_session("old", "stale", id, 9999).unwrap(),
            Some(renewed)
        );
        take_session_for_logout(None).unwrap();
        assert!(replace_renewed_session("new", "stale", id, 9999)
            .unwrap()
            .is_none());
        TEST_STORE.with(|dir| *dir.borrow_mut() = None);
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn private_file_writer_uses_owner_only_permissions_and_replaces_atomically() {
        let root = tempfile::tempdir().unwrap();
        let private_dir = root.path().join("private");
        let path = private_dir.join("session.enc");

        crate::remote_persistence::write_private_bytes(&path, b"first").unwrap();
        crate::remote_persistence::write_private_bytes(&path, b"second").unwrap();

        assert_eq!(std::fs::read(&path).unwrap(), b"second");
        assert_eq!(
            std::fs::metadata(&private_dir)
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(std::fs::read_dir(&private_dir)
            .unwrap()
            .all(|entry| entry.unwrap().path() == path));
    }
}
