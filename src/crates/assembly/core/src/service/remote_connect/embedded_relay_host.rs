//! Host capability required by Remote Connect LAN orchestration.
//!
//! Product assembly decides when the embedded relay is needed. Concrete
//! listener, router, static-file, and task lifecycle details belong to the app
//! host implementing this port.

use anyhow::Result;
use openbitfun_services_integrations::remote_connect::DeviceIdentity;

/// An owner device token minted directly in the embedded relay's database.
///
/// The embedded LAN Relay is process-local and fully owned by the host, so
/// provisioning its owner device is not a trust-boundary crossing. The token
/// targets the same account the host signed in with on its configured relay,
/// preserving same-account semantics for LAN control.
pub struct EmbeddedOwnerSession {
    pub token: String,
}

#[async_trait::async_trait]
pub trait EmbeddedRelayHost: Send + Sync {
    async fn start(&self, port: u16, static_dir: Option<String>) -> Result<()>;

    async fn stop(&self);

    /// Mint (or reuse) the embedded relay's owner session for `user_id`.
    /// Default hosts without a local database answer with an error; LAN
    /// start then fails loudly instead of silently degrading.
    async fn provision_owner_session(
        &self,
        _relay_url: &str,
        _user_id: &str,
        _username: &str,
        _device: &DeviceIdentity,
    ) -> Result<EmbeddedOwnerSession> {
        anyhow::bail!("this host cannot provision embedded relay sessions")
    }
}
