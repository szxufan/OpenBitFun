use super::super::SessionManager;
use super::support::{in_memory_test_manager, test_manager, TestWorkspace};
use crate::agentic::core::{ProcessingPhase, Session, SessionConfig, SessionState, TurnStats};
use crate::agentic::persistence::PersistenceManager;
use crate::service::session::{DialogTurnData, DialogTurnKind, TurnStatus};
use openbitfun_runtime_ports::{AgentTurnSettlementResult, AgentTurnSettlementStatus};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

#[tokio::test]
async fn background_delivery_is_bound_to_an_admitted_run_and_survives_completion_and_checkpoints() {
    use openbitfun_core_types::background_delivery::{
        BackgroundDelivery, BackgroundReport, BackgroundReportDisposition as Disposition,
        BackgroundReportPolicy as Policy, BACKGROUND_DELIVERY_METADATA_KEY,
    };
    let workspace = TestWorkspace::new();
    let persistence = Arc::new(PersistenceManager::new(workspace.path_manager()).unwrap());
    let manager = test_manager(persistence.clone());
    let session = manager
        .create_session(
            "Background delivery".into(),
            "Standard".into(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let pending = BackgroundDelivery::pending(
        "generic-monitor".into(),
        "background-run".into(),
        Policy::Conditional,
    );
    let turn = crate::service::session::DialogTurnData::new(
        "background-run".into(),
        0,
        session.session_id.clone(),
        crate::service::session::UserMessageData {
            id: "input".into(),
            content: "Only report changes.".into(),
            timestamp: 1,
            metadata: Some(serde_json::json!({BACKGROUND_DELIVERY_METADATA_KEY:pending})),
        },
    );
    persistence
        .save_dialog_turn(workspace.path(), &turn)
        .await
        .unwrap();
    manager
        .sessions
        .get_mut(&session.session_id)
        .unwrap()
        .dialog_turn_ids
        .push(turn.turn_id.clone());
    let proposal = BackgroundReport {
        disposition: Disposition::Quiet,
        reason: "No reportable changes.".into(),
    };
    manager
        .propose_background_report(&session.session_id, &turn.turn_id, proposal.clone())
        .await
        .unwrap();
    let admitted = manager
        .background_delivery_for_turn(&session.session_id, &turn.turn_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(admitted.proposal, Some(proposal.clone()));
    let resolved = openbitfun_agent_runtime::scheduler::result_delivery::resolve_report(
        admitted,
        true,
        "complete",
        Some(proposal.clone()),
        "structured",
        2,
    );
    manager
        .set_background_delivery_for_turn(&session.session_id, &turn.turn_id, &resolved)
        .await
        .unwrap();
    manager
        .complete_dialog_turn(
            &session.session_id,
            &turn.turn_id,
            "Nothing changed; remaining quiet.".into(),
            &[],
            TurnStats {
                total_rounds: 1,
                total_tools: 0,
                total_tokens: 0,
                duration_ms: 10,
            },
            Some("complete".into()),
            Some(true),
        )
        .await
        .unwrap();
    let completed = persistence
        .load_dialog_turn(workspace.path(), &session.session_id, 0)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        completed.status,
        crate::service::session::TurnStatus::Completed
    );
    assert!(!completed.model_rounds.is_empty());
    let raw_rounds = serde_json::to_value(&completed.model_rounds).unwrap();
    let mut checkpoint = completed;
    checkpoint.user_message.metadata =
        Some(serde_json::json!({BACKGROUND_DELIVERY_METADATA_KEY:pending}));
    persistence
        .save_dialog_turn(workspace.path(), &checkpoint)
        .await
        .unwrap();
    let saved = persistence
        .load_dialog_turn(workspace.path(), &session.session_id, 0)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        serde_json::to_value(&saved.model_rounds).unwrap(),
        raw_rounds
    );
    assert_eq!(
        BackgroundDelivery::from_metadata(saved.user_message.metadata.as_ref(), &saved.turn_id)
            .unwrap()
            .disposition,
        Disposition::Quiet
    );
    assert!(manager
        .propose_background_report(&session.session_id, &turn.turn_id, proposal.clone())
        .await
        .is_err());
    let mut resumed = saved;
    resumed.recovery_epoch = Some(1);
    resumed.status = crate::service::session::TurnStatus::InProgress;
    persistence
        .save_dialog_turn(workspace.path(), &resumed)
        .await
        .unwrap();
    let recovering = manager
        .background_delivery_for_turn(&session.session_id, &turn.turn_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(recovering.execution_generation, 1);
    assert_eq!(recovering.disposition, Disposition::Pending);
    assert!(recovering.resolved_at_ms.is_none());
    let recovered = openbitfun_agent_runtime::scheduler::result_delivery::resolve_report(
        recovering,
        true,
        "complete",
        Some(proposal.clone()),
        "structured",
        4,
    );
    manager
        .set_background_delivery_for_turn(&session.session_id, &turn.turn_id, &recovered)
        .await
        .unwrap();
    // A delayed observer snapshot must retain both the new decision and the older audit record.
    persistence
        .save_dialog_turn(workspace.path(), &resumed)
        .await
        .unwrap();
    let recovered = persistence
        .load_dialog_turn(workspace.path(), &session.session_id, 0)
        .await
        .unwrap()
        .unwrap();
    let metadata = recovered.user_message.metadata.unwrap();
    assert_eq!(
        metadata[BACKGROUND_DELIVERY_METADATA_KEY]["executionGeneration"],
        1
    );
    assert_eq!(
        metadata[BACKGROUND_DELIVERY_METADATA_KEY]["disposition"],
        "report"
    );
    assert_eq!(
        metadata["backgroundDeliveryHistory"][0]["disposition"],
        "quiet"
    );
    assert_eq!(
        serde_json::to_value(&recovered.model_rounds).unwrap(),
        raw_rounds
    );
    let ordinary = crate::service::session::DialogTurnData::new(
        "ordinary".into(),
        1,
        session.session_id.clone(),
        crate::service::session::UserMessageData {
            id: "manual".into(),
            content: "Tell me the last result.".into(),
            timestamp: 3,
            metadata: None,
        },
    );
    persistence
        .save_dialog_turn(workspace.path(), &ordinary)
        .await
        .unwrap();
    manager
        .sessions
        .get_mut(&session.session_id)
        .unwrap()
        .dialog_turn_ids
        .push(ordinary.turn_id.clone());
    assert!(manager
        .propose_background_report(&session.session_id, &ordinary.turn_id, proposal)
        .await
        .is_err());
}

#[tokio::test]
async fn refreshed_turn_settlement_results_remain_bounded_and_clear_with_the_session() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager);
    let result = |status| AgentTurnSettlementResult {
        status,
        final_response: None,
        finish_reason: None,
    };

    manager.record_turn_settlement_result(
        "session-refresh",
        "turn-refresh",
        result(AgentTurnSettlementStatus::Cancelled),
    );
    for index in 0..1_023 {
        manager.record_turn_settlement_result(
            "other-session",
            &format!("turn-{index}"),
            result(AgentTurnSettlementStatus::Completed),
        );
    }
    manager.record_turn_settlement_result(
        "session-refresh",
        "turn-refresh",
        result(AgentTurnSettlementStatus::Completed),
    );
    manager.record_turn_settlement_result(
        "other-session",
        "turn-overflow",
        result(AgentTurnSettlementStatus::Completed),
    );

    assert_eq!(
        manager
            .turn_settlement_result("session-refresh", "turn-refresh")
            .map(|result| result.status),
        Some(AgentTurnSettlementStatus::Completed)
    );
    manager.clear_turn_settlement_results("session-refresh");
    assert!(manager
        .turn_settlement_result("session-refresh", "turn-refresh")
        .is_none());
}

#[tokio::test]
async fn reset_session_state_if_processing_ignores_a_newer_turn() {
    let manager = in_memory_test_manager();
    let session_id = Uuid::new_v4().to_string();
    let mut session = Session::new_with_id(
        session_id.clone(),
        "Active session".to_string(),
        "agent".to_string(),
        SessionConfig::default(),
    );
    session.state = SessionState::Processing {
        current_turn_id: "turn-2".to_string(),
        phase: ProcessingPhase::Thinking,
    };
    manager.sessions.insert(session_id.clone(), session);

    manager.reset_session_state_if_processing(&session_id, "turn-1");

    let session = manager
        .get_session(&session_id)
        .expect("session should remain available");
    assert!(matches!(
        session.state,
        SessionState::Processing {
            ref current_turn_id,
            ..
        } if current_turn_id == "turn-2"
    ));
}

#[tokio::test]
async fn reset_session_state_if_processing_resets_the_matching_turn() {
    let manager = in_memory_test_manager();
    let session_id = Uuid::new_v4().to_string();
    let mut session = Session::new_with_id(
        session_id.clone(),
        "Active session".to_string(),
        "agent".to_string(),
        SessionConfig::default(),
    );
    session.state = SessionState::Processing {
        current_turn_id: "turn-1".to_string(),
        phase: ProcessingPhase::Thinking,
    };
    manager.sessions.insert(session_id.clone(), session);

    manager.reset_session_state_if_processing(&session_id, "turn-1");

    let session = manager
        .get_session(&session_id)
        .expect("session should remain available");
    assert!(matches!(session.state, SessionState::Idle));
}

#[tokio::test]
async fn update_session_state_for_turn_if_processing_ignores_a_newer_turn() {
    let manager = in_memory_test_manager();
    let session_id = Uuid::new_v4().to_string();
    let mut session = Session::new_with_id(
        session_id.clone(),
        "Active session".to_string(),
        "agent".to_string(),
        SessionConfig::default(),
    );
    session.state = SessionState::Processing {
        current_turn_id: "turn-2".to_string(),
        phase: ProcessingPhase::Thinking,
    };
    manager.sessions.insert(session_id.clone(), session);

    let updated = manager
        .update_session_state_for_turn_if_processing(&session_id, "turn-1", SessionState::Idle)
        .await
        .expect("conditional state update should not fail");

    let session = manager
        .get_session(&session_id)
        .expect("session should remain available");
    assert!(!updated);
    assert!(matches!(
        session.state,
        SessionState::Processing {
            ref current_turn_id,
            ..
        } if current_turn_id == "turn-2"
    ));
}

#[tokio::test]
async fn update_session_state_for_turn_if_processing_updates_matching_turn() {
    let manager = in_memory_test_manager();
    let session_id = Uuid::new_v4().to_string();
    let mut session = Session::new_with_id(
        session_id.clone(),
        "Active session".to_string(),
        "agent".to_string(),
        SessionConfig::default(),
    );
    session.state = SessionState::Processing {
        current_turn_id: "turn-1".to_string(),
        phase: ProcessingPhase::Thinking,
    };
    manager.sessions.insert(session_id.clone(), session);

    let updated = manager
        .update_session_state_for_turn_if_processing(&session_id, "turn-1", SessionState::Idle)
        .await
        .expect("conditional state update should not fail");

    let session = manager
        .get_session(&session_id)
        .expect("session should remain available");
    assert!(updated);
    assert!(matches!(session.state, SessionState::Idle));
}

#[tokio::test]
async fn voice_exchange_survives_restore_and_idempotent_replay() {
    let workspace = TestWorkspace::new();
    let persistence = Arc::new(PersistenceManager::new(workspace.path_manager()).unwrap());
    let manager = test_manager(persistence.clone());
    let session = manager
        .create_session(
            "Voice".into(),
            "agent".into(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    for _ in 0..2 {
        manager
            .append_voice_exchange(
                &session.session_id,
                "voice-1",
                "Remember this".into(),
                "I will".into(),
            )
            .await
            .unwrap();
    }
    assert_eq!(
        manager
            .get_messages(&session.session_id)
            .await
            .unwrap()
            .len(),
        2
    );
    let turns = persistence
        .load_session_turns(workspace.path(), &session.session_id)
        .await
        .unwrap();
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].kind, DialogTurnKind::UserDialog);
    assert_eq!(SessionManager::build_messages_from_turns(&turns).len(), 2);
    // The existing persisted UserDialog shape is enough; no new enum is required.
    let encoded = serde_json::to_value(&turns[0]).unwrap();
    let decoded: DialogTurnData = serde_json::from_value(encoded).unwrap();
    assert_eq!(decoded.user_message.content, "Remember this");
    assert!(manager
        .unload_session_from_memory(&session.session_id)
        .await
        .expect("first writer should unload"));
    drop(manager);
    let restored = test_manager(persistence);
    restored
        .restore_session(workspace.path(), &session.session_id)
        .await
        .unwrap();
    restored
        .append_voice_exchange(
            &session.session_id,
            "voice-1",
            "Remember this".into(),
            "I will".into(),
        )
        .await
        .unwrap();
    assert_eq!(
        restored
            .get_messages(&session.session_id)
            .await
            .unwrap()
            .len(),
        2
    );
}

#[tokio::test]
async fn voice_exchange_does_not_overwrite_a_running_turn() {
    let workspace = TestWorkspace::new();
    let manager = test_manager(Arc::new(
        PersistenceManager::new(workspace.path_manager()).unwrap(),
    ));
    let session = manager
        .create_session(
            "Voice".into(),
            "agent".into(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    manager.sessions.get_mut(&session.session_id).unwrap().state = SessionState::Processing {
        current_turn_id: "running".into(),
        phase: ProcessingPhase::Thinking,
    };
    assert!(manager
        .append_voice_exchange(&session.session_id, "voice-2", "Hello".into(), "Hi".into())
        .await
        .is_err());
    let unchanged = manager.get_session(&session.session_id).unwrap();
    assert!(matches!(unchanged.state, SessionState::Processing { .. }));
    assert!(unchanged.dialog_turn_ids.is_empty());
}

#[tokio::test]
async fn append_completed_local_command_turn_persists_without_model_context() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let session = manager
        .create_session(
            "Usage session".to_string(),
            "agent".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");

    let turn = manager
        .append_completed_local_command_turn(
            &session.session_id,
            "# Session Usage Report".to_string(),
            Some("local-usage-1".to_string()),
            Some(42),
            Some(json!({
                "localCommandKind": "usage_report",
                "modelVisible": false,
            })),
        )
        .await
        .expect("local command turn should persist");

    assert_eq!(turn.kind, DialogTurnKind::LocalCommand);
    assert_eq!(turn.status, TurnStatus::Completed);

    let active = manager
        .get_session(&session.session_id)
        .expect("session should remain active");
    assert_eq!(active.dialog_turn_ids, vec!["local-usage-1".to_string()]);
    assert!(manager
        .context_store
        .get_context_messages(&session.session_id)
        .is_empty());

    let persisted_turns = persistence_manager
        .load_session_turns(workspace.path(), &session.session_id)
        .await
        .expect("turns should load");
    assert_eq!(persisted_turns.len(), 1);
    assert_eq!(persisted_turns[0].kind, DialogTurnKind::LocalCommand);
    assert!(SessionManager::build_messages_from_turns(&persisted_turns).is_empty());

    let metadata = persistence_manager
        .load_session_metadata(workspace.path(), &session.session_id)
        .await
        .expect("metadata should load")
        .expect("metadata should exist");
    assert_eq!(metadata.turn_count, 1);
}

#[tokio::test]
async fn append_completed_local_command_turn_waits_for_session_mutation() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = Arc::new(test_manager(persistence_manager));
    let session = manager
        .create_session(
            "Serialized local command".to_string(),
            "agent".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");
    let mutation = manager
        .acquire_session_mutation(&session.session_id)
        .await
        .expect("hold mutation boundary");
    let append_manager = manager.clone();
    let append_session_id = session.session_id.clone();
    let append = tokio::spawn(async move {
        append_manager
            .append_completed_local_command_turn(
                &append_session_id,
                "usage report".to_string(),
                Some("usage-turn".to_string()),
                Some(1),
                None,
            )
            .await
    });
    tokio::time::sleep(Duration::from_millis(50)).await;

    assert!(!append.is_finished());
    drop(mutation);
    append
        .await
        .expect("append task should join")
        .expect("append should succeed after mutation releases");
}

#[tokio::test]
async fn restore_session_resets_processing_state_without_marking_unread_completion() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let session_id = Uuid::new_v4().to_string();
    let mut session = Session::new_with_id(
        session_id.clone(),
        "Legacy processing session".to_string(),
        "agent".to_string(),
        SessionConfig {
            tool_presentation_mode: Default::default(),
            workspace_path: Some(workspace.path().to_string_lossy().to_string()),
            ..Default::default()
        },
    );
    session.state = SessionState::Processing {
        current_turn_id: "turn-1".to_string(),
        phase: ProcessingPhase::Thinking,
    };

    persistence_manager
        .save_session(workspace.path(), &session)
        .await
        .expect("session should save");
    persistence_manager
        .save_session_state(workspace.path(), &session_id, &session.state)
        .await
        .expect("processing state should save");

    let manager = test_manager(persistence_manager.clone());
    let restored = manager
        .restore_session(workspace.path(), &session_id)
        .await
        .expect("session should restore");
    let metadata = persistence_manager
        .load_session_metadata(workspace.path(), &session_id)
        .await
        .expect("metadata should load")
        .expect("metadata should exist");

    assert!(matches!(restored.state, SessionState::Idle));
    assert_eq!(metadata.unread_completion, None);
}
