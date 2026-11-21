//! A chat and its in-flight turns share the same renewable control credential.
use crate::remote_connect::account::{
    AccountClient, AccountSession, DeviceInfo, RelayUnauthorized,
};
use anyhow::{anyhow, Result};
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};
use tokio::sync::Mutex;

type Issued = (String, AccountSession);
type Provider = Arc<dyn Fn() -> Pin<Box<dyn Future<Output = Option<Issued>> + Send>> + Send + Sync>;
static PROVIDER: OnceLock<Provider> = OnceLock::new();
const LIFETIME: i64 = 24 * 3600;

pub fn set_delegated_identity_provider<F, Fut>(f: F)
where
    F: Fn() -> Fut + Send + Sync + 'static,
    Fut: Future<Output = Option<Issued>> + Send + 'static,
{
    let _ = PROVIDER.set(Arc::new(move || Box::pin(f())));
}

pub async fn issue() -> Option<(String, Delegation)> {
    let (url, session) = PROVIDER.get()?().await?;
    Some((url, session.into()))
}

#[derive(Debug, Clone)]
pub struct Delegation(Arc<Mutex<Credential>>);

#[derive(Debug)]
struct Credential {
    session: AccountSession,
    issued_at: i64,
    retry_at: i64,
}

impl From<AccountSession> for Delegation {
    fn from(session: AccountSession) -> Self {
        Self(Arc::new(Mutex::new(Credential {
            session,
            issued_at: chrono::Utc::now().timestamp(),
            retry_at: 0,
        })))
    }
}

impl Delegation {
    pub async fn session(&self, url: &str) -> Result<AccountSession> {
        self.refresh(url, None, PROVIDER.get()).await
    }

    async fn refresh(
        &self,
        url: &str,
        rejected: Option<&str>,
        provider: Option<&Provider>,
    ) -> Result<AccountSession> {
        let mut current = self.0.lock().await;
        let now = chrono::Utc::now().timestamp();
        let age = now.saturating_sub(current.issued_at);
        let rejected_here = rejected == Some(current.session.token.as_str());
        if !rejected_here && (0..LIFETIME / 2).contains(&age) {
            return Ok(current.session.clone());
        }
        if now >= current.retry_at {
            if let Some(provider) = provider {
                if let Some((relay, session)) = provider().await {
                    if !openbitfun_product_domains::account::same_relay_deployment(&relay, url)
                        || session.user_id != current.session.user_id
                    {
                        anyhow::bail!("Bot account changed during credential renewal");
                    }
                    *current = Credential {
                        session,
                        issued_at: now,
                        retry_at: 0,
                    };
                    return Ok(current.session.clone());
                }
            }
            current.retry_at = now + 60;
        }
        if !rejected_here && (0..LIFETIME).contains(&age) {
            return Ok(current.session.clone());
        }
        Err(anyhow!(
            "Could not renew the bot credential. Check the host account and Relay connection."
        ))
    }

    pub async fn list_devices(&self, relay: &str) -> Result<Vec<DeviceInfo>> {
        self.list_devices_with(relay, PROVIDER.get()).await
    }

    async fn list_devices_with(
        &self,
        relay: &str,
        provider: Option<&Provider>,
    ) -> Result<Vec<DeviceInfo>> {
        let client = AccountClient::new();
        let session = self.refresh(relay, None, provider).await?;
        match client.list_devices(relay, &session).await {
            Err(error) if error.is::<RelayUnauthorized>() => {
                let replacement = self.refresh(relay, Some(&session.token), provider).await?;
                client.list_devices(relay, &replacement).await
            }
            result => result,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn session(token: &str, user: &str) -> AccountSession {
        AccountSession::new(token.into(), user.into(), [1; 32])
    }

    fn issuer(url: String, user: &'static str) -> Provider {
        Arc::new(move || {
            Box::pin(std::future::ready(Some((
                url.clone(),
                session("replacement", user),
            ))))
        })
    }

    #[tokio::test]
    async fn a_remote_turn_and_its_chat_share_renewal_and_keep_their_original_owner() {
        let relay = "http://localhost";
        let chat = Delegation::from(session("original", "alice"));
        let turn = chat.clone();
        let provider = issuer(relay.into(), "alice");
        assert_eq!(
            chat.refresh(relay, None, Some(&provider))
                .await
                .unwrap()
                .token,
            "original"
        );
        chat.0.lock().await.issued_at -= LIFETIME / 2;
        assert!(turn
            .refresh(relay, None, Some(&issuer(relay.into(), "bob")))
            .await
            .is_err());
        assert_eq!(
            turn.refresh(relay, None, Some(&provider))
                .await
                .unwrap()
                .token,
            "replacement"
        );
        assert_eq!(
            chat.refresh(relay, Some("original"), None)
                .await
                .unwrap()
                .token,
            "replacement"
        );
    }

    #[tokio::test]
    async fn device_list_retries_once_with_a_fresh_delegation_after_401() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        for second_status in ["200 OK", "401 Unauthorized"] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let url = format!("http://{}", listener.local_addr().unwrap());
            let server = tokio::spawn(async move {
                for (token, status) in [
                    ("original", "401 Unauthorized"),
                    ("replacement", second_status),
                ] {
                    let (socket, _) = listener.accept().await.unwrap();
                    let mut stream = BufReader::new(socket);
                    let mut headers = String::new();
                    while !headers.ends_with("\r\n\r\n") {
                        assert!(stream.read_line(&mut headers).await.unwrap() > 0);
                    }
                    assert!(headers.starts_with("GET /api/devices "));
                    assert!(headers
                        .to_ascii_lowercase()
                        .contains(&format!("authorization: bearer {token}")));
                    let reply = format!("HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n[]");
                    stream.get_mut().write_all(reply.as_bytes()).await.unwrap();
                }
            });
            let account = Delegation::from(session("original", "alice"));
            let provider = issuer(url.clone(), "alice");
            let result = account.list_devices_with(&url, Some(&provider)).await;
            server.await.unwrap();
            if second_status == "200 OK" {
                assert!(result.unwrap().is_empty());
            } else {
                assert!(result.unwrap_err().is::<RelayUnauthorized>());
            }
        }
    }
}
