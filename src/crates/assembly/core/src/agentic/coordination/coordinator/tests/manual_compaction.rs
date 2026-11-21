use super::super::test_support::test_coordinator;
#[cfg(feature = "external-sources")]
use super::super::test_support::test_persistent_coordinator;
use super::super::{
    runtime_transcript_messages_from_turns, ContextCompactionOutcome, ConversationCoordinator,
    ManualCompactionCommitGate,
};
#[cfg(feature = "external-sources")]
use crate::agentic::core::SessionAgentRouteOwner;
use crate::agentic::core::{Message, MessageContent, SessionConfig, SessionState};
use crate::agentic::events::{AgenticEvent, EventQueue, EventQueueConfig};
use crate::agentic::persistence::PersistenceManager;
use crate::agentic::session::{
    PromptCachePolicy, SessionContextStore, SessionManager, SessionManagerConfig,
};
use crate::infrastructure::PathManager;
use crate::service::session::{DialogTurnData, DialogTurnKind, TurnStatus, UserMessageData};
use crate::util::errors::OpenBitFunError;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[test]
fn manual_compaction_cancellation_wins_before_commit() {
    let gate = ManualCompactionCommitGate::planning();

    assert!(gate.try_cancel());
    assert!(!gate.try_begin_commit());
}

#[test]
fn manual_compaction_commit_rejects_late_cancellation() {
    let gate = ManualCompactionCommitGate::planning();

    assert!(gate.try_begin_commit());
    assert!(!gate.try_cancel());
}

#[cfg(feature = "external-sources")]
#[tokio::test]
async fn manual_compaction_fails_closed_before_admission_when_external_agent_is_unavailable() {
    let (coordinator, session_manager) = test_persistent_coordinator();
    let workspace = tempfile::tempdir().expect("workspace");
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(workspace.path());
    let session_id = format!("external-compact-{}", uuid::Uuid::new_v4());
    let external_agent_id = format!("missing-external-{}", uuid::Uuid::new_v4());
    session_manager
        .create_session_with_id(
            Some(session_id.clone()),
            "External compaction".to_string(),
            external_agent_id.clone(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
        .await
        .expect("create session");
    session_manager
        .update_session_agent_binding(
            &session_id,
            &external_agent_id,
            SessionAgentRouteOwner::External,
            Some("test:external".to_string()),
        )
        .await
        .expect("persist external route owner");

    let error = match coordinator
        .start_manual_compaction_task(session_id.clone(), None)
        .await
    {
        Ok(_) => panic!("manual compaction must not bypass an unavailable external route"),
        Err(error) => error,
    };

    assert!(error.to_string().contains("candidate_unavailable"));
    let session = session_manager
        .get_session(&session_id)
        .expect("session remains loaded");
    assert!(matches!(session.state, SessionState::Idle));
    assert!(session.dialog_turn_ids.is_empty());
}

#[test]
fn manual_compaction_transcript_restores_user_and_tool_payload() {
    let outcome = ContextCompactionOutcome {
        compression_id: "compression-1".to_string(),
        compression_count: 2,
        tokens_before: 80_000,
        tokens_after: 20_000,
        compression_ratio: 0.25,
        duration_ms: 42,
        has_summary: true,
        summary_source: "model".to_string(),
        applied: true,
    };
    let mut turn = DialogTurnData::new_with_kind(
        DialogTurnKind::ManualCompaction,
        "compact-turn".to_string(),
        1,
        "session".to_string(),
        None,
        UserMessageData {
            id: "compact-user".to_string(),
            content: "/compact".to_string(),
            timestamp: 10,
            metadata: Some(ConversationCoordinator::manual_compaction_metadata()),
        },
    );
    turn.model_rounds = vec![
        ConversationCoordinator::build_manual_compaction_round_completed(
            &turn.turn_id,
            &outcome,
            Some(128_000),
        ),
    ];
    turn.status = TurnStatus::Completed;

    let transcript = runtime_transcript_messages_from_turns(&[turn.clone()], None);

    assert_eq!(transcript.len(), 3);
    assert_eq!(transcript[0].role, "user");
    assert_eq!(transcript[0].turn_id.as_deref(), Some("compact-turn"));
    match &transcript[1].content {
        openbitfun_runtime_ports::TranscriptContent::Mixed { tool_calls, .. } => {
            assert_eq!(tool_calls.len(), 1);
            assert_eq!(tool_calls[0].tool_id, "compression-1");
            assert_eq!(tool_calls[0].tool_name, "ContextCompression");
        }
        other => panic!("expected restored tool call, got {other:?}"),
    }
    match &transcript[2].content {
        openbitfun_runtime_ports::TranscriptContent::ToolResult {
            tool_id,
            result,
            is_error,
            ..
        } => {
            assert_eq!(tool_id, "compression-1");
            assert_eq!(result["applied"], true);
            assert!(!is_error);
        }
        other => panic!("expected restored tool result, got {other:?}"),
    }

    turn.status = TurnStatus::Error;
    turn.error = Some("Manual compaction was applied, but terminal persistence failed".to_string());
    let failed_transcript = runtime_transcript_messages_from_turns(&[turn.clone()], None);
    match &failed_transcript[1].content {
        openbitfun_runtime_ports::TranscriptContent::Mixed { text, .. } => {
            assert_eq!(
                text,
                "[Error: Manual compaction was applied, but terminal persistence failed]"
            );
        }
        other => panic!("expected restored failure text, got {other:?}"),
    }

    turn.status = TurnStatus::Cancelled;
    let cancelled_transcript = runtime_transcript_messages_from_turns(&[turn], None);
    match &cancelled_transcript[1].content {
        openbitfun_runtime_ports::TranscriptContent::Mixed { text, .. } => {
            assert!(
                text.is_empty(),
                "cancelled turns must not restore their internal failure marker"
            );
        }
        other => panic!("expected restored cancelled tool call, got {other:?}"),
    }
}

#[test]
fn manual_compaction_failure_round_preserves_runtime_identity_and_error() {
    let round = ConversationCoordinator::build_manual_compaction_round_failed(
        "compact-turn",
        "compression-runtime".to_string(),
        "summary request failed",
        Some(128_000),
    );

    assert_eq!(round.tool_items.len(), 1);
    let tool = &round.tool_items[0];
    assert_eq!(tool.id, "compression-runtime");
    assert_eq!(tool.tool_call.id, "compression-runtime");
    let result = tool.tool_result.as_ref().expect("failure result");
    assert!(!result.success);
    assert_eq!(result.result["error"], "summary request failed");
    assert_eq!(result.error.as_deref(), Some("summary request failed"));
}

#[tokio::test]
async fn manual_compaction_cancelled_before_setup_preserves_context_and_settles_turn() {
    let (coordinator, session_manager) = test_coordinator();
    let workspace = tempfile::tempdir().unwrap();
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(workspace.path());
    let session = session_manager
        .create_session(
            "Cancelled compaction".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
                ..SessionConfig::default()
            },
        )
        .await
        .unwrap();
    let session_id = session.session_id.clone();
    let original = vec![Message::user("Keep this context".to_string())];
    session_manager
        .replace_context_messages(&session_id, original.clone())
        .await;
    let turn_id = session_manager
        .start_maintenance_turn(
            &session_id,
            "/compact".to_string(),
            None,
            Some(ConversationCoordinator::manual_compaction_metadata()),
        )
        .await
        .unwrap();
    let token = CancellationToken::new();
    token.cancel();
    let result = ConversationCoordinator::execute_manual_compaction_task(
        session_manager.clone(),
        coordinator.execution_engine.clone(),
        coordinator.event_queue.clone(),
        original,
        session_id.clone(),
        turn_id.clone(),
        0,
        "Standard".to_string(),
        None,
        None,
        None,
        token,
        Arc::new(ManualCompactionCommitGate::planning()),
    )
    .await;
    assert!(matches!(result, Err(OpenBitFunError::Cancelled(_))));
    let session = session_manager.get_session(&session_id).unwrap();
    assert!(matches!(session.state, SessionState::Idle));
    assert_eq!(session.compression_state.compression_count, 0);
    let messages = session_manager
        .get_context_messages(&session_id)
        .await
        .unwrap();
    assert_eq!(messages.len(), 1);
    assert!(
        matches!(&messages[0].content, MessageContent::Text(text) if text == "Keep this context")
    );
}

#[tokio::test]
async fn applied_manual_compaction_emits_failed_terminal_when_turn_persistence_fails() {
    let root = tempfile::tempdir().expect("test root");
    let workspace = root.path().join("workspace");
    std::fs::create_dir_all(&workspace).expect("workspace should exist");
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(&workspace);
    let path_manager = Arc::new(PathManager::with_user_root_for_tests(
        root.path().join("user-root"),
    ));
    let persistence =
        Arc::new(PersistenceManager::new(path_manager.clone()).expect("persistence manager"));
    let session_manager = SessionManager::new(
        Arc::new(SessionContextStore::new()),
        persistence,
        SessionManagerConfig {
            max_active_sessions: 8,
            session_idle_timeout: Duration::from_secs(3600),
            auto_save_interval: Duration::from_secs(300),
            enable_persistence: true,
            prompt_cache_policy: PromptCachePolicy::default(),
        },
    );
    let session = session_manager
        .create_session(
            "Persistence failure".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");
    let turn_id = session_manager
        .start_maintenance_turn(
            &session.session_id,
            "/compact".to_string(),
            Some("compact-turn".to_string()),
            Some(ConversationCoordinator::manual_compaction_metadata()),
        )
        .await
        .expect("maintenance turn should start");

    let turns_dir = path_manager
        .project_sessions_dir(&workspace)
        .join(&session.session_id)
        .join("turns");
    std::fs::remove_dir_all(&turns_dir).expect("turn directory should be removable");
    std::fs::write(&turns_dir, b"block turn persistence").expect("turn path should become a file");

    let event_queue = EventQueue::new(EventQueueConfig::default());
    let result = ConversationCoordinator::finalize_manual_compaction_success(
        &session_manager,
        &event_queue,
        &session.session_id,
        &turn_id,
        &ContextCompactionOutcome {
            compression_id: "compression-1".to_string(),
            compression_count: 1,
            tokens_before: 80_000,
            tokens_after: 20_000,
            compression_ratio: 0.25,
            duration_ms: 42,
            has_summary: true,
            summary_source: "model".to_string(),
            applied: true,
        },
        Some(128_000),
    )
    .await;

    assert!(result.is_err());
    assert!(matches!(
        session_manager
            .get_session(&session.session_id)
            .expect("session should remain available")
            .state,
        SessionState::Idle
    ));
    let events = event_queue.dequeue_batch(10).await;
    let terminal_events = events
        .iter()
        .filter(|envelope| {
            matches!(
                envelope.event,
                AgenticEvent::DialogTurnCompleted { .. }
                    | AgenticEvent::DialogTurnFailed { .. }
                    | AgenticEvent::DialogTurnCancelled { .. }
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(terminal_events.len(), 1);
    assert!(matches!(
        terminal_events[0].event,
        AgenticEvent::DialogTurnFailed {
            ref turn_id,
            ref error,
            ..
        } if turn_id == "compact-turn" && error.contains("was applied")
    ));
}
