//! CLI Peer Device Mode host.
//!
//! Desktop controllers reach this process over the same HostInvoke / DeviceEvent
//! envelopes used for Desktop peers. Execution goes through Core services
//! (no webview / Tauri bridge).

mod args;
mod bootstrap;
mod commands;
mod control;
#[cfg(test)]
mod deny;
mod dispatch;
mod fanout;
mod state;
mod workspace_dto;

pub(crate) use bootstrap::ensure_peer_host_ready;
pub(crate) use control::detach_controller as remove_controller;
pub(crate) use dispatch::{handle_device_event_command, handle_host_invoke};

pub(crate) async fn update_controller_presence(online_device_ids: Vec<String>) {
    control::retain_online_controllers(online_device_ids.iter().map(String::as_str)).await;
}
