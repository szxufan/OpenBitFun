//! Host-owned execution facts for ACP prompts. Independent of the protocol
//! session mutex, which is held for the complete streaming request.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use openbitfun_core::service::session::DialogTurnData;
use openbitfun_events::AgenticEvent;
use serde_json::{json, Value};

/// Host-injected observation port. The ACP client does not select a Runtime
/// implementation; hosts adapt their existing ordered journal to these facts.
pub trait AcpProjectionJournal: Send + Sync {
    fn record(&self, event: &AgenticEvent) -> Option<AcpEventCursor>;
    fn snapshot(&self, session: &str) -> AcpEventProjectionSnapshot;
    fn pending_checkpoint(&self, session: &str) -> Vec<AgenticEvent>;
    fn checkpoint_saved(&self, session: &str);
}

#[derive(Debug, Clone)]
pub struct AcpEventCursor {
    pub stream_id: String,
    pub cursor: u64,
}

#[derive(Debug, Clone)]
pub struct AcpEventProjectionSnapshot {
    pub session_id: String,
    pub stream_id: String,
    pub cursor: u64,
    pub active_turn_id: Option<String>,
    pub events: Vec<AgenticEvent>,
}

fn attach_cursor(payload: &mut Value, cursor: AcpEventCursor) {
    payload["__openbitfunRuntimeStreamId"] = cursor.stream_id.into();
    payload["__openbitfunRuntimeEventCursor"] = cursor.cursor.into();
}

struct SubmissionReceipt {
    client: String,
    input: String,
    original_input: String,
}

impl SubmissionReceipt {
    fn matches(&self, client: &str, input: &str, original_input: &str) -> Result<bool, String> {
        if self.client == client && self.input == input && self.original_input == original_input {
            Ok(true)
        } else {
            Err("ACP turn id was already accepted with different input".into())
        }
    }
}

#[derive(Debug, Clone)]
pub struct AcpExecutionSnapshot {
    pub running: bool,
    pub cancelling: bool,
    pub projection: AcpEventProjectionSnapshot,
}

struct Execution {
    transcript: Option<DialogTurnData>,
    awaiting_history: bool,
    prompt_submitted: bool,
    turn_id: String,
    running: bool,
    cancelling: bool,
    // Wakes a prompt that is still waiting for its session before submission.
    pending_cancel: tokio::sync::watch::Sender<bool>,
    // Keep admission receipts for the lifetime of the ACP session, like its
    // remote session handle. Explicit session release drops both. A frontend
    // reload/retry must not submit an accepted prompt a second time.
    receipts: HashMap<String, SubmissionReceipt>,
}

struct State {
    journal: Option<Arc<dyn AcpProjectionJournal>>,
    sessions: HashMap<String, Execution>,
}

pub(super) struct AcpExecutions(Mutex<State>);

impl Default for AcpExecutions {
    fn default() -> Self {
        Self(Mutex::new(State {
            journal: None,
            sessions: HashMap::new(),
        }))
    }
}

impl AcpExecutions {
    pub(super) fn configure_journal(
        &self,
        journal: Arc<dyn AcpProjectionJournal>,
    ) -> Result<(), String> {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if !state.sessions.is_empty() {
            return Err("Cannot replace the ACP journal after prompt admission".into());
        }
        state.journal = Some(journal);
        Ok(())
    }

    pub(super) fn already_admitted(
        &self,
        session: &str,
        turn: &str,
        client: &str,
        input: &str,
        original_input: &str,
    ) -> Result<bool, String> {
        if session.trim().is_empty() || turn.trim().is_empty() || client.trim().is_empty() {
            return Err("ACP session, turn and client ids are required".into());
        }
        let state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        state
            .sessions
            .get(session)
            .and_then(|e| e.receipts.get(turn))
            .map_or(Ok(false), |receipt| {
                receipt.matches(client, input, original_input)
            })
    }

    /// `None` is an identical, already accepted request, not a new prompt.
    pub(super) fn admit(
        &self,
        session: &str,
        turn: &str,
        client: &str,
        input: &str,
        original_input: &str,
        transcript: Option<(DialogTurnData, bool)>,
    ) -> Result<Option<Value>, String> {
        let fingerprint = SubmissionReceipt {
            client: client.into(),
            input: input.into(),
            original_input: original_input.into(),
        };
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let journal = state
            .journal
            .clone()
            .ok_or("ACP execution projection is unavailable")?;
        let execution = state
            .sessions
            .entry(session.into())
            .or_insert_with(|| Execution {
                transcript: None,
                awaiting_history: false,
                prompt_submitted: false,
                turn_id: String::new(),
                running: false,
                cancelling: false,
                pending_cancel: tokio::sync::watch::channel(false).0,
                receipts: HashMap::new(),
            });
        if let Some(accepted) = execution.receipts.get(turn) {
            return accepted
                .matches(client, input, original_input)
                .map(|_| None);
        }
        if execution.running {
            return Err("ACP session is still executing its previous turn".into());
        }
        execution.awaiting_history = transcript.as_ref().is_some_and(|(_, pending)| *pending);
        let transcript = transcript.map(|(data, _)| data);
        let turn_index = transcript
            .as_ref()
            .filter(|_| !execution.awaiting_history)
            .map(|data| data.turn_index);
        let metadata = transcript.as_ref().map(|data| {
            let mut fields = data
                .user_message
                .metadata
                .clone()
                .unwrap_or_else(|| json!({}));
            if let Some(object) = fields.as_object_mut() {
                object.insert("acpRuntimeOwnsHistory".into(), true.into());
            }
            fields
        });
        execution.transcript = transcript;
        execution.prompt_submitted = false;
        execution.turn_id = turn.into();
        execution.running = true;
        execution.cancelling = false;
        execution.pending_cancel = tokio::sync::watch::channel(false).0;
        execution.receipts.insert(turn.into(), fingerprint);
        let event = AgenticEvent::DialogTurnStarted {
            session_id: session.into(),
            turn_id: turn.into(),
            turn_index: turn_index.unwrap_or(0),
            user_input: input.into(),
            original_user_input: Some(original_input.into()),
            user_message_metadata: metadata,
            session_binding: None,
        };
        let cursor = journal.record(&event).expect("start has a journal cursor");
        let mut projected = openbitfun_events::project_agentic_frontend_event(event)
            .expect("start has a frontend projection")
            .payload;
        projected["turnIndex"] = serde_json::json!(turn_index);
        attach_cursor(&mut projected, cursor);
        Ok(Some(projected))
    }

    pub(super) fn cancel(
        &self,
        session: &str,
        expected_turn: Option<&str>,
        send: impl FnOnce() -> Result<(), String>,
    ) -> Result<(), String> {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(execution) = state.sessions.get_mut(session) {
            if expected_turn.is_some_and(|turn| turn != execution.turn_id) {
                return Err("ACP cancellation belongs to a retired turn".into());
            }
            if !execution.running {
                return Ok(());
            }
            send()?;
            // Notification acceptance is not execution completion.
            execution.cancelling = true;
            Ok(())
        } else if expected_turn.is_some() {
            Err("ACP execution state is unavailable".into())
        } else {
            send()
        }
    }

    /// An accepted turn can still be waiting for its agent or its saved session.
    pub(super) fn cancel_pending(&self, session: &str, expected_turn: Option<&str>) -> bool {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(execution) = state.sessions.get_mut(session) {
            if execution.running
                && !execution.prompt_submitted
                && expected_turn.is_none_or(|id| id == execution.turn_id)
            {
                execution.cancelling = true;
                execution.pending_cancel.send_replace(true);
                return true;
            }
        }
        false
    }

    /// Subscribe only while this exact turn still owns an unsent prompt.
    pub(super) fn pending_cancellation(
        &self,
        session: &str,
        turn: &str,
    ) -> Option<tokio::sync::watch::Receiver<bool>> {
        let state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let execution = state.sessions.get(session)?;
        (execution.turn_id == turn && execution.running && !execution.prompt_submitted)
            .then(|| execution.pending_cancel.subscribe())
    }

    pub(super) fn is_cancelling(&self, session: &str) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .sessions
            .get(session)
            .is_some_and(|execution| execution.cancelling)
    }

    /// Record before publishing, under the same lock as execution facts.
    /// The live envelope and recovery snapshot therefore describe one order.
    pub(super) fn record(&self, name: &str, payload: &mut Value) -> Result<(), String> {
        let Some(event) = projected_event(name, payload)? else {
            return Ok(());
        };
        let session = event.session_id().ok_or("ACP event has no session")?;
        let turn = payload["turnId"].as_str().ok_or("ACP event has no turn")?;
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let execution = state
            .sessions
            .get(session)
            .ok_or("ACP turn was not admitted")?;
        if execution.turn_id != turn || !execution.running {
            return Err("ACP event belongs to a retired turn".into());
        }
        if let Some(cursor) = state
            .journal
            .as_ref()
            .ok_or("ACP execution projection is unavailable")?
            .record(&event)
        {
            attach_cursor(payload, cursor);
        }
        let execution = state.sessions.get_mut(session).expect("checked execution");
        if let Some(data) = execution.transcript.as_mut() {
            super::transcript::apply_event(data, &event, super::transcript::now_ms());
            if !execution.prompt_submitted
                && matches!(
                    event,
                    AgenticEvent::DialogTurnCancelled { .. }
                        | AgenticEvent::DialogTurnFailed { .. }
                )
            {
                data.finish_reason = Some(super::session_persistence::UNSENT_FINISH_REASON.into());
            }
        } else if matches!(
            event,
            AgenticEvent::DialogTurnCompleted { .. }
                | AgenticEvent::DialogTurnCancelled { .. }
                | AgenticEvent::DialogTurnFailed { .. }
        ) {
            execution.running = false;
            execution.cancelling = false;
        }
        Ok(())
    }

    pub(super) fn journal(&self) -> Result<Arc<dyn AcpProjectionJournal>, String> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .journal
            .clone()
            .ok_or_else(|| "ACP execution journal unavailable".into())
    }

    pub(super) fn has_session(&self, session: &str) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .sessions
            .contains_key(session)
    }

    pub(super) fn is_running(&self, session: &str) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .sessions
            .get(session)
            .is_some_and(|x| x.running)
    }

    /// Queue the wire request before cancellation can switch to the remote handle.
    /// `admitted` is the turn this request was accepted for. A request whose turn
    /// was released or replaced while its agent started is never sent.
    pub(super) fn submit_prompt<T>(
        &self,
        session: &str,
        admitted: Option<&str>,
        send: impl FnOnce(Option<String>) -> T,
    ) -> Option<T> {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let execution = state.sessions.get_mut(session).filter(|e| e.running);
        if let Some(turn) = admitted {
            if execution
                .as_ref()
                .is_none_or(|e| e.turn_id != turn || e.prompt_submitted)
            {
                return None;
            }
        }
        if let Some(execution) = execution {
            if execution.cancelling {
                return None;
            }
            execution.prompt_submitted = true;
            let message_id = execution
                .transcript
                .as_ref()
                .map(|data| data.user_message.id.clone());
            return Some(send(message_id));
        }
        Some(send(None))
    }

    pub(super) fn completed_transcript(&self, session: &str) -> Option<(DialogTurnData, bool)> {
        let state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let execution = state.sessions.get(session)?;
        let data = execution.transcript.as_ref()?;
        (data.status != openbitfun_core::service::session::TurnStatus::InProgress)
            .then(|| (data.clone(), execution.awaiting_history))
    }

    pub(super) fn pending_turn(&self, session: &str) -> Option<DialogTurnData> {
        let state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let execution = state.sessions.get(session)?;
        execution
            .transcript
            .as_ref()
            .filter(|_| execution.awaiting_history)
            .cloned()
    }

    pub(super) fn placed_pending_turn(&self, session: &str, index: usize) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(execution) = state.sessions.get_mut(session) {
            if let Some(data) = execution.transcript.as_mut() {
                data.turn_index = index;
            }
            execution.awaiting_history = false;
        }
    }

    pub(super) fn finish_turn(&self, session: &str, checkpoint_saved: bool) {
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(execution) = state.sessions.get_mut(session) {
            execution.running = false;
            execution.cancelling = false;
            if checkpoint_saved {
                execution.transcript = None;
            }
        }
        if checkpoint_saved {
            if let Some(journal) = &state.journal {
                journal.checkpoint_saved(session);
            }
        }
    }

    pub(super) fn snapshot(&self, session: &str) -> Option<AcpExecutionSnapshot> {
        let state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let execution = state.sessions.get(session)?;
        let mut projection = state.journal.as_ref()?.snapshot(session);
        if execution.running
            && projection.events.last().is_some_and(|event| {
                matches!(
                    event,
                    AgenticEvent::DialogTurnCompleted { .. }
                        | AgenticEvent::DialogTurnFailed { .. }
                        | AgenticEvent::DialogTurnCancelled { .. }
                )
            })
        {
            projection.events.pop();
            projection.cursor = projection.cursor.saturating_sub(1);
        }
        Some(AcpExecutionSnapshot {
            running: execution.running,
            cancelling: execution.cancelling,
            projection,
        })
    }

    pub(super) fn release(&self, session: &str) {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .sessions
            .remove(session);
    }
}

// ACP's existing frontend projection is preserved for live consumers. Translate
// its portable lifecycle/content facts into the existing journal contract; no
// Tauri, window, transport or filesystem dependency enters this owner.
fn projected_event(name: &str, payload: &Value) -> Result<Option<AgenticEvent>, String> {
    let (kind, defaults) = match name {
        "agentic://dialog-turn-started" => (
            "DialogTurnStarted",
            json!({"turn_index":0,"user_message_metadata":null}),
        ),
        "agentic://model-round-started" => (
            "ModelRoundStarted",
            json!({"model_config_id":"","effective_model_name":""}),
        ),
        "agentic://model-round-completed" => (
            "ModelRoundCompleted",
            json!({"model_config_id":"","effective_model_name":""}),
        ),
        "agentic://text-chunk" if payload["contentType"] == "thinking" => (
            "ThinkingChunk",
            json!({"content":payload["text"],"is_end":payload["isThinkingEnd"].as_bool().unwrap_or(false)}),
        ),
        "agentic://text-chunk" => ("TextChunk", json!({})),
        "agentic://tool-event" => ("ToolEvent", json!({})),
        "agentic://dialog-turn-completed" => (
            "DialogTurnCompleted",
            json!({"total_rounds":0,"total_tools":0,"duration_ms":0}),
        ),
        "agentic://dialog-turn-cancelled" => ("DialogTurnCancelled", json!({})),
        "agentic://dialog-turn-failed" => ("DialogTurnFailed", json!({})),
        _ => return Ok(None),
    };
    let mut event = defaults;
    event["type"] = kind.into();
    for (source, target) in [
        ("sessionId", "session_id"),
        ("turnId", "turn_id"),
        ("roundId", "round_id"),
        ("roundIndex", "round_index"),
        ("userInput", "user_input"),
        ("originalUserInput", "original_user_input"),
        ("hasToolCalls", "has_tool_calls"),
        ("toolEvent", "tool_event"),
        ("text", "text"),
        ("error", "error"),
        ("errorCategory", "error_category"),
        ("errorDetail", "error_detail"),
    ] {
        if let Some(value) = payload.get(source) {
            event[target] = value.clone();
        }
    }
    serde_json::from_value(event)
        .map(Some)
        .map_err(|e| format!("Invalid ACP projection: {e}"))
}

#[cfg(test)]
pub(in crate::client) mod tests {
    use super::*;
    #[derive(Default)]
    pub(in crate::client) struct Journal {
        events: Mutex<Vec<AgenticEvent>>,
        pending: Mutex<Vec<AgenticEvent>>,
    }
    impl AcpProjectionJournal for Journal {
        fn pending_checkpoint(&self, _: &str) -> Vec<AgenticEvent> {
            self.pending.lock().unwrap().clone()
        }
        fn checkpoint_saved(&self, _: &str) {
            self.pending.lock().unwrap().clear();
        }
        fn record(&self, event: &AgenticEvent) -> Option<AcpEventCursor> {
            self.pending.lock().unwrap().push(event.clone());
            let mut events = self.events.lock().unwrap();
            events.push(event.clone());
            Some(AcpEventCursor {
                stream_id: "host".into(),
                cursor: events.len() as u64,
            })
        }
        fn snapshot(&self, session: &str) -> AcpEventProjectionSnapshot {
            let events = self.events.lock().unwrap().clone();
            AcpEventProjectionSnapshot {
                session_id: session.into(),
                stream_id: "host".into(),
                cursor: events.len() as u64,
                active_turn_id: events.iter().rev().find_map(|e| {
                    if let AgenticEvent::DialogTurnStarted { turn_id, .. } = e {
                        Some(turn_id.clone())
                    } else {
                        None
                    }
                }),
                events,
            }
        }
    }
    fn store() -> AcpExecutions {
        let store = AcpExecutions::default();
        store
            .configure_journal(Arc::new(Journal::default()))
            .unwrap();
        store
    }
    fn emit(store: &AcpExecutions, name: &str, extra: Value) -> Value {
        let mut payload = json!({"sessionId":"s","turnId":"t"});
        payload
            .as_object_mut()
            .unwrap()
            .extend(extra.as_object().unwrap().clone());
        store.record(name, &mut payload).unwrap();
        payload
    }

    #[test]
    fn admission_is_idempotent_and_does_not_replace_running_or_cancelling_work() {
        let store = store();
        assert!(store
            .admit("s", "t", "client", "hello", "hello", None)
            .unwrap()
            .is_some());
        assert_eq!(
            store.admit("s", "t", "client", "hello", "hello", None),
            Ok(None)
        );
        assert!(store
            .admit("s", "t", "client", "different", "different", None)
            .is_err());
        store.cancel("s", Some("t"), || Ok(())).unwrap();
        assert!(store.snapshot("s").unwrap().running);
        assert!(store
            .admit("s", "next", "client", "next", "next", None)
            .is_err());
        emit(&store, "agentic://dialog-turn-cancelled", json!({}));
        assert!(!store.snapshot("s").unwrap().running);
        assert!(store
            .admit("s", "next", "client", "next", "next", None)
            .unwrap()
            .is_some());
        let mut sent = false;
        assert!(store
            .cancel("s", Some("t"), || {
                sent = true;
                Ok(())
            })
            .is_err());
        assert!(!sent);
        assert!(store
            .record(
                "agentic://dialog-turn-failed",
                &mut json!({"sessionId":"s","turnId":"t","error":"late"})
            )
            .is_err());
        assert!(store.snapshot("s").unwrap().running);

        // A delayed worker reaches the send boundary after another turn takes over.
        assert!(store
            .submit_prompt("s", Some("t"), |_| panic!("retired prompt sent"))
            .is_none());
        assert!(store.submit_prompt("s", Some("next"), |_| ()).is_some());

        let mut failed = json!({
            "sessionId": "s", "turnId": "next", "error": "Stop was not confirmed",
            "errorCategory": "unknown",
            "errorDetail": {"category": "unknown", "recoveryConstraint": "acp_stop_unconfirmed", "retryable": false}
        });
        store
            .record("agentic://dialog-turn-failed", &mut failed)
            .unwrap();
        let snapshot = store.snapshot("s").unwrap();
        assert!(!snapshot.running);
        let replay = serde_json::to_value(snapshot.projection.events.last().unwrap()).unwrap();
        assert_eq!(replay["error_category"], failed["errorCategory"]);
        assert_eq!(replay["error_detail"], failed["errorDetail"]);
        store.release("s");
        assert!(store
            .submit_prompt("s", Some("next"), |_| panic!("released prompt sent"))
            .is_none());
    }

    #[test]
    fn reload_snapshot_has_start_text_and_terminal_with_the_same_live_cursors() {
        let store = store();
        store
            .admit("s", "t", "client", "hello", "hello", None)
            .unwrap();
        emit(
            &store,
            "agentic://model-round-started",
            json!({"roundId":"r","roundIndex":0}),
        );
        for text in ["one", "two"] {
            emit(
                &store,
                "agentic://text-chunk",
                json!({"roundId":"r","text":text}),
            );
        }
        let active = store.snapshot("s").unwrap();
        assert!(active.running);
        assert_eq!(active.projection.cursor, 4);
        assert!(active
            .projection
            .events
            .iter()
            .any(|e| matches!(e,AgenticEvent::TextChunk{text,..} if text=="two")));
        let live = emit(&store, "agentic://dialog-turn-completed", json!({}));
        let terminal = store.snapshot("s").unwrap();
        assert!(!terminal.running);
        assert_eq!(
            terminal.projection.cursor,
            live["__openbitfunRuntimeEventCursor"].as_u64().unwrap()
        );
        assert!(matches!(
            terminal.projection.events.last(),
            Some(AgenticEvent::DialogTurnCompleted { .. })
        ));
        assert_eq!(
            store.admit("s", "t", "client", "hello", "hello", None),
            Ok(None)
        );
    }
}
