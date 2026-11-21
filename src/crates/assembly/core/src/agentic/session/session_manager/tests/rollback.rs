use super::super::SessionManager;
use super::support::{test_manager, TestWorkspace};
use crate::agentic::core::SessionConfig;
use crate::agentic::persistence::PersistenceManager;
use crate::service::session::{DialogTurnData, UserMessageData};
use crate::util::errors::OpenBitFunError;
use std::sync::Arc;

#[tokio::test]
async fn rollback_context_deletes_persisted_turns_from_target() {
    use crate::agentic::execution::edit_constraint_guard::{
        ConstraintExtractionRecord, ConstraintMatcher, ConstraintOperationScope,
        ConstraintRevocation, ConstraintSource, ExtractedConstraint, ExtractionStatus,
        ModelExtractionStatus,
    };

    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let session = manager
        .create_session(
            "Rollback session".to_string(),
            "agent".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");
    let test_constraint = ExtractedConstraint {
        id: "deterministic:test_files".to_string(),
        description: "do not modify tests".to_string(),
        operation_scope: ConstraintOperationScope::All,
        matcher: ConstraintMatcher::TestFiles,
        source: ConstraintSource::Deterministic,
        source_text: Some("Do not modify tests.".to_string()),
    };
    manager
        .remember_edit_constraint_extraction(
            &session.session_id,
            ConstraintExtractionRecord {
                message_sha256: "turn-0-hash".to_string(),
                dialog_turn_id: Some("turn-0".to_string()),
                status: ExtractionStatus::Extracted,
                constraints: vec![test_constraint.clone()],
                deterministic_constraint_count: 1,
                model_attempts: 0,
                active_constraint_ids: Vec::new(),
                revocation_authorized: true,
                model_status: ModelExtractionStatus::NotRun,
                model_constraints: Vec::new(),
                model_revocations: Vec::new(),
                revoked_constraint_ids: Vec::new(),
                unmatched_revocation_ids: Vec::new(),
                input_chars: 20,
                prompt_chars: 20,
                input_truncated: false,
                latency_ms: 1,
                extracted_at_ms: 1,
                failure: None,
                response_excerpt: None,
            },
        )
        .await;
    manager
        .remember_edit_constraint_agent_created_paths(
            &session.session_id,
            vec!["tests/kept-repro.rs".to_string()],
            "turn-0",
        )
        .await;
    manager
        .remember_edit_constraint_extraction(
            &session.session_id,
            ConstraintExtractionRecord {
                message_sha256: "turn-1-hash".to_string(),
                dialog_turn_id: Some("turn-1".to_string()),
                status: ExtractionStatus::Extracted,
                constraints: Vec::new(),
                deterministic_constraint_count: 0,
                model_attempts: 1,
                active_constraint_ids: vec![test_constraint.id.clone()],
                revocation_authorized: true,
                model_status: ModelExtractionStatus::Parsed,
                model_constraints: Vec::new(),
                model_revocations: vec![ConstraintRevocation {
                    constraint_id: test_constraint.id.clone(),
                    description: "tests may now be modified".to_string(),
                }],
                revoked_constraint_ids: vec![test_constraint.id.clone()],
                unmatched_revocation_ids: Vec::new(),
                input_chars: 24,
                prompt_chars: 24,
                input_truncated: false,
                latency_ms: 1,
                extracted_at_ms: 2,
                failure: None,
                response_excerpt: None,
            },
        )
        .await;
    manager
        .remember_edit_constraint_agent_created_paths(
            &session.session_id,
            vec!["tests/future-repro.rs".to_string()],
            "turn-1",
        )
        .await;

    for index in 0..3 {
        let mut turn = DialogTurnData::new(
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
        turn.agent_type = Some(if index == 0 {
            "Standard".to_string()
        } else {
            "Cowork".to_string()
        });
        persistence_manager
            .save_dialog_turn(workspace.path(), &turn)
            .await
            .expect("turn should save");
    }

    {
        let mut active = manager
            .sessions
            .get_mut(&session.session_id)
            .expect("session should be active");
        active.dialog_turn_ids = vec![
            "turn-0".to_string(),
            "turn-1".to_string(),
            "turn-2".to_string(),
        ];
        active.last_user_dialog_agent_type = Some("Cowork".to_string());
    }
    persistence_manager
        .save_turn_context_snapshot(
            workspace.path(),
            &session.session_id,
            0,
            &[crate::agentic::core::Message::user("prompt 0".to_string())],
        )
        .await
        .expect("snapshot 0 should save");
    persistence_manager
        .save_turn_context_snapshot(
            workspace.path(),
            &session.session_id,
            1,
            &[
                crate::agentic::core::Message::user("prompt 0".to_string()),
                crate::agentic::core::Message::user("prompt 1".to_string()),
            ],
        )
        .await
        .expect("snapshot 1 should save");

    let revision = crate::agentic::session::FileRevision {
        modified_ns: 7,
        byte_len: 42,
        content_sha256: [7; 32],
    };
    manager.record_review_read(&session.session_id, "src/auth.rs", revision, 1, 20, 20);
    assert!(manager
        .review_read_coverage(&session.session_id, "src/auth.rs", revision, 1, 20)
        .is_some());

    manager
        .rollback_context_to_turn_start(workspace.path(), &session.session_id, 1)
        .await
        .expect("rollback should succeed");

    assert!(manager
        .review_read_coverage(&session.session_id, "src/auth.rs", revision, 1, 20)
        .is_none());

    let turns = persistence_manager
        .load_session_turns(workspace.path(), &session.session_id)
        .await
        .expect("turns should load");
    assert_eq!(turns.len(), 1);
    assert_eq!(turns[0].user_message.content, "prompt 0");
    assert_eq!(turns[0].agent_type.as_deref(), Some("Standard"));
    assert!(persistence_manager
        .load_turn_context_snapshot(workspace.path(), &session.session_id, 1)
        .await
        .expect("snapshot load should succeed")
        .is_none());
    assert_eq!(
        manager.edit_constraints(&session.session_id),
        Some(vec![test_constraint.clone()])
    );
    assert_eq!(
        manager
            .edit_constraint_state(&session.session_id)
            .expect("constraint state should remain cached")
            .agent_created_paths,
        vec!["tests/kept-repro.rs".to_string()]
    );

    manager.evict_loaded_session_for_test(&session.session_id);
    let restored = manager
        .restore_session(workspace.path(), &session.session_id)
        .await
        .expect("session should restore");
    assert_eq!(restored.dialog_turn_ids, vec!["turn-0".to_string()]);
    assert_eq!(
        restored.last_user_dialog_agent_type.as_deref(),
        Some("Standard")
    );
    assert_eq!(
        manager
            .context_store
            .get_context_messages(&session.session_id)
            .len(),
        1
    );

    let metadata = persistence_manager
        .load_session_metadata(workspace.path(), &session.session_id)
        .await
        .expect("metadata should load")
        .expect("metadata should exist");
    assert_eq!(metadata.turn_count, 1);
    let restored_state = SessionManager::edit_constraint_state_from_metadata(Some(&metadata))
        .expect("constraint metadata should restore");
    assert_eq!(restored_state.constraints, vec![test_constraint]);
    assert_eq!(
        restored_state.agent_created_paths,
        vec!["tests/kept-repro.rs".to_string()]
    );
}

#[tokio::test]
async fn staged_revert_filters_runtime_restore_and_transcript_without_deleting_turns() {
    use crate::agentic::session::revert::{
        SessionRevertPhase, SessionRevertState, SESSION_REVERT_SCHEMA_VERSION,
    };

    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let session = manager
        .create_session(
            "Staged revert".to_string(),
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

    let legacy_error = manager
        .validate_rollback_context_to_turn_start_locked(workspace.path(), &session.session_id, 1)
        .await
        .expect_err("legacy rollback must not truncate a staged Session suffix");
    assert!(matches!(legacy_error, OpenBitFunError::OutcomeUnknown(_)));

    manager
        .apply_staged_revert_context_locked(
            workspace.path(),
            &session.session_id,
            state.boundary_turn,
        )
        .await
        .expect("staged context should apply");
    assert_eq!(
        manager
            .get_session(&session.session_id)
            .expect("session should remain active")
            .dialog_turn_ids,
        vec!["turn-0".to_string(), "turn-1".to_string()]
    );
    assert_eq!(
        persistence_manager
            .load_session_turns(workspace.path(), &session.session_id)
            .await
            .expect("hidden turns should remain persisted")
            .len(),
        3
    );

    manager.evict_loaded_session_for_test(&session.session_id);
    let restored = manager
        .restore_session(workspace.path(), &session.session_id)
        .await
        .expect("staged session should restore");
    assert_eq!(restored.dialog_turn_ids.len(), 2);
    assert_eq!(
        manager
            .context_store
            .get_context_messages(&session.session_id)
            .len(),
        2
    );
    let _mutation = manager
        .acquire_session_mutation(&session.session_id)
        .await
        .expect("transcript mutation");
    assert_eq!(
        manager
            .load_persisted_transcript_turns_locked(&session.session_id)
            .await
            .expect("transcript should load")
            .expect("transcript should be persisted")
            .len(),
        2
    );
}

#[tokio::test]
async fn rollback_context_failure_preserves_turn_history() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let session = manager
        .create_session(
            "Rollback failure".to_string(),
            "agent".to_string(),
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
    }
    manager
        .sessions
        .get_mut(&session.session_id)
        .expect("session should be active")
        .dialog_turn_ids = vec!["turn-0".to_string(), "turn-1".to_string()];

    let error = manager
        .rollback_context_to_turn_start(workspace.path(), &session.session_id, 1)
        .await
        .expect_err("missing context snapshot must fail rollback");

    assert!(error.to_string().contains("context snapshot"), "{error}");
    assert_eq!(
        manager
            .get_session(&session.session_id)
            .expect("session remains loaded")
            .dialog_turn_ids,
        vec!["turn-0".to_string(), "turn-1".to_string()]
    );
    assert_eq!(
        persistence_manager
            .load_session_turns(workspace.path(), &session.session_id)
            .await
            .expect("turns should remain")
            .len(),
        2
    );
}

#[tokio::test]
async fn rollback_context_waits_for_the_session_mutation_boundary() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = Arc::new(test_manager(persistence_manager.clone()));
    let session = manager
        .create_session(
            "Serialized rollback".to_string(),
            "agent".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");
    let turn = DialogTurnData::new(
        "turn-0".to_string(),
        0,
        session.session_id.clone(),
        UserMessageData {
            id: "turn-0-user".to_string(),
            content: "prompt".to_string(),
            timestamp: 0,
            metadata: None,
        },
    );
    persistence_manager
        .save_dialog_turn(workspace.path(), &turn)
        .await
        .expect("turn should save");
    manager
        .sessions
        .get_mut(&session.session_id)
        .expect("session should be active")
        .dialog_turn_ids = vec!["turn-0".to_string()];

    let mutation = manager
        .acquire_session_mutation(&session.session_id)
        .await
        .expect("hold mutation boundary");
    let rollback_manager = manager.clone();
    let rollback_workspace = workspace.path().to_path_buf();
    let rollback_session_id = session.session_id.clone();
    let rollback = tokio::spawn(async move {
        rollback_manager
            .rollback_context_to_turn_start(&rollback_workspace, &rollback_session_id, 0)
            .await
    });
    tokio::task::yield_now().await;
    assert!(!rollback.is_finished());

    drop(mutation);
    rollback
        .await
        .expect("rollback task should join")
        .expect("rollback should succeed after mutation releases");
    assert!(persistence_manager
        .load_session_turns(workspace.path(), &session.session_id)
        .await
        .expect("turns should load")
        .is_empty());
}

#[tokio::test]
async fn rollback_to_empty_history_clears_last_user_dialog_agent_type() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let session = manager
        .create_session(
            "Rollback empty history".to_string(),
            "Cowork".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");

    let mut turn = DialogTurnData::new(
        "turn-0".to_string(),
        0,
        session.session_id.clone(),
        UserMessageData {
            id: "turn-0-user".to_string(),
            content: "plan prompt".to_string(),
            timestamp: 0,
            metadata: None,
        },
    );
    turn.agent_type = Some("Cowork".to_string());
    persistence_manager
        .save_dialog_turn(workspace.path(), &turn)
        .await
        .expect("turn should save");

    {
        let mut active = manager
            .sessions
            .get_mut(&session.session_id)
            .expect("session should be active");
        active.dialog_turn_ids = vec!["turn-0".to_string()];
        active.last_user_dialog_agent_type = Some("Cowork".to_string());
    }

    manager
        .rollback_context_to_turn_start(workspace.path(), &session.session_id, 0)
        .await
        .expect("rollback should succeed");

    let active = manager
        .get_session(&session.session_id)
        .expect("session should remain in memory");
    assert_eq!(active.agent_type, "Cowork");
    assert_eq!(active.last_user_dialog_agent_type, None);
}
