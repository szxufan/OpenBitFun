use super::super::{SessionManager, SessionManagerConfig};
use super::support::{evidence_event_ids, test_manager, test_manager_with_config, TestWorkspace};
use crate::agentic::core::SessionConfig;
use crate::agentic::persistence::PersistenceManager;
use crate::agentic::session::{PersistedEvidenceLedgerFile, PromptCachePolicy};
use crate::service::session::{DialogTurnData, UserMessageData};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

struct StagedEvidenceSession {
    session_id: String,
    ledger_path: PathBuf,
}

async fn create_staged_evidence_session(
    manager: &SessionManager,
    persistence_manager: &PersistenceManager,
    workspace: &TestWorkspace,
    turn_count: usize,
) -> StagedEvidenceSession {
    let session = manager
        .create_session(
            "Staged evidence".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");
    for index in 0..turn_count {
        let turn = DialogTurnData::new(
            format!("turn-{index}"),
            index,
            session.session_id.clone(),
            UserMessageData {
                id: format!("turn-{index}-user"),
                content: format!("prompt {index}"),
                timestamp: index as u64,
                metadata: None,
            },
        );
        persistence_manager
            .save_dialog_turn(workspace.path(), &turn)
            .await
            .expect("turn should save");
        let messages = (0..=index)
            .map(|message_index| {
                crate::agentic::core::Message::user(format!("prompt {message_index}"))
            })
            .collect::<Vec<_>>();
        persistence_manager
            .save_turn_context_snapshot(workspace.path(), &session.session_id, index, &messages)
            .await
            .expect("context snapshot should save");
    }
    manager
        .sessions
        .get_mut(&session.session_id)
        .expect("session should be active")
        .dialog_turn_ids = (0..turn_count)
        .map(|index| format!("turn-{index}"))
        .collect();
    for index in 0..turn_count {
        manager
            .record_subagent_partial_timeout(
                &session.session_id,
                &format!("turn-{index}"),
                "ReviewSecurity",
                &format!("Partial turn {index}"),
                Some("timeout"),
            )
            .await
            .expect("turn evidence should persist");
    }
    let storage_path = manager
        .effective_session_storage_path(&session.session_id)
        .await
        .expect("storage path");
    let ledger_path = storage_path
        .join(&session.session_id)
        .join("evidence-ledger.json");
    StagedEvidenceSession {
        session_id: session.session_id,
        ledger_path,
    }
}

#[tokio::test]
async fn staged_revert_filters_memory_but_keeps_evidence_sidecar() {
    use crate::agentic::session::revert::{
        SessionRevertPhase, SessionRevertState, SESSION_REVERT_SCHEMA_VERSION,
    };

    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let session = manager
        .create_session(
            "Staged revert evidence".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");

    for index in 0..3 {
        let turn = DialogTurnData::new(
            format!("turn-{index}"),
            index,
            session.session_id.clone(),
            UserMessageData {
                id: format!("turn-{index}-user"),
                content: format!("prompt {index}"),
                timestamp: index as u64,
                metadata: None,
            },
        );
        persistence_manager
            .save_dialog_turn(workspace.path(), &turn)
            .await
            .expect("turn should save");
        let messages = (0..=index)
            .map(|message_index| {
                crate::agentic::core::Message::user(format!("prompt {message_index}"))
            })
            .collect::<Vec<_>>();
        persistence_manager
            .save_turn_context_snapshot(workspace.path(), &session.session_id, index, &messages)
            .await
            .expect("context snapshot should save");
    }
    manager
        .sessions
        .get_mut(&session.session_id)
        .expect("session should be active")
        .dialog_turn_ids = vec![
        "turn-0".to_string(),
        "turn-1".to_string(),
        "turn-2".to_string(),
    ];
    let turn_0_event = manager
        .record_subagent_partial_timeout(
            &session.session_id,
            "turn-0",
            "ReviewSecurity",
            "Partial turn 0",
            Some("timeout"),
        )
        .await
        .expect("turn-0 evidence should persist");
    let turn_1_event = manager
        .record_subagent_partial_timeout(
            &session.session_id,
            "turn-1",
            "ReviewTests",
            "Partial turn 1",
            Some("timeout"),
        )
        .await
        .expect("turn-1 evidence should persist");
    let turn_2_event = manager
        .record_subagent_partial_timeout(
            &session.session_id,
            "turn-2",
            "Edit",
            "Partial turn 2",
            Some("timeout"),
        )
        .await
        .expect("turn-2 evidence should persist");
    let storage_path = manager
        .effective_session_storage_path(&session.session_id)
        .await
        .expect("storage path");
    let ledger_path = storage_path
        .join(&session.session_id)
        .join("evidence-ledger.json");
    let stored_before: PersistedEvidenceLedgerFile =
        serde_json::from_slice(&std::fs::read(&ledger_path).expect("ledger sidecar should exist"))
            .expect("ledger sidecar should deserialize");
    assert_eq!(
        stored_before
            .events
            .iter()
            .map(|event| event.event_id.clone())
            .collect::<Vec<_>>(),
        vec![
            turn_0_event.event_id.clone(),
            turn_1_event.event_id.clone(),
            turn_2_event.event_id.clone(),
        ]
    );

    let state = SessionRevertState {
        schema_version: SESSION_REVERT_SCHEMA_VERSION,
        boundary_turn: 2,
        original_turn_end: 3,
        phase: SessionRevertPhase::Staged,
        workspace_checkpoint: Vec::new(),
    };
    persistence_manager
        .save_session_revert_state(workspace.path(), &session.session_id, &state)
        .await
        .expect("staged revert should persist");

    manager
        .apply_staged_revert_context_locked(
            workspace.path(),
            &session.session_id,
            state.boundary_turn,
        )
        .await
        .expect("staged context should apply");

    assert!(manager
        .evidence_events_for_turn(&session.session_id, "turn-2")
        .is_empty());
    assert_eq!(
        manager.evidence_events_for_turn(&session.session_id, "turn-1"),
        vec![turn_1_event.clone()]
    );
    let stored_after: PersistedEvidenceLedgerFile = serde_json::from_slice(
        &std::fs::read(&ledger_path).expect("ledger sidecar should still exist"),
    )
    .expect("ledger sidecar should deserialize");
    assert_eq!(
        stored_after.events,
        vec![
            turn_0_event.clone(),
            turn_1_event.clone(),
            turn_2_event.clone()
        ]
    );

    assert!(manager
        .unload_session_from_memory(&session.session_id)
        .await
        .expect("session should unload"));
    let restored = manager
        .restore_session(workspace.path(), &session.session_id)
        .await
        .expect("staged session should restore");
    assert_eq!(
        restored.dialog_turn_ids,
        vec!["turn-0".to_string(), "turn-1".to_string()]
    );
    assert_eq!(
        manager.evidence_events_for_turn(&session.session_id, "turn-1"),
        vec![turn_1_event.clone()]
    );
    assert!(manager
        .evidence_events_for_turn(&session.session_id, "turn-2")
        .is_empty());
    assert_eq!(
        evidence_event_ids(&ledger_path),
        vec![
            turn_0_event.event_id.clone(),
            turn_1_event.event_id.clone(),
            turn_2_event.event_id.clone(),
        ]
    );
    assert_eq!(
        manager
            .evidence_summary_for_session(&session.session_id, 10)
            .partial_subagent_results
            .len(),
        2
    );
}

#[tokio::test]
async fn staged_undo_then_redo_restores_evidence_from_sidecar() {
    use crate::agentic::session::revert::{
        SessionRevertPhase, SessionRevertState, SESSION_REVERT_SCHEMA_VERSION,
    };

    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let staged =
        create_staged_evidence_session(&manager, &persistence_manager, &workspace, 3).await;
    let original_ledger = evidence_event_ids(&staged.ledger_path);
    assert_eq!(original_ledger.len(), 3);

    let state = SessionRevertState {
        schema_version: SESSION_REVERT_SCHEMA_VERSION,
        boundary_turn: 2,
        original_turn_end: 3,
        phase: SessionRevertPhase::Staged,
        workspace_checkpoint: Vec::new(),
    };
    persistence_manager
        .save_session_revert_state(workspace.path(), &staged.session_id, &state)
        .await
        .expect("staged undo should persist");
    let _mutation = manager
        .acquire_session_mutation(&staged.session_id)
        .await
        .expect("session mutation");
    manager
        .apply_staged_revert_context_locked(
            workspace.path(),
            &staged.session_id,
            state.boundary_turn,
        )
        .await
        .expect("staged undo should apply");
    assert!(manager
        .evidence_events_for_turn(&staged.session_id, "turn-2")
        .is_empty());
    assert_eq!(evidence_event_ids(&staged.ledger_path), original_ledger);

    // Redo clears the staged boundary back to the original end. The intact
    // sidecar must repopulate memory with the hidden turn evidence.
    manager
        .apply_staged_revert_context_locked(
            workspace.path(),
            &staged.session_id,
            state.original_turn_end,
        )
        .await
        .expect("redo should reapply the full boundary");
    assert_eq!(
        manager
            .evidence_events_for_turn(&staged.session_id, "turn-2")
            .len(),
        1
    );
    persistence_manager
        .delete_session_revert_state(workspace.path(), &staged.session_id)
        .await
        .expect("redo marker should clear");
    assert_eq!(
        manager
            .get_session(&staged.session_id)
            .expect("session should remain active")
            .dialog_turn_ids,
        vec![
            "turn-0".to_string(),
            "turn-1".to_string(),
            "turn-2".to_string()
        ]
    );
    assert_eq!(evidence_event_ids(&staged.ledger_path), original_ledger);
    assert_eq!(
        manager
            .evidence_summary_for_session(&staged.session_id, 10)
            .partial_subagent_results
            .len(),
        3
    );
}

#[tokio::test]
async fn consecutive_staged_undo_and_redo_keep_sidecar_evidence() {
    use crate::agentic::session::revert::{
        SessionRevertPhase, SessionRevertState, SESSION_REVERT_SCHEMA_VERSION,
    };

    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let staged =
        create_staged_evidence_session(&manager, &persistence_manager, &workspace, 3).await;
    let original_ledger = evidence_event_ids(&staged.ledger_path);
    assert_eq!(original_ledger.len(), 3);
    let _mutation = manager
        .acquire_session_mutation(&staged.session_id)
        .await
        .expect("session mutation");

    for boundary in [2usize, 1, 0] {
        let state = SessionRevertState {
            schema_version: SESSION_REVERT_SCHEMA_VERSION,
            boundary_turn: boundary,
            original_turn_end: 3,
            phase: SessionRevertPhase::Staged,
            workspace_checkpoint: Vec::new(),
        };
        persistence_manager
            .save_session_revert_state(workspace.path(), &staged.session_id, &state)
            .await
            .expect("staged undo should persist");
        manager
            .apply_staged_revert_context_locked(workspace.path(), &staged.session_id, boundary)
            .await
            .expect("staged undo should apply");
    }
    assert!(manager
        .evidence_events_for_turn(&staged.session_id, "turn-2")
        .is_empty());
    assert!(manager
        .evidence_events_for_turn(&staged.session_id, "turn-1")
        .is_empty());
    assert_eq!(evidence_event_ids(&staged.ledger_path), original_ledger);

    for boundary in [1usize, 2, 3] {
        let state = SessionRevertState {
            schema_version: SESSION_REVERT_SCHEMA_VERSION,
            boundary_turn: boundary,
            original_turn_end: 3,
            phase: SessionRevertPhase::Staged,
            workspace_checkpoint: Vec::new(),
        };
        persistence_manager
            .save_session_revert_state(workspace.path(), &staged.session_id, &state)
            .await
            .expect("staged redo should persist");
        manager
            .apply_staged_revert_context_locked(workspace.path(), &staged.session_id, boundary)
            .await
            .expect("staged redo should apply");
    }
    persistence_manager
        .delete_session_revert_state(workspace.path(), &staged.session_id)
        .await
        .expect("redo marker should clear");
    assert_eq!(
        manager
            .get_session(&staged.session_id)
            .expect("session should remain active")
            .dialog_turn_ids,
        vec![
            "turn-0".to_string(),
            "turn-1".to_string(),
            "turn-2".to_string()
        ]
    );
    assert_eq!(evidence_event_ids(&staged.ledger_path), original_ledger);
    assert_eq!(
        manager
            .evidence_events_for_turn(&staged.session_id, "turn-2")
            .len(),
        1
    );
    assert_eq!(
        manager
            .evidence_events_for_turn(&staged.session_id, "turn-1")
            .len(),
        1
    );
}

#[tokio::test]
async fn restoring_clearing_phase_keeps_redo_evidence_available() {
    use crate::agentic::session::revert::{
        SessionRevertPhase, SessionRevertState, SESSION_REVERT_SCHEMA_VERSION,
    };

    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let staged =
        create_staged_evidence_session(&manager, &persistence_manager, &workspace, 3).await;
    let original_ledger = evidence_event_ids(&staged.ledger_path);
    assert_eq!(original_ledger.len(), 3);

    let state = SessionRevertState {
        schema_version: SESSION_REVERT_SCHEMA_VERSION,
        boundary_turn: 3,
        original_turn_end: 3,
        phase: SessionRevertPhase::Clearing,
        workspace_checkpoint: Vec::new(),
    };
    persistence_manager
        .save_session_revert_state(workspace.path(), &staged.session_id, &state)
        .await
        .expect("clearing marker should persist");
    assert!(manager
        .unload_session_from_memory(&staged.session_id)
        .await
        .expect("session should unload"));

    manager
        .restore_session(workspace.path(), &staged.session_id)
        .await
        .expect("clearing session should restore");
    let _mutation = manager
        .acquire_session_mutation(&staged.session_id)
        .await
        .expect("session mutation");
    manager
        .apply_staged_revert_context_locked(
            workspace.path(),
            &staged.session_id,
            state.boundary_turn,
        )
        .await
        .expect("clearing boundary should reapply");
    persistence_manager
        .delete_session_revert_state(workspace.path(), &staged.session_id)
        .await
        .expect("clearing marker should clear");
    assert_eq!(
        manager
            .get_session(&staged.session_id)
            .expect("session should restore")
            .dialog_turn_ids,
        vec![
            "turn-0".to_string(),
            "turn-1".to_string(),
            "turn-2".to_string()
        ]
    );
    assert_eq!(
        manager
            .evidence_events_for_turn(&staged.session_id, "turn-2")
            .len(),
        1
    );
    assert_eq!(evidence_event_ids(&staged.ledger_path), original_ledger);
    assert_eq!(
        manager
            .evidence_summary_for_session(&staged.session_id, 10)
            .partial_subagent_results
            .len(),
        3
    );
}

#[tokio::test]
async fn staged_revert_prunes_evidence_sidecar_without_automatic_persistence() {
    use crate::agentic::session::revert::{
        SessionRevertPhase, SessionRevertState, SESSION_REVERT_SCHEMA_VERSION,
    };

    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let writer = test_manager(persistence_manager.clone());
    let session = writer
        .create_session(
            "Staged revert explicit history evidence".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");

    for index in 0..2 {
        let turn = DialogTurnData::new(
            format!("turn-{index}"),
            index,
            session.session_id.clone(),
            UserMessageData {
                id: format!("turn-{index}-user"),
                content: format!("prompt {index}"),
                timestamp: index as u64,
                metadata: None,
            },
        );
        persistence_manager
            .save_dialog_turn(workspace.path(), &turn)
            .await
            .expect("turn should save");
        let messages = (0..=index)
            .map(|message_index| {
                crate::agentic::core::Message::user(format!("prompt {message_index}"))
            })
            .collect::<Vec<_>>();
        persistence_manager
            .save_turn_context_snapshot(workspace.path(), &session.session_id, index, &messages)
            .await
            .expect("context snapshot should save");
    }
    writer
        .sessions
        .get_mut(&session.session_id)
        .expect("session should be active")
        .dialog_turn_ids = vec!["turn-0".to_string(), "turn-1".to_string()];
    let turn_0_event = writer
        .record_subagent_partial_timeout(
            &session.session_id,
            "turn-0",
            "ReviewSecurity",
            "Partial turn 0",
            Some("timeout"),
        )
        .await
        .expect("turn-0 evidence should persist");
    let _turn_1_event = writer
        .record_subagent_partial_timeout(
            &session.session_id,
            "turn-1",
            "ReviewTests",
            "Partial turn 1",
            Some("timeout"),
        )
        .await
        .expect("turn-1 evidence should persist");
    let storage_path = writer
        .effective_session_storage_path(&session.session_id)
        .await
        .expect("storage path");
    persistence_manager
        .save_session_revert_state(
            workspace.path(),
            &session.session_id,
            &SessionRevertState {
                schema_version: SESSION_REVERT_SCHEMA_VERSION,
                boundary_turn: 1,
                original_turn_end: 2,
                phase: SessionRevertPhase::Committing,
                workspace_checkpoint: Vec::new(),
            },
        )
        .await
        .expect("staged revert should persist");
    assert!(writer
        .unload_session_from_memory(&session.session_id)
        .await
        .expect("session should unload"));

    let manager = test_manager_with_config(
        persistence_manager.clone(),
        SessionManagerConfig {
            max_active_sessions: 100,
            session_idle_timeout: Duration::from_secs(3600),
            auto_save_interval: Duration::from_secs(300),
            enable_persistence: false,
            prompt_cache_policy: PromptCachePolicy::default(),
        },
    );
    let restored = manager
        .restore_session(workspace.path(), &session.session_id)
        .await
        .expect("explicit history should restore");
    assert_eq!(restored.dialog_turn_ids, vec!["turn-0".to_string()]);
    manager
        .commit_staged_revert_context_locked(&storage_path, &session.session_id, 1)
        .await
        .expect("staged revert should commit without automatic persistence");

    let stored: PersistedEvidenceLedgerFile = serde_json::from_slice(
        &std::fs::read(
            storage_path
                .join(&session.session_id)
                .join("evidence-ledger.json"),
        )
        .expect("ledger sidecar should still exist"),
    )
    .expect("ledger sidecar should deserialize");
    assert_eq!(stored.events, vec![turn_0_event.clone()]);
    assert_eq!(
        manager.evidence_events_for_turn(&session.session_id, "turn-0"),
        vec![turn_0_event]
    );
    assert!(manager
        .evidence_events_for_turn(&session.session_id, "turn-1")
        .is_empty());
    assert_eq!(
        manager
            .evidence_summary_for_session(&session.session_id, 10)
            .partial_subagent_results
            .len(),
        1
    );
}

#[tokio::test]
async fn legacy_rollback_prunes_evidence_ledger_to_surviving_turn_ids() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let session = manager
        .create_session(
            "Rollback evidence".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");

    for index in 0..2 {
        let turn = DialogTurnData::new(
            format!("turn-{index}"),
            index,
            session.session_id.clone(),
            UserMessageData {
                id: format!("turn-{index}-user"),
                content: format!("prompt {index}"),
                timestamp: index as u64,
                metadata: None,
            },
        );
        persistence_manager
            .save_dialog_turn(workspace.path(), &turn)
            .await
            .expect("turn should save");
        let messages = (0..=index)
            .map(|message_index| {
                crate::agentic::core::Message::user(format!("prompt {message_index}"))
            })
            .collect::<Vec<_>>();
        persistence_manager
            .save_turn_context_snapshot(workspace.path(), &session.session_id, index, &messages)
            .await
            .expect("context snapshot should save");
    }
    manager
        .sessions
        .get_mut(&session.session_id)
        .expect("session should be active")
        .dialog_turn_ids = vec!["turn-0".to_string(), "turn-1".to_string()];
    let turn_0_event = manager
        .record_subagent_partial_timeout(
            &session.session_id,
            "turn-0",
            "ReviewSecurity",
            "Partial turn 0",
            Some("timeout"),
        )
        .await
        .expect("turn-0 evidence should persist");
    let _turn_1_event = manager
        .record_subagent_partial_timeout(
            &session.session_id,
            "turn-1",
            "ReviewTests",
            "Partial turn 1",
            Some("timeout"),
        )
        .await
        .expect("turn-1 evidence should persist");

    manager
        .rollback_context_to_turn_start(workspace.path(), &session.session_id, 1)
        .await
        .expect("rollback should succeed");

    assert!(manager
        .evidence_events_for_turn(&session.session_id, "turn-1")
        .is_empty());
    assert_eq!(
        manager.evidence_events_for_turn(&session.session_id, "turn-0"),
        vec![turn_0_event.clone()]
    );
    let storage_path = manager
        .effective_session_storage_path(&session.session_id)
        .await
        .expect("storage path");
    let stored: PersistedEvidenceLedgerFile = serde_json::from_slice(
        &std::fs::read(
            storage_path
                .join(&session.session_id)
                .join("evidence-ledger.json"),
        )
        .expect("ledger sidecar should exist"),
    )
    .expect("ledger sidecar should deserialize");
    assert_eq!(stored.events, vec![turn_0_event.clone()]);

    assert!(manager
        .unload_session_from_memory(&session.session_id)
        .await
        .expect("session should unload"));
    let restored = manager
        .restore_session(workspace.path(), &session.session_id)
        .await
        .expect("rolled-back session should restore");
    assert_eq!(restored.dialog_turn_ids, vec!["turn-0".to_string()]);
    assert_eq!(
        manager.evidence_events_for_turn(&session.session_id, "turn-0"),
        vec![turn_0_event]
    );
    assert!(manager
        .evidence_events_for_turn(&session.session_id, "turn-1")
        .is_empty());
    let contract = manager
        .compression_contract_for_session(&session.session_id, 10)
        .expect("compression contract should be available");
    assert!(
        contract
            .subagent_statuses
            .iter()
            .all(|item| item.target != "ReviewTests"),
        "rolled-back turn evidence must not enter the compression contract"
    );
    assert!(contract
        .subagent_statuses
        .iter()
        .any(|item| item.target == "ReviewSecurity"));
    assert_eq!(
        manager
            .evidence_summary_for_session(&session.session_id, 10)
            .partial_subagent_results
            .len(),
        1
    );
}
