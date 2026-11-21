use super::super::SessionManager;
use super::support::{in_memory_test_manager, test_manager, TestWorkspace};
use crate::agentic::core::{ProcessingPhase, SessionConfig, SessionState};
use crate::agentic::persistence::PersistenceManager;
use crate::service::session::SessionKind;
use std::sync::Arc;

#[tokio::test]
async fn session_storage_identity_rejects_same_id_in_another_workspace() {
    let workspace = TestWorkspace::new();
    let other_workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager);
    let session_id = "shared-session-id";

    assert!(manager
        .claim_session_storage_path(session_id, workspace.path(), true)
        .expect("first workspace claim"));
    let error = manager
        .claim_session_storage_path(session_id, other_workspace.path(), true)
        .expect_err("a second workspace must not reuse an active session id");

    let message = error.to_string();
    assert!(message.contains(session_id));
    assert!(message.contains("another workspace"));
}

#[tokio::test]
async fn failed_claim_does_not_release_a_concurrent_same_workspace_claim() {
    let workspace = TestWorkspace::new();
    let other_workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager);
    let session_id = "concurrent-restore-session";

    let first_claim = manager
        .claim_session_storage_path(session_id, workspace.path(), true)
        .expect("first restore claim");
    manager
        .claim_session_storage_path(session_id, workspace.path(), true)
        .expect("concurrent restore in the same workspace");

    manager.release_failed_session_storage_path_claim(session_id, workspace.path(), first_claim);

    let error = manager
        .claim_session_storage_path(session_id, other_workspace.path(), true)
        .expect_err("a concurrent same-workspace restore must retain the binding");
    assert!(error.to_string().contains("another workspace"));
}

#[tokio::test]
async fn ephemeral_session_creation_rejects_active_duplicate_but_allows_evicted_id_reuse() {
    let workspace = TestWorkspace::new();
    let manager = in_memory_test_manager();
    let session_id = "reusable-session-id";
    let config = SessionConfig {
        tool_presentation_mode: Default::default(),
        workspace_path: Some(workspace.path().to_string_lossy().to_string()),
        ..Default::default()
    };

    manager
        .create_session_with_id_and_details(
            Some(session_id.to_string()),
            "Original".to_string(),
            "Standard".to_string(),
            config.clone(),
            None,
            SessionKind::EphemeralChild,
        )
        .await
        .expect("first session should create");
    let duplicate = manager
        .create_session_with_id_and_details(
            Some(session_id.to_string()),
            "Duplicate".to_string(),
            "Standard".to_string(),
            config.clone(),
            None,
            SessionKind::EphemeralChild,
        )
        .await
        .expect_err("an active duplicate must fail");
    assert!(duplicate.to_string().contains("already exists"));

    manager.evict_loaded_session_for_test(session_id);
    manager
        .create_session_with_id_and_details(
            Some(session_id.to_string()),
            "Recreated".to_string(),
            "Standard".to_string(),
            config,
            None,
            SessionKind::EphemeralChild,
        )
        .await
        .expect("an evicted same-workspace session id should be reusable");
}

#[tokio::test]
async fn persistent_session_creation_rejects_an_evicted_on_disk_id_without_overwriting_turns() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let sessions_dir = persistence_manager
        .path_manager()
        .project_sessions_dir(workspace.path());
    let manager = test_manager(persistence_manager);
    let session_id = "persisted-session-id";
    let config = SessionConfig {
        tool_presentation_mode: Default::default(),
        workspace_path: Some(workspace.path().to_string_lossy().to_string()),
        ..Default::default()
    };

    manager
        .create_session_with_id(
            Some(session_id.to_string()),
            "Original".to_string(),
            "Standard".to_string(),
            config.clone(),
        )
        .await
        .expect("first persistent session should create");
    let turns_dir = sessions_dir.join(session_id).join("turns");
    std::fs::create_dir_all(&turns_dir).expect("turns directory");
    let sentinel = turns_dir.join("existing-turn.json");
    std::fs::write(&sentinel, b"existing history").expect("persisted turn sentinel");
    manager.evict_loaded_session_for_test(session_id);

    let error = manager
        .create_session_with_id(
            Some(session_id.to_string()),
            "Replacement".to_string(),
            "Standard".to_string(),
            config,
        )
        .await
        .expect_err("an evicted persistent session id must not be reused");

    assert!(error.to_string().contains("already exists"));
    assert_eq!(
        std::fs::read(&sentinel).expect("existing turns must remain untouched"),
        b"existing history"
    );
    assert!(manager.get_session(session_id).is_none());
}

#[tokio::test]
async fn invalid_fixed_session_id_does_not_claim_or_insert_runtime_state() {
    let workspace = TestWorkspace::new();
    let manager = in_memory_test_manager();
    let invalid_id = "../other-session";

    let error = manager
        .create_session_with_id(
            Some(invalid_id.to_string()),
            "Invalid".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect_err("path-like session ids must be rejected");

    assert!(error.to_string().contains("session_id"));
    assert!(manager.get_session(invalid_id).is_none());
    assert!(manager.session_storage_path_index.get(invalid_id).is_none());
}

#[tokio::test]
async fn persistent_session_creation_failure_does_not_publish_runtime_state() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let session_id = "failed-persistent-session";
    persistence_manager.fail_next_session_state_write_for_test(session_id);
    let manager = test_manager(persistence_manager.clone());
    let config = SessionConfig {
        tool_presentation_mode: Default::default(),
        workspace_path: Some(workspace.path().to_string_lossy().to_string()),
        ..Default::default()
    };

    manager
        .create_session_with_id(
            Some(session_id.to_string()),
            "Must not become visible".to_string(),
            "Standard".to_string(),
            config.clone(),
        )
        .await
        .expect_err("state persistence failure must fail session creation");

    assert!(manager.get_session(session_id).is_none());
    assert!(manager.session_storage_path_index.get(session_id).is_none());
    assert!(!persistence_manager
        .session_storage_exists(workspace.path(), session_id)
        .expect("session storage existence"));
    assert!(persistence_manager
        .load_session_metadata(workspace.path(), session_id)
        .await
        .expect("load session metadata")
        .is_none());

    manager
        .create_session_with_id(
            Some(session_id.to_string()),
            "Retry succeeds".to_string(),
            "Standard".to_string(),
            config,
        )
        .await
        .expect("retry should not be blocked by partial persistence");
}

#[tokio::test]
async fn session_storage_reads_never_commit_or_follow_uncommitted_bindings() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let sessions_dir = persistence_manager
        .path_manager()
        .project_sessions_dir(workspace.path());
    let manager = test_manager(persistence_manager);
    let session = manager
        .create_session(
            "Storage binding".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");
    let session_id = session.session_id.clone();
    manager
        .sessions
        .get_mut(&session_id)
        .expect("loaded session")
        .dialog_turn_ids
        .push("turn-without-binding".to_string());

    manager.session_storage_path_index.remove(&session_id);
    let resolved = manager
        .require_session_storage_path(&session_id)
        .await
        .expect("config resolution");
    assert_eq!(
        SessionManager::normalize_session_storage_path(&resolved),
        SessionManager::normalize_session_storage_path(&sessions_dir)
    );
    assert!(
        manager.storage_path_binding_for_test(&session_id).is_none(),
        "a read must not pin the currently resolved storage"
    );

    let in_flight = workspace.path().join("in-flight-claim");
    assert!(manager
        .claim_session_storage_path(&session_id, &in_flight, true)
        .expect("pending claim"));
    assert!(manager
        .require_session_storage_path(&session_id)
        .await
        .is_err());
    assert!(manager
        .effective_session_storage_path(&session_id)
        .await
        .is_none());
    assert!(
        manager
            .persistent_model_exchange_trace_dir(&session_id)
            .await
            .is_none(),
        "an uncommitted claim must not become a storage fallback"
    );
    // Dispatch admission must surface an unresolvable binding instead of
    // reporting that no interrupted turn holds the queue.
    assert!(manager
        .latest_dialog_turn_holds_dispatch(&session_id)
        .await
        .is_err());
    assert!(manager
        .abandon_interrupted_dialog_turn(&session_id, None)
        .await
        .is_err());
}

#[tokio::test]
async fn loaded_session_identity_check_preserves_processing_state() {
    let workspace = TestWorkspace::new();
    let manager = in_memory_test_manager();
    let session = manager
        .create_session(
            "Active".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");
    let storage_path = manager
        .session_storage_path_index
        .get(&session.session_id)
        .expect("storage binding")
        .path
        .clone();
    manager
        .sessions
        .get_mut(&session.session_id)
        .expect("active session")
        .state = SessionState::Processing {
        current_turn_id: "turn-active".to_string(),
        phase: ProcessingPhase::Thinking,
    };

    assert!(manager
        .is_session_loaded_from_storage_path(&storage_path, &session.session_id)
        .expect("identity check"));
    assert!(matches!(
        manager.get_session(&session.session_id).expect("session").state,
        SessionState::Processing { ref current_turn_id, .. }
            if current_turn_id == "turn-active"
    ));
}

#[tokio::test]
async fn restoring_an_already_loaded_session_preserves_processing_state() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager);
    let session = manager
        .create_session(
            "Active".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");
    manager
        .sessions
        .get_mut(&session.session_id)
        .expect("active session")
        .state = SessionState::Processing {
        current_turn_id: "turn-active".to_string(),
        phase: ProcessingPhase::Thinking,
    };

    manager
        .restore_session(workspace.path(), &session.session_id)
        .await
        .expect("idempotent restore");

    assert!(matches!(
        manager.get_session(&session.session_id).expect("session").state,
        SessionState::Processing { ref current_turn_id, .. }
            if current_turn_id == "turn-active"
    ));
}
