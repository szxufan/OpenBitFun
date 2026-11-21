use super::super::PermissionMode;
use super::support::{in_memory_test_manager, test_manager, TestWorkspace};
use crate::agentic::core::{ProcessingPhase, Session, SessionConfig, SessionState};
use crate::agentic::persistence::PersistenceManager;
use crate::service::session::SessionKind;
use std::sync::Arc;
use uuid::Uuid;

#[tokio::test]
async fn session_permission_read_preserves_override_legacy_default_and_live_turn_authority() {
    use openbitfun_services_core::session::SessionStorageLayout;

    let workspace = TestWorkspace::new();
    let paths = workspace.path_manager();
    let persistence = Arc::new(PersistenceManager::new(paths.clone()).unwrap());
    let manager = test_manager(persistence.clone());
    let id = Uuid::new_v4().to_string();
    let mut session = Session::new_with_id(
        id.clone(),
        "Permission header".into(),
        "Standard".into(),
        SessionConfig {
            workspace_path: Some(workspace.path().to_string_lossy().into_owned()),
            ..Default::default()
        },
    );
    let storage_path = paths.project_sessions_dir(workspace.path());
    let layout = SessionStorageLayout::new(&storage_path);
    for mode in [
        None,
        Some(PermissionMode::Ask),
        Some(PermissionMode::AutoApprove),
        Some(PermissionMode::FullAccess),
    ] {
        session.config.permission_mode = mode;
        persistence
            .save_session(workspace.path(), &session)
            .await
            .unwrap();
        let header = manager
            .read_session_header(Some(&storage_path), &id, false)
            .await
            .unwrap();
        // Read the session override as stored. None continues to mean
        // inherit; a query must not resolve or persist a global default.
        assert_eq!(header.config.permission_mode, mode);
        assert!(manager.get_session(&id).is_none());
        assert!(manager.active_session_permits.is_empty());
        assert!(!manager.context_store.has_session(&id));
    }
    let state_path = layout.state_path(&id);
    let mut state: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&state_path).unwrap()).unwrap();
    state["runtime_state"] = serde_json::to_value(SessionState::Processing {
        current_turn_id: "interrupted".into(),
        phase: ProcessingPhase::Thinking,
    })
    .unwrap();
    std::fs::write(&state_path, serde_json::to_vec(&state).unwrap()).unwrap();
    let header = manager
        .read_session_header(Some(&storage_path), &id, false)
        .await
        .unwrap();
    assert!(matches!(header.state, SessionState::Idle));
    assert_eq!(
        manager.active_turn_permission_mode(&id, "interrupted"),
        None
    );
    assert!(manager.get_session(&id).is_none());

    std::fs::remove_file(&state_path).unwrap();
    let legacy = manager
        .read_session_header(Some(&storage_path), &id, false)
        .await
        .unwrap();
    assert_eq!(legacy.config.permission_mode, None);
    assert!(!state_path.exists());
    // Without a workspace or live Session, the override is unknown rather
    // than inherited. Do not manufacture a null/default permission reply.
    assert!(manager.read_session_header(None, &id, false).await.is_err());

    session.state = SessionState::Processing {
        current_turn_id: "live-turn".into(),
        phase: ProcessingPhase::Thinking,
    };
    manager.sessions.insert(id.clone(), session);
    assert!(manager.set_active_turn_permission_mode(&id, "live-turn", PermissionMode::Ask));
    let live = manager.read_session_header(None, &id, false).await.unwrap();
    assert_eq!(
        live.config.permission_mode,
        Some(PermissionMode::FullAccess)
    );
    assert_eq!(
        manager.active_turn_permission_mode(&id, "live-turn"),
        Some(PermissionMode::Ask)
    );
    assert_eq!(
        manager.active_turn_permission_mode(&id, "interrupted"),
        None
    );
}

#[tokio::test]
async fn session_permission_mode_is_per_session_and_clearable() {
    let workspace = TestWorkspace::new();
    let manager = in_memory_test_manager();
    let config = SessionConfig {
        tool_presentation_mode: Default::default(),
        workspace_path: Some(workspace.path().to_string_lossy().to_string()),
        ..Default::default()
    };
    let first = manager
        .create_session_with_id_and_details(
            None,
            "First".to_string(),
            "Standard".to_string(),
            config.clone(),
            None,
            SessionKind::Standard,
        )
        .await
        .expect("create first session");
    let second = manager
        .create_session_with_id_and_details(
            None,
            "Second".to_string(),
            "Standard".to_string(),
            config,
            None,
            SessionKind::Standard,
        )
        .await
        .expect("create second session");

    // A new session starts without an override and follows the default.
    assert_eq!(manager.session_permission_mode(&first.session_id), None);

    manager
        .update_session_permission_mode(&first.session_id, Some(PermissionMode::FullAccess))
        .await
        .expect("set first session mode");

    // The selection stays inside the session it was made in.
    assert_eq!(
        manager.session_permission_mode(&first.session_id),
        Some(PermissionMode::FullAccess)
    );
    assert_eq!(manager.session_permission_mode(&second.session_id), None);

    // Clearing returns the session to the user-level default.
    manager
        .update_session_permission_mode(&first.session_id, None)
        .await
        .expect("clear first session mode");
    assert_eq!(manager.session_permission_mode(&first.session_id), None);
}

#[tokio::test]
async fn session_permission_mode_update_rejects_a_missing_session() {
    let manager = in_memory_test_manager();

    let error = manager
        .update_session_permission_mode("missing-session", Some(PermissionMode::AutoApprove))
        .await
        .expect_err("unknown session must not silently succeed");
    assert!(matches!(
        error,
        crate::util::errors::OpenBitFunError::NotFound(_)
    ));
}

#[tokio::test]
async fn active_turn_permission_mode_is_exact_mutable_and_stale_safe() {
    let manager = in_memory_test_manager();
    let session_id = Uuid::new_v4().to_string();
    let mut session = Session::new_with_id(
        session_id.clone(),
        "Active permission session".to_string(),
        "Standard".to_string(),
        SessionConfig::default(),
    );
    session.state = SessionState::Processing {
        current_turn_id: "turn-1".to_string(),
        phase: ProcessingPhase::Thinking,
    };
    manager.sessions.insert(session_id.clone(), session);

    assert!(manager.set_active_turn_permission_mode(&session_id, "turn-1", PermissionMode::Ask,));
    assert!(manager.set_active_turn_permission_mode(
        &session_id,
        "turn-1",
        PermissionMode::FullAccess,
    ));
    assert_eq!(
        manager.active_turn_permission_mode(&session_id, "turn-1"),
        Some(PermissionMode::FullAccess),
    );
    assert!(!manager.set_active_turn_permission_mode(
        &session_id,
        "stale-turn",
        PermissionMode::AutoApprove,
    ));

    manager
        .sessions
        .get_mut(&session_id)
        .expect("active session")
        .state = SessionState::Processing {
        current_turn_id: "turn-2".to_string(),
        phase: ProcessingPhase::Thinking,
    };
    assert_eq!(
        manager.active_turn_permission_mode(&session_id, "turn-1"),
        None,
    );
    assert!(manager.set_active_turn_permission_mode(
        &session_id,
        "turn-2",
        PermissionMode::AutoApprove,
    ));
    assert!(!manager.clear_active_turn_permission_mode(&session_id, "turn-1"));
    assert_eq!(
        manager.active_turn_permission_mode(&session_id, "turn-2"),
        Some(PermissionMode::AutoApprove),
    );
    assert!(manager.clear_active_turn_permission_mode(&session_id, "turn-2"));
    assert_eq!(
        manager.active_turn_permission_mode(&session_id, "turn-2"),
        None,
    );
}
