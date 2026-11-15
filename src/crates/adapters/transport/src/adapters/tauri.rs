//! Tauri transport adapter.
//!
//! This adapter owns only Tauri delivery. Agentic event names and payload
//! shapes are projected by `openbitfun-events`.

#[cfg(feature = "tauri-adapter")]
use crate::traits::TransportAdapter;
use async_trait::async_trait;
use log::warn;
use openbitfun_events::{project_agentic_frontend_event, AgenticEvent};
#[cfg(feature = "tauri-adapter")]
use serde::{Deserialize, Serialize};
use std::fmt;

#[cfg(feature = "tauri-adapter")]
use tauri::{AppHandle, Emitter};

#[cfg(feature = "tauri-adapter")]
pub const TAURI_EVENT_BATCH_NAME: &str = "agentic://event-batch";

#[cfg(feature = "tauri-adapter")]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TauriBatchedEvent {
    pub event: String,
    pub payload: serde_json::Value,
}

#[cfg(feature = "tauri-adapter")]
pub struct TauriTransportAdapter {
    app_handle: AppHandle,
}

#[cfg(feature = "tauri-adapter")]
fn is_batchable_streaming_event(item: &TauriBatchedEvent) -> bool {
    if item.event == "agentic://text-chunk" {
        return true;
    }
    if item.event != "agentic://tool-event" {
        return false;
    }
    let Some(tool_event) = item.payload.get("toolEvent") else {
        return false;
    };
    let tool_name = tool_event
        .get("effective_tool_name")
        .and_then(|v| v.as_str())
        .or_else(|| tool_event.get("tool_name").and_then(|v| v.as_str()));
    if tool_name == Some("AskUserQuestion") {
        return false;
    }
    matches!(
        tool_event.get("event_type").and_then(|v| v.as_str()),
        Some("ParamsPartial" | "Progress" | "Streaming" | "StreamChunk" | "GenUIStream")
    )
}

#[cfg(feature = "tauri-adapter")]
impl TauriTransportAdapter {
    pub fn new(app_handle: AppHandle) -> Self {
        Self { app_handle }
    }

    /// Deliver ordered frontend events while coalescing only contiguous runs of
    /// high-frequency streaming chunk events onto `TAURI_EVENT_BATCH_NAME`.
    /// Lifecycle and state-transition events always emit on their dedicated
    /// channel in arrival order so standalone listeners receive them even when
    /// `AgenticEventListener` is inactive.
    pub fn emit_batch(&self, events: Vec<TauriBatchedEvent>) -> anyhow::Result<()> {
        if events.is_empty() {
            return Ok(());
        }
        let mut streaming_run: Vec<TauriBatchedEvent> = Vec::new();
        let flush_run = |run: &mut Vec<TauriBatchedEvent>| -> anyhow::Result<()> {
            match run.len() {
                0 => Ok(()),
                1 => {
                    let single = run.pop().expect("single item checked above");
                    self.app_handle.emit(&single.event, single.payload)?;
                    Ok(())
                }
                _ => {
                    let batch = std::mem::take(run);
                    self.app_handle.emit(TAURI_EVENT_BATCH_NAME, batch)?;
                    Ok(())
                }
            }
        };

        for item in events {
            if is_batchable_streaming_event(&item) {
                streaming_run.push(item);
            } else {
                flush_run(&mut streaming_run)?;
                self.app_handle.emit(&item.event, item.payload)?;
            }
        }
        flush_run(&mut streaming_run)?;
        Ok(())
    }
}

#[cfg(feature = "tauri-adapter")]
impl fmt::Debug for TauriTransportAdapter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TauriTransportAdapter")
            .field("adapter_type", &"tauri")
            .finish()
    }
}

#[cfg(feature = "tauri-adapter")]
#[async_trait]
impl TransportAdapter for TauriTransportAdapter {
    async fn emit_event(&self, event: AgenticEvent) -> anyhow::Result<()> {
        let Some(projected) = project_agentic_frontend_event(event) else {
            warn!("Unhandled AgenticEvent type in TauriAdapter");
            return Ok(());
        };
        self.app_handle
            .emit(projected.event_name.as_str(), projected.payload)?;
        Ok(())
    }

    async fn emit_generic(
        &self,
        event_name: &str,
        payload: serde_json::Value,
    ) -> anyhow::Result<()> {
        self.app_handle.emit(event_name, payload)?;
        Ok(())
    }
}

#[cfg(all(test, feature = "tauri-adapter"))]
mod tests {
    use super::{is_batchable_streaming_event, TauriBatchedEvent};
    use serde_json::json;

    #[test]
    fn only_high_frequency_streaming_events_are_batchable() {
        assert!(is_batchable_streaming_event(&TauriBatchedEvent {
            event: "agentic://text-chunk".to_string(),
            payload: json!({ "sessionId": "s1", "text": "hi" }),
        }));
        assert!(is_batchable_streaming_event(&TauriBatchedEvent {
            event: "agentic://tool-event".to_string(),
            payload: json!({
                "sessionId": "s1",
                "toolEvent": { "event_type": "ParamsPartial", "tool_name": "Edit" }
            }),
        }));
        assert!(!is_batchable_streaming_event(&TauriBatchedEvent {
            event: "agentic://tool-event".to_string(),
            payload: json!({
                "sessionId": "s1",
                "toolEvent": { "event_type": "ParamsPartial", "tool_name": "AskUserQuestion" }
            }),
        }));
        assert!(!is_batchable_streaming_event(&TauriBatchedEvent {
            event: "agentic://tool-event".to_string(),
            payload: json!({
                "sessionId": "s1",
                "toolEvent": { "event_type": "Completed", "tool_name": "Edit" }
            }),
        }));
        assert!(!is_batchable_streaming_event(&TauriBatchedEvent {
            event: "agentic://dialog-turn-completed".to_string(),
            payload: json!({ "sessionId": "s1", "turnId": "t1" }),
        }));
    }
}
