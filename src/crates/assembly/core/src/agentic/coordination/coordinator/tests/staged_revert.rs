use super::super::test_support::{test_coordinator, test_persistent_coordinator};
use super::super::SessionRelationshipKind;
use crate::agentic::core::{Message, SessionConfig, SessionKind, TurnStats};
use crate::agentic::session::SessionManager;
use crate::service::session::{SessionMetadata, SessionRelationship};
use openbitfun_runtime_ports::{AgentLocalCommandTurnPort, AgentLocalCommandTurnRecordRequest};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

async fn create_two_turn_session(
    session_manager: &SessionManager,
    workspace: &std::path::Path,
    session_id: &str,
) -> PathBuf {
    session_manager
        .create_session_with_id(
            Some(session_id.to_string()),
            "Reverted".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
        .await
        .expect("create session");
    for (turn_id, prompt) in [("turn-0", "first"), ("turn-1", "second")] {
        session_manager
            .start_dialog_turn(
                session_id,
                "Standard".to_string(),
                prompt.to_string(),
                Some(turn_id.to_string()),
                None,
                None,
            )
            .await
            .expect("start persisted turn");
        session_manager
            .complete_dialog_turn(
                session_id,
                turn_id,
                format!("reply to {prompt}"),
                &[],
                TurnStats::default(),
                Some("complete".to_string()),
                Some(true),
            )
            .await
            .expect("complete persisted turn");
        session_manager.reset_session_state_if_processing(session_id, turn_id);
    }
    let storage_path = session_manager
        .effective_session_storage_path(session_id)
        .await
        .expect("session storage path");
    storage_path
}

async fn create_staged_two_turn_session(
    session_manager: &SessionManager,
    workspace: &std::path::Path,
    session_id: &str,
) -> PathBuf {
    let storage_path = create_two_turn_session(session_manager, workspace, session_id).await;
    session_manager
        .persistence_manager()
        .save_session_revert_state(
            &storage_path,
            session_id,
            &crate::agentic::session::revert::SessionRevertState {
                schema_version: crate::agentic::session::revert::SESSION_REVERT_SCHEMA_VERSION,
                boundary_turn: 1,
                original_turn_end: 2,
                phase: crate::agentic::session::revert::SessionRevertPhase::Staged,
                workspace_checkpoint: Vec::new(),
            },
        )
        .await
        .expect("stage session revert");
    let mutation = session_manager
        .acquire_session_mutation(session_id)
        .await
        .expect("session mutation");
    session_manager
        .apply_staged_revert_context_locked(&storage_path, session_id, 1)
        .await
        .expect("apply staged context");
    drop(mutation);
    storage_path
}

#[tokio::test]
async fn stale_targeted_revert_keeps_an_existing_staged_suffix() {
    let (coordinator, session_manager) = test_persistent_coordinator();
    let workspace = tempfile::tempdir().expect("workspace");
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(workspace.path());
    let session_id = format!("targeted-stale-{}", uuid::Uuid::new_v4());
    let storage_path =
        create_staged_two_turn_session(session_manager.as_ref(), workspace.path(), &session_id)
            .await;
    let marker_before = session_manager
        .persistence_manager()
        .load_session_revert_state(&storage_path, &session_id)
        .await
        .expect("load staged marker")
        .expect("staged marker");

    let error = coordinator
        .apply_targeted_session_revert_locked(
            &storage_path,
            &session_id,
            "turn-0",
            Some(0),
            Some("stale-catalog-revision"),
        )
        .await
        .expect_err("stale target must fail before mutation");
    assert!(matches!(
        error,
        crate::util::errors::OpenBitFunError::Validation(_)
    ));

    let marker_after = session_manager
        .persistence_manager()
        .load_session_revert_state(&storage_path, &session_id)
        .await
        .expect("reload staged marker")
        .expect("staged marker must remain");
    assert_eq!(marker_after, marker_before);
    let turns = session_manager
        .persistence_manager()
        .load_session_turns(&storage_path, &session_id)
        .await
        .expect("load physical turns");
    assert_eq!(
        turns
            .iter()
            .map(|turn| turn.turn_id.as_str())
            .collect::<Vec<_>>(),
        vec!["turn-0", "turn-1"]
    );
}

#[tokio::test]
async fn staged_revert_is_committed_before_local_and_maintenance_turns() {
    let (coordinator, session_manager) = test_persistent_coordinator();
    let workspace = tempfile::tempdir().expect("workspace");
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(workspace.path());

    let local_session_id = format!("local-revert-{}", uuid::Uuid::new_v4());
    let local_storage = create_staged_two_turn_session(
        session_manager.as_ref(),
        workspace.path(),
        &local_session_id,
    )
    .await;
    let child_session_id = format!("{local_session_id}-child");
    let grandchild_session_id = format!("{local_session_id}-grandchild");
    let mut child = SessionMetadata::new(
        child_session_id.clone(),
        "Hidden child".to_string(),
        "Explore".to_string(),
        "model".to_string(),
    );
    child.session_kind = SessionKind::Subagent;
    child.relationship = Some(SessionRelationship {
        kind: Some(SessionRelationshipKind::Subagent),
        parent_session_id: Some(local_session_id.clone()),
        parent_request_id: None,
        parent_dialog_turn_id: Some("turn-1".to_string()),
        parent_turn_index: Some(1),
        parent_tool_call_id: Some("tool-child".to_string()),
        subagent_type: Some("Explore".to_string()),
        continuation_policy: None,
    });
    child.workspace_path = Some(workspace.path().to_string_lossy().into_owned());
    session_manager
        .persistence_manager()
        .save_session_metadata(&local_storage, &child)
        .await
        .expect("hidden child metadata");
    let mut grandchild = SessionMetadata::new(
        grandchild_session_id.clone(),
        "Hidden grandchild".to_string(),
        "Explore".to_string(),
        "model".to_string(),
    );
    grandchild.session_kind = SessionKind::Subagent;
    grandchild.relationship = Some(SessionRelationship {
        kind: Some(SessionRelationshipKind::Subagent),
        parent_session_id: Some(child_session_id.clone()),
        parent_request_id: None,
        parent_dialog_turn_id: Some("child-turn".to_string()),
        parent_turn_index: Some(0),
        parent_tool_call_id: Some("tool-grandchild".to_string()),
        subagent_type: Some("Explore".to_string()),
        continuation_policy: None,
    });
    grandchild.workspace_path = Some(workspace.path().to_string_lossy().into_owned());
    session_manager
        .persistence_manager()
        .save_session_metadata(&local_storage, &grandchild)
        .await
        .expect("hidden grandchild metadata");
    AgentLocalCommandTurnPort::record_completed_local_command_turn(
        &coordinator,
        AgentLocalCommandTurnRecordRequest {
            session_id: local_session_id.clone(),
            content: "/usage".to_string(),
            turn_id: Some("local-turn".to_string()),
            timestamp_ms: None,
            metadata: serde_json::Map::new(),
        },
    )
    .await
    .expect("record local command after staged undo");
    let local_turns = session_manager
        .persistence_manager()
        .load_session_turns(&local_storage, &local_session_id)
        .await
        .expect("load local turns");
    assert_eq!(
        local_turns
            .iter()
            .map(|turn| turn.turn_id.as_str())
            .collect::<Vec<_>>(),
        vec!["turn-0", "local-turn"]
    );
    assert!(session_manager
        .persistence_manager()
        .load_session_revert_state(&local_storage, &local_session_id)
        .await
        .expect("load local marker")
        .is_none());
    for discarded_session_id in [&child_session_id, &grandchild_session_id] {
        assert!(session_manager
            .persistence_manager()
            .load_session_metadata(&local_storage, discarded_session_id)
            .await
            .expect("discarded child metadata lookup")
            .is_none());
    }

    let maintenance_session_id = format!("compact-revert-{}", uuid::Uuid::new_v4());
    let maintenance_storage = create_staged_two_turn_session(
        session_manager.as_ref(),
        workspace.path(),
        &maintenance_session_id,
    )
    .await;
    let task = coordinator
        .start_manual_compaction_task(
            maintenance_session_id.clone(),
            Some("maintenance-turn".to_string()),
        )
        .await
        .expect("start maintenance after staged undo");
    let maintenance_turns = session_manager
        .persistence_manager()
        .load_session_turns(&maintenance_storage, &maintenance_session_id)
        .await
        .expect("load maintenance turns");
    assert_eq!(
        maintenance_turns
            .iter()
            .map(|turn| turn.turn_id.as_str())
            .collect::<Vec<_>>(),
        vec!["turn-0", "maintenance-turn"]
    );
    assert!(session_manager
        .persistence_manager()
        .load_session_revert_state(&maintenance_storage, &maintenance_session_id)
        .await
        .expect("load maintenance marker")
        .is_none());
    coordinator
        .cancel_dialog_turn(&maintenance_session_id, &task.turn_id)
        .await
        .expect("cancel maintenance task");
    let _ = tokio::time::timeout(Duration::from_secs(5), task.completion).await;
}

#[tokio::test]
async fn mutating_restore_reconciles_a_marker_written_before_workspace_apply() {
    let (coordinator, session_manager) = test_persistent_coordinator();
    let workspace = tempfile::tempdir().expect("workspace");
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(workspace.path());
    let file_path = workspace.path().join("src/lib.rs");
    std::fs::create_dir_all(file_path.parent().expect("file parent")).expect("create file parent");
    tokio::fs::write(&file_path, "before\n")
        .await
        .expect("write original file");
    let session_id = format!("restore-revert-{}", uuid::Uuid::new_v4());
    let storage_path =
        create_two_turn_session(session_manager.as_ref(), workspace.path(), &session_id).await;
    let record =
        crate::service::workspace::legacy_compat::register_local_fixture(workspace.path(), None)
            .await;
    let snapshot_manager =
        crate::service::snapshot::get_or_create_snapshot_manager(&record.id, None)
            .await
            .expect("snapshot manager");
    let operation_id = snapshot_manager
        .record_file_change(
            &session_id,
            1,
            file_path.clone(),
            crate::service::snapshot::types::OperationType::Modify,
            "Edit".to_string(),
        )
        .await
        .expect("record file change");
    tokio::fs::write(&file_path, "after\n")
        .await
        .expect("write changed file");
    snapshot_manager
        .get_snapshot_service()
        .read()
        .await
        .complete_file_modification(&session_id, &operation_id, 1)
        .await
        .expect("complete file change");

    let mut state = crate::agentic::session::revert::SessionRevertState {
        schema_version: crate::agentic::session::revert::SESSION_REVERT_SCHEMA_VERSION,
        boundary_turn: 1,
        original_turn_end: 2,
        phase: crate::agentic::session::revert::SessionRevertPhase::Applying,
        workspace_checkpoint: Vec::new(),
    };
    snapshot_manager
        .prepare_workspace_revert(&session_id, &mut state)
        .await
        .expect("prepare staged checkpoint");
    session_manager
        .persistence_manager()
        .save_session_revert_state(&storage_path, &session_id, &state)
        .await
        .expect("persist marker before workspace apply");

    coordinator
        .restore_session_from_storage_path(&storage_path, &session_id)
        .await
        .expect("restore should reconcile staged workspace");

    assert_eq!(
        tokio::fs::read_to_string(&file_path)
            .await
            .expect("read reconciled file"),
        "before\n"
    );
    assert_eq!(
        session_manager
            .get_session(&session_id)
            .expect("restored session")
            .dialog_turn_ids,
        vec!["turn-0"]
    );
    let staged = session_manager
        .persistence_manager()
        .load_session_revert_state(&storage_path, &session_id)
        .await
        .expect("load staged marker")
        .expect("staged marker should remain");
    assert_eq!(
        staged.phase,
        crate::agentic::session::revert::SessionRevertPhase::Staged
    );

    tokio::fs::write(&file_path, "external edit\n")
        .await
        .expect("write external edit after successful undo");
    coordinator
        .commit_session_revert_before_submission(&session_id)
        .await
        .expect("commit stable staged boundary");
    assert_eq!(
        tokio::fs::read_to_string(&file_path)
            .await
            .expect("read external edit after commit"),
        "external edit\n"
    );
    assert!(session_manager
        .persistence_manager()
        .load_session_revert_state(&storage_path, &session_id)
        .await
        .expect("load committed marker")
        .is_none());
}

#[tokio::test]
async fn coordinator_delete_reconciles_an_unfinished_revert_before_cleanup() {
    let (coordinator, session_manager) = test_persistent_coordinator();
    let workspace = tempfile::tempdir().expect("workspace");
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(workspace.path());
    let session_id = format!("delete-revert-{}", uuid::Uuid::new_v4());
    let storage_path =
        create_two_turn_session(session_manager.as_ref(), workspace.path(), &session_id).await;
    let state = crate::agentic::session::revert::SessionRevertState {
        schema_version: crate::agentic::session::revert::SESSION_REVERT_SCHEMA_VERSION,
        boundary_turn: 1,
        original_turn_end: 2,
        phase: crate::agentic::session::revert::SessionRevertPhase::Applying,
        workspace_checkpoint: Vec::new(),
    };
    session_manager
        .persistence_manager()
        .save_session_revert_state(&storage_path, &session_id, &state)
        .await
        .expect("pending marker");

    coordinator
        .delete_session(workspace.path(), &session_id)
        .await
        .expect("coordinator should reconcile before deleting");

    assert!(session_manager.get_session(&session_id).is_none());
    assert!(session_manager
        .persistence_manager()
        .load_session_revert_state(&storage_path, &session_id)
        .await
        .expect("deleted marker load")
        .is_none());
}

#[tokio::test]
async fn transcript_read_waits_for_session_history_mutation_before_loading_turns() {
    let (coordinator, session_manager) = test_persistent_coordinator();
    let coordinator = Arc::new(coordinator);
    let workspace = tempfile::tempdir().expect("workspace");
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(workspace.path());
    let session_id = format!("transcript-mutation-{}", uuid::Uuid::new_v4());
    let storage_path =
        create_two_turn_session(session_manager.as_ref(), workspace.path(), &session_id).await;

    let mutation = session_manager
        .acquire_session_mutation(&session_id)
        .await
        .expect("simulated revert mutation");
    let reader = coordinator.clone();
    let read_session_id = session_id.clone();
    let transcript_task = tokio::spawn(async move {
        openbitfun_runtime_ports::SessionTranscriptReader::read_session_transcript(
            reader.as_ref(),
            openbitfun_runtime_ports::SessionTranscriptRequest {
                session_id: read_session_id,
                turn_id: None,
            },
        )
        .await
    });
    tokio::task::yield_now().await;
    assert!(
        !transcript_task.is_finished(),
        "transcript reads must share the session history mutation boundary"
    );

    session_manager
        .persistence_manager()
        .delete_turns_from(&storage_path, &session_id, 1)
        .await
        .expect("commit simulated suffix deletion");
    drop(mutation);

    let transcript = transcript_task
        .await
        .expect("transcript task")
        .expect("transcript after mutation");
    assert!(transcript
        .messages
        .iter()
        .all(|message| message.turn_id.as_deref() != Some("turn-1")));
    assert!(transcript
        .messages
        .iter()
        .any(|message| message.turn_id.as_deref() == Some("turn-0")));
}

#[tokio::test]
async fn transient_transcript_locked_fallback_does_not_reenter_session_mutation() {
    let (coordinator, session_manager) = test_coordinator();
    let workspace = tempfile::tempdir().expect("transient workspace");
    crate::service::workspace::legacy_compat::register_local_fixture_blocking(workspace.path());
    let session = session_manager
        .create_session(
            "Transient transcript".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
        .await
        .expect("transient session");
    session_manager
        .add_message(
            &session.session_id,
            Message::user("visible transient context".to_string()),
        )
        .await
        .expect("transient context message");

    let transcript = tokio::time::timeout(
        Duration::from_secs(1),
        openbitfun_runtime_ports::SessionTranscriptReader::read_session_transcript(
            &coordinator,
            openbitfun_runtime_ports::SessionTranscriptRequest {
                session_id: session.session_id,
                turn_id: None,
            },
        ),
    )
    .await
    .expect("transient transcript must not deadlock")
    .expect("transient transcript");

    assert_eq!(transcript.messages.len(), 1);
    assert!(matches!(
        &transcript.messages[0].content,
        openbitfun_runtime_ports::TranscriptContent::Text(text)
            if text == "visible transient context"
    ));
}
