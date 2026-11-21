use super::super::test_support::test_persistent_coordinator;
use super::super::{
    commit_interrupted_turn_intent, lineage_active_turn_after_transcript,
    lineage_post_admission_cancellation_error, lineage_session_is_settling_without_active_state,
    revoke_interrupted_turn_intent_or_observe_commit, validate_required_lineage_turns_settled,
    ActiveSubagentExecution, ConversationCoordinator, InterruptedTurnIntentState,
};
use crate::agentic::core::{
    Message, ProcessingPhase, SessionConfig, SessionKind, SessionState, ToolCall,
};
use crate::agentic::events::AgenticEvent;
use crate::agentic::execution::{ExecutionResult, FinishReason};
use crate::agentic::tools::pipeline::{ToolExecutionContext, ToolExecutionOptions, ToolTask};
use crate::service::session::{DialogTurnData, TurnStatus, UserMessageData};
use crate::util::errors::OpenBitFunError;
use openbitfun_runtime_ports::AgentTurnSettlementStatus;
use openbitfun_runtime_ports::DelegationPolicy;
use std::collections::HashMap;
use std::time::Duration;
use tokio_util::sync::CancellationToken;

#[test]
fn desktop_cards_turn_origin_survives_recovery_without_tainting_local_turns() {
    use openbitfun_agent_runtime::remote_file_delivery::TOOL_CONTEXT_PEER_DEVICE_CONTROL_KEY;
    use std::collections::HashMap;

    // A legacy/local payload has no source marker. New host-stamped
    // metadata round-trips through persisted turns and queue entries.
    for metadata in [
        None,
        Some(serde_json::json!({"surface": "flow_chat"})),
        Some(serde_json::json!({"peer_device_control": true, "requestId": "peer-turn"})),
        Some(serde_json::json!({"peer_device_control": false})),
    ] {
        let restored: Option<serde_json::Value> =
            serde_json::from_str(&serde_json::to_string(&metadata).unwrap()).unwrap();
        let mut original_context = HashMap::new();
        super::super::copy_turn_control_context(&mut original_context, metadata.as_ref());
        let mut recovered_context = HashMap::new();
        super::super::copy_turn_control_context(&mut recovered_context, restored.as_ref());
        assert_eq!(original_context, recovered_context);
        let expected = metadata
            .as_ref()
            .and_then(|value| value.get(TOOL_CONTEXT_PEER_DEVICE_CONTROL_KEY))
            .and_then(serde_json::Value::as_bool)
            .map(|value| value.to_string());
        assert_eq!(
            recovered_context.get(TOOL_CONTEXT_PEER_DEVICE_CONTROL_KEY),
            expected.as_ref()
        );
    }
    let mut next_local_turn = HashMap::new();
    super::super::copy_turn_control_context(&mut next_local_turn, None);
    assert!(!next_local_turn.contains_key(TOOL_CONTEXT_PEER_DEVICE_CONTROL_KEY));
}

#[test]
fn interrupted_turn_intent_commit_wins_over_timeout_revocation() {
    let intents = dashmap::DashMap::new();
    intents.insert(
        "session\u{1f}turn".to_string(),
        InterruptedTurnIntentState::Pending,
    );

    assert!(commit_interrupted_turn_intent(
        &intents,
        "session\u{1f}turn"
    ));
    assert!(revoke_interrupted_turn_intent_or_observe_commit(
        &intents,
        "session\u{1f}turn"
    ));
    assert_eq!(
        *intents.get("session\u{1f}turn").unwrap(),
        InterruptedTurnIntentState::Committed
    );
}

#[test]
fn interrupted_turn_timeout_revocation_blocks_late_commit() {
    let intents = dashmap::DashMap::new();
    intents.insert(
        "session\u{1f}turn".to_string(),
        InterruptedTurnIntentState::Pending,
    );

    assert!(!revoke_interrupted_turn_intent_or_observe_commit(
        &intents,
        "session\u{1f}turn"
    ));
    assert!(!commit_interrupted_turn_intent(
        &intents,
        "session\u{1f}turn"
    ));
    assert_eq!(
        *intents.get("session\u{1f}turn").unwrap(),
        InterruptedTurnIntentState::Revoked
    );
}

#[test]
fn terminal_persisted_turn_is_not_replayed_as_active() {
    assert_eq!(
        lineage_active_turn_after_transcript(
            Some("turn-1".to_string()),
            Some("turn-1".to_string()),
            Some(&TurnStatus::Completed),
        ),
        None
    );
    assert_eq!(
        lineage_active_turn_after_transcript(
            Some("turn-1".to_string()),
            Some("turn-1".to_string()),
            Some(&TurnStatus::InProgress),
        )
        .as_deref(),
        Some("turn-1")
    );
}

#[test]
fn idle_session_with_in_flight_execution_is_not_published_as_settled() {
    assert!(lineage_session_is_settling_without_active_state(None, 1));
    assert!(!lineage_session_is_settling_without_active_state(
        Some("turn-1"),
        1
    ));
    assert!(!lineage_session_is_settling_without_active_state(None, 0));
}

#[test]
fn lineage_read_barrier_requires_each_turn_to_be_durably_terminal() {
    let turn = |turn_id: &str, status| {
        let mut turn = DialogTurnData::new(
            turn_id.to_string(),
            0,
            "session-1".to_string(),
            UserMessageData {
                id: format!("{turn_id}-user"),
                content: "question".to_string(),
                timestamp: 1,
                metadata: None,
            },
        );
        turn.status = status;
        turn
    };
    let turns = vec![
        turn("turn-settled", TurnStatus::Cancelled),
        turn("turn-active", TurnStatus::InProgress),
    ];

    validate_required_lineage_turns_settled(&turns, &["turn-settled".to_string()])
        .expect("terminal turn should satisfy the barrier");
    for required in ["turn-active", "turn-missing"] {
        let error = validate_required_lineage_turns_settled(&turns, &[required.to_string()])
            .expect_err("non-terminal or absent turns must keep the read uncertain");
        assert_eq!(
            error.kind,
            openbitfun_runtime_ports::PortErrorKind::OutcomeUnknown
        );
    }
}

#[test]
fn post_admission_cancellation_errors_are_outcome_unknown() {
    for source_error in [
        crate::util::errors::OpenBitFunError::Timeout("drain deadline".to_string()),
        crate::util::errors::OpenBitFunError::Session("state persistence failed".to_string()),
    ] {
        let error = lineage_post_admission_cancellation_error(source_error, "session-1", "turn-1");

        assert!(matches!(
            error,
            crate::util::errors::OpenBitFunError::OutcomeUnknown(message)
                if message.contains("session_id=session-1")
                    && message.contains("turn_id=turn-1")
        ));
    }
}

#[tokio::test]
async fn post_admission_state_write_failure_still_delivers_all_cancellation_signals() {
    let (coordinator, session_manager) = test_persistent_coordinator();
    let workspace = tempfile::tempdir().expect("workspace");
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(workspace.path());
    let session_id = format!("lineage-cancel-{}", uuid::Uuid::new_v4());
    let turn_id = format!("turn-{}", uuid::Uuid::new_v4());
    session_manager
        .create_session_with_id(
            Some(session_id.clone()),
            "Cancellation fault".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
        .await
        .expect("create persistent session");
    session_manager
        .update_session_state(
            &session_id,
            SessionState::Processing {
                current_turn_id: turn_id.clone(),
                phase: ProcessingPhase::ToolCalling,
            },
        )
        .await
        .expect("mark turn active");
    let storage_path = session_manager
        .effective_session_storage_path(&session_id)
        .await
        .expect("session storage path");

    let engine_token = CancellationToken::new();
    coordinator
        .execution_engine
        .register_cancel_token(&turn_id, engine_token.clone());

    let tool_id = format!("tool-{}", uuid::Uuid::new_v4());
    coordinator
        .tool_pipeline
        .insert_tool_task_for_test(ToolTask::new(
            ToolCall {
                tool_id: tool_id.clone(),
                tool_name: "Read".to_string(),
                arguments: serde_json::json!({}),
                ..Default::default()
            },
            ToolExecutionContext {
                session_id: session_id.clone(),
                dialog_turn_id: turn_id.clone(),
                round_id: "round-1".to_string(),
                attempt_id: None,
                attempt_index: None,
                agent_type: "Standard".to_string(),
                workspace: None,
                model_client: None,
                primary_model_facts: Default::default(),
                context_vars: HashMap::new(),
                subagent_parent_info: None,
                permission_delegation: None,
                delegation_policy: DelegationPolicy::top_level(),
                deferred_tools: Vec::new(),
                loaded_deferred_tool_specs: Vec::new(),
                allowed_tools: Vec::new(),
                runtime_tool_restrictions: Default::default(),
                steering_interrupt: None,
                workspace_services: None,
                terminal_port: None,
                remote_exec_port: None,
            },
            ToolExecutionOptions::default(),
        ))
        .await;

    let descendant_token = CancellationToken::new();
    coordinator.active_subagent_executions.insert(
        "child-session".to_string(),
        ActiveSubagentExecution {
            parent_session_id: session_id.clone(),
            parent_dialog_turn_id: turn_id.clone(),
            subagent_session_id: "child-session".to_string(),
            subagent_dialog_turn_id: "child-turn".to_string(),
            cancel_token: descendant_token.clone(),
        },
    );
    session_manager
        .persistence_manager()
        .fail_next_session_state_write_for_test(&session_id);

    let error = coordinator
        .cancel_loaded_lineage_session_in_storage(
            &storage_path,
            &session_id,
            Some(&turn_id),
            Duration::from_secs(1),
        )
        .await
        .expect_err("admitted persistence failure must remain outcome-unknown");

    assert!(matches!(
        error,
        crate::util::errors::OpenBitFunError::OutcomeUnknown(message)
            if message.contains("Injected session state write failure")
    ));
    assert!(engine_token.is_cancelled());
    assert!(
        coordinator
            .tool_pipeline
            .tool_task_is_cancelled_for_test(&tool_id),
        "tool cancellation must run before the state write error is returned"
    );
    assert!(descendant_token.is_cancelled());
}

#[tokio::test]
async fn completed_persisted_turn_emits_a_durable_history_fence() {
    let workspace = tempfile::tempdir().expect("workspace");
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(workspace.path());
    let (coordinator, session_manager) = test_persistent_coordinator();
    let session = session_manager
        .create_session(
            "Durable completion".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
        .await
        .expect("create session");
    let turn_id = session_manager
        .start_dialog_turn(
            &session.session_id,
            "Standard".to_string(),
            "finish".to_string(),
            Some("turn-durable-fence".to_string()),
            None,
            None,
        )
        .await
        .expect("start turn");
    let message = Message::assistant("complete response".to_string())
        .with_turn_id(turn_id.clone())
        .with_round_id("round-final".to_string());

    ConversationCoordinator::persist_completed_dialog_turn(
        coordinator.event_queue.as_ref(),
        session_manager.as_ref(),
        None,
        &session.session_id,
        &turn_id,
        &ExecutionResult {
            final_message: message.clone(),
            total_rounds: 1,
            success: true,
            new_messages: vec![message],
            finish_reason: FinishReason::Complete,
            total_tools: 0,
            duration_ms: 1,
            partial_recovery_reason: None,
            effective_finish_reason: "complete".to_string(),
            has_final_response: true,
        },
        None,
    )
    .await;

    assert_eq!(
        session_manager
            .turn_settlement_result(&session.session_id, &turn_id)
            .and_then(|result| result.final_response),
        Some("complete response".to_string())
    );

    let events = coordinator.event_queue.dequeue_batch(10).await;
    assert!(events.iter().any(|envelope| matches!(
        &envelope.event,
        AgenticEvent::SessionHistoryChanged {
            session_id,
            settled_turn_id: Some(settled_turn_id),
        } if session_id == &session.session_id && settled_turn_id == &turn_id
    )));
    session_manager
        .delete_session_by_id(&session.session_id)
        .await
        .expect("clean up persisted test session");
}

#[tokio::test]
async fn steering_handoff_settles_without_fabricating_a_final_response() {
    let workspace = tempfile::tempdir().expect("workspace");
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(workspace.path());
    let (coordinator, session_manager) = test_persistent_coordinator();
    let session = session_manager
        .create_session(
            "Durable completion".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
        .await
        .expect("create session");
    let turn_id = session_manager
        .start_dialog_turn(
            &session.session_id,
            "Standard".to_string(),
            "finish".to_string(),
            Some("turn-durable-fence".to_string()),
            None,
            None,
        )
        .await
        .expect("start turn");
    let message = Message::assistant("complete response".to_string())
        .with_turn_id(turn_id.clone())
        .with_round_id("round-final".to_string());

    ConversationCoordinator::persist_completed_dialog_turn(
        coordinator.event_queue.as_ref(),
        session_manager.as_ref(),
        None,
        &session.session_id,
        &turn_id,
        &ExecutionResult {
            final_message: message.clone(),
            total_rounds: 1,
            success: true,
            new_messages: vec![message],
            finish_reason: FinishReason::Complete,
            total_tools: 0,
            duration_ms: 1,
            partial_recovery_reason: None,
            effective_finish_reason: "user_steering".to_string(),
            has_final_response: false,
        },
        None,
    )
    .await;

    assert_eq!(
        session_manager
            .turn_settlement_result(&session.session_id, &turn_id)
            .and_then(|result| result.final_response),
        None
    );

    assert_eq!(
        session_manager
            .turn_settlement_result(&session.session_id, &turn_id)
            .unwrap()
            .status,
        AgentTurnSettlementStatus::Completed
    );
    let events = coordinator.event_queue.dequeue_batch(10).await;
    assert!(events.iter().any(|envelope| matches!(
        &envelope.event,
        AgenticEvent::SessionHistoryChanged {
            session_id,
            settled_turn_id: Some(settled_turn_id),
        } if session_id == &session.session_id && settled_turn_id == &turn_id
    )));
    session_manager
        .delete_session_by_id(&session.session_id)
        .await
        .expect("clean up persisted test session");
}

#[tokio::test]
async fn transient_turns_keep_authoritative_terminal_results() {
    let workspace = tempfile::tempdir().expect("workspace");
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(workspace.path());
    let (coordinator, session_manager) = test_persistent_coordinator();
    let session = session_manager
        .create_transient_session_with_id_and_details(
            Some("transient-session".to_string()),
            "Transient completion".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
                ..Default::default()
            },
            None,
            SessionKind::Standard,
        )
        .await
        .expect("create transient session");
    let turn_id = session_manager
        .start_dialog_turn(
            &session.session_id,
            "Standard".to_string(),
            "finish".to_string(),
            Some("turn-transient-result".to_string()),
            None,
            None,
        )
        .await
        .expect("start turn");
    let intermediate = Message::assistant("intermediate tool-round text".to_string())
        .with_turn_id(turn_id.clone())
        .with_round_id("round-tool".to_string());
    let final_message = Message::assistant("authoritative final answer".to_string())
        .with_turn_id(turn_id.clone())
        .with_round_id("round-final".to_string());

    ConversationCoordinator::persist_completed_dialog_turn(
        coordinator.event_queue.as_ref(),
        session_manager.as_ref(),
        None,
        &session.session_id,
        &turn_id,
        &ExecutionResult {
            final_message: final_message.clone(),
            total_rounds: 2,
            success: true,
            new_messages: vec![intermediate, final_message],
            finish_reason: FinishReason::Complete,
            total_tools: 1,
            duration_ms: 1,
            partial_recovery_reason: None,
            effective_finish_reason: "complete".to_string(),
            has_final_response: true,
        },
        None,
    )
    .await;

    let settlement = session_manager
        .turn_settlement_result(&session.session_id, &turn_id)
        .expect("transient turn settlement result");
    assert_eq!(settlement.status, AgentTurnSettlementStatus::Completed);
    assert_eq!(
        settlement.final_response.as_deref(),
        Some("authoritative final answer")
    );
    assert_eq!(settlement.finish_reason.as_deref(), Some("complete"));

    let cancelled_turn_id = session_manager
        .start_dialog_turn(
            &session.session_id,
            "Standard".to_string(),
            "cancel".to_string(),
            Some("turn-transient-cancelled".to_string()),
            None,
            None,
        )
        .await
        .expect("start cancelled turn");
    ConversationCoordinator::persist_cancelled_dialog_turn(
        coordinator.event_queue.as_ref(),
        session_manager.as_ref(),
        None,
        &session.session_id,
        &cancelled_turn_id,
        true,
    )
    .await;
    let cancelled = session_manager
        .turn_settlement_result(&session.session_id, &cancelled_turn_id)
        .expect("cancelled turn settlement result");
    assert_eq!(cancelled.status, AgentTurnSettlementStatus::Cancelled);
    assert_eq!(cancelled.final_response, None);
    assert_eq!(cancelled.finish_reason.as_deref(), Some("cancelled"));

    let failed_turn_id = session_manager
        .start_dialog_turn(
            &session.session_id,
            "Standard".to_string(),
            "fail".to_string(),
            Some("turn-transient-failed".to_string()),
            None,
            None,
        )
        .await
        .expect("start failed turn");
    ConversationCoordinator::persist_failed_dialog_turn(
        coordinator.event_queue.as_ref(),
        session_manager.as_ref(),
        None,
        &session.session_id,
        &failed_turn_id,
        &OpenBitFunError::AIClient("provider failed".to_string()),
        true,
    )
    .await;
    let failed = session_manager
        .turn_settlement_result(&session.session_id, &failed_turn_id)
        .expect("failed turn settlement result");
    assert_eq!(failed.status, AgentTurnSettlementStatus::Failed);
    assert_eq!(failed.final_response, None);
    assert_eq!(failed.finish_reason.as_deref(), Some("failed"));
}
