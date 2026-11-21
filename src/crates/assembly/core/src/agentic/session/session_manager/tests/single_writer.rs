use super::support::{test_manager, TestWorkspace};
use crate::agentic::core::{ProcessingPhase, Session, SessionConfig, SessionState};
use crate::agentic::persistence::PersistenceManager;
use std::sync::Arc;

#[tokio::test]
async fn a_persisted_session_has_one_writer_across_managers() {
    let workspace = TestWorkspace::new();
    let path_manager = workspace.path_manager();
    let first = test_manager(Arc::new(
        PersistenceManager::new(path_manager.clone()).expect("first persistence manager"),
    ));
    let second = test_manager(Arc::new(
        PersistenceManager::new(path_manager).expect("second persistence manager"),
    ));
    let session = first
        .create_session(
            "Single writer".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("first writer should create the session");

    let duplicate = first
        .create_session_with_id(
            Some(session.session_id.clone()),
            "Duplicate".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect_err("the current manager must reject an already-loaded Session ID");
    assert!(matches!(
        duplicate,
        crate::util::errors::OpenBitFunError::Validation(ref message)
            if message.contains("already exists")
    ));

    let error = second
        .restore_session(workspace.path(), &session.session_id)
        .await
        .expect_err("second writer must fail immediately");
    assert!(matches!(
        error,
        crate::util::errors::OpenBitFunError::SessionInUse { ref session_id }
            if session_id == &session.session_id
    ));

    let (view, _) = second
        .restore_session_view(workspace.path(), &session.session_id)
        .await
        .expect("read-only view must remain available");
    assert_eq!(view.session_id, session.session_id);

    assert!(first
        .unload_session_from_memory(&session.session_id)
        .await
        .expect("first writer should unload"));
    second
        .restore_session(workspace.path(), &session.session_id)
        .await
        .expect("writer should transfer after successful unload");
}

#[tokio::test]
async fn different_sessions_in_the_same_workspace_can_have_different_writers() {
    let workspace = TestWorkspace::new();
    let path_manager = workspace.path_manager();
    let persistence = Arc::new(
        PersistenceManager::new(path_manager.clone()).expect("fixture persistence manager"),
    );
    let config = SessionConfig {
        tool_presentation_mode: Default::default(),
        workspace_path: Some(workspace.path().to_string_lossy().to_string()),
        ..Default::default()
    };
    let first_session = Session::new_with_id(
        "first-workspace-session".to_string(),
        "First".to_string(),
        "Standard".to_string(),
        config.clone(),
    );
    let second_session = Session::new_with_id(
        "second-workspace-session".to_string(),
        "Second".to_string(),
        "Standard".to_string(),
        config,
    );
    persistence
        .save_session(workspace.path(), &first_session)
        .await
        .expect("first fixture");
    persistence
        .save_session(workspace.path(), &second_session)
        .await
        .expect("second fixture");
    let first = test_manager(Arc::new(
        PersistenceManager::new(path_manager.clone()).expect("first persistence manager"),
    ));
    let second = test_manager(Arc::new(
        PersistenceManager::new(path_manager).expect("second persistence manager"),
    ));

    first
        .restore_session(workspace.path(), &first_session.session_id)
        .await
        .expect("first Session writer");
    second
        .restore_session(workspace.path(), &second_session.session_id)
        .await
        .expect("second Session writer in the same workspace");
}

#[tokio::test]
async fn workspace_path_aliases_cannot_bypass_session_single_writer() {
    let workspace = TestWorkspace::new();
    let path_manager = workspace.path_manager();
    let first = test_manager(Arc::new(
        PersistenceManager::new(path_manager.clone()).expect("first persistence manager"),
    ));
    let second = test_manager(Arc::new(
        PersistenceManager::new(path_manager).expect("second persistence manager"),
    ));
    let session = first
        .create_session(
            "Aliased workspace".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("first writer");

    let error = second
        .restore_session(&workspace.path().join("."), &session.session_id)
        .await
        .expect_err("workspace alias must identify the same Session");
    assert!(matches!(
        error,
        crate::util::errors::OpenBitFunError::SessionInUse { .. }
    ));
}

#[tokio::test]
async fn a_failed_restore_does_not_keep_the_session_write_lock() {
    let workspace = TestWorkspace::new();
    let path_manager = workspace.path_manager();
    let first_persistence =
        Arc::new(PersistenceManager::new(path_manager.clone()).expect("first persistence manager"));
    let first = test_manager(first_persistence.clone());
    let session_id = "restore-after-failure";

    first
        .restore_session(workspace.path(), session_id)
        .await
        .expect_err("missing Session restore should fail");

    let fixture = Session::new_with_id(
        session_id.to_string(),
        "Recovered fixture".to_string(),
        "Standard".to_string(),
        SessionConfig {
            tool_presentation_mode: Default::default(),
            workspace_path: Some(workspace.path().to_string_lossy().to_string()),
            ..Default::default()
        },
    );
    first_persistence
        .save_session(workspace.path(), &fixture)
        .await
        .expect("persist recovered fixture");
    let second = test_manager(Arc::new(
        PersistenceManager::new(path_manager).expect("second persistence manager"),
    ));
    second
        .restore_session(workspace.path(), session_id)
        .await
        .expect("failed restore must release the temporary writer lock");
}

#[tokio::test]
async fn a_failed_create_does_not_keep_the_session_write_lock() {
    let workspace = TestWorkspace::new();
    let path_manager = workspace.path_manager();
    let first_persistence =
        Arc::new(PersistenceManager::new(path_manager.clone()).expect("first persistence manager"));
    let first = test_manager(first_persistence.clone());
    let session_id = "create-after-failure";
    first_persistence.fail_next_session_state_write_for_test(session_id);

    first
        .create_session_with_id(
            Some(session_id.to_string()),
            "Failed fixture".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect_err("injected persistence failure");

    let second = test_manager(Arc::new(
        PersistenceManager::new(path_manager).expect("second persistence manager"),
    ));
    second
        .create_session_with_id(
            Some(session_id.to_string()),
            "Recovered fixture".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("failed create must release the temporary writer lock");
}

#[tokio::test]
async fn failed_unload_save_keeps_the_session_write_lock() {
    let workspace = TestWorkspace::new();
    let path_manager = workspace.path_manager();
    let first_persistence =
        Arc::new(PersistenceManager::new(path_manager.clone()).expect("first persistence manager"));
    let first = test_manager(first_persistence.clone());
    let second = test_manager(Arc::new(
        PersistenceManager::new(path_manager).expect("second persistence manager"),
    ));
    let session = first
        .create_session(
            "Unload failure".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("first writer");
    first_persistence.fail_next_session_state_write_for_test(&session.session_id);

    first
        .unload_session_from_memory(&session.session_id)
        .await
        .expect_err("injected unload save failure");
    let error = second
        .restore_session(workspace.path(), &session.session_id)
        .await
        .expect_err("failed unload save must retain writer ownership");
    assert!(matches!(
        error,
        crate::util::errors::OpenBitFunError::SessionInUse { .. }
    ));
}

#[tokio::test]
async fn rejected_unload_keeps_the_session_write_lock() {
    let workspace = TestWorkspace::new();
    let path_manager = workspace.path_manager();
    let first = test_manager(Arc::new(
        PersistenceManager::new(path_manager.clone()).expect("first persistence manager"),
    ));
    let second = test_manager(Arc::new(
        PersistenceManager::new(path_manager).expect("second persistence manager"),
    ));
    let session = first
        .create_session(
            "Processing".to_string(),
            "Standard".to_string(),
            SessionConfig {
                tool_presentation_mode: Default::default(),
                workspace_path: Some(workspace.path().to_string_lossy().to_string()),
                ..Default::default()
            },
        )
        .await
        .expect("first writer");
    first
        .sessions
        .get_mut(&session.session_id)
        .expect("loaded Session")
        .state = SessionState::Processing {
        current_turn_id: "active-turn".to_string(),
        phase: ProcessingPhase::Thinking,
    };

    first
        .unload_session_from_memory(&session.session_id)
        .await
        .expect_err("processing Session must not unload");
    let error = second
        .restore_session(workspace.path(), &session.session_id)
        .await
        .expect_err("failed unload must retain writer ownership");
    assert!(matches!(
        error,
        crate::util::errors::OpenBitFunError::SessionInUse { .. }
    ));
}
