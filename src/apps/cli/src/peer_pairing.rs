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
        /// Peer device id from the account device list.
        #[arg(long)]
        peer: String,
    },
    /// Accept a pairing code generated on a peer device.
    Complete {
        /// The pairing code copied from the peer.
        #[arg(long)]
        code: String,
    },
    /// List pairing entries for the logged-in account.
    List,
    /// Remove one pairing; traffic with that peer returns to the legacy key.
    Remove {
        #[arg(long)]
        peer: String,
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
        session: AccountSession::new(
            loaded.token,
            loaded.user_id,
            loaded.master_key,
        ),
        relay_url: loaded.relay_url,
        device_id,
    })
}

pub async fn run(action: PeerPairingAction) -> Result<()> {
    match action {
        PeerPairingAction::Offer { peer } => offer(&peer).await,
        PeerPairingAction::Complete { code } => complete(&code).await,
        PeerPairingAction::List => list().await,
        PeerPairingAction::Remove { peer } => remove(&peer).await,
    }
}

async fn offer(peer_device_id: &str) -> Result<()> {
    let peer_device_id = peer_device_id.trim();
    anyhow::ensure!(!peer_device_id.is_empty(), "--peer is required");
    let context = load_pairing_context()?;
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

async fn complete(code: &str) -> Result<()> {
    let code = code.trim();
    anyhow::ensure!(!code.is_empty(), "--code is required");
    let payload = pairing::parse_psk_pairing_code(code).context("invalid pairing code")?;
    let context = load_pairing_context()?;
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
            &payload.offerer_device_id,
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
            entry
                .peer_name
                .as_deref()
                .unwrap_or_default(),
        );
    }
    Ok(())
}

async fn remove(peer_device_id: &str) -> Result<()> {
    let peer_device_id = peer_device_id.trim();
    anyhow::ensure!(!peer_device_id.is_empty(), "--peer is required");
    let context = load_pairing_context()?;
    let removed = psk_store::remove_entry(
        &context.relay_url,
        &context.session.user_id,
        peer_device_id,
    )
    .context("update pairing vault")?;
    if removed {
        println!("Removed pairing with {peer_device_id}; traffic uses the relay-directory key.");
    } else {
        println!("No pairing stored for {peer_device_id}.");
    }
    Ok(())
}
