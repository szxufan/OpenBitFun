//! CLI commands for out-of-band PSK device pairing.
//!
//! Pairing upgrades a same-account peer link from relay-directory trust to an
//! out-of-band shared secret, so a malicious relay cannot impersonate either
//! device. `pair-offer` prints a transferable code; the peer completes it in
//! its own UI (desktop dialog or another CLI) and both sides switch their
//! pairwise encryption to the PSK derivation.

use anyhow::{Context, Result};

use openbitfun_core::service::remote_connect::pairing;
use openbitfun_core::service::remote_connect::psk_store::{self, PeerPskStatus};
use openbitfun_core::service::remote_connect::session_store;
use openbitfun_core::service::remote_connect::{AccountClient, AccountSession, DeviceIdentity};

#[derive(Debug, clap::Subcommand)]
pub enum PeerPairingAction {
    /// Generate a pairing code for one peer device (transfer it yourself).
    Offer {
        /// Peer device id from the account device list. Required for a
        /// same-account offer; ignored when `--grant` names a shared host.
        #[arg(long)]
        peer: Option<String>,
        /// Cross-account shared host: mint a grant-keyed pairing offer instead
        /// of a same-account device offer. Requires an accepted grant id.
        #[arg(long)]
        grant: Option<String>,
    },
    /// Accept a pairing code generated on a peer device.
    Complete {
        /// The pairing code copied from the peer.
        #[arg(long)]
        code: String,
        /// Cross-account shared host: resolve the host key through this grant
        /// (the account directory stays closed to grantees).
        #[arg(long)]
        grant: Option<String>,
    },
    /// List pairing entries for the logged-in account.
    List,
    /// Remove one pairing; traffic with that peer returns to the legacy key.
    Remove {
        #[arg(long)]
        peer: String,
    },
    /// Show or change this host's pairing enforcement policy. Enforcement is
    /// ON by default: only paired devices may control this machine.
    Policy {
        /// Set whether unpaired peer devices are refused control entirely.
        #[arg(long)]
        require_paired: Option<bool>,
    },
    /// Manage cross-account device grants (shared hosts).
    Grants {
        #[command(subcommand)]
        action: GrantAction,
    },
}

#[derive(Debug, clap::Subcommand)]
pub enum GrantAction {
    /// Offer this machine to another account. Only the shared machine's own
    /// device session can mint an invite.
    Offer {
        /// Grantee account id to share this machine with.
        #[arg(long)]
        grantee: String,
        /// Grant scope: drive (default) or observe (read-only).
        #[arg(long, default_value = "drive")]
        scope: String,
        /// Optional unix expiry.
        #[arg(long)]
        expires_at: Option<i64>,
    },
    /// Redeem a one-time invite as the logged-in (grantee) account.
    Accept {
        #[arg(long)]
        grant: String,
        #[arg(long)]
        secret: String,
    },
    /// List owned (management) and received (shared-with-me) grants.
    List,
    /// Revoke a grant this account owns.
    Revoke {
        #[arg(long)]
        grant: String,
    },
}

struct PairingContext {
    session: AccountSession,
    relay_url: String,
    device_id: String,
}

fn load_pairing_context() -> Result<PairingContext> {
    let loaded = session_store::load_session_detailed()
        .context("read persisted account session")?
        .ok_or_else(|| {
            anyhow::anyhow!("no relay account on this machine; log in before pairing")
        })?;
    let device_id = match loaded.device_id {
        Some(id) if !id.trim().is_empty() => id,
        _ => {
            DeviceIdentity::from_current_machine()
                .context("resolve local device identity")?
                .device_id
        }
    };
    Ok(PairingContext {
        session: AccountSession::new(loaded.token, loaded.user_id, loaded.master_key),
        relay_url: loaded.relay_url,
        device_id,
    })
}

pub async fn run(action: PeerPairingAction) -> Result<()> {
    match action {
        PeerPairingAction::Offer { peer, grant } => {
            offer(peer.as_deref().unwrap_or(""), grant.as_deref()).await
        }
        PeerPairingAction::Complete { code, grant } => complete(&code, grant.as_deref()).await,
        PeerPairingAction::List => list().await,
        PeerPairingAction::Remove { peer } => remove(&peer).await,
        PeerPairingAction::Policy { require_paired } => policy(require_paired).await,
        PeerPairingAction::Grants { action } => run_grants(action).await,
    }
}

async fn run_grants(action: GrantAction) -> Result<()> {
    let context = load_pairing_context()?;
    let client = AccountClient::new();
    match action {
        GrantAction::Offer {
            grantee,
            scope,
            expires_at,
        } => {
            anyhow::ensure!(
                scope == "drive" || scope == "observe",
                "scope must be drive or observe"
            );
            let offered = client
                .offer_device_grant(
                    &context.relay_url,
                    &context.session,
                    &context.device_id,
                    &grantee,
                    &scope,
                    expires_at,
                )
                .await
                .context("offer device grant")?;
            println!(
                "Grant {} offered to {} (scope {}).",
                offered.grant.grant_id, grantee, offered.grant.scope
            );
            println!(
                "One-time invite secret (shown once, expires at {}):",
                offered.invite_expires_at
            );
            println!("{}", offered.invite_secret);
        }
        GrantAction::Accept { grant, secret } => {
            let accepted = client
                .accept_device_grant(&context.relay_url, &context.session, &grant, &secret)
                .await
                .context("accept device grant")?;
            println!(
                "Accepted grant {} for device {}.",
                accepted.grant_id, accepted.device_id
            );
        }
        GrantAction::List => {
            let list = client
                .list_device_grants(&context.relay_url, &context.session)
                .await
                .context("list device grants")?;
            println!("Owned grants:");
            for g in list.owned {
                println!(
                    "  {}  -> {}  device {}  {}  {}",
                    g.grant_id, g.grantee_user_id, g.device_id, g.scope, g.state
                );
            }
            println!("Shared with me:");
            for entry in list.received {
                println!(
                    "  {}  owner {}  device {} ({})  {}  online={}",
                    entry.grant.grant_id,
                    entry.grant.owner_user_id,
                    entry.grant.device_id,
                    entry.device_name,
                    entry.grant.scope,
                    entry.online
                );
            }
        }
        GrantAction::Revoke { grant } => {
            client
                .revoke_device_grant(&context.relay_url, &context.session, &grant)
                .await
                .context("revoke device grant")?;
            println!("Revoked grant {grant}.");
        }
    }
    Ok(())
}

async fn offer(peer_device_id: &str, grant: Option<&str>) -> Result<()> {
    let context = load_pairing_context()?;
    if let Some(grant_id) = grant.map(str::trim).filter(|id| !id.is_empty()) {
        let (entry, psk, _grantee) = context
            .session
            .offer_grant_pairing(&context.relay_url, grant_id)
            .await
            .context("offer grant pairing")?;
        let payload = pairing::PskPairingPayload {
            psk,
            offerer_device_id: context.device_id.clone(),
            offerer_public_key: openbitfun_core::service::remote_connect::device_crypto::public_key(
                &context.session.master_key,
            ),
        };
        let code = pairing::encode_psk_pairing_code(&payload).context("encode pairing code")?;
        println!("Pairing code for shared-host grant {grant_id} (transfer it to the grantee):");
        println!("{code}");
        println!("Pairing id: {}", entry.psk_id);
        return Ok(());
    }
    let peer_device_id = peer_device_id.trim();
    anyhow::ensure!(!peer_device_id.is_empty(), "--peer is required");
    let psk = pairing::generate_psk();
    let entry = psk_store::store_entry(
        &context.relay_url,
        &context.session.user_id,
        peer_device_id,
        &psk,
        PeerPskStatus::Offered,
        None,
    )
    .context("store pairing offer")?;
    let payload = pairing::PskPairingPayload {
        psk,
        offerer_device_id: context.device_id.clone(),
        offerer_public_key: openbitfun_core::service::remote_connect::device_crypto::public_key(
            &context.session.master_key,
        ),
    };
    let code = pairing::encode_psk_pairing_code(&payload).context("encode pairing code")?;
    println!("Pairing code (transfer it to the peer over a channel you trust):");
    println!("{code}");
    println!("Pairing id: {}", entry.psk_id);
    println!(
        "Traffic with this peer keeps the relay-directory key until the peer completes pairing."
    );
    Ok(())
}

async fn complete(code: &str, grant: Option<&str>) -> Result<()> {
    let code = code.trim();
    anyhow::ensure!(!code.is_empty(), "--code is required");
    let payload = pairing::parse_psk_pairing_code(code).context("invalid pairing code")?;
    let context = load_pairing_context()?;
    // For a shared host the host key is resolved through the grant's peer-key
    // route and the pairing is stored under the composite PeerTarget; the raw
    // offerer device id must not collide with the internal composite namespace.
    let (peer_key, entry) = match grant.map(str::trim).filter(|id| !id.is_empty()) {
        Some(grant_id) => {
            anyhow::ensure!(
                !payload.offerer_device_id.starts_with("grant-v1:"),
                "invalid pairing code: offerer device id collides with the composite namespace"
            );
            let entry = context
                .session
                .complete_grant_pairing(
                    &context.relay_url,
                    grant_id,
                    &payload.offerer_device_id,
                    &payload.psk,
                    payload.offerer_public_key,
                )
                .await
                .context("complete grant pairing")?;
            let target = context
                .session
                .grants
                .resolve_pairing_target(
                    &AccountClient::new(),
                    &context.relay_url,
                    &context.session,
                    grant_id,
                    &payload.offerer_device_id,
                )
                .await
                .context("resolve grant pairing target")?;
            (target.key(), entry)
        }
        None => {
            let directory_key = context
                .session
                .peer_public_key(&context.relay_url, &payload.offerer_device_id)
                .await
                .context("resolve offerer device key from the relay directory")?;
            anyhow::ensure!(
                directory_key == payload.offerer_public_key,
                "pairing refused: the offerer's relay directory key does not match the pairing code; \
                 the directory may be tampered with"
            );
            let entry = psk_store::store_entry(
                &context.relay_url,
                &context.session.user_id,
                &payload.offerer_device_id,
                &payload.psk,
                PeerPskStatus::Paired,
                None,
            )
            .context("store completed pairing")?;
            (payload.offerer_device_id.clone(), entry)
        }
    };
    let command_json = serde_json::json!({
        "cmd": "host_invoke",
        "command": "peer_psk_confirm",
        "args": {
            "psk_id": entry.psk_id,
            "controller_device_id": context.device_id,
        }
    })
    .to_string();
    let confirmed = match AccountClient::new()
        .device_rpc(
            &context.relay_url,
            &context.session,
            &peer_key,
            &command_json,
        )
        .await
    {
        Ok(_) => true,
        Err(error) => {
            tracing::warn!("Pairing stored, but the confirmation RPC failed: {error}");
            false
        }
    };
    println!(
        "Paired with device {} (pairing id {}).",
        payload.offerer_device_id, entry.psk_id
    );
    if confirmed {
        println!("The peer acknowledged; both sides now use the paired key.");
    } else {
        println!(
            "The peer did not acknowledge (offline or older version). The pairing activates \
             when the peer next answers; re-run this command to resend the confirmation."
        );
    }
    Ok(())
}

async fn list() -> Result<()> {
    let context = load_pairing_context()?;
    let entries = psk_store::list_entries(&context.relay_url, &context.session.user_id)
        .context("read pairing vault")?;
    if entries.is_empty() {
        println!("No device pairings on this machine.");
        return Ok(());
    }
    for entry in entries {
        println!(
            "{}  {}  pairing id {}  {}",
            entry.peer_device_id,
            match entry.status {
                PeerPskStatus::Offered => "offered",
                PeerPskStatus::Paired => "paired ",
            },
            entry.psk_id,
            entry.peer_name.as_deref().unwrap_or_default(),
        );
    }
    Ok(())
}

async fn remove(peer_device_id: &str) -> Result<()> {
    let peer_device_id = peer_device_id.trim();
    anyhow::ensure!(!peer_device_id.is_empty(), "--peer is required");
    let context = load_pairing_context()?;
    let removed =
        psk_store::remove_entry(&context.relay_url, &context.session.user_id, peer_device_id)
            .context("update pairing vault")?;
    if removed {
        println!("Removed pairing with {peer_device_id}; traffic uses the relay-directory key.");
    } else {
        println!("No pairing stored for {peer_device_id}.");
    }
    Ok(())
}

async fn policy(require_paired: Option<bool>) -> Result<()> {
    match require_paired {
        Some(value) => {
            psk_store::store_policy(&psk_store::PairingPolicy {
                require_paired_controller: value,
            })
            .context("store pairing policy")?;
            println!(
                "Pairing enforcement is now {}. Unpaired peer devices {} control this machine.",
                if value { "ON" } else { "OFF" },
                if value {
                    "can no longer"
                } else {
                    "are again allowed to"
                },
            );
        }
        None => {
            let current = psk_store::load_policy().context("read pairing policy")?;
            println!(
                "Pairing enforcement: {} (only paired devices may control this machine)",
                if current.require_paired_controller {
                    "ON"
                } else {
                    "OFF"
                }
            );
            println!("Change it with `peer pair policy --require-paired <true|false>`.");
        }
    }
    Ok(())
}
