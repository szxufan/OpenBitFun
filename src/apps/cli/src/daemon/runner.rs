//! Headless daemon main loop.
//!
//! The daemon is a full Peer Device Mode host without the TUI: it initializes
//! the same core services as the interactive CLI, restores the persisted
//! account session, and holds the relay device-routing connection so this
//! device stays reachable whenever the machine is up.

use std::time::Duration;

use anyhow::{anyhow, Result};

use openbitfun_core::service::remote_connect::DeviceIdentity;

use crate::{runtime, BootstrapProfile};

use super::pid;

/// Run the daemon in the foreground until signalled to stop, or until the
/// relay rejects the account token (logout / token revoked elsewhere).
pub(crate) async fn run_daemon() -> Result<()> {
    if pid::is_daemon_running() {
        return Err(anyhow!(
            "another openbitfun daemon is already running (see `openbitfun daemon status`)"
        ));
    }

    // The daemon is not bound to the caller's cwd; peer commands carry their
    // own workspace paths. Home is a stable root for the runtime context.
    let workspace_root = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    let runtime = crate::initialize_core_services(
        &workspace_root,
        runtime::approval::CliApprovalPolicy::Ask,
        BootstrapProfile::Interactive,
    )
    .await?;

    let account = runtime.account_runtime();
    let mut routing_started_by_login = false;
    if let Some(user_id) = account.try_restore_session().await {
        tracing::info!("Daemon restored account session for user {user_id}");
    } else {
        // Session-expiry path: attempt one silent renewal from the persisted
        // refresh token before asking a human to sign in again.
        match account.try_refresh_relogin().await {
            Ok(login) => {
                tracing::info!(
                    "Daemon renewed the account session for user {}",
                    login.user_id
                );
                // The renewed login already adopted device routing.
                routing_started_by_login = true;
            }
            Err(error) => {
                tracing::warn!("Daemon could not renew the persisted account session: {error:#}");
                return Err(anyhow!(
                    "not logged in; run `openbitfun login` (or `/login` in the TUI), then start the daemon again"
                ));
            }
        }
    }

    let device =
        DeviceIdentity::from_current_machine().map_err(|e| anyhow!("detect device: {e}"))?;
    if !routing_started_by_login {
        account.restore_device_routing(&device.device_name).await?;
    }

    pid::write_pid_file()?;
    tracing::info!("openbitfun daemon running (pid {})", std::process::id());

    let mut expired_check = tokio::time::interval(Duration::from_secs(5));
    expired_check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = shutdown_signal() => {
                tracing::info!("Daemon shutdown signal received");
                break;
            }
            _ = expired_check.tick() => {
                if account.is_token_expired() {
                    // Session-expiry path: attempt one silent renewal before
                    // exiting for an interactive sign-in. Renewal adopts a
                    // fresh account context, which first retires the
                    // background routing owner; when this daemon IS the owner
                    // recorded in the pid file, an in-process renewal would
                    // signal itself and reject the renewed credential, so it
                    // is only attempted when no live owner is recorded.
                    if !pid::is_daemon_running() {
                        match account.try_refresh_relogin().await {
                            Ok(login) => {
                                tracing::info!(
                                    "Daemon renewed the account session for user {}",
                                    login.user_id
                                );
                                continue;
                            }
                            Err(error) => {
                                tracing::warn!(
                                    "Daemon could not renew the expired account session: {error:#}"
                                );
                            }
                        }
                    }
                    // Exit 0 on purpose: re-authentication needs a human, so
                    // Restart=on-failure must not loop the daemon.
                    tracing::warn!(
                        "Account token rejected by the relay; daemon exiting; run `openbitfun login` (or `/login` in the TUI) to sign in again"
                    );
                    break;
                }
            }
        }
    }

    runtime.account_routing().stop_device_routing().await;
    pid::remove_pid_file();
    crate::shutdown_mcp_servers().await;
    tracing::info!("openbitfun daemon stopped");
    Ok(())
}

#[cfg(unix)]
async fn shutdown_signal() {
    use tokio::signal::unix::{signal, SignalKind};
    match (
        signal(SignalKind::terminate()),
        signal(SignalKind::interrupt()),
    ) {
        (Ok(mut sigterm), Ok(mut sigint)) => {
            tokio::select! {
                _ = sigterm.recv() => {}
                _ = sigint.recv() => {}
            }
        }
        _ => {
            tracing::warn!(
                "Failed to install signal handlers; daemon can only stop on token expiry"
            );
            std::future::pending::<()>().await;
        }
    }
}

#[cfg(not(unix))]
async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}
