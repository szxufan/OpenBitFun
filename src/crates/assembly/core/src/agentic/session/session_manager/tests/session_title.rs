use super::super::SessionManager;
use super::support::{test_manager, TestWorkspace};
use crate::agentic::core::SessionConfig;
use crate::agentic::persistence::PersistenceManager;
use crate::util::errors::OpenBitFunError;
use std::sync::Arc;

#[tokio::test]
async fn background_title_update_cannot_recreate_storage_during_deletion() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let sessions_dir = persistence_manager
        .path_manager()
        .project_sessions_dir(workspace.path());
    let manager = Arc::new(test_manager(persistence_manager.clone()));
    let session = manager
        .create_session(
            "Original".to_string(),
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
    let deletion_guard = manager
        .acquire_session_mutation(&session_id)
        .await
        .expect("deletion mutation guard");

    let title_manager = manager.clone();
    let title_session_id = session_id.clone();
    let title_update = tokio::spawn(async move {
        title_manager
            .update_session_title_if_current(&title_session_id, "Original", "Generated title")
            .await
    });
    tokio::task::yield_now().await;
    assert!(
        !title_update.is_finished(),
        "title persistence must wait for the deletion mutation boundary"
    );

    persistence_manager
        .delete_session(&sessions_dir, &session_id)
        .await
        .expect("persistence deletion");
    manager.evict_loaded_session_for_test(&session_id);
    manager.session_storage_path_index.remove(&session_id);
    drop(deletion_guard);

    let error = title_update
        .await
        .expect("title task should not panic")
        .expect_err("deleted session title update must fail");
    assert!(error.to_string().contains("not found"));
    assert!(!sessions_dir.join(&session_id).exists());
}

#[tokio::test]
async fn manual_title_equal_to_default_blocks_delayed_generation_after_restore() {
    let workspace = TestWorkspace::new();
    let persistence_manager = Arc::new(PersistenceManager::new(workspace.path_manager()).unwrap());
    let manager = test_manager(persistence_manager);
    let session = manager
        .create_session(
            "New Session".into(),
            "Claw".into(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    manager
        .update_session_title(&session.session_id, "New Session")
        .await
        .unwrap();
    assert!(!manager
        .update_session_title_if_current(&session.session_id, "New Session", "Generated topic")
        .await
        .unwrap());
    manager.evict_loaded_session_for_test(&session.session_id);
    let restored = manager
        .restore_session(workspace.path(), &session.session_id)
        .await
        .unwrap();
    assert_eq!(restored.session_name, "New Session");
    assert!(!manager
        .update_session_title_if_current(&session.session_id, "New Session", "Another topic")
        .await
        .unwrap());
}

#[tokio::test]
async fn failed_title_persistence_does_not_publish_the_new_name_in_memory() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let session = manager
        .create_session(
            "Original".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");

    persistence_manager.fail_next_session_metadata_write_for_test(&session.session_id);

    manager
        .update_session_title(&session.session_id, "Not persisted")
        .await
        .expect_err("failed metadata persistence must reject the title update");

    let loaded = manager
        .get_session(&session.session_id)
        .expect("session must remain loaded");
    assert_eq!(loaded.session_name, "Original");
}

#[tokio::test]
async fn session_title_update_does_not_rewrite_the_runtime_state_file() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let session = manager
        .create_session(
            "Original".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");
    persistence_manager.fail_next_session_state_write_for_test(&session.session_id);

    manager
        .update_session_title(&session.session_id, "Renamed")
        .await
        .expect("title update must not rewrite runtime state");
    manager.evict_loaded_session_for_test(&session.session_id);

    let restored = manager
        .restore_session(workspace.path(), &session.session_id)
        .await
        .expect("metadata-only title update should remain restorable");
    assert_eq!(restored.session_name, "Renamed");
}

#[tokio::test]
async fn failed_title_index_update_rolls_back_persisted_metadata() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let session = manager
        .create_session(
            "Original".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");
    let sessions_dir = persistence_manager
        .path_manager()
        .project_sessions_dir(workspace.path());
    let index_path = sessions_dir.join("index.json");
    std::fs::remove_file(&index_path).expect("replace index file");
    std::fs::create_dir(&index_path).expect("create invalid index directory");

    manager
        .update_session_title(&session.session_id, "Renamed")
        .await
        .expect_err("index failure must reject the title update");

    assert_eq!(
        manager
            .get_session(&session.session_id)
            .expect("session remains loaded")
            .session_name,
        "Original"
    );
    let metadata = persistence_manager
        .load_session_metadata(&sessions_dir, &session.session_id)
        .await
        .expect("metadata should remain readable")
        .expect("metadata should exist");
    assert_eq!(metadata.session_name, "Original");
}

#[tokio::test]
async fn failed_title_rollback_reports_an_unknown_outcome() {
    let workspace = TestWorkspace::new();
    let persistence_manager =
        Arc::new(PersistenceManager::new(workspace.path_manager()).expect("persistence manager"));
    let manager = test_manager(persistence_manager.clone());
    let session = manager
        .create_session(
            "Original".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
                ..Default::default()
            },
        )
        .await
        .expect("session should create");
    let sessions_dir = persistence_manager
        .path_manager()
        .project_sessions_dir(workspace.path());
    let index_path = sessions_dir.join("index.json");
    std::fs::remove_file(&index_path).expect("replace index file");
    std::fs::create_dir(&index_path).expect("create invalid index directory");
    persistence_manager.fail_next_session_metadata_rollback_for_test(&session.session_id);

    let error = manager
        .update_session_title(&session.session_id, "Renamed")
        .await
        .expect_err("failed rollback must not report a definite failure");

    assert!(matches!(error, OpenBitFunError::OutcomeUnknown(_)));
    let metadata = persistence_manager
        .load_session_metadata(&sessions_dir, &session.session_id)
        .await
        .expect("metadata should remain readable")
        .expect("metadata should exist");
    assert_eq!(metadata.session_name, "Renamed");
}

#[test]
fn fallback_session_title_uses_sentence_break_when_available() {
    let title = SessionManager::fallback_session_title(
        "Fix the flaky integration test. Add logging for retries.",
        20,
    );

    assert_eq!(title, "Fix the flaky...");
}

#[test]
fn fallback_session_title_appends_ellipsis_when_truncated_without_sentence_break() {
    let title =
        SessionManager::fallback_session_title("Implement session title generation fallback", 12);

    assert_eq!(title, "Implement...");
}

#[test]
fn fallback_session_title_uses_default_for_blank_input() {
    let title = SessionManager::fallback_session_title("   ", 20);

    assert_eq!(title, "New Session");
}
