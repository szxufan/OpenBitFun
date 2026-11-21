use super::super::SessionManager;
use super::support::{
    interrupted_turn_test_model_fingerprint, interrupted_turn_test_reasoning_fingerprint,
    reopen_interrupted_turn_for_test, test_manager, TestWorkspace,
};
use crate::agentic::core::{Message, SessionConfig, SessionState, ToolCall, ToolResult, TurnStats};
use crate::agentic::persistence::PersistenceManager;
use crate::service::session::{DialogTurnRecoveryStatus, TurnStatus};
use openbitfun_runtime_ports::{AgentTurnSettlementResult, AgentTurnSettlementStatus};
use serde_json::json;
use std::sync::Arc;

#[tokio::test]
async fn recovered_completion_appends_rounds_and_turn_usage_after_interruption() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let session = manager
        .create_session(
            "Recovery append".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..SessionConfig::default()
            },
        )
        .await
        .expect("session should create");
    let turn_id = manager
        .start_dialog_turn(
            &session.session_id,
            "Standard".to_string(),
            "finish the task".to_string(),
            Some("turn-append".to_string()),
            None,
            Some(serde_json::json!({
                "resolved_permission_mode": "ask",
                "runtime_resolved_model_id": "model-original",
                "runtime_model_binding_fingerprint": interrupted_turn_test_model_fingerprint(),
                "runtime_reasoning_preset": null,
                "runtime_reasoning_selection": null,
                "runtime_reasoning_fingerprint": interrupted_turn_test_reasoning_fingerprint(None)
            })),
        )
        .await
        .expect("turn should start");
    let details = |attempt: &str| {
        Some(json!({"openbitfunModelRequest": {
            "version": 1, "roundId": attempt, "attemptId": attempt, "attemptIndex": 1,
            "startedAt": chrono::Utc::now(), "authentication": "api_key",
            "requestedModel": "model", "outcome": "completed", "usageKnown": true
        }}))
    };
    let ledger = crate::service::token_usage::TokenUsageService::new(workspace.path_manager())
        .await
        .unwrap();
    ledger
        .record_usage(
            "config".into(),
            "model".into(),
            session.session_id.clone(),
            turn_id.clone(),
            28928,
            97,
            None,
            details("before"),
            false,
        )
        .await
        .unwrap();
    let mut persisted_turn = persistence_manager
        .load_dialog_turn(workspace.path(), &session.session_id, 0)
        .await
        .expect("turn should load")
        .expect("turn should exist");
    persisted_turn.model_rounds = SessionManager::build_model_rounds_from_messages(
        &[Message::assistant("before interruption".to_string())
            .with_turn_id(turn_id.clone())
            .with_round_id("round-before".to_string())],
        &turn_id,
        1,
    );
    persistence_manager
        .save_dialog_turn(workspace.path(), &persisted_turn)
        .await
        .expect("existing round should persist");
    manager
        .mark_dialog_turn_interrupted(&session.session_id, &turn_id)
        .await
        .expect("turn should become interrupted");
    manager.record_turn_settlement_result(
        &session.session_id,
        &turn_id,
        AgentTurnSettlementResult {
            status: AgentTurnSettlementStatus::Cancelled,
            final_response: None,
            finish_reason: Some("interrupted".to_string()),
        },
    );
    manager
        .update_session_state_for_turn_if_processing(
            &session.session_id,
            &turn_id,
            SessionState::Idle,
        )
        .await
        .expect("session should settle idle");
    let plan = reopen_interrupted_turn_for_test(&manager, &session.session_id, &turn_id, 0)
        .await
        .expect("turn should reopen");
    assert!(manager
        .turn_settlement_result(&session.session_id, &turn_id)
        .is_none());

    ledger
        .record_usage(
            "config".into(),
            "model".into(),
            session.session_id.clone(),
            turn_id.clone(),
            42794,
            237,
            None,
            details("after"),
            false,
        )
        .await
        .unwrap();
    manager
        .complete_recovered_dialog_turn(
            &session.session_id,
            &turn_id,
            plan.execution_generation,
            "after recovery".to_string(),
            &[Message::assistant("after recovery".to_string())
                .with_turn_id(turn_id.clone())
                .with_round_id("round-after".to_string())],
            TurnStats {
                total_rounds: 1,
                total_tools: 0,
                total_tokens: 0,
                duration_ms: 1,
            },
            Some("complete".to_string()),
            Some(true),
        )
        .await
        .expect("recovered completion should persist");

    let completed = persistence_manager
        .load_dialog_turn(workspace.path(), &session.session_id, 0)
        .await
        .expect("turn should load")
        .expect("turn should exist");
    assert_eq!(
        completed
            .token_usage
            .as_ref()
            .and_then(|usage| usage.output_tokens),
        Some(334)
    );
    assert_eq!(completed.model_rounds.len(), 2);
    assert_eq!(completed.model_rounds[0].round_index, 0);
    assert_eq!(completed.model_rounds[1].round_index, 1);
    assert_eq!(
        completed.model_rounds[1].text_items[0].content,
        "after recovery"
    );
    assert!(completed.recovery.is_none());
    assert_eq!(completed.recovery_epoch, Some(plan.execution_generation));
    assert_eq!(completed.finish_reason.as_deref(), Some("complete"));
    assert_eq!(completed.has_final_response, Some(true));
}

#[tokio::test]
async fn repeated_interruption_persists_recovered_generation_rounds() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let session = manager
        .create_session(
            "Repeated recovery".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..SessionConfig::default()
            },
        )
        .await
        .expect("session should create");
    let turn_id = manager
        .start_dialog_turn(
            &session.session_id,
            "Standard".to_string(),
            "continue through two interruptions".to_string(),
            Some("turn-repeat-interruption".to_string()),
            None,
            Some(serde_json::json!({
                "resolved_permission_mode": "ask",
                "runtime_resolved_model_id": "model-original",
                "runtime_model_binding_fingerprint": interrupted_turn_test_model_fingerprint(),
                "runtime_reasoning_preset": null,
                "runtime_reasoning_selection": null,
                "runtime_reasoning_fingerprint": interrupted_turn_test_reasoning_fingerprint(None)
            })),
        )
        .await
        .expect("turn should start");
    manager
        .mark_dialog_turn_interrupted(&session.session_id, &turn_id)
        .await
        .expect("first generation should interrupt");
    manager
        .update_session_state_for_turn_if_processing(
            &session.session_id,
            &turn_id,
            SessionState::Idle,
        )
        .await
        .expect("session should settle idle");
    let first_recovery =
        reopen_interrupted_turn_for_test(&manager, &session.session_id, &turn_id, 0)
            .await
            .expect("first recovery should reopen");

    let assistant = Message::assistant_with_tools(
        String::new(),
        vec![ToolCall {
            tool_id: "tool-recovered".to_string(),
            tool_name: "Read".to_string(),
            arguments: json!({"path": "README.md"}),
            raw_arguments: None,
            is_error: false,
            parse_error: None,
            recovered_from_truncation: false,
            repair_kind: Default::default(),
        }],
    )
    .with_turn_id(turn_id.clone())
    .with_round_id("recovered-round".to_string());
    let tool_result = Message::tool_result(ToolResult {
        tool_id: "tool-recovered".to_string(),
        tool_name: "Read".to_string(),
        effective_tool_name: None,
        result: json!({"content": "safe boundary"}),
        result_for_assistant: Some("safe boundary".to_string()),
        is_error: false,
        duration_ms: Some(1),
        image_attachments: None,
    })
    .with_turn_id(turn_id.clone())
    .with_round_id("recovered-round".to_string());
    manager
        .mark_dialog_turn_interrupted_with_messages(
            &session.session_id,
            &turn_id,
            &[assistant, tool_result],
        )
        .await
        .expect("recovered generation should interrupt durably");
    manager
        .update_session_state_for_turn_if_processing(
            &session.session_id,
            &turn_id,
            SessionState::Idle,
        )
        .await
        .expect("session should settle idle again");

    let persisted = persistence_manager
        .load_dialog_turn(workspace.path(), &session.session_id, 0)
        .await
        .expect("turn should load")
        .expect("turn should exist");
    assert_eq!(persisted.model_rounds.len(), 1);
    assert_eq!(persisted.model_rounds[0].round_index, 0);
    assert_eq!(
        persisted.model_rounds[0].tool_items[0]
            .tool_result
            .as_ref()
            .map(|result| result.success),
        Some(true),
    );
    let next_recovery = reopen_interrupted_turn_for_test(
        &manager,
        &session.session_id,
        &turn_id,
        first_recovery.execution_generation,
    )
    .await
    .expect("second recovery should reopen after appended history");
    assert_eq!(next_recovery.initial_round_index, 1);
}

#[tokio::test]
async fn first_interruption_merges_a_partially_persisted_round_prefix() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let session = manager
        .create_session(
            "Interrupted merge".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..SessionConfig::default()
            },
        )
        .await
        .expect("session should create");
    let turn_id = manager
        .start_dialog_turn(
            &session.session_id,
            "Standard".to_string(),
            "merge frontend and runtime rounds".to_string(),
            Some("turn-prefix-merge".to_string()),
            None,
            Some(serde_json::json!({
                "resolved_permission_mode": "ask",
                "runtime_resolved_model_id": "model-original",
                "runtime_model_binding_fingerprint": interrupted_turn_test_model_fingerprint(),
                "runtime_reasoning_preset": null,
                "runtime_reasoning_selection": null,
                "runtime_reasoning_fingerprint": interrupted_turn_test_reasoning_fingerprint(None)
            })),
        )
        .await
        .expect("turn should start");
    let persisted_prefix = Message::assistant("frontend prefix".to_string())
        .with_turn_id(turn_id.clone())
        .with_round_id("round-shared".to_string());
    let mut persisted_turn = persistence_manager
        .load_dialog_turn(workspace.path(), &session.session_id, 0)
        .await
        .expect("turn should load")
        .expect("turn should exist");
    persisted_turn.model_rounds =
        SessionManager::build_model_rounds_from_messages(&[persisted_prefix], &turn_id, 1);
    persistence_manager
        .save_dialog_turn(workspace.path(), &persisted_turn)
        .await
        .expect("frontend prefix should persist");

    let runtime_rounds = [
        Message::assistant("runtime authoritative prefix".to_string())
            .with_turn_id(turn_id.clone())
            .with_round_id("round-shared".to_string()),
        Message::assistant("runtime missing tail".to_string())
            .with_turn_id(turn_id.clone())
            .with_round_id("round-tail".to_string()),
    ];
    manager
        .mark_dialog_turn_interrupted_with_messages(&session.session_id, &turn_id, &runtime_rounds)
        .await
        .expect("interruption should merge the runtime journal");

    let persisted = persistence_manager
        .load_dialog_turn(workspace.path(), &session.session_id, 0)
        .await
        .expect("turn should load")
        .expect("turn should exist");
    assert_eq!(persisted.model_rounds.len(), 2);
    assert_eq!(persisted.model_rounds[0].id, "round-shared");
    assert_eq!(persisted.model_rounds[0].round_index, 0);
    assert_eq!(
        persisted.model_rounds[0].text_items[0].content,
        "runtime authoritative prefix"
    );
    assert_eq!(persisted.model_rounds[1].id, "round-tail");
    assert_eq!(persisted.model_rounds[1].round_index, 1);
}

#[tokio::test]
async fn restore_reopens_a_recovering_turn_when_the_session_write_was_lost() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let session = manager
        .create_session(
            "Recovery restart".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                model_id: Some("primary".to_string()),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..SessionConfig::default()
            },
        )
        .await
        .expect("session should create");
    let turn_id = manager
        .start_dialog_turn(
            &session.session_id,
            "Standard".to_string(),
            "finish after restart".to_string(),
            Some("turn-restart".to_string()),
            None,
            Some(serde_json::json!({
                "resolved_permission_mode": "ask",
                "runtime_resolved_model_id": "model-original",
                "runtime_model_binding_fingerprint": interrupted_turn_test_model_fingerprint(),
                "runtime_reasoning_preset": null,
                "runtime_reasoning_selection": null,
                "runtime_reasoning_fingerprint": interrupted_turn_test_reasoning_fingerprint(None)
            })),
        )
        .await
        .expect("turn should start");
    manager
        .mark_dialog_turn_interrupted(&session.session_id, &turn_id)
        .await
        .expect("turn should become interrupted");
    manager
        .update_session_state_for_turn_if_processing(
            &session.session_id,
            &turn_id,
            SessionState::Idle,
        )
        .await
        .expect("session should settle idle");
    let first_recovery =
        reopen_interrupted_turn_for_test(&manager, &session.session_id, &turn_id, 0)
            .await
            .expect("turn should start recovering");
    assert_eq!(first_recovery.execution_generation, 1);

    // Simulate a crash after the recovering Turn rename but before the
    // Processing Session rename. The older Idle Session file must not hide
    // the abandoned execution generation forever.
    let mut stale_session = persistence_manager
        .load_session(workspace.path(), &session.session_id)
        .await
        .expect("persisted session should load");
    stale_session.state = SessionState::Idle;
    persistence_manager
        .save_session(workspace.path(), &stale_session)
        .await
        .expect("stale idle session should persist");
    manager.evict_loaded_session_for_test(&session.session_id);
    drop(manager);

    let restored_manager = test_manager(persistence_manager.clone());
    let (restored_session, restored_turns) = restored_manager
        .restore_session_with_turns(workspace.path(), &session.session_id)
        .await
        .expect("restart should restore the session");

    assert!(matches!(restored_session.state, SessionState::Idle));
    let restored_turn = restored_turns.last().expect("latest turn should restore");
    assert_eq!(restored_turn.status, TurnStatus::Cancelled);
    assert_eq!(
        restored_turn
            .recovery
            .as_ref()
            .map(|recovery| (recovery.status, recovery.execution_generation)),
        Some((DialogTurnRecoveryStatus::Interrupted, 1)),
    );

    let second_recovery =
        reopen_interrupted_turn_for_test(&restored_manager, &session.session_id, &turn_id, 1)
            .await
            .expect("the interrupted generation should remain recoverable");
    assert_eq!(second_recovery.execution_generation, 2);
}
