#[cfg(feature = "tauri-adapter")]
pub mod tauri;

#[cfg(feature = "tauri-adapter")]
pub use tauri::{TauriBatchedEvent, TauriTransportAdapter, TAURI_EVENT_BATCH_NAME};
